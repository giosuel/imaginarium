// SPDX-License-Identifier: AGPL-3.0-or-later

//! Fuzzy command search for the palette.
//!
//! Budget: **< 1 ms per keystroke over 2,000 commands** (M2 gate). The
//! index pre-lowercases every searchable field once, so searching
//! allocates only the result list.
//!
//! # Ranking
//! Each query word must fuzzy-match (as a subsequence) one field of the
//! command: title, synonyms, category or id. A word's score rewards
//! matches at word starts, consecutive runs and a match at the very start,
//! and penalizes gaps. Fields are weighted (title > synonyms > id >
//! category). Exact and prefix title matches get a bonus. Finally a small
//! **frecency** boost (how often and how recently the command was used)
//! breaks ties in favor of the user's habits, without overriding clearly
//! better text matches.

use std::collections::HashMap;

use crate::id::CommandId;

/// Field weights.
const WEIGHT_TITLE: f64 = 1.0;
const WEIGHT_SYNONYM: f64 = 0.8;
const WEIGHT_ID: f64 = 0.6;
const WEIGHT_CATEGORY: f64 = 0.5;

/// Per-character scoring.
const SCORE_MATCH: f64 = 1.0;
const BONUS_WORD_START: f64 = 6.0;
const BONUS_CONSECUTIVE: f64 = 4.0;
const BONUS_FIELD_START: f64 = 4.0;
/// The whole query word is the start of a word in the field ("tab" in
/// "table"): beats a scattered match that happens to hit two word starts
/// ("tab" in "text box above").
const BONUS_WORD_PREFIX: f64 = 6.0;
const PENALTY_GAP: f64 = 0.2;
const MAX_GAP_PENALTY: f64 = 3.0;

/// Whole-query bonuses on the title.
const BONUS_EXACT_TITLE: f64 = 20.0;
const BONUS_TITLE_PREFIX: f64 = 10.0;

/// Upper bound of the frecency boost. It is kept well below a single
/// word-start bonus, so usage only reorders comparably good matches.
const MAX_FRECENCY: f64 = 5.0;

/// How a user has used one command.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    /// Number of invocations.
    pub count: u32,
    /// Unix time of the last invocation, in seconds.
    pub last_used: u64,
}

/// Usage statistics per command, which drive frecency. The app owns and
/// persists them. Time is passed in explicitly, which keeps the ranking
/// deterministic and testable.
#[derive(Clone, Debug, Default)]
pub struct UsageStats {
    usage: HashMap<CommandId, Usage>,
}

impl UsageStats {
    /// No usage yet.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records one invocation at `now` (Unix seconds).
    pub fn record(&mut self, id: CommandId, now: u64) {
        let usage = self.usage.entry(id).or_default();
        usage.count = usage.count.saturating_add(1);
        usage.last_used = usage.last_used.max(now);
    }

    /// Usage of one command.
    pub fn get(&self, id: CommandId) -> Usage {
        self.usage.get(&id).copied().unwrap_or_default()
    }

    /// Frecency boost in `0.0..=MAX_FRECENCY`. Frequency and recency each
    /// contribute half of the range, so neither can mask the other:
    /// frequency grows logarithmically and saturates at 100 uses, and
    /// recency halves every day.
    pub fn boost(&self, id: CommandId, now: u64) -> f64 {
        let usage = self.get(id);
        if usage.count == 0 {
            return 0.0;
        }
        let frequency = (f64::from(usage.count).ln_1p() / 100f64.ln_1p()).min(1.0);
        let age_days = now.saturating_sub(usage.last_used) as f64 / 86_400.0;
        let recency = 0.5f64.powf(age_days);
        MAX_FRECENCY * 0.5 * (frequency + recency)
    }
}

/// One search result.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SearchHit {
    /// The matching command.
    pub id: CommandId,
    /// Relevance (higher is better). Only comparable within one search.
    pub score: f64,
}

