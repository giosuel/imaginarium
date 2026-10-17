// SPDX-License-Identifier: AGPL-3.0-or-later

//! Nodes to canvas items (`canvas.md` §2): one item per visible node that
//! paints something, with its world transform, z key, look and bounds.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use imaginarium_model::view::NodeView;
use imaginarium_model::{ChangeSet, DocumentView, NodeId, NodeKind, Parent, StyleId};
use imaginarium_render::canvas::ItemId;
use imaginarium_render::kurbo::{Affine, Point, Rect, Shape, Size, Vec2};
use imaginarium_render::peniko::Color;
use imaginarium_text::{Align, Fonts, TextBlock, TextStyle};
use imaginarium_ui::ThemeKind;

use crate::shapes::{Outline, shape_path};
use crate::style::{self, ShapeStyle, TextLook};

/// The gap between neighbouring z keys after a full walk, so a node
/// inserted between two others gets a key between theirs: 32 halvings
/// before a re-key, and room for far more items than a board holds.
pub(crate) const Z_GAP: u64 = 1 << 32;

/// The keys of a full walk start here, so the space below the first
/// item (where nodes sent to the back go) halves 60 times.
pub(crate) const Z_START: u64 = 1 << 60;

/// The space between a shape's edge and its label, in points.
const LABEL_PADDING: f64 = 8.0;

/// The colors a board takes from its theme: frames without a style of
/// their own, and text without a color.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoardColors {
    /// Which theme, for the palette's versions.
    pub theme: ThemeKind,
    /// Text without a color of its own.
    pub text: Color,
    /// A frame's background.
    pub frame_bg: Color,
    /// A frame's border.
    pub frame_border: Color,
}

impl BoardColors {
    /// The colors of `theme` (the mockups' board colors, `shell.md` §7).
    pub fn of(theme: &imaginarium_ui::Theme) -> Self {
        let kind = theme.kind();
        let (frame_bg, frame_border) = match kind {
            ThemeKind::Dark => (
                Color::from_rgb8(0x1A, 0x1A, 0x1E),
                Color::from_rgb8(0x33, 0x33, 0x3B),
            ),
            ThemeKind::Light => (
                Color::from_rgb8(0xFF, 0xFF, 0xFF),
                Color::from_rgb8(0xD9, 0xD9, 0xDE),
            ),
        };
        Self {
            theme: kind,
            text: theme.color(imaginarium_ui::Token::Text),
            frame_bg,
            frame_border,
        }
    }
}

/// A frame around an item: its children are clipped to its box.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Clip {
    /// The frame's box space to document space.
    pub world: Affine,
    /// The frame's box, in its box space.
    pub rect: Rect,
    /// Its corner radius: children are clipped to the rounded box.
    pub radius: f64,
}

/// Laid-out text in an item, at `origin` in box space.
#[derive(Clone)]
pub(crate) struct ItemText {
    pub block: Rc<TextBlock>,
    /// Lines are centered within `extent` (labels), not at its start.
    pub centered: bool,
    pub origin: Point,
    /// The area its lines line up in.
    pub extent: Size,
    pub look: TextLook,
}

/// What an item paints.
#[derive(Clone)]
pub(crate) enum Look {
    /// A shape: its outline, style and label.
    Shape {
        outline: Rc<Outline>,
        style: ShapeStyle,
        label: Option<ItemText>,
    },
    /// A text box.
    Text {
        text: Option<ItemText>,
        opacity: f32,
    },
    /// A frame's background and border (its children are items of their
    /// own, clipped to it).
    Frame { style: ShapeStyle },
    /// A kind this version doesn't draw yet: a dashed box (schema rule 4).
    Placeholder,
}

/// A node on the canvas.
#[derive(Clone)]
pub(crate) struct Item {
    pub id: ItemId,
    pub z: u64,
    /// Box space (`(0, 0)` to `size`) to document space.
    pub world: Affine,
    pub size: Size,
    pub look: Look,
    /// What its pixels can touch, in document space.
    pub bounds: Rect,
    /// The frames it is inside, outermost first.
    pub clips: Rc<[Clip]>,
    /// The frame's name, for frames (drawn on top of the tiles).
    pub name: Option<String>,
}

/// Where a visible node sits: its box space, and the clips its children
/// are under. Kept for every visible node, groups included, so a subtree
/// can be built again on its own.
#[derive(Clone)]
pub(crate) struct Place {
    /// Box space to document space.
    pub world: Affine,
    /// The clips of its children: its own clips, and itself for a frame.
    pub inner: Rc<[Clip]>,
}

