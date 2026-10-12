// SPDX-License-Identifier: AGPL-3.0-or-later

//! CRDT evaluation for M1.5 (ADR-0009): Loro vs Yrs vs Automerge on
//! Imaginarium's workloads, built from the `imaginarium-test-utils`
//! fixtures. Prints a Markdown report.
//!
//! ```text
//! cargo run --release -- [--runs N] [--quick] [--engine loro|yrs|automerge] [--out FILE]
//! ```
//!
//! Times are medians over `--runs` repetitions (whole-document operations)
//! or over all operations of one run (per-edit operations, with p99).
//! Memory is counted by the global allocator: *retained* is what the open
//! document holds, *peak* the high-water mark while opening it.

mod engine;
mod engine_automerge;
mod engine_loro;
mod engine_yrs;

use std::hint::black_box;
use std::time::{Duration, Instant};

use engine::{Board, Features, TextDoc};
use imaginarium_test_utils::fixtures::{BoardFixture, DocumentFixture};
use imaginarium_test_utils::rng::Rng;
use peak_alloc::PeakAlloc;

#[global_allocator]
static ALLOCATOR: PeakAlloc = PeakAlloc;

/// Seed of the edit sequences. Never change it.
const EDIT_SEED: u64 = 0x00C0_DA7A_0015_0001;
/// zstd level of `.imag` records (`imaginarium-format`).
const ZSTD_LEVEL: i32 = 3;

struct Config {
    runs: usize,
    board: BoardFixture,
    document: DocumentFixture,
    drags: usize,
    history_drags: usize,
    keystrokes: usize,
    concurrent: usize,
}

/// One measured value: section, metric, formatted value.
struct Entry {
    section: &'static str,
    metric: String,
    value: String,
}

struct Results(Vec<Entry>);

impl Results {
    fn add(&mut self, section: &'static str, metric: impl Into<String>, value: impl Into<String>) {
        self.0.push(Entry {
            section,
            metric: metric.into(),
            value: value.into(),
        });
    }
}

fn duration(d: Duration) -> String {
    let us = d.as_secs_f64() * 1e6;
    if us < 1_000.0 {
        format!("{us:.1} µs")
    } else if us < 1_000_000.0 {
        format!("{:.2} ms", us / 1_000.0)
    } else {
        format!("{:.2} s", us / 1_000_000.0)
    }
}

fn bytes(n: usize) -> String {
    if n < 10 * 1024 {
        format!("{n} B")
    } else if n < 10 * 1024 * 1024 {
        format!("{:.1} KiB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", n as f64 / (1024.0 * 1024.0))
    }
}

fn zstd_len(data: &[u8]) -> usize {
    zstd::bulk::compress(data, ZSTD_LEVEL)
        .map(|c| c.len())
        .unwrap_or(0)
}

/// Median wall time of `runs` calls. Results are dropped outside the timing.
fn median_time<R>(runs: usize, mut f: impl FnMut() -> R) -> Duration {
    let mut times: Vec<Duration> = (0..runs.max(1))
        .map(|_| {
            let start = Instant::now();
            let result = f();
            let elapsed = start.elapsed();
            drop(black_box(result));
            elapsed
        })
        .collect();
    times.sort_unstable();
    times[times.len() / 2]
}

/// Retained and peak heap bytes of building `f`'s result.
fn memory<T>(f: impl FnOnce() -> T) -> (T, usize, usize) {
    let base = ALLOCATOR.current_usage();
    ALLOCATOR.reset_peak_usage();
    let value = f();
    let retained = ALLOCATOR.current_usage().saturating_sub(base);
    let peak = ALLOCATOR.peak_usage().saturating_sub(base);
    (value, retained, peak)
}

fn percentile<T: Ord + Copy>(values: &mut [T], p: f64) -> T {
    values.sort_unstable();
    let index = ((values.len() as f64 - 1.0) * p).round() as usize;
    values[index]
}

fn per_op(values: &mut [Duration]) -> String {
    let median = percentile(values, 0.5);
    let p99 = percentile(values, 0.99);
    format!("{} (p99 {})", duration(median), duration(p99))
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "**no**" }
}

/// Progress on stderr, so a slow section is visible while it runs.
fn progress(what: &str) {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    let start = START.get_or_init(Instant::now);
    eprintln!("[{:>7.1} s] {what}", start.elapsed().as_secs_f64());
}