/// Searchable data of one command, pre-lowercased.
#[derive(Clone, Debug)]
struct Entry {
    id: CommandId,
    /// How many of `fields` are synonyms (they come first).
    synonyms: usize,
    title_len: usize,
    title: Field,
    fields: Vec<(Field, f64)>,
}

/// A lowercased field with precomputed word starts and a character mask.
#[derive(Clone, Debug)]
struct Field {
    chars: Vec<char>,
    word_start: Vec<bool>,
    /// Which characters occur (see [`char_mask`]), so a search can skip the
    /// field when it lacks a query character.
    mask: u64,
}

impl Field {
    fn new(text: &str) -> Self {
        let chars: Vec<char> = text.chars().flat_map(char::to_lowercase).collect();
        let word_start = (0..chars.len())
            .map(|i| {
                i == 0
                    || is_separator(chars[i - 1])
                    || (chars[i].is_alphanumeric() && !chars[i - 1].is_alphanumeric())
            })
            .collect();
        let mask = char_mask(&chars);
        Self {
            chars,
            word_start,
            mask,
        }
    }

    /// Whether the field may contain every character of a word with `mask`.
    /// Exact for ASCII letters and digits. Other characters share bits,
    /// which can only cause false "maybe"s, never false rejections.
    fn may_contain(&self, mask: u64) -> bool {
        self.mask & mask == mask
    }
}

/// 64-bit character set: a–z and 0–9 get their own bits, and every other
/// character is hashed into the remaining 28.
fn char_mask(chars: &[char]) -> u64 {
    chars.iter().fold(0, |mask, &c| {
        let bit = match c {
            'a'..='z' => c as u32 - 'a' as u32,
            '0'..='9' => 26 + c as u32 - '0' as u32,
            _ => 36 + (c as u32 % 28),
        };
        mask | (1 << bit)
    })
}

fn is_separator(c: char) -> bool {
    c.is_whitespace() || matches!(c, '.' | '_' | '-' | '/' | ':' | '(' | ')')
}

/// Search index over a set of commands. Build it once with
/// [`Registry::search_index`](crate::Registry::search_index), rebuild it
/// when commands are added (add-ons), and query it on every keystroke.
#[derive(Clone, Debug, Default)]
pub struct SearchIndex {
    entries: Vec<Entry>,
}

impl SearchIndex {
    /// An empty index.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a command's searchable texts.
    pub fn insert(&mut self, id: CommandId, title: &str, synonyms: &[&str], category: &str) {
        let mut fields = Vec::with_capacity(synonyms.len() + 2);
        fields.extend(synonyms.iter().map(|s| (Field::new(s), WEIGHT_SYNONYM)));
        fields.push((Field::new(id.as_str()), WEIGHT_ID));
        if !category.is_empty() {
            fields.push((Field::new(category), WEIGHT_CATEGORY));
        }
        self.entries.push(Entry {
            id,
            synonyms: synonyms.len(),
            title_len: title.chars().count(),
            title: Field::new(title),
            fields,
        });
    }

    /// Number of indexed commands.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the index is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Returns up to `limit` best matches, best first. Ties are ordered by
    /// shorter title, then id. An empty query lists the user's most frecent
    /// commands (the palette's initial view).
    pub fn search(
        &self,
        query: &str,
        usage: &UsageStats,
        now: u64,
        limit: usize,
    ) -> Vec<SearchHit> {
        let query: String = query.chars().flat_map(char::to_lowercase).collect();
        let words: Vec<(Vec<char>, u64)> = query
            .split(is_separator)
            .filter(|w| !w.is_empty())
            .map(|w| {
                let chars: Vec<char> = w.chars().collect();
                let mask = char_mask(&chars);
                (chars, mask)
            })
            .collect();
        let whole: Vec<char> = query.trim().chars().collect();

        let mut hits: Vec<(SearchHit, usize)> = self
            .entries
            .iter()
            .filter_map(|entry| {
                let text = if words.is_empty() {
                    if usage.get(entry.id).count == 0 {
                        return None;
                    }
                    0.0
                } else {
                    score_entry(entry, &words, &whole)?
                };
                let hit = SearchHit {
                    id: entry.id,
                    score: text + usage.boost(entry.id, now),
                };
                Some((hit, entry.title_len))
            })
            .collect();

        if limit == 0 {
            return Vec::new();
        }
        let order = |(a, a_len): &(SearchHit, usize), (b, b_len): &(SearchHit, usize)| {
            b.score
                .total_cmp(&a.score)
                .then(a_len.cmp(b_len))
                .then(a.id.cmp(&b.id))
        };
        // A one-letter query can match every command: select the best
        // `limit` in O(n), then sort only those.
        if hits.len() > limit {
            hits.select_nth_unstable_by(limit - 1, order);
            hits.truncate(limit);
        }
        hits.sort_by(order);
        hits.into_iter().map(|(hit, _)| hit).collect()
    }
}

