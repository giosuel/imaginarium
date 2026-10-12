// SPDX-License-Identifier: AGPL-3.0-or-later

//! Palette search benchmark.
//!
//! Budget guarded here (PLAN §10, M2 gate): **searching 2,000 commands takes
//! < 1 ms per keystroke.** Every prefix of a typed query is measured,
//! because the palette searches on each keystroke.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use imaginarium_commands::{CommandId, SearchIndex, UsageStats};
use imaginarium_test_utils::rng::Rng;

/// Seed for the synthetic command set. Never change it.
const SEED: u64 = 0x00C0_33A2_D500_2000;
const COMMANDS: usize = 2_000;

const VERBS: &[&str] = &[
    "insert",
    "align",
    "toggle",
    "show",
    "hide",
    "export",
    "import",
    "convert",
    "duplicate",
    "group",
    "ungroup",
    "rotate",
    "flip",
    "lock",
    "unlock",
    "select",
    "copy",
    "paste",
    "format",
    "apply",
    "remove",
    "reset",
    "open",
    "close",
    "move",
    "resize",
    "distribute",
    "generate",
];
const OBJECTS: &[&str] = &[
    "table",
    "shape",
    "image",
    "text",
    "paragraph",
    "slide",
    "page",
    "layer",
    "connector",
    "style",
    "theme",
    "grid",
    "guide",
    "comment",
    "header",
    "footer",
    "chart",
    "background",
    "selection",
    "frame",
    "column",
    "row",
    "list",
    "link",
    "brush",
    "mask",
    "template",
    "version",
];
const QUALIFIERS: &[&str] = &[
    "",
    "",
    "",
    "left",
    "right",
    "center",
    "top",
    "bottom",
    "all",
    "horizontally",
    "vertically",
    "as pdf",
    "as svg",
    "to front",
    "to back",
    "with ai",
    "from clipboard",
    "by 90°",
];
const AREAS: &[&str] = &[
    "board", "text", "slides", "layout", "paint", "file", "view", "ai", "arrange",
];

/// Leaks the strings: the index needs `&'static str` ids, and the bench
/// builds its data once.
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// Builds the index and returns it with all command ids.
fn build_index() -> (SearchIndex, Vec<CommandId>) {
    let mut rng = Rng::new(SEED);
    let mut index = SearchIndex::new();
    let mut ids = Vec::with_capacity(COMMANDS);
    for i in 0..COMMANDS {
        let verb = *rng.pick(VERBS);
        let object = *rng.pick(OBJECTS);
        let qualifier = *rng.pick(QUALIFIERS);
        let area = *rng.pick(AREAS);
        let mut title = format!("{verb} {object}");
        if !qualifier.is_empty() {
            title.push(' ');
            title.push_str(qualifier);
        }
        let mut chars = title.chars();
        let title = match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => title,
        };
        let id = leak(format!("{area}.{verb}_{object}_{i}"));
        let synonym_a = leak(format!("{} {object}", rng.pick(VERBS)));
        let synonym_b = leak(format!("{object} {}", rng.pick(OBJECTS)));
        let synonyms: &'static [&'static str] =
            Box::leak(vec![synonym_a, synonym_b].into_boxed_slice());
        index.insert(CommandId::new(id), &title, synonyms, area);
        ids.push(CommandId::new(id));
    }
    (index, ids)
}

fn bench_search(c: &mut Criterion) {
    let (index, ids) = build_index();
    let mut usage = UsageStats::new();
    let mut rng = Rng::new(SEED ^ 1);
    // Usage history for 10 % of the commands, so frecency is exercised.
    for _ in 0..200 {
        usage.record(*rng.pick(&ids), 1_000);
    }

    let mut group = c.benchmark_group("commands/search_2000");
    let typed = "align center";
    let mut queries: Vec<String> = (1..=typed.len()).map(|n| typed[..n].to_owned()).collect();
    queries.extend(["ins tab", "xp pdf", "zzzz", ""].map(str::to_owned));
    for query in &queries {
        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{query:?}")),
            query,
            |b, q| {
                b.iter(|| black_box(index.search(black_box(q), &usage, 2_000, 50)));
            },
        );
    }
    group.finish();
}

criterion_group!(benches, bench_search);
criterion_main!(benches);