/// A laid-out text and what it was laid out from, so building an item
/// again (a moved shape) doesn't lay its text out again.
struct CachedText {
    key: TextKey,
    block: Rc<TextBlock>,
}

#[derive(Clone, PartialEq)]
struct TextKey {
    text: String,
    look: TextLook,
    width: f32,
    align: Align,
    line_height: f32,
}

/// What an update changed on the canvas.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Update {
    /// Nodes whose items are new or changed (their z, bounds or look).
    pub upserted: Vec<NodeId>,
    /// Nodes whose items only got new keys in the same order: nothing to
    /// redraw.
    pub rekeyed: Vec<NodeId>,
    /// Items that are gone.
    pub removed: Vec<ItemId>,
}

/// Every item of a board, by node.
#[derive(Default)]
pub(crate) struct Items {
    pub by_node: HashMap<NodeId, Item>,
    pub by_item: HashMap<ItemId, NodeId>,
    /// The item id of every node that had an item, kept while the node
    /// lives, so it keeps its id when it is built again.
    ids: HashMap<NodeId, ItemId>,
    places: HashMap<NodeId, Place>,
    texts: HashMap<NodeId, CachedText>,
    /// The frames among the items (their names are drawn on top).
    pub frames: HashSet<NodeId>,
    /// The items by the style they use, for style changes.
    by_style: HashMap<StyleId, HashSet<NodeId>>,
    /// Item ids are handed out once and never reused.
    next_item: u64,
}

/// The transform from a node's box to its parent's space: to its corner,
/// then rotated around its center (schema-v1.md §5.2).
pub(crate) fn local_transform(x: f64, y: f64, w: f64, h: f64, degrees: f64) -> Affine {
    let mut local = Affine::translate(Vec2::new(x, y));
    if degrees != 0.0 && degrees.is_finite() {
        local *= Affine::rotate_about(degrees.to_radians(), Point::new(w / 2.0, h / 2.0));
    }
    local
}

impl Items {
    /// The item id of `node`, kept for the node's life on this canvas.
    fn item_id(&mut self, node: NodeId) -> ItemId {
        if let Some(id) = self.ids.get(&node) {
            return *id;
        }
        self.next_item += 1;
        let id = ItemId(self.next_item);
        self.ids.insert(node, id);
        self.by_item.insert(id, node);
        id
    }

    /// Builds every item of `view`, in z order with `Z_GAP` between keys.
    #[cfg(test)]
    pub(crate) fn build(view: &DocumentView, fonts: &mut Fonts, colors: &BoardColors) -> Self {
        let mut items = Self::default();
        items.rebuild(view, fonts, colors);
        items
    }

    /// Builds every item again (a new theme), keeping the item ids and the
    /// text layouts that still fit.
    pub(crate) fn rebuild(&mut self, view: &DocumentView, fonts: &mut Fonts, colors: &BoardColors) {
        self.by_node.clear();
        self.places.clear();
        self.frames.clear();
        self.by_style.clear();
        self.ids.retain(|node, _| view.node(*node).is_some());
        let ids = &self.ids;
        self.by_item.retain(|_, node| ids.contains_key(node));
        self.texts.retain(|node, _| view.node(*node).is_some());
        let mut z = Z_START;
        let mut walker = Walker {
            view,
            fonts,
            colors,
            items: self,
            z: Some(&mut z),
            built: Vec::new(),
        };
        walker.children(Parent::Root, Affine::IDENTITY, &Rc::from([]));
    }

    /// Where a node paints in the stacking order: its first item's key
    /// (a group's first descendant's).
    pub(crate) fn order_key(&self, view: &DocumentView, node: NodeId) -> Option<u64> {
        self.first_key(view, node, &HashSet::new())
    }

    /// A visible node's box space to document space (groups too).
    pub(crate) fn world_of(&self, node: NodeId) -> Option<Affine> {
        self.places.get(&node).map(|place| place.world)
    }

    /// Whether a node is visible (it, and every ancestor, isn't hidden).
    pub(crate) fn is_visible(&self, node: NodeId) -> bool {
        self.places.contains_key(&node)
    }

    /// A node's parent's box space to document space (the board's for
    /// root nodes); `None` when the parent isn't visible.
    pub(crate) fn parent_world(&self, view: &DocumentView, node: NodeId) -> Option<Affine> {
        self.parent_place(view, node).map(|place| place.world)
    }