/// Where a query matched one command, for highlighting it
/// ([`SearchIndex::explain`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Explanation {
    /// The matched characters of the title, as character indices, in
    /// order. (Indices into the lowercased title, which are the title's own
    /// for all but a few scripts.)
    pub title: Vec<usize>,
    /// The synonym that matched words the title didn't, if any: its index
    /// among the command's synonyms, and its matched characters.
    pub synonym: Option<(usize, Vec<usize>)>,
}

impl SearchIndex {
    /// Where `query` matches the command `id`: for each word, the field it
    /// matched best, as [`SearchIndex::search`] scores it. `None` if `id`
    /// isn't indexed or the query doesn't match it. For the few rows a
    /// palette shows, not for every command per keystroke.
    pub fn explain(&self, id: CommandId, query: &str) -> Option<Explanation> {
        let entry = self.entries.iter().find(|entry| entry.id == id)?;
        let query: String = query.chars().flat_map(char::to_lowercase).collect();
        let mut explanation = Explanation::default();
        for word in query.split(is_separator).filter(|w| !w.is_empty()) {
            let word: Vec<char> = word.chars().collect();
            let title = fuzzy_match(&word, &entry.title).map(|(s, at)| (s * WEIGHT_TITLE, at));
            let other = entry
                .fields
                .iter()
                .enumerate()
                .filter_map(|(index, (field, weight))| {
                    fuzzy_match(&word, field).map(|(s, at)| (s * weight, index, at))
                })
                .max_by(|a, b| a.0.total_cmp(&b.0));
            let title_wins = match (&title, &other) {
                (Some((title, _)), Some((other, _, _))) => title >= other,
                (Some(_), None) => true,
                (None, _) => false,
            };
            match (title, other) {
                (Some((_, at)), _) if title_wins => explanation.title.extend(at),
                (_, Some((_, index, at))) => {
                    if index < entry.synonyms && explanation.synonym.is_none() {
                        explanation.synonym = Some((index, at));
                    }
                }
                _ => return None,
            }
        }
        explanation.title.sort_unstable();
        explanation.title.dedup();
        Some(explanation)
    }
}

/// [`fuzzy_score`], with the matched positions of its best start. Kept
/// apart so the search itself allocates nothing per field; a test checks
/// that both agree.
fn fuzzy_match(word: &[char], field: &Field) -> Option<(f64, Vec<usize>)> {
    let (&first, rest) = word.split_first()?;
    let text = &field.chars;
    if word.len() > text.len() {
        return None;
    }
    let mut best: Option<(f64, Vec<usize>)> = None;
    for start in (0..text.len()).filter(|&i| text[i] == first) {
        let mut score = SCORE_MATCH + start_bonus(field, start);
        if start == 0 {
            score += BONUS_FIELD_START;
        }
        let mut positions = vec![start];
        let mut previous = start;
        let mut matched = true;
        let mut contiguous = field.word_start[start];
        for &c in rest {
            let Some(offset) = text[previous + 1..].iter().position(|&t| t == c) else {
                matched = false;
                break;
            };
            let position = previous + 1 + offset;
            score += SCORE_MATCH + start_bonus(field, position);
            if offset == 0 {
                score += BONUS_CONSECUTIVE;
            } else {
                contiguous = false;
                score -= (offset as f64 * PENALTY_GAP).min(MAX_GAP_PENALTY);
            }
            positions.push(position);
            previous = position;
        }
        if !matched {
            break;
        }
        if contiguous {
            score += BONUS_WORD_PREFIX;
        }
        if best.as_ref().is_none_or(|(b, _)| score > *b) {
            best = Some((score, positions));
        }
    }
    best
}