fn board_suite<B: Board>(cfg: &Config, out: &mut Results) {
    const OPEN: &str = "Board: open and save";
    const EDIT: &str = "Board: live edits";
    const HISTORY: &str = "Board: after a long editing session";
    const CONCURRENT: &str = "Board: concurrent editing";

    progress("board: open and save");
    let shapes: Vec<_> = cfg.board.shapes().collect();
    let n = shapes.len();

    let build = median_time(cfg.runs, || B::create(1, &shapes));
    out.add(
        OPEN,
        format!("Build {n} shapes in one transaction"),
        duration(build),
    );
    let (mut board, retained, _) = memory(|| B::create(1, &shapes));
    out.add(OPEN, "Memory after building", bytes(retained));
    let snapshot = board.save();
    let save = median_time(cfg.runs, || board.save());
    out.add(OPEN, "Save snapshot", duration(save));
    out.add(
        OPEN,
        "Snapshot size (raw / zstd)",
        format!("{} / {}", bytes(snapshot.len()), bytes(zstd_len(&snapshot))),
    );
    drop(board);

    let load = median_time(cfg.runs, || B::load(2, &snapshot));
    out.add(OPEN, "Load snapshot", duration(load));
    let open = median_time(cfg.runs, || {
        let board = B::load(2, &snapshot);
        black_box(board.read_geometry());
        board
    });
    out.add(
        OPEN,
        "Load snapshot and read every geometry",
        duration(open),
    );
    let (opened, retained, peak) = memory(|| {
        let board = B::load(2, &snapshot);
        black_box(board.read_geometry());
        board
    });
    assert_eq!(opened.len(), n);
    out.add(
        OPEN,
        "Memory after opening (retained / peak)",
        format!("{} / {}", bytes(retained), bytes(peak)),
    );
    drop(opened);

    progress("board: live edits");
    // Live edits: one drag step per transaction, sent to a second peer.
    let mut a = B::load(1, &snapshot);
    let mut b = B::load(2, &snapshot);
    let mut rng = Rng::new(EDIT_SEED);
    let (mut local, mut export, mut remote) = (Vec::new(), Vec::new(), Vec::new());
    let mut sizes = Vec::new();
    for _ in 0..cfg.drags {
        let index = rng.below(n as u64) as usize;
        let (x, y) = (rng.range_f64(0.0, 20_000.0), rng.range_f64(0.0, 20_000.0));
        let version = a.version();
        let start = Instant::now();
        a.move_shape(index, x, y);
        local.push(start.elapsed());
        let start = Instant::now();
        let update = a.edit_update(&version);
        export.push(start.elapsed());
        sizes.push(update.len());
        let start = Instant::now();
        b.apply(&update);
        remote.push(start.elapsed());
    }
    let mut total: Vec<Duration> = local.iter().zip(&export).map(|(l, e)| *l + *e).collect();
    out.add(
        EDIT,
        format!("Local edit including its update ({} drag steps)", cfg.drags),
        per_op(&mut total),
    );
    out.add(EDIT, "… of which the edit", per_op(&mut local));
    out.add(EDIT, "… of which encoding the update", per_op(&mut export));
    out.add(
        EDIT,
        "Update size (median / p99)",
        format!(
            "{} / {}",
            bytes(percentile(&mut sizes, 0.5)),
            bytes(percentile(&mut sizes, 0.99))
        ),
    );
    out.add(EDIT, "Apply a remote update", per_op(&mut remote));
    out.add(EDIT, "Peers converged", yes_no(a.state() == b.state()));
    drop(b);

    progress("board: long session");
    // A long session: many more drags, then save and reopen.
    for _ in cfg.drags..cfg.history_drags {
        let index = rng.below(n as u64) as usize;
        a.move_shape(
            index,
            rng.range_f64(0.0, 20_000.0),
            rng.range_f64(0.0, 20_000.0),
        );
    }
    let long = a.save();
    out.add(
        HISTORY,
        format!(
            "Snapshot size after {} drags (raw / zstd)",
            cfg.history_drags
        ),
        format!("{} / {}", bytes(long.len()), bytes(zstd_len(&long))),
    );
    match a.save_trimmed() {
        Some(trimmed) => out.add(
            HISTORY,
            "Snapshot without history (raw / zstd)",
            format!("{} / {}", bytes(trimmed.len()), bytes(zstd_len(&trimmed))),
        ),
        None => out.add(
            HISTORY,
            "Snapshot without history (raw / zstd)",
            "not supported",
        ),
    }
    drop(a);
    let open = median_time(cfg.runs, || {
        let board = B::load(2, &long);
        black_box(board.read_geometry());
        board
    });
    out.add(HISTORY, "Load it and read every geometry", duration(open));
    let (opened, retained, _) = memory(|| {
        let board = B::load(2, &long);
        black_box(board.read_geometry());
        board
    });
    out.add(HISTORY, "Memory after opening", bytes(retained));
    drop(opened);

    progress("board: concurrent");
    // Concurrent: overlapping moves and new shapes, merged both ways.
    let mut a = B::load(3, &snapshot);
    let mut b = B::load(4, &snapshot);
    let (va, vb) = (a.version(), b.version());
    let half = cfg.concurrent / 2;
    for i in 0..cfg.concurrent {
        a.move_shape(i % n, 1.0, i as f64);
        b.move_shape((i + half) % n, 2.0, i as f64);
    }
    for shape in shapes.iter().take(cfg.concurrent / 5) {
        b.add_shape(shape);
    }
    let (ua, ub) = (a.updates_since(&va), b.updates_since(&vb));
    let start = Instant::now();
    a.apply(&ub);
    let merge = start.elapsed();
    b.apply(&ua);
    out.add(
        CONCURRENT,
        format!(
            "Merge a peer's offline session ({} moves, {} new shapes)",
            cfg.concurrent,
            cfg.concurrent / 5
        ),
        format!("{} ({})", duration(merge), bytes(ub.len())),
    );
    out.add(
        CONCURRENT,
        "Peers converged",
        yes_no(a.state() == b.state()),
    );
    drop((a, b));

    let mut a = B::load(5, &snapshot);
    let mut b = B::load(6, &snapshot);
    let (va, vb) = (a.version(), b.version());
    a.reparent(0, 1);
    b.reparent(1, 0);
    let (ua, ub) = (a.updates_since(&va), b.updates_since(&vb));
    a.apply(&ub);
    b.apply(&ua);
    out.add(
        CONCURRENT,
        "No cycle after concurrent moves (A into B, B into A)",
        yes_no(!a.has_cycle() && !b.has_cycle()),
    );
}