    /// The place of a node's parent: the board's for root nodes, `None`
    /// when the parent isn't visible.
    fn parent_place(&self, view: &DocumentView, node: NodeId) -> Option<Place> {
        match view.node(node)?.parent() {
            Parent::Root => Some(Place {
                world: Affine::IDENTITY,
                inner: Rc::from([]),
            }),
            Parent::Node(parent) => self.places.get(&parent).cloned(),
        }
    }

    /// Drops a node's item and place. `deleted`: the node is gone from the
    /// document, so its id and text go too.
    fn drop_node(&mut self, node: NodeId, deleted: bool, removed: &mut Vec<ItemId>) {
        if let Some(item) = self.by_node.remove(&node) {
            removed.push(item.id);
        }
        self.frames.remove(&node);
        for nodes in self.by_style.values_mut() {
            nodes.remove(&node);
        }
        self.places.remove(&node);
        if deleted {
            if let Some(id) = self.ids.remove(&node) {
                self.by_item.remove(&id);
            }
            self.texts.remove(&node);
        }
    }

    /// Brings the items up to date with `changes`, which `view` already
    /// contains (`canvas.md` §3): only what the changes name is built
    /// again. Several change sets waiting together must be merged first
    /// ([`merge`]): each is applied against the latest view.
    pub(crate) fn apply(
        &mut self,
        view: &DocumentView,
        changes: &ChangeSet,
        fonts: &mut Fonts,
        colors: &BoardColors,
    ) -> Update {
        let mut removed = Vec::new();
        for node in &changes.deleted {
            self.drop_node(*node, true, &mut removed);
        }
        // Subtrees to build again (structure, boxes, visibility), and nodes
        // whose own look changed.
        let structural: HashSet<NodeId> = changes
            .created
            .iter()
            .chain(&changes.moved)
            .copied()
            .collect();
        let mut subtrees: HashSet<NodeId> = structural.clone();
        let mut singles: HashSet<NodeId> = HashSet::new();
        for (node, what) in &changes.changed {
            if what.geometry || what.kind || (what.props && self.visibility_flips(view, *node)) {
                subtrees.insert(*node);
            } else if what.style || what.text || what.props {
                singles.insert(*node);
            }
        }
        if !changes.styles.is_empty() {
            for style in affected_styles(view, &changes.styles) {
                if let Some(nodes) = self.by_style.get(&style) {
                    singles.extend(nodes.iter().copied());
                }
            }
        }
        let under =
            |node: NodeId, set: &HashSet<NodeId>| ancestors(view, node).any(|up| set.contains(&up));
        let roots: Vec<NodeId> = subtrees
            .iter()
            .copied()
            .filter(|node| view.node(*node).is_some() && !under(*node, &subtrees))
            .collect();
        singles.retain(|node| !subtrees.contains(node) && !under(*node, &subtrees));

        // Keys: kept where the order held; the others go between their
        // neighbours below.
        let mut keep: HashMap<NodeId, u64> = HashMap::new();
        let mut built = Vec::new();
        let mut walks = Vec::new();
        for root in roots {
            // A node keeps its key unless it, or an ancestor in the
            // subtree, was created or moved (a node moved into a frame
            // whose box changed is under that frame's subtree too).
            let mut marked = Vec::new();
            subtree_marked(view, root, &structural, false, &mut marked);
            let nodes: Vec<NodeId> = marked.iter().map(|(node, _)| *node).collect();
            for (node, reordered) in &marked {
                if let Some(item) = self.by_node.get(node)
                    && !reordered
                {
                    keep.insert(*node, item.z);
                }
                self.drop_node(*node, false, &mut removed);
            }
            if let Some(place) = self.parent_place(view, root) {
                built.extend(self.build_from(view, fonts, colors, root, &place, true));
                walks.push((root, nodes));
            }
        }
        for node in singles {
            let Some(z) = self.by_node.get(&node).map(|item| item.z) else {
                continue;
            };
            let Some(place) = self.parent_place(view, node) else {
                continue;
            };
            keep.insert(node, z);
            // Its style may be another now.
            for nodes in self.by_style.values_mut() {
                nodes.remove(&node);
            }
            // Its children keep their items: only its own is replaced.
            built.extend(self.build_from(view, fonts, colors, node, &place, false));
        }
        let mut pending: HashSet<NodeId> = HashSet::new();
        for node in &built {
            if let Some(item) = self.by_node.get_mut(node) {
                match keep.get(node) {
                    Some(z) => item.z = *z,
                    None => {
                        pending.insert(*node);
                    }
                }
            }
        }
        let mut upserted: Vec<NodeId> = built
            .iter()
            .copied()
            .filter(|node| self.by_node.contains_key(node))
            .collect();
        let mut rekeyed = Vec::new();
        if !pending.is_empty() && !self.key_between_neighbours(view, &walks, &mut pending) {
            let built: HashSet<NodeId> = upserted.iter().copied().collect();
            rekeyed = self
                .rekey(view)
                .into_iter()
                .filter(|node| !built.contains(node))
                .collect();
        }
        // An item built again keeps its id: it isn't removed from the canvas.
        let kept: HashSet<ItemId> = upserted
            .iter()
            .filter_map(|node| self.by_node.get(node).map(|item| item.id))
            .collect();
        removed.retain(|id| !kept.contains(id));
        removed.sort_unstable();
        removed.dedup();
        upserted.dedup();
        Update {
            upserted,
            rekeyed,
            removed,
        }
    }