/// Total text score of an entry, or `None` if any query word doesn't match.
fn score_entry(entry: &Entry, words: &[(Vec<char>, u64)], whole: &[char]) -> Option<f64> {
    let mut total = 0.0;
    for (word, mask) in words {
        let score_in = |field: &Field, weight: f64| {
            if field.may_contain(*mask) {
                fuzzy_score(word, field).map(|s| s * weight)
            } else {
                None
            }
        };
        let best = entry
            .fields
            .iter()
            .filter_map(|(field, weight)| score_in(field, *weight))
            .chain(score_in(&entry.title, WEIGHT_TITLE))
            .fold(None, |best: Option<f64>, s| {
                Some(best.map_or(s, |b| b.max(s)))
            })?;
        total += best;
    }
    let title = &entry.title.chars;
    if title.as_slice() == whole {
        total += BONUS_EXACT_TITLE;
    } else if title.starts_with(whole) {
        total += BONUS_TITLE_PREFIX;
    }
    Some(total)
}

/// Fuzzy subsequence score of `word` in `field`, or `None` if `word` isn't
/// a subsequence. Tries every occurrence of the first character as a start
/// and keeps the best greedy match. Fields are short (titles, synonyms), so
/// this stays cheap.
fn fuzzy_score(word: &[char], field: &Field) -> Option<f64> {
    let (&first, rest) = word.split_first()?;
    let text = &field.chars;
    if word.len() > text.len() {
        return None;
    }
    let mut best: Option<f64> = None;
    for start in (0..text.len()).filter(|&i| text[i] == first) {
        let mut score = SCORE_MATCH + start_bonus(field, start);
        if start == 0 {
            score += BONUS_FIELD_START;
        }
        let mut previous = start;
        let mut matched = true;
        let mut contiguous = field.word_start[start];
        for &c in rest {
            let Some(offset) = text[previous + 1..].iter().position(|&t| t == c) else {
                matched = false;
                break;
            };
            let position = previous + 1 + offset;
            score += SCORE_MATCH + start_bonus(field, position);
            if offset == 0 {
                score += BONUS_CONSECUTIVE;
            } else {
                contiguous = false;
                score -= (offset as f64 * PENALTY_GAP).min(MAX_GAP_PENALTY);
            }
            previous = position;
        }
        if !matched {
            // Later starts only leave less text, so they can't match either.
            break;
        }
        if contiguous {
            score += BONUS_WORD_PREFIX;
        }
        best = Some(best.map_or(score, |b: f64| b.max(score)));
    }
    best
}