fn text_suite<T: TextDoc>(cfg: &Config, out: &mut Results) {
    const OPEN: &str = "Document: open and save";
    const TYPING: &str = "Document: typing";
    const CONCURRENT: &str = "Document: concurrent typing";

    progress("document: open and save");
    let paragraphs: Vec<_> = cfg.document.paragraphs().collect();
    let chars: usize = paragraphs.iter().map(|p| p.text.chars().count() + 1).sum();

    let build = median_time(cfg.runs, || T::create(1, &paragraphs));
    out.add(
        OPEN,
        format!(
            "Build {} paragraphs ({:.0}k chars) in one transaction",
            paragraphs.len(),
            chars as f64 / 1000.0
        ),
        duration(build),
    );
    let (mut doc, retained, _) = memory(|| T::create(1, &paragraphs));
    assert_eq!(doc.len(), chars);
    out.add(OPEN, "Memory after building", bytes(retained));
    let snapshot = doc.save();
    let save = median_time(cfg.runs, || doc.save());
    out.add(OPEN, "Save snapshot", duration(save));
    out.add(
        OPEN,
        "Snapshot size (raw / zstd)",
        format!("{} / {}", bytes(snapshot.len()), bytes(zstd_len(&snapshot))),
    );
    drop(doc);
    let load = median_time(cfg.runs, || T::load(2, &snapshot));
    out.add(OPEN, "Load snapshot", duration(load));
    let open = median_time(cfg.runs, || {
        let doc = T::load(2, &snapshot);
        black_box(doc.text().len());
        doc
    });
    out.add(OPEN, "Load snapshot and read the text", duration(open));
    let (opened, retained, peak) = memory(|| {
        let doc = T::load(2, &snapshot);
        black_box(doc.text().len());
        doc
    });
    out.add(
        OPEN,
        "Memory after opening (retained / peak)",
        format!("{} / {}", bytes(retained), bytes(peak)),
    );
    drop(opened);

    progress("document: typing");
    // Typing: one keystroke per transaction; the cursor jumps every 100 keys.
    let mut a = T::load(1, &snapshot);
    let mut b = T::load(2, &snapshot);
    let mut rng = Rng::new(EDIT_SEED);
    let mut pos = chars / 2;
    let (mut local, mut export, mut remote) = (Vec::new(), Vec::new(), Vec::new());
    let mut sizes = Vec::new();
    let letters: Vec<char> = "the quick brown fox jumps over the lazy dog "
        .chars()
        .collect();
    for i in 0..cfg.keystrokes {
        if i % 100 == 0 && i > 0 {
            pos = rng.below(a.len() as u64) as usize;
        }
        let key = letters[i % letters.len()].to_string();
        let version = a.version();
        let start = Instant::now();
        a.insert(pos, &key);
        local.push(start.elapsed());
        let start = Instant::now();
        let update = a.edit_update(&version);
        export.push(start.elapsed());
        sizes.push(update.len());
        let start = Instant::now();
        b.apply(&update);
        remote.push(start.elapsed());
        pos += 1;
    }
    let mut total: Vec<Duration> = local.iter().zip(&export).map(|(l, e)| *l + *e).collect();
    out.add(
        TYPING,
        format!(
            "Local keystroke including its update ({} keys)",
            cfg.keystrokes
        ),
        per_op(&mut total),
    );
    out.add(TYPING, "… of which the edit", per_op(&mut local));
    out.add(
        TYPING,
        "… of which encoding the update",
        per_op(&mut export),
    );
    out.add(
        TYPING,
        "Update size (median / p99)",
        format!(
            "{} / {}",
            bytes(percentile(&mut sizes, 0.5)),
            bytes(percentile(&mut sizes, 0.99))
        ),
    );
    out.add(TYPING, "Apply a remote update", per_op(&mut remote));
    out.add(TYPING, "Peers converged", yes_no(a.state() == b.state()));
    let after = a.save();
    out.add(
        TYPING,
        "Snapshot growth per keystroke (raw / zstd)",
        format!(
            "{:.1} B / {:.1} B",
            (after.len() as f64 - snapshot.len() as f64) / cfg.keystrokes as f64,
            (zstd_len(&after) as f64 - zstd_len(&snapshot) as f64) / cfg.keystrokes as f64
        ),
    );
    if let Some(trimmed) = a.save_trimmed() {
        out.add(
            TYPING,
            "Snapshot without history (raw / zstd)",
            format!("{} / {}", bytes(trimmed.len()), bytes(zstd_len(&trimmed))),
        );
    } else {
        out.add(
            TYPING,
            "Snapshot without history (raw / zstd)",
            "not supported",
        );
    }
    drop((a, b));

    progress("document: concurrent");
    // Concurrent typing at the same spot: merged runs must not interleave.
    let mut a = T::load(3, &snapshot);
    let mut b = T::load(4, &snapshot);
    let (va, vb) = (a.version(), b.version());
    let at = chars / 3;
    let upper: String = (0..cfg.concurrent)
        .map(|i| char::from(b'A' + (i % 26) as u8))
        .collect();
    let lower = upper.to_lowercase();
    for (i, (u, l)) in upper.chars().zip(lower.chars()).enumerate() {
        a.insert(at + i, &u.to_string());
        b.insert(at + i, &l.to_string());
    }
    let (ua, ub) = (a.updates_since(&va), b.updates_since(&vb));
    let start = Instant::now();
    a.apply(&ub);
    let merge = start.elapsed();
    b.apply(&ua);
    out.add(
        CONCURRENT,
        format!("Merge a peer's offline typing ({} keys)", cfg.concurrent),
        format!("{} ({})", duration(merge), bytes(ub.len())),
    );
    out.add(
        CONCURRENT,
        "Peers converged",
        yes_no(a.state() == b.state()),
    );
    let text = a.text();
    out.add(
        CONCURRENT,
        "Both runs stay contiguous (no interleaving)",
        yes_no(text.contains(&upper) && text.contains(&lower)),
    );
}