    /// Whether a props change shows or hides `node`'s subtree: it is
    /// hidden but still placed, or shown under a placed parent but not.
    fn visibility_flips(&self, view: &DocumentView, node: NodeId) -> bool {
        let Some(view_node) = view.node(node) else {
            return false;
        };
        let placed = self.places.contains_key(&node);
        if view_node.hidden() {
            placed
        } else {
            !placed && self.parent_place(view, node).is_some()
        }
    }

    /// Builds `node` (with its subtree when `recurse`) under `place`.
    /// Returns the nodes walked.
    fn build_from(
        &mut self,
        view: &DocumentView,
        fonts: &mut Fonts,
        colors: &BoardColors,
        node: NodeId,
        place: &Place,
        recurse: bool,
    ) -> Vec<NodeId> {
        let mut walker = Walker {
            view,
            fonts,
            colors,
            items: self,
            z: None,
            built: Vec::new(),
        };
        walker.node(node, place.world, &place.inner, recurse);
        walker.built
    }

    /// The key of the item that paints just before `node`'s subtree, by
    /// the tree (not a walk of the whole board): the last item in an
    /// earlier sibling's subtree, else the parent's own item, going up.
    /// Items still waiting for keys (`pending`) don't count.
    fn key_before(&self, view: &DocumentView, node: NodeId, pending: &HashSet<NodeId>) -> u64 {
        let mut current = node;
        loop {
            let parent = view
                .node(current)
                .map_or(Parent::Root, |node| node.parent());
            let siblings: Vec<NodeId> = view.children(parent).collect();
            let at = siblings.iter().position(|id| *id == current).unwrap_or(0);
            for sibling in siblings[..at].iter().rev() {
                if let Some(z) = self.last_key(view, *sibling, pending) {
                    return z;
                }
            }
            match parent {
                Parent::Root => return 0,
                Parent::Node(parent) => {
                    if let Some(item) = self.by_node.get(&parent)
                        && !pending.contains(&parent)
                    {
                        return item.z;
                    }
                    current = parent;
                }
            }
        }
    }

    /// The key of the item that paints just after `node`'s subtree: the
    /// first item in a later sibling's subtree, going up. `None` at the top.
    fn key_after(
        &self,
        view: &DocumentView,
        node: NodeId,
        pending: &HashSet<NodeId>,
    ) -> Option<u64> {
        let mut current = node;
        loop {
            let parent = view
                .node(current)
                .map_or(Parent::Root, |node| node.parent());
            let siblings: Vec<NodeId> = view.children(parent).collect();
            let at = siblings
                .iter()
                .position(|id| *id == current)
                .unwrap_or(siblings.len());
            for sibling in siblings.iter().skip(at + 1) {
                if let Some(z) = self.first_key(view, *sibling, pending) {
                    return Some(z);
                }
            }
            match parent {
                Parent::Root => return None,
                Parent::Node(parent) => current = parent,
            }
        }
    }

    /// The key of the last item of `node`'s subtree in paint order.
    fn last_key(
        &self,
        view: &DocumentView,
        node: NodeId,
        pending: &HashSet<NodeId>,
    ) -> Option<u64> {
        let children: Vec<NodeId> = view.children(Parent::Node(node)).collect();
        for child in children.iter().rev() {
            if let Some(z) = self.last_key(view, *child, pending) {
                return Some(z);
            }
        }
        self.by_node
            .get(&node)
            .filter(|_| !pending.contains(&node))
            .map(|item| item.z)
    }