fn start_bonus(field: &Field, position: usize) -> f64 {
    if field.word_start[position] {
        BONUS_WORD_START
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index() -> SearchIndex {
        let mut index = SearchIndex::new();
        let commands: &[(&str, &str, &[&str], &str)] = &[
            ("text.bold", "Bold", &["fett", "strong"], "Text"),
            ("text.italic", "Italic", &["kursiv"], "Text"),
            ("table.insert", "Insert table", &["grid"], "Table"),
            (
                "table.insert_bold_row",
                "Insert bold header row",
                &[],
                "Table",
            ),
            (
                "arrange.align_center",
                "Align center",
                &["middle"],
                "Arrange",
            ),
            ("arrange.align_left", "Align left", &[], "Arrange"),
            (
                "board.insert_shape",
                "Insert shape",
                &["rectangle", "ellipse"],
                "Board",
            ),
            ("file.save", "Save", &["store"], "File"),
            ("file.save_as", "Save as…", &[], "File"),
            ("view.toggle_grid", "Show grid", &[], "View"),
        ];
        for (id, title, synonyms, category) in commands {
            index.insert(CommandId::new(id), title, synonyms, category);
        }
        index
    }

    fn top(index: &SearchIndex, query: &str) -> Vec<&'static str> {
        index
            .search(query, &UsageStats::new(), 0, 3)
            .into_iter()
            .map(|hit| hit.id.as_str())
            .collect()
    }

    #[test]
    fn exact_and_prefix_titles_rank_first() {
        let index = index();
        assert_eq!(top(&index, "bold")[0], "text.bold");
        assert_eq!(top(&index, "save")[0], "file.save");
        assert_eq!(top(&index, "save a")[0], "file.save_as");
        assert_eq!(top(&index, "Bo")[0], "text.bold", "case-insensitive");
    }

    #[test]
    fn multi_word_and_abbreviated_queries() {
        let index = index();
        assert_eq!(top(&index, "ins tab")[0], "table.insert");
        assert_eq!(top(&index, "al cen")[0], "arrange.align_center");
        assert_eq!(
            top(&index, "ac")[0],
            "arrange.align_center",
            "word-start initials"
        );
        assert_eq!(top(&index, "ins shp")[0], "board.insert_shape");
    }

    #[test]
    fn a_word_prefix_beats_a_scattered_match_over_word_starts() {
        // "tab" hits two word starts in "text box above" (t…a b), but is the
        // start of "table": the table wins.
        let mut index = SearchIndex::new();
        index.insert(
            CommandId::new("a.text_box"),
            "Insert text box above",
            &[],
            "View",
        );
        index.insert(CommandId::new("b.table"), "Insert table", &[], "Edit");
        assert_eq!(top(&index, "ins tab")[0], "b.table");
        assert_eq!(top(&index, "tab")[0], "b.table");
    }

    #[test]
    fn synonyms_ids_and_categories_match() {
        let index = index();
        assert_eq!(top(&index, "fett")[0], "text.bold", "synonym (German)");
        assert_eq!(top(&index, "kursiv")[0], "text.italic");
        assert_eq!(top(&index, "ellipse")[0], "board.insert_shape");
        assert_eq!(top(&index, "toggle_grid")[0], "view.toggle_grid", "id");
        assert!(
            top(&index, "arrange").contains(&"arrange.align_left"),
            "category"
        );
    }

    #[test]
    fn non_matching_queries_return_nothing() {
        let index = index();
        assert!(top(&index, "xyzzy").is_empty());
        assert!(top(&index, "bold zzz").is_empty(), "every word must match");
    }

    #[test]
    fn frecency_breaks_ties_but_not_clear_wins() {
        let index = index();
        let now = 1_000_000;
        // Without usage, "Align left" wins the tie (shorter title).
        assert_eq!(top(&index, "align")[0], "arrange.align_left");
        let mut usage = UsageStats::new();
        for _ in 0..20 {
            usage.record(CommandId::new("arrange.align_center"), now);
        }
        let hits = index.search("align", &usage, now, 2);
        assert_eq!(
            hits[0].id.as_str(),
            "arrange.align_center",
            "used one wins the tie"
        );

        // Heavy use of a weaker match doesn't beat an exact title match.
        for _ in 0..1_000 {
            usage.record(CommandId::new("table.insert_bold_row"), now);
        }
        assert_eq!(
            index.search("bold", &usage, now, 1)[0].id.as_str(),
            "text.bold"
        );
    }

    #[test]
    fn frecency_decays_and_is_bounded() {
        let id = CommandId::new("text.bold");
        let mut usage = UsageStats::new();
        assert_eq!(usage.boost(id, 0), 0.0);
        for _ in 0..1_000 {
            usage.record(id, 0);
        }
        let fresh = usage.boost(id, 0);
        let week_old = usage.boost(id, 7 * 86_400);
        assert!(fresh <= MAX_FRECENCY, "bounded: {fresh}");
        assert!(
            fresh > week_old && week_old > 0.0,
            "decays: {fresh} vs {week_old}"
        );

        // Recency matters even for heavily used commands, and frequency
        // matters among equally recent ones.
        let mut light = UsageStats::new();
        light.record(id, 0);
        assert!(usage.boost(id, 0) > light.boost(id, 0));
        assert!(
            light.boost(id, 0) > usage.boost(id, 30 * 86_400),
            "a fresh single use beats a month-old habit"
        );
    }

    #[test]
    fn empty_query_lists_frecent_commands() {
        let index = index();
        let mut usage = UsageStats::new();
        usage.record(CommandId::new("file.save"), 100);
        usage.record(CommandId::new("text.bold"), 50);
        let hits = index.search("  ", &usage, 100, 10);
        let ids: Vec<&str> = hits.iter().map(|h| h.id.as_str()).collect();
        assert_eq!(ids, ["file.save", "text.bold"]);
    }

    #[test]
    fn char_mask_never_rejects_a_real_match() {
        for (word, text) in [
            ("ab", "a b"),
            ("é", "café"),
            ("°", "rotate by 90°"),
            ("9", "f9"),
        ] {
            let field = Field::new(text);
            let word: Vec<char> = word.chars().collect();
            assert!(field.may_contain(char_mask(&word)), "{text}");
            assert!(fuzzy_score(&word, &field).is_some(), "{text}");
        }
        assert!(!Field::new("bold").may_contain(char_mask(&['z'])));
    }

    #[test]
    fn limit_keeps_the_best_hits_in_order() {
        let index = index();
        let all = index.search("i", &UsageStats::new(), 0, 100);
        let two = index.search("i", &UsageStats::new(), 0, 2);
        assert_eq!(&all[..2], &two[..]);
        assert!(index.search("i", &UsageStats::new(), 0, 0).is_empty());
    }

    #[test]
    fn ties_are_deterministic() {
        let index = index();
        assert_eq!(top(&index, "insert"), top(&index, "insert"));
        // "Insert shape" and "Insert table" score and measure the same, so
        // the id decides. The longer "Insert bold header row" comes last.
        assert_eq!(
            top(&index, "insert"),
            [
                "board.insert_shape",
                "table.insert",
                "table.insert_bold_row"
            ]
        );
    }

    #[test]
    fn explain_points_at_the_matched_title_letters_or_synonym() {
        let index = index();
        let bold = index.explain(CommandId::new("text.bold"), "bo").unwrap();
        assert_eq!(bold.title, vec![0, 1]);
        assert_eq!(bold.synonym, None);
        // "fett" is a synonym of Bold: the synonym is named, the title not.
        let fett = index.explain(CommandId::new("text.bold"), "fett").unwrap();
        assert!(fett.title.is_empty());
        assert_eq!(fett.synonym, Some((0, vec![0, 1, 2, 3])));
        let table = index
            .explain(CommandId::new("table.insert"), "ins tab")
            .unwrap();
        assert_eq!(table.title, vec![0, 1, 2, 7, 8, 9]);
        assert!(index.explain(CommandId::new("text.bold"), "zzz").is_none());
        assert!(index.explain(CommandId::new("no.such"), "bo").is_none());
    }

    #[test]
    fn explain_scores_as_the_search_does() {
        let words = ["al", "cen", "bo", "ins", "tb", "xyz", "save", "row", "ow"];
        let fields = [
            "Align center",
            "Insert bold header row",
            "Save as…",
            "table.insert",
        ];
        for field in fields {
            let field = Field::new(field);
            for word in words {
                let word: Vec<char> = word.chars().collect();
                assert_eq!(
                    fuzzy_score(&word, &field),
                    fuzzy_match(&word, &field).map(|(score, _)| score)
                );
            }
        }
    }
}