fn features_suite(features: Features, out: &mut Results) {
    const SECTION: &str = "Features";
    out.add(
        SECTION,
        "Movable tree (no cycles from concurrent moves)",
        yes_no(features.movable_tree),
    );
    out.add(SECTION, "History trimming", features.history_trimming);
    out.add(
        SECTION,
        "Collaboration-aware undo (undoes only the local change)",
        match features.collaborative_undo {
            Some(ok) => yes_no(ok).to_owned(),
            None => "not available".to_owned(),
        },
    );
}

fn run_engine<B: Board, T: TextDoc>(cfg: &Config, features: fn() -> Features) -> Results {
    let mut out = Results(Vec::new());
    features_suite(features(), &mut out);
    board_suite::<B>(cfg, &mut out);
    text_suite::<T>(cfg, &mut out);
    out
}

fn machine() -> String {
    let cpu = std::fs::read_to_string("/proc/cpuinfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("model name"))
                .map(|l| l.split(':').nth(1).unwrap_or("").trim().to_owned())
        })
        .unwrap_or_else(|| "unknown CPU".into());
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(0);
    format!("{cpu}, {cores} threads available")
}

fn load_average() -> String {
    std::fs::read_to_string("/proc/loadavg")
        .ok()
        .and_then(|s| s.split_whitespace().next().map(str::to_owned))
        .unwrap_or_else(|| "?".into())
}

