// SPDX-License-Identifier: AGPL-3.0-or-later

//! Deterministic sample text for the board's cards. Every few cards use
//! another script, so shaping, bidirectional text and font fallback get
//! exercised (CJK and emoji need system fonts; the bundled Inter covers
//! Latin, Greek and Cyrillic).

const TITLES: &[&str] = &[
    "Quarterly planning",
    "Design review",
    "Launch checklist",
    "Customer interviews",
    "Budget draft",
    "Team offsite",
    "Roadmap ideas",
    "Bug triage",
    "Hiring plan",
    "Research notes",
    "Release notes",
    "Brand refresh",
    "Onboarding flow",
    "Retrospective",
    "Pricing study",
    "Weekly sync",
];

const WORDS: &[&str] = &[
    "align",
    "on",
    "the",
    "scope",
    "before",
    "we",
    "commit",
    "draft",
    "share",
    "with",
    "design",
    "and",
    "ask",
    "for",
    "feedback",
    "by",
    "Friday",
    "collect",
    "open",
    "questions",
    "update",
    "timeline",
    "review",
    "numbers",
    "next",
    "steps",
    "owner",
    "follow",
    "up",
    "notes",
    "from",
    "call",
    "priorities",
    "for",
    "Q4",
    "risks",
    "dependencies",
    "metrics",
    "goals",
    "summary",
];

/// Cards in other scripts: a title and a body.
const WORLD: &[(&str, &str)] = &[
    ("会議のメモ", "来週までに資料を準備して、チームと共有する。"),
    ("ملاحظات الاجتماع", "مراجعة الخطة مع الفريق يوم الخميس."),
    ("סיכום פגישה", "לשלוח את הטיוטה לצוות עד יום שני."),
    ("회의록 정리", "다음 주까지 일정과 예산을 확인하기."),
    ("बैठक के नोट्स", "अगले सप्ताह तक योजना की समीक्षा करें।"),
    (
        "Launch 🚀 checklist ✅",
        "Ship it 🎉 then celebrate with the team 🍕",
    ),
    (
        "Заметки к встрече",
        "Обновить план и отправить его команде.",
    ),
    ("项目计划", "下周完成设计评审，然后开始开发。"),
    (
        "Σημειώσεις",
        "Ενημέρωση του χρονοδιαγράμματος έως την Παρασκευή.",
    ),
    (
        "Naïve café résumé",
        "Ünïcödé façade: Ærøskøbing, Œuvre, Straße.",
    ),
];

/// A small deterministic generator (SplitMix64), so every run shows the
/// same board.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// The title of card `index`.
pub(crate) fn title(index: usize) -> String {
    if index % 7 == 3 {
        return WORLD[(index / 7) % WORLD.len()].0.to_owned();
    }
    let base = TITLES[index % TITLES.len()];
    format!("{base} #{}", index + 1)
}

/// The body of card `index`: one or two sentences.
pub(crate) fn body(index: usize) -> String {
    if index % 7 == 3 {
        return WORLD[(index / 7) % WORLD.len()].1.to_owned();
    }
    let mut state = mix(index as u64);
    let words = 8 + (state % 9) as usize;
    let mut text = String::new();
    for i in 0..words {
        state = mix(state);
        let word = WORDS[(state % WORDS.len() as u64) as usize];
        if i == 0 {
            let mut chars = word.chars();
            if let Some(first) = chars.next() {
                text.extend(first.to_uppercase());
                text.push_str(chars.as_str());
            }
        } else {
            text.push(' ');
            text.push_str(word);
        }
    }
    text.push('.');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_is_deterministic_and_varied() {
        assert_eq!(body(5), body(5));
        assert_ne!(body(5), body(6));
        assert_eq!(title(0), "Quarterly planning #1");
        assert_eq!(title(3), "会議のメモ");
        assert!(body(0).ends_with('.'));
    }
}