    /// The key of the first item of `node`'s subtree in paint order.
    fn first_key(
        &self,
        view: &DocumentView,
        node: NodeId,
        pending: &HashSet<NodeId>,
    ) -> Option<u64> {
        if let Some(item) = self.by_node.get(&node)
            && !pending.contains(&node)
        {
            return Some(item.z);
        }
        view.children(Parent::Node(node))
            .find_map(|child| self.first_key(view, child, pending))
    }

    /// Gives the `pending` items of the subtrees built again (`walks`:
    /// each root and its nodes in paint order) keys between their
    /// neighbours'. `false` when a gap is too small.
    fn key_between_neighbours(
        &mut self,
        view: &DocumentView,
        walks: &[(NodeId, Vec<NodeId>)],
        pending: &mut HashSet<NodeId>,
    ) -> bool {
        // Whole subtrees waiting for keys (created or moved), side by side
        // under one parent, are keyed together, reading the parent's
        // children once: a paste of 1,000 shapes read the board's 11,000
        // twice per shape.
        let mut by_parent: HashMap<Parent, HashMap<NodeId, usize>> = HashMap::new();
        for (index, (root, nodes)) in walks.iter().enumerate() {
            let whole = nodes
                .iter()
                .filter(|node| self.by_node.contains_key(node))
                .all(|node| pending.contains(node));
            if whole && let Some(node) = view.node(*root) {
                by_parent
                    .entry(node.parent())
                    .or_default()
                    .insert(*root, index);
            }
        }
        for (parent, roots) in by_parent {
            let siblings: Vec<NodeId> = view.children(parent).collect();
            let mut at = 0;
            while at < siblings.len() {
                if !roots.contains_key(&siblings[at]) {
                    at += 1;
                    continue;
                }
                let start = at;
                while at < siblings.len() && roots.contains_key(&siblings[at]) {
                    at += 1;
                }
                let run: Vec<NodeId> = siblings[start..at]
                    .iter()
                    .flat_map(|root| walks[roots[root]].1.iter().copied())
                    .filter(|node| self.by_node.contains_key(node))
                    .collect();
                if run.is_empty() {
                    continue;
                }
                let low = siblings[..start]
                    .iter()
                    .rev()
                    .find_map(|sibling| self.last_key(view, *sibling, pending))
                    .unwrap_or_else(|| match parent {
                        Parent::Root => 0,
                        Parent::Node(parent) => match self.by_node.get(&parent) {
                            Some(item) if !pending.contains(&parent) => item.z,
                            _ => self.key_before(view, parent, pending),
                        },
                    });
                let high = siblings[at..]
                    .iter()
                    .find_map(|sibling| self.first_key(view, *sibling, pending))
                    .or_else(|| match parent {
                        Parent::Root => None,
                        Parent::Node(parent) => self.key_after(view, parent, pending),
                    });
                if !self.spread(&run, low, high, pending) {
                    return false;
                }
            }
        }
        for (root, nodes) in walks {
            let painting: Vec<NodeId> = nodes
                .iter()
                .copied()
                .filter(|node| self.by_node.contains_key(node))
                .collect();
            let mut index = 0;
            while index < painting.len() {
                if !pending.contains(&painting[index]) {
                    index += 1;
                    continue;
                }
                let start = index;
                while index < painting.len() && pending.contains(&painting[index]) {
                    index += 1;
                }
                let low = match start.checked_sub(1) {
                    Some(before) => self.by_node[&painting[before]].z,
                    None => self.key_before(view, *root, pending),
                };
                let high = match painting.get(index) {
                    Some(after) => Some(self.by_node[after].z),
                    None => self.key_after(view, *root, pending),
                };
                let run = painting[start..index].to_vec();
                if !self.spread(&run, low, high, pending) {
                    return false;
                }
            }
        }
        true
    }

    /// Gives `run` keys evenly between `low` and `high` (`None`: nothing
    /// above). `false` when they don't fit.
    fn spread(
        &mut self,
        run: &[NodeId],
        low: u64,
        high: Option<u64>,
        pending: &mut HashSet<NodeId>,
    ) -> bool {
        let count = run.len() as u64;
        let high = high
            .unwrap_or_else(|| low.saturating_add(count.saturating_add(1).saturating_mul(Z_GAP)));
        if high <= low || high - low <= count {
            return false;
        }
        let step = (high - low) / (count + 1);
        for (offset, node) in run.iter().enumerate() {
            if let Some(item) = self.by_node.get_mut(node) {
                item.z = low + step * (offset as u64 + 1);
            }
            pending.remove(node);
        }
        true
    }

