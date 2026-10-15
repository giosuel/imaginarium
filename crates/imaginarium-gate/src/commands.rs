// SPDX-License-Identifier: AGPL-3.0-or-later

//! 2,000 synthetic commands for the palette, indexed by the real
//! `imaginarium-commands` search (criterion 3).

use imaginarium_commands::{CommandId, SearchHit, SearchIndex, UsageStats};

const VERBS: &[(&str, &[&str])] = &[
    ("Insert", &["add", "new"]),
    ("Delete", &["remove", "erase"]),
    ("Duplicate", &["clone", "copy"]),
    ("Align", &[]),
    ("Distribute", &["space evenly"]),
    ("Group", &["combine"]),
    ("Ungroup", &["split group"]),
    ("Lock", &["freeze"]),
    ("Unlock", &[]),
    ("Show", &["reveal"]),
    ("Hide", &["conceal"]),
    ("Export", &["save as"]),
    ("Import", &["load"]),
    ("Format", &["style"]),
    ("Rename", &[]),
    ("Move", &[]),
    ("Resize", &["scale"]),
    ("Rotate", &["turn"]),
    ("Flip", &["mirror"]),
    ("Zoom to", &["fit"]),
    ("Select", &[]),
    ("Convert", &["change into"]),
    ("Merge", &["join"]),
    ("Sort", &["order"]),
    ("Link", &["connect"]),
];

const OBJECTS: &[&str] = &[
    "table",
    "image",
    "shape",
    "text box",
    "sticky note",
    "connector",
    "frame",
    "slide",
    "page",
    "layer",
    "comment",
    "chart",
    "column",
    "row",
    "cell",
    "footnote",
    "header",
    "footer",
    "section",
    "bookmark",
];

const QUALIFIERS: &[&str] = &["", " left", " right", " above", " below"];

const CATEGORIES: &[&str] = &[
    "Edit", "Insert", "Arrange", "View", "Format", "File", "Table", "Slide", "Page", "Text",
];

/// One command of the palette.
pub(crate) struct Command {
    pub(crate) title: String,
    pub(crate) category: &'static str,
    pub(crate) shortcut: Option<String>,
}

/// The commands and their search index.
pub(crate) struct Commands {
    pub(crate) commands: Vec<Command>,
    ids: Vec<CommandId>,
    index: SearchIndex,
    usage: UsageStats,
}

impl Commands {
    /// `count` commands (titles from verbs × objects × qualifiers).
    pub(crate) fn new(count: usize) -> Self {
        let mut commands = Vec::with_capacity(count);
        let mut ids = Vec::with_capacity(count);
        let mut index = SearchIndex::new();
        'outer: for qualifier in QUALIFIERS {
            for (v, (verb, synonyms)) in VERBS.iter().enumerate() {
                for (o, object) in OBJECTS.iter().enumerate() {
                    let n = commands.len();
                    if n == count {
                        break 'outer;
                    }
                    let title = format!("{verb} {object}{qualifier}");
                    let category = CATEGORIES[(v + o) % CATEGORIES.len()];
                    // Ids must be 'static; the gate builds them once.
                    let id: &'static str = Box::leak(format!("gate.command_{n}").into_boxed_str());
                    let id = CommandId::new(id);
                    index.insert(id, &title, synonyms, category);
                    let shortcut = (n % 9 == 0).then(|| {
                        let letter = char::from(b'A' + (n / 9 % 26) as u8);
                        if n % 2 == 0 {
                            format!("Ctrl+{letter}")
                        } else {
                            format!("Ctrl+Shift+{letter}")
                        }
                    });
                    commands.push(Command {
                        title,
                        category,
                        shortcut,
                    });
                    ids.push(id);
                }
            }
        }
        Self {
            commands,
            ids,
            index,
            usage: UsageStats::new(),
        }
    }

    /// The number of commands.
    pub(crate) fn len(&self) -> usize {
        self.commands.len()
    }

    /// The best `limit` matches for `query`, as command indices.
    pub(crate) fn search(&self, query: &str, limit: usize) -> Vec<usize> {
        self.index
            .search(query, &self.usage, 0, limit)
            .into_iter()
            .filter_map(|SearchHit { id, .. }| id.as_str().rsplit('_').next()?.parse().ok())
            .collect()
    }

    /// Records that command `index` ran (recently used ones list first for
    /// an empty query).
    pub(crate) fn ran(&mut self, index: usize, now: u64) {
        if let Some(&id) = self.ids.get(index) {
            self.usage.record(id, now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_thousand_distinct_commands_are_searchable() {
        let commands = Commands::new(2_000);
        assert_eq!(commands.len(), 2_000);
        let mut titles: Vec<&str> = commands.commands.iter().map(|c| c.title.as_str()).collect();
        titles.sort_unstable();
        titles.dedup();
        assert_eq!(titles.len(), 2_000);
        let hits = commands.search("ins tab", 10);
        assert_eq!(commands.commands[hits[0]].title, "Insert table");
        // Synonyms count too.
        let hits = commands.search("remove image", 10);
        assert!(commands.commands[hits[0]].title.starts_with("Delete image"));
    }
}
