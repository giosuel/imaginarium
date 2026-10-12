// SPDX-License-Identifier: AGPL-3.0-or-later

//! What each CRDT engine must do for the workloads. One implementation per
//! library, each mapping our schema (`docs/design/file-format.md` §3.2) as
//! closely as the library allows.

use imaginarium_test_utils::fixtures::{ParagraphSpec, ShapeKind, ShapeSpec};

/// A board: a node hierarchy where each node has a kind, a geometry map
/// (`x`, `y`, `w`, `h`, `r`), a fill and, for text shapes, a text container.
///
/// Loro stores the hierarchy in its movable tree. Yrs and Automerge have no
/// tree type, so they store what we would have to build ourselves: a map of
/// nodes, each with a parent id (`p`) and an order key (`z`).
pub trait Board: Sized {
    /// A version, for exporting the changes since it.
    type Version;
    /// The full document state, for convergence checks.
    type State: PartialEq + std::fmt::Debug;

    /// Builds a board in one transaction (like opening an import or a paste).
    fn create(peer: u64, shapes: &[ShapeSpec]) -> Self;
    /// Loads a saved snapshot and finds the node handles (like opening a file).
    fn load(peer: u64, bytes: &[u8]) -> Self;
    /// Saves a full snapshot (state and history).
    fn save(&mut self) -> Vec<u8>;
    /// Saves a snapshot without history, if the library can trim history.
    fn save_trimmed(&mut self) -> Option<Vec<u8>>;
    /// Number of nodes.
    fn len(&self) -> usize;
    /// Reads every node's geometry, as building the view model does on open.
    fn read_geometry(&self) -> f64;
    /// Moves node `index` (one transaction, like a drag step).
    fn move_shape(&mut self, index: usize, x: f64, y: f64);
    /// Adds a node at the end (one transaction).
    fn add_shape(&mut self, shape: &ShapeSpec);
    /// Makes node `child` a child of node `parent` (one transaction).
    fn reparent(&mut self, child: usize, parent: usize);
    /// Whether following the parent links of any node loops.
    fn has_cycle(&mut self) -> bool;
    /// The current version.
    fn version(&mut self) -> Self::Version;
    /// The changes since `version`, encoded for the wire (catch-up sync).
    fn updates_since(&mut self, version: &Self::Version) -> Vec<u8>;
    /// The update of the last local edit, the way the library sends live
    /// edits. By default the changes since `version` (the version before
    /// the edit). Yrs overrides it: its catch-up encoding repeats the whole
    /// delete set, so live edits use the transaction's own update.
    fn edit_update(&mut self, version: &Self::Version) -> Vec<u8> {
        self.updates_since(version)
    }
    /// Applies changes from another peer.
    fn apply(&mut self, update: &[u8]);
    /// The full state.
    fn state(&mut self) -> Self::State;
}

/// A text flow: one rich text with paragraph marks (heading level, list
/// item), as a document's body.
pub trait TextDoc: Sized {
    /// A version, for exporting the changes since it.
    type Version;
    /// The text, for convergence checks.
    type State: PartialEq + std::fmt::Debug;

    /// Builds the document in one transaction (like importing a DOCX).
    fn create(peer: u64, paragraphs: &[ParagraphSpec]) -> Self;
    /// Loads a saved snapshot.
    fn load(peer: u64, bytes: &[u8]) -> Self;
    /// Saves a full snapshot (state and history).
    fn save(&mut self) -> Vec<u8>;
    /// Saves a snapshot without history, if the library can trim history.
    fn save_trimmed(&mut self) -> Option<Vec<u8>>;
    /// Length in characters.
    fn len(&self) -> usize;
    /// The plain text, as layout reads it on open.
    fn text(&self) -> String;
    /// Inserts `s` at character `pos` (one transaction, like a keystroke).
    fn insert(&mut self, pos: usize, s: &str);
    /// The current version.
    fn version(&mut self) -> Self::Version;
    /// The changes since `version`, encoded for the wire (catch-up sync).
    fn updates_since(&mut self, version: &Self::Version) -> Vec<u8>;
    /// The update of the last local edit, the way the library sends live
    /// edits. By default the changes since `version` (the version before
    /// the edit). Yrs overrides it: its catch-up encoding repeats the whole
    /// delete set, so live edits use the transaction's own update.
    fn edit_update(&mut self, version: &Self::Version) -> Vec<u8> {
        self.updates_since(version)
    }
    /// Applies changes from another peer.
    fn apply(&mut self, update: &[u8]);
    /// The state.
    fn state(&mut self) -> Self::State;
}

/// What a library offers beyond the measured workloads.
pub struct Features {
    /// A tree type with moves that never create cycles.
    pub movable_tree: bool,
    /// Can save without history (shallow snapshot), and how.
    pub history_trimming: &'static str,
    /// Result of the collaboration-aware undo check: `Some(true)` if undo
    /// reverted only the local change, `None` if the library has no undo.
    pub collaborative_undo: Option<bool>,
}

/// The `k` value of a shape.
pub fn kind_name(kind: ShapeKind) -> &'static str {
    match kind {
        ShapeKind::Rectangle => "rect",
        ShapeKind::Ellipse => "ellipse",
        ShapeKind::Line => "line",
        ShapeKind::Text => "text",
    }
}

/// The map key of node `index` (Yrs, Automerge).
pub fn node_key(index: usize) -> String {
    format!("n{index:06}")
}

/// An order key that sorts like `index` (Yrs, Automerge).
pub fn order_key(index: usize) -> String {
    format!("a{index:08}")
}

/// Whether parent links (`parent[i]` is the index of node `i`'s parent, or
/// `None` for the root) contain a cycle.
pub fn links_have_cycle(parent: &[Option<usize>]) -> bool {
    parent.iter().enumerate().any(|(start, _)| {
        let mut node = start;
        for _ in 0..=parent.len() {
            match parent[node] {
                Some(next) => node = next,
                None => return false,
            }
        }
        true
    })
}