    /// New keys for every item, in z order. Returns every node.
    fn rekey(&mut self, view: &DocumentView) -> Vec<NodeId> {
        let mut z = Z_START;
        let mut nodes = Vec::new();
        for node in view.walk() {
            if let Some(item) = self.by_node.get_mut(&node) {
                z += Z_GAP;
                item.z = z;
                nodes.push(node);
            }
        }
        nodes
    }
}

/// One change set for several that wait together: what any of them
/// names. Applied against the view after all of them, it updates what
/// they would have one by one.
pub(crate) fn merge(sets: Vec<ChangeSet>) -> ChangeSet {
    let mut sets = sets.into_iter();
    let Some(mut merged) = sets.next() else {
        return ChangeSet::default();
    };
    for set in sets {
        merged.created.extend(set.created);
        merged.deleted.extend(set.deleted);
        merged.moved.extend(set.moved);
        merged.styles.extend(set.styles);
        merged.assets.extend(set.assets);
        merged.meta |= set.meta;
        merged.pages |= set.pages;
        merged.ext |= set.ext;
        for (node, what) in set.changed {
            match merged.changed.iter_mut().find(|(id, _)| *id == node) {
                Some((_, into)) => {
                    into.kind |= what.kind;
                    into.geometry |= what.geometry;
                    into.style |= what.style;
                    into.text |= what.text;
                    into.assets |= what.assets;
                    into.ext |= what.ext;
                    into.generation |= what.generation;
                    into.props |= what.props;
                }
                None => merged.changed.push((node, what)),
            }
        }
    }
    for list in [&mut merged.created, &mut merged.deleted, &mut merged.moved] {
        list.sort_unstable();
        list.dedup();
    }
    merged.styles.sort_unstable();
    merged.styles.dedup();
    merged.changed.sort_unstable_by_key(|(node, _)| *node);
    merged
}

/// The node and its descendants in paint order, each with whether it or
/// an ancestor (from `node` down) is in `set`.
fn subtree_marked(
    view: &DocumentView,
    node: NodeId,
    set: &HashSet<NodeId>,
    above: bool,
    out: &mut Vec<(NodeId, bool)>,
) {
    let marked = above || set.contains(&node);
    out.push((node, marked));
    for child in view.children(Parent::Node(node)) {
        subtree_marked(view, child, set, marked, out);
    }
}

/// A node's ancestors, nearest first.
fn ancestors(view: &DocumentView, node: NodeId) -> impl Iterator<Item = NodeId> + '_ {
    let mut next = view.node(node).map(|node| node.parent());
    std::iter::from_fn(move || match next? {
        Parent::Root => None,
        Parent::Node(parent) => {
            next = view.node(parent).map(|node| node.parent());
            Some(parent)
        }
    })
}

/// The styles a change to `changed` affects: those and every style that
/// inherits from one of them.
fn affected_styles(view: &DocumentView, changed: &[StyleId]) -> HashSet<StyleId> {
    let changed: HashSet<&StyleId> = changed.iter().collect();
    let mut affected: HashSet<StyleId> = changed.iter().map(|style| (*style).clone()).collect();
    for style in view.style_ids() {
        let mut seen = HashSet::new();
        let mut next = Some(style);
        while let Some(id) = next {
            if !seen.insert(id) {
                break;
            }
            if changed.contains(id) {
                affected.insert(style.clone());
                break;
            }
            next = view.style(id).and_then(|style| style.parent());
        }
    }
    affected
}

struct Walker<'a> {
    view: &'a DocumentView,
    fonts: &'a mut Fonts,
    colors: &'a BoardColors,
    items: &'a mut Items,
    /// Numbers the items in walk order (a full build); `None` leaves the
    /// keys to the caller.
    z: Option<&'a mut u64>,
    /// The nodes walked.
    built: Vec<NodeId>,
}