fn report(
    cfg: &Config,
    columns: &[(&str, Results)],
    started_load: &str,
    elapsed: Duration,
) -> String {
    let mut md = String::new();
    md.push_str("# CRDT benchmark results\n\n");
    md.push_str(&format!(
        "- Machine: {}. Load average {} at the start, {} at the end. Total {}.\n",
        machine(),
        started_load,
        load_average(),
        duration(elapsed)
    ));
    md.push_str(&format!(
        "- Fixtures: board {} shapes, document {} pages. {} runs per whole-document measurement.\n",
        cfg.board.shapes, cfg.document.pages, cfg.runs
    ));
    md.push_str(
        "- Times: medians; per-edit rows show the median (and p99) over every edit of one run.\n- Yrs encodes a live edit's update inside its transaction (`TransactionMut::encode_update_v2`, the way Yjs sends live edits), so for Yrs that cost is part of \"the edit\".\n\n",
    );
    let first = &columns[0].1;
    let mut sections: Vec<&'static str> = Vec::new();
    for entry in &first.0 {
        if !sections.contains(&entry.section) {
            sections.push(entry.section);
        }
    }
    for section in sections {
        md.push_str(&format!("## {section}\n\n| Metric |"));
        for (name, _) in columns {
            md.push_str(&format!(" {name} |"));
        }
        md.push_str("\n|---|");
        md.push_str(&"---|".repeat(columns.len()));
        md.push('\n');
        for (row, entry) in first
            .0
            .iter()
            .enumerate()
            .filter(|(_, e)| e.section == section)
        {
            md.push_str(&format!("| {} |", entry.metric));
            for (_, results) in columns {
                md.push_str(&format!(
                    " {} |",
                    results.0.get(row).map_or("", |e| e.value.as_str())
                ));
            }
            md.push('\n');
        }
        md.push('\n');
    }
    md
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("crdt-bench [--runs N] [--quick] [--engine loro|yrs|automerge] [--out FILE]");
        return;
    }
    let quick = args.iter().any(|a| a == "--quick");
    let cfg = Config {
        runs: flag("--runs")
            .and_then(|r| r.parse().ok())
            .unwrap_or(if quick { 1 } else { 5 }),
        board: if quick {
            BoardFixture::SMALL
        } else {
            BoardFixture::LARGE
        },
        document: if quick {
            DocumentFixture::SMALL
        } else {
            DocumentFixture::LARGE
        },
        drags: if quick { 50 } else { 1_000 },
        history_drags: if quick { 200 } else { 10_000 },
        keystrokes: if quick { 200 } else { 2_000 },
        concurrent: if quick { 20 } else { 500 },
    };
    let only = flag("--engine");
    let wanted = |name: &str| only.as_deref().is_none_or(|o| o == name);
    let started_load = load_average();
    let started = Instant::now();

    let mut columns: Vec<(&str, Results)> = Vec::new();
    if wanted("loro") {
        progress("== loro");
        columns.push((
            "Loro 1.16.2",
            run_engine::<engine_loro::LoroBoard, engine_loro::LoroTextDoc>(
                &cfg,
                engine_loro::features,
            ),
        ));
    }
    if wanted("yrs") {
        progress("== yrs");
        columns.push((
            "Yrs 0.28.0",
            run_engine::<engine_yrs::YrsBoard, engine_yrs::YrsTextDoc>(&cfg, engine_yrs::features),
        ));
    }
    if wanted("automerge") {
        progress("== automerge");
        columns.push((
            "Automerge 0.12.0",
            run_engine::<engine_automerge::AutomergeBoard, engine_automerge::AutomergeTextDoc>(
                &cfg,
                engine_automerge::features,
            ),
        ));
    }
    if columns.is_empty() {
        eprintln!("no engine selected");
        std::process::exit(2);
    }
    let md = report(&cfg, &columns, &started_load, started.elapsed());
    match flag("--out") {
        Some(path) => {
            std::fs::write(&path, &md).unwrap_or_else(|err| panic!("can't write {path}: {err}"))
        }
        None => print!("{md}"),
    }
}