impl Walker<'_> {
    fn children(&mut self, parent: Parent, world: Affine, clips: &Rc<[Clip]>) {
        let children: Vec<NodeId> = self.view.children(parent).collect();
        for child in children {
            self.node(child, world, clips, true);
        }
    }

    /// Builds `id` under a parent at `parent_world`, and its children when
    /// `recurse`.
    fn node(&mut self, id: NodeId, parent_world: Affine, clips: &Rc<[Clip]>, recurse: bool) {
        let Some(node) = self.view.node(id) else {
            return;
        };
        if node.hidden() {
            return;
        }
        self.built.push(id);
        let geometry = node.geometry().unwrap_or_default();
        let size = Size::new(geometry.w.max(0.0), geometry.h.max(0.0));
        let world = parent_world
            * local_transform(geometry.x, geometry.y, size.width, size.height, geometry.r);
        let mut inner = clips.clone();
        if matches!(node.kind(), NodeKind::Frame) {
            let style = frame_style(self.view, id, self.colors);
            let mut frames: Vec<Clip> = clips.to_vec();
            frames.push(Clip {
                world,
                rect: Rect::from_origin_size(Point::ZERO, size),
                radius: style.radius.min(size.width.min(size.height) / 2.0),
            });
            inner = Rc::from(frames);
        }
        self.items.places.insert(
            id,
            Place {
                world,
                inner: inner.clone(),
            },
        );
        match node.kind() {
            // Groups paint nothing themselves; their children are placed in
            // the group's space.
            NodeKind::Group | NodeKind::Page | NodeKind::Slide => {}
            NodeKind::Frame => {
                let style = frame_style(self.view, id, self.colors);
                let rect = Rect::from_origin_size(Point::ZERO, size);
                let bounds = world.transform_rect_bbox(rect).inflate(
                    style.stroke_width / 2.0 + 1.0,
                    style.stroke_width / 2.0 + 1.0,
                );
                let name = node
                    .name()
                    .map(str::to_owned)
                    .or_else(|| Some("Frame".to_owned()));
                self.push(id, world, size, Look::Frame { style }, bounds, clips, name);
            }
            NodeKind::Shape => {
                let style = style::shape_style(self.view, id, self.colors.theme);
                let outline = shape_path(node, size, style.radius);
                let label = self.text(node, size, Align::Center, style::LABEL_SIZE, 500.0, 1.3);
                let reach = if style.stroke.is_some() {
                    style.stroke_width / 2.0 + 1.0
                } else {
                    1.0
                };
                let mut bounds = (world * outline.path.clone())
                    .bounding_box()
                    .inflate(reach, reach);
                if let Some(label) = &label {
                    bounds = bounds.union(text_bounds(world, label));
                }
                let look = Look::Shape {
                    outline: Rc::new(outline),
                    style,
                    label,
                };
                self.push(id, world, size, look, bounds, clips, None);
            }
            NodeKind::Text => {
                let text = self.text(node, size, Align::Start, style::TEXT_SIZE, 400.0, 1.5);
                let mut bounds = world
                    .transform_rect_bbox(Rect::from_origin_size(Point::ZERO, size))
                    .inflate(1.0, 1.0);
                if let Some(text) = &text {
                    bounds = bounds.union(text_bounds(world, text));
                }
                let opacity = style::shape_style(self.view, id, self.colors.theme).opacity;
                self.push(
                    id,
                    world,
                    size,
                    Look::Text { text, opacity },
                    bounds,
                    clips,
                    None,
                );
            }
            // Images, connectors and paint layers come in later milestones;
            // newer kinds are leaf boxes (schema rule 4).
            _ => {
                let bounds = world
                    .transform_rect_bbox(Rect::from_origin_size(Point::ZERO, size))
                    .inflate(2.0, 2.0);
                self.push(id, world, size, Look::Placeholder, bounds, clips, None);
            }
        }
        if recurse {
            self.children(Parent::Node(id), world, &inner);
        }
    }

    /// The node's text laid out in its box: wrapped at the box's width,
    /// at the top for text boxes (`Align::Start`), in the middle for
    /// labels (`Align::Center`).
    fn text(
        &mut self,
        node: &NodeView,
        size: Size,
        align: Align,
        default_size: f32,
        weight: f32,
        line_height: f32,
    ) -> Option<ItemText> {
        let text = node.text()?.to_plain_string();
        let text = text.strip_suffix('\n').unwrap_or(&text);
        if text.trim().is_empty() {
            return None;
        }
        let look = style::text_look(
            self.view,
            node.id(),
            self.colors.theme,
            self.colors.text,
            default_size,
            weight,
        );
        let padding = if align == Align::Center {
            LABEL_PADDING
        } else {
            0.0
        };
        let width = (size.width - 2.0 * padding).max(1.0) as f32;
        let text_style = TextStyle::new(look.size)
            .weight(look.weight)
            .color(look.color)
            .line_height(line_height)
            .align(align);
        let key = TextKey {
            text: text.to_owned(),
            look,
            width,
            align,
            line_height,
        };
        let block = match self.items.texts.get(&node.id()) {
            Some(cached) if cached.key == key => cached.block.clone(),
            _ => {
                let block = Rc::new(TextBlock::new(self.fonts, text, &text_style, Some(width)));
                self.items.texts.insert(
                    node.id(),
                    CachedText {
                        key,
                        block: block.clone(),
                    },
                );
                block
            }
        };
        let laid = block.size();
        // Lines line up within the wrapping width, so the block spans it.
        let extent = Size::new(laid.width.max(f64::from(width)), laid.height);
        let origin = if align == Align::Center {
            // A label's middle: the box's, or a triangle's centroid.
            let triangle = node
                .prop("shape")
                .and_then(imaginarium_model::Value::as_str)
                == Some("triangle");
            let middle = if triangle {
                size.height * 2.0 / 3.0
            } else {
                size.height / 2.0
            };
            Point::new(
                padding,
                (middle - laid.height / 2.0)
                    .max(0.0)
                    .min((size.height - laid.height).max(0.0)),
            )
        } else {
            Point::ZERO
        };
        Some(ItemText {
            block,
            centered: align == Align::Center,
            origin,
            extent,
            look,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        node: NodeId,
        world: Affine,
        size: Size,
        look: Look,
        bounds: Rect,
        clips: &Rc<[Clip]>,
        name: Option<String>,
    ) {
        // Clipped by its frames: it can't paint outside them.
        let bounds = clips.iter().fold(bounds, |bounds, clip| {
            bounds.intersect(clip.world.transform_rect_bbox(clip.rect).inflate(1.0, 1.0))
        });
        let z = match &mut self.z {
            Some(z) => {
                **z += Z_GAP;
                **z
            }
            None => 0,
        };
        let id = self.items.item_id(node);
        if name.is_some() {
            self.items.frames.insert(node);
        }
        if let Some(style) = self.view.node(node).and_then(|node| node.style()) {
            self.items
                .by_style
                .entry(style.clone())
                .or_default()
                .insert(node);
        }
        self.items.by_node.insert(
            node,
            Item {
                id,
                z,
                world,
                size,
                look,
                bounds,
                clips: clips.clone(),
                name,
            },
        );
    }
}

/// Where an item's text can paint, in document space.
fn text_bounds(world: Affine, text: &ItemText) -> Rect {
    let rect = Rect::from_origin_size(text.origin, text.extent);
    // Glyphs reach past their line boxes (accents, descenders, italics):
    // a share of the size around it.
    let reach = (f64::from(text.look.size) * 0.25).max(2.0);
    world.transform_rect_bbox(rect).inflate(reach, reach)
}

/// How a shape or frame looks as the board draws it, in `theme`: a
/// frame with the theme's frame colors, 1 pt border and 4 pt corners
/// where it has none of its own (panels show this, not the shape
/// defaults).
pub fn node_look(view: &DocumentView, node: NodeId, theme: &imaginarium_ui::Theme) -> ShapeStyle {
    let is_frame = view
        .node(node)
        .is_some_and(|node| matches!(node.kind(), imaginarium_model::NodeKind::Frame));
    if is_frame {
        frame_style(view, node, &BoardColors::of(theme))
    } else {
        style::shape_style(view, node, theme.kind())
    }
}

/// A frame's style: its own, else the theme's frame colors, a 1 pt border
/// and 4 pt corners.
fn frame_style(view: &DocumentView, node: NodeId, colors: &BoardColors) -> ShapeStyle {
    let own = style::shape_style(view, node, colors.theme);
    let has = |prop: &str| view.style_prop(node, prop).is_some();
    // A color that doesn't parse counts as absent, as for shapes.
    let color = |prop: &str, default: Color| match style::color_prop(view, node, prop) {
        Some(color) => color.map(|color| style::shown(color, colors.theme)),
        None => Some(default),
    };
    ShapeStyle {
        fill: color("fill", colors.frame_bg),
        stroke: color("stroke", colors.frame_border),
        stroke_width: if has("stroke_w") {
            own.stroke_width
        } else {
            1.0
        },
        opacity: own.opacity,
        radius: if has("radius") { own.radius } else { 4.0 },
    }
}
