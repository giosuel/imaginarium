// SPDX-License-Identifier: AGPL-3.0-or-later

//! Typed edits of one transaction (schema-v1.md).
//!
//! Child containers under map keys (a node's `g`, `s`, `t`, `a`, `x`,
//! `meta.required_features`, asset entries) are Loro *mergeable* children:
//! peers that create them concurrently get the same container, so neither
//! peer's first edit is lost (schema-v1.md §3).
//!
//! Every method validates its input against the current state *before* it
//! changes anything, so a failing call leaves the transaction as it was.
//! Commands therefore fail cleanly. If a closure still returns an error
//! after some edits, [`Document::transact`](crate::Document::transact)
//! reverts them.
//!
//! Edits that destroy containers (deleting a node, a style or an asset
//! entry, replacing `gen`) wait until the closure has succeeded. Reverting
//! a destroyed container re-creates it under a new id, and every undo step
//! that touched the old one would then do nothing. Until then, the deleted
//! things are gone for this transaction's reads and checks.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use loro::{
    Container, LoroDoc, LoroMap, LoroText, LoroTree, LoroValue, TreeID, TreeParentId,
    ValueOrContainer,
};

use crate::error::{ModelError, Result};
use crate::ids::{BlobId, NodeId, Parent, StyleId};
use crate::schema::{self, DocumentKind, NodeKind, meta, node, root};
use crate::value::{Geometry, Value};
use crate::view::DocumentView;

/// The edits of one transaction. Obtained from
/// [`Document::transact`](crate::Document::transact).
pub struct Transaction<'d> {
    doc: &'d LoroDoc,
    tree: LoroTree,
    /// Identity → tree id, as of the start of the transaction.
    handles: imbl::HashMap<NodeId, TreeID>,
    owners: imbl::HashMap<TreeID, NodeId>,
    /// Nodes created in this transaction.
    created: HashMap<NodeId, TreeID>,
    created_owners: HashMap<TreeID, NodeId>,
    /// Destructive edits, applied in order by [`Transaction::finish`].
    deferred: Vec<Deferred>,
    /// Nodes whose deletion is deferred (their subtrees go with them).
    deleting: HashSet<TreeID>,
    /// Styles whose deletion is deferred.
    deleting_styles: HashSet<StyleId>,
}

/// An edit that destroys a container (see the module docs).
enum Deferred {
    DeleteNode(TreeID),
    SetGeneration(TreeID, Vec<(String, Value)>),
    DeleteStyle(StyleId),
    RemoveAsset(BlobId),
}

impl<'d> Transaction<'d> {
    pub(crate) fn new(doc: &'d LoroDoc, view: &DocumentView) -> Self {
        Self {
            doc,
            tree: doc.get_tree(root::NODES),
            handles: view.handles.clone(),
            owners: view.owners.clone(),
            created: HashMap::new(),
            created_owners: HashMap::new(),
            deferred: Vec::new(),
            deleting: HashSet::new(),
            deleting_styles: HashSet::new(),
        }
    }

    /// Applies the deferred destructive edits. Called once the closure has
    /// succeeded.
    pub(crate) fn finish(self) -> Result<()> {
        for edit in self.deferred {
            match edit {
                Deferred::DeleteNode(tid) => {
                    // A node inside a subtree deleted before it is gone already.
                    if !self.tree.is_node_deleted(&tid).unwrap_or(true) {
                        self.tree.delete(tid)?;
                    }
                }
                Deferred::SetGeneration(tid, generation) => {
                    let meta = self.tree.get_meta(tid)?;
                    if generation.is_empty() {
                        meta.delete(node::GEN)?;
                    } else {
                        let map = meta.insert_container(node::GEN, LoroMap::new())?;
                        for (key, value) in &generation {
                            set_or_delete(&map, key, value)?;
                        }
                    }
                }
                Deferred::DeleteStyle(style) => {
                    self.doc.get_map(root::STYLES).delete(style.as_str())?;
                }
                Deferred::RemoveAsset(blob) => {
                    self.doc.get_map(root::ASSETS).delete(&blob.to_string())?;
                }
            }
        }
        Ok(())
    }

    /// Whether `tid` or one of its ancestors is being deleted.
    fn is_deleting(&self, tid: TreeID) -> bool {
        if self.deleting.is_empty() {
            return false;
        }
        let mut current = TreeParentId::Node(tid);
        while let TreeParentId::Node(node) = current {
            if self.deleting.contains(&node) {
                return true;
            }
            current = self.tree.parent(node).unwrap_or(TreeParentId::Root);
        }
        false
    }

    /// The position in the tree's own child list of `parent` that index
    /// `index` among the remaining children (without `moving`, and without
    /// nodes being deleted) stands for.
    fn tree_index(&self, parent: TreeParentId, index: usize, moving: Option<TreeID>) -> usize {
        if self.deleting.is_empty() {
            return index; // the same list (callers checked the range)
        }
        let all: Vec<TreeID> = self
            .tree
            .children(parent)
            .unwrap_or_default()
            .into_iter()
            .filter(|&tid| Some(tid) != moving)
            .collect();
        all.iter()
            .enumerate()
            .filter(|&(_, &tid)| !self.deleting.contains(&tid))
            .nth(index)
            .map_or(all.len(), |(position, _)| position)
    }

    /// The tree id of a node, if the node is known.
    fn handle(&self, id: NodeId) -> Option<TreeID> {
        self.created
            .get(&id)
            .or_else(|| self.handles.get(&id))
            .copied()
    }

    /// The identity of a tree node.
    fn identity(&self, tid: TreeID) -> NodeId {
        self.created_owners
            .get(&tid)
            .or_else(|| self.owners.get(&tid))
            .copied()
            .unwrap_or(NodeId::Tree(tid))
    }

    fn tree_parent(&self, parent: Parent) -> Result<TreeParentId> {
        match parent {
            Parent::Root => Ok(TreeParentId::Root),
            Parent::Node(id) => Ok(TreeParentId::Node(self.live(id)?)),
        }
    }

    /// The tree id of a live node.
    fn live(&self, id: NodeId) -> Result<TreeID> {
        match self.handle(id) {
            Some(tid)
                if self.tree.contains(tid)
                    && !self.tree.is_node_deleted(&tid).unwrap_or(true)
                    && !self.is_deleting(tid) =>
            {
                Ok(tid)
            }
            _ => Err(ModelError::NoSuchNode(id)),
        }
    }

    // ---- reading the current state (including this transaction's edits) --

    /// Whether `id` is a live node.
    pub fn contains(&self, id: NodeId) -> bool {
        self.live(id).is_ok()
    }

    /// The children of `parent`, in order.
    pub fn children(&self, parent: Parent) -> Vec<NodeId> {
        let Ok(tree_parent) = self.tree_parent(parent) else {
            return Vec::new();
        };
        self.tree
            .children(tree_parent)
            .unwrap_or_default()
            .into_iter()
            .filter(|tid| !self.deleting.contains(tid))
            .map(|tid| self.identity(tid))
            .collect()
    }

    /// The length of a node's text in characters (0 without text).
    pub fn text_len(&self, id: NodeId) -> Result<usize> {
        Ok(self.text(id, false)?.map_or(0, |t| t.len_unicode()))
    }

    // ---- document metadata -----------------------------------------------

    /// Sets `meta.title`.
    pub fn set_title(&mut self, title: &str) -> Result<()> {
        self.meta_map().insert(meta::TITLE, title)?;
        Ok(())
    }

    /// Sets `meta.language` (BCP 47).
    pub fn set_language(&mut self, language: &str) -> Result<()> {
        self.meta_map().insert(meta::LANGUAGE, language)?;
        Ok(())
    }

    /// Sets the default view.
    pub fn set_document_kind(&mut self, kind: DocumentKind) -> Result<()> {
        self.meta_map().insert(meta::KIND, kind.as_str())?;
        Ok(())
    }

    /// Adds a feature to `meta.required_features` (schema-v1.md §10.2).
    pub fn require_feature(&mut self, feature: &str) -> Result<()> {
        if feature.is_empty() {
            return Err(ModelError::Invalid("empty feature name".into()));
        }
        let list = self
            .meta_map()
            .ensure_mergeable_list(meta::REQUIRED_FEATURES)?;
        let present = list
            .get_deep_value()
            .into_list()
            .map(|l| {
                l.iter()
                    .any(|v| v.as_string().is_some_and(|s| s.as_str() == feature))
            })
            .unwrap_or(false);
        if !present {
            list.push(feature)?;
        }
        Ok(())
    }

    /// Sets the document's own value of the setting `key`
    /// (`meta.settings`); `Value::Null` removes it, so the setting follows
    /// the user's again. The value is a setting's stored form (an enum's
    /// id, a color's hex), which the settings layer checks.
    pub fn set_document_setting(&mut self, key: &str, value: Value) -> Result<()> {
        // As the settings registry checks keys: dotted `snake_case`, each
        // segment starting with a letter.
        let dotted = key.split('.').count() >= 2
            && key.split('.').all(|part| {
                let mut chars = part.chars();
                chars.next().is_some_and(|c| c.is_ascii_lowercase())
                    && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
            });
        if !dotted {
            return Err(ModelError::Invalid(format!("`{key}` isn't a setting key")));
        }
        if !matches!(
            value,
            Value::Null | Value::Bool(_) | Value::Int(_) | Value::Number(_) | Value::String(_)
        ) {
            return Err(ModelError::Invalid(format!(
                "`{key}`: a setting's value is a plain value (bool, int, number or string)"
            )));
        }
        // Removing from a document without settings changes nothing.
        let has_settings = self.meta_map().get(meta::SETTINGS).is_some();
        if value == Value::Null && !has_settings {
            return Ok(());
        }
        let settings = self.meta_map().ensure_mergeable_map(meta::SETTINGS)?;
        set_or_delete(&settings, key, &value)
    }

    pub(crate) fn init_meta(&mut self, kind: DocumentKind, created: i64) -> Result<()> {
        let meta = self.meta_map();
        meta.insert(meta::SCHEMA_VERSION, schema::SCHEMA_VERSION)?;
        meta.insert(meta::KIND, kind.as_str())?;
        meta.insert(meta::CREATED, created)?;
        Ok(())
    }

    pub(crate) fn set_schema_version(&mut self, version: i64) -> Result<()> {
        self.meta_map().insert(meta::SCHEMA_VERSION, version)?;
        Ok(())
    }

    // ---- nodes -------------------------------------------------------------

    /// Creates a node under `parent`, at `index` among its siblings (`None`
    /// appends, which puts it in front on a board).
    pub fn create_node(
        &mut self,
        parent: Parent,
        index: Option<usize>,
        kind: &NodeKind,
    ) -> Result<NodeId> {
        let tree_parent = self.tree_parent(parent)?;
        let mut id = NodeId::generate()?;
        while self.handle(id).is_some() {
            id = NodeId::generate()?;
        }
        let tid = match index {
            Some(index) => {
                let len = self.children(parent).len();
                if index > len {
                    return Err(ModelError::OutOfRange {
                        what: "child index",
                        value: index,
                        len,
                    });
                }
                self.tree
                    .create_at(tree_parent, self.tree_index(tree_parent, index, None))?
            }
            None => self.tree.create(tree_parent)?,
        };
        let meta = self.tree.get_meta(tid)?;
        if let NodeId::Stored(stored) = id {
            meta.insert(
                node::ID,
                i64::try_from(stored).map_err(|_| ModelError::Invalid("node id".into()))?,
            )?;
        }
        meta.insert(node::KIND, kind.as_str())?;
        if matches!(kind, NodeKind::Text) {
            meta.ensure_mergeable_text(node::TEXT)?;
        }
        self.created.insert(id, tid);
        self.created_owners.insert(tid, id);
        Ok(id)
    }

    /// Deletes a node and its subtree.
    pub fn delete_node(&mut self, id: NodeId) -> Result<()> {
        let tid = self.live(id)?;
        self.deleting.insert(tid);
        self.deferred.push(Deferred::DeleteNode(tid));
        Ok(())
    }

    /// Moves a node under `parent`, at `index` among the new siblings
    /// (`None` appends).
    pub fn move_node(&mut self, id: NodeId, parent: Parent, index: Option<usize>) -> Result<()> {
        let tid = self.live(id)?;
        let tree_parent = self.tree_parent(parent)?;
        let mut ancestor = tree_parent;
        while let TreeParentId::Node(next) = ancestor {
            if next == tid {
                return Err(ModelError::Cycle(id));
            }
            ancestor = self.tree.parent(next).unwrap_or(TreeParentId::Root);
        }
        match index {
            Some(index) => {
                let siblings = self.children(parent);
                let len = siblings.len() - usize::from(siblings.contains(&id));
                if index > len {
                    return Err(ModelError::OutOfRange {
                        what: "child index",
                        value: index,
                        len,
                    });
                }
                self.tree.mov_to(
                    tid,
                    tree_parent,
                    self.tree_index(tree_parent, index, Some(tid)),
                )?;
            }
            None => self.tree.mov(tid, tree_parent)?,
        }
        Ok(())
    }

    /// Changes a node's kind.
    pub fn set_kind(&mut self, id: NodeId, kind: &NodeKind) -> Result<()> {
        self.node_meta(id)?.insert(node::KIND, kind.as_str())?;
        Ok(())
    }

    /// Sets a node's box. Only coordinates that change are written, so
    /// concurrent edits of different coordinates both survive.
    pub fn set_geometry(&mut self, id: NodeId, geometry: Geometry) -> Result<()> {
        for v in [geometry.x, geometry.y, geometry.w, geometry.h, geometry.r] {
            if !v.is_finite() {
                return Err(ModelError::Invalid("geometry must be finite".into()));
            }
        }
        if geometry.w < 0.0 || geometry.h < 0.0 {
            return Err(ModelError::Invalid("width and height must be ≥ 0".into()));
        }
        let meta = self.node_meta(id)?;
        let g = meta.ensure_mergeable_map(node::GEOMETRY)?;
        for (key, value) in [
            ("x", geometry.x),
            ("y", geometry.y),
            ("w", geometry.w),
            ("h", geometry.h),
        ] {
            if current_number(&g, key) != Some(value) {
                g.insert(key, value)?;
            }
        }
        match (current_number(&g, "r"), geometry.r) {
            (None, 0.0) => {}
            (Some(_), 0.0) => g.delete("r")?,
            (current, r) if current != Some(r) => g.insert("r", r)?,
            _ => {}
        }
        Ok(())
    }

    /// Sets a scalar key of a node (`name`, `hidden`, `locked`, `shape`,
    /// `from`, `to`, …). `Value::Null` deletes it. The keys the model
    /// manages (`k` and the containers) are refused.
    pub fn set_prop(&mut self, id: NodeId, key: &str, value: Value) -> Result<()> {
        if key.is_empty() || key == node::KIND || key == node::ID || node::CONTAINERS.contains(&key)
        {
            return Err(ModelError::Invalid(format!(
                "`{key}` can't be set as a property"
            )));
        }
        set_or_delete(&self.node_meta(id)?, key, &value)
    }

    /// Sets or clears a node's style.
    pub fn set_style(&mut self, id: NodeId, style: Option<&StyleId>) -> Result<()> {
        let meta = self.node_meta(id)?;
        let s = meta.ensure_mergeable_map(node::STYLE)?;
        match style {
            Some(style) => s.insert(node::STYLE_ID, style.as_str())?,
            None => s.delete(node::STYLE_ID)?,
        }
        Ok(())
    }

    /// Sets a local style override (`Value::Null` removes it).
    pub fn set_style_override(&mut self, id: NodeId, prop: &str, value: Value) -> Result<()> {
        if prop.is_empty() || prop == node::STYLE_ID {
            return Err(ModelError::Invalid(format!(
                "`{prop}` isn't a style property"
            )));
        }
        let meta = self.node_meta(id)?;
        let s = meta.ensure_mergeable_map(node::STYLE)?;
        set_or_delete(&s, prop, &value)
    }

    /// Sets or removes an asset reference (`src`, `poster`, …).
    pub fn set_asset(&mut self, id: NodeId, role: &str, blob: Option<BlobId>) -> Result<()> {
        if role.is_empty() {
            return Err(ModelError::Invalid("empty asset role".into()));
        }
        let meta = self.node_meta(id)?;
        let a = meta.ensure_mergeable_map(node::ASSETS)?;
        match blob {
            Some(blob) => a.insert(role, blob.to_string())?,
            None => a.delete(role)?,
        }
        Ok(())
    }

    /// Sets an add-on's node-level data (`Value::Null` removes it).
    pub fn set_node_ext(&mut self, id: NodeId, addon: &str, value: Value) -> Result<()> {
        check_addon_id(addon)?;
        let meta = self.node_meta(id)?;
        let x = meta.ensure_mergeable_map(node::EXT)?;
        set_or_delete(&x, addon, &value)
    }

    /// Replaces a node's AI provenance (schema-v1.md §5.4).
    pub fn set_generation(&mut self, id: NodeId, generation: &[(&str, Value)]) -> Result<()> {
        let tid = self.live(id)?;
        let generation = generation
            .iter()
            .map(|(key, value)| ((*key).to_owned(), value.clone()))
            .collect();
        self.deferred.push(Deferred::SetGeneration(tid, generation));
        Ok(())
    }

    // ---- text --------------------------------------------------------------

    /// Inserts text at character `pos`. Inserting into an empty text also
    /// adds the final `\n` the schema requires (schema-v1.md §6).
    pub fn insert_text(&mut self, id: NodeId, pos: usize, text: &str) -> Result<()> {
        // Check before creating the text container.
        let len = self.text_len(id)?;
        if pos > len {
            return Err(ModelError::OutOfRange {
                what: "text position",
                value: pos,
                len,
            });
        }
        if text.is_empty() {
            return Ok(());
        }
        let target = self.text(id, true)?.ok_or(ModelError::NoSuchNode(id))?;
        target.insert(pos, text)?;
        // At the end (after the final newline, or into an empty text) the
        // text starts a paragraph of its own, which needs its newline
        // (schema-v1.md §6).
        if pos == len && !text.ends_with('\n') {
            target.insert(pos + text.chars().count(), "\n")?;
        }
        Ok(())
    }

    /// Deletes `len` characters at `pos`. A range that ends with the
    /// text's final newline keeps it, unless it covers the whole text
    /// (schema-v1.md §6).
    pub fn delete_text(&mut self, id: NodeId, pos: usize, len: usize) -> Result<()> {
        let target = self
            .text(id, false)?
            .ok_or(ModelError::Invalid(format!("node {id} has no text")))?;
        let total = target.len_unicode();
        if pos.checked_add(len).is_none_or(|end| end > total) {
            return Err(ModelError::OutOfRange {
                what: "text range end",
                value: pos.saturating_add(len),
                len: total,
            });
        }
        let keeps_final_newline =
            len > 0 && pos > 0 && pos + len == total && target.slice(total - 1, total)? == "\n";
        let len = len - usize::from(keeps_final_newline);
        if len > 0 {
            target.delete(pos, len)?;
        }
        Ok(())
    }

    /// Sets a character mark (schema-v1.md §6.1) or an add-on's
    /// namespaced mark over `range`.
    pub fn mark_text(
        &mut self,
        id: NodeId,
        range: Range<usize>,
        key: &str,
        value: Value,
    ) -> Result<()> {
        let target = self.marked_text(id, &range, key)?;
        if matches!(value, Value::Null) {
            return Err(ModelError::Invalid(
                "use unmark_text to remove a mark".into(),
            ));
        }
        if !range.is_empty() {
            target.mark(range, key, value.to_loro())?;
        }
        Ok(())
    }

    /// Removes a character mark over `range`.
    pub fn unmark_text(&mut self, id: NodeId, range: Range<usize>, key: &str) -> Result<()> {
        let target = self.marked_text(id, &range, key)?;
        if !range.is_empty() {
            target.unmark(range, key)?;
        }
        Ok(())
    }

    /// Sets a paragraph mark (schema-v1.md §6.2) on the paragraph that
    /// contains character `pos`. `Value::Null` removes it.
    pub fn set_paragraph_mark(
        &mut self,
        id: NodeId,
        pos: usize,
        key: &str,
        value: Value,
    ) -> Result<()> {
        if !schema::mark(key).is_some_and(|m| m.paragraph) {
            return Err(ModelError::Invalid(format!(
                "`{key}` isn't a paragraph mark"
            )));
        }
        let target = self
            .text(id, false)?
            .ok_or(ModelError::Invalid(format!("node {id} has no text")))?;
        let len = target.len_unicode();
        if pos >= len {
            return Err(ModelError::OutOfRange {
                what: "text position",
                value: pos,
                len,
            });
        }
        let newline = match (find_newline(&target, pos)?, &value) {
            (Some(at), _) => at,
            // An unterminated last paragraph has no mark to remove.
            (None, Value::Null) => return Ok(()),
            (None, _) => {
                target.insert(len, "\n")?;
                len
            }
        };
        match value {
            Value::Null => target.unmark(newline..newline + 1, key)?,
            value => target.mark(newline..newline + 1, key, value.to_loro())?,
        }
        Ok(())
    }

    // ---- styles, assets, pages, document add-on data -----------------------

    /// Creates a style with a new random id.
    pub fn create_style(&mut self, name: &str, parent: Option<&StyleId>) -> Result<StyleId> {
        let id = StyleId::generate()?;
        let map = self
            .doc
            .get_map(root::STYLES)
            .insert_container(id.as_str(), LoroMap::new())?;
        map.insert("name", name)?;
        if let Some(parent) = parent {
            map.insert("parent", parent.as_str())?;
        }
        Ok(id)
    }

    /// Sets a style property (`Value::Null` removes it).
    pub fn set_style_prop(&mut self, style: &StyleId, prop: &str, value: Value) -> Result<()> {
        if prop.is_empty() {
            return Err(ModelError::Invalid("empty style property".into()));
        }
        let map = map_at(&self.doc.get_map(root::STYLES), style.as_str())
            .filter(|_| !self.deleting_styles.contains(style))
            .ok_or_else(|| ModelError::Invalid(format!("no style {style}")))?;
        set_or_delete(&map, prop, &value)
    }

    /// Deletes a style. Nodes that use it fall back to the defaults.
    pub fn delete_style(&mut self, style: &StyleId) -> Result<()> {
        if self.deleting_styles.insert(style.clone()) {
            self.deferred.push(Deferred::DeleteStyle(style.clone()));
        }
        Ok(())
    }

    /// Adds an asset entry, or sets properties of an existing one
    /// (schema-v1.md §9).
    pub fn put_asset(&mut self, blob: BlobId, props: &[(&str, Value)]) -> Result<()> {
        // Removed earlier in this transaction: the entry starts over, as it
        // would have if the removal had happened right away.
        if let Some(at) = self
            .deferred
            .iter()
            .position(|edit| matches!(edit, Deferred::RemoveAsset(b) if *b == blob))
        {
            self.deferred.remove(at);
            self.doc.get_map(root::ASSETS).delete(&blob.to_string())?;
        }
        let map = self
            .doc
            .get_map(root::ASSETS)
            .ensure_mergeable_map(&blob.to_string())?;
        for (key, value) in props {
            set_or_delete(&map, key, value)?;
        }
        Ok(())
    }

    /// Removes an asset entry.
    pub fn remove_asset(&mut self, blob: BlobId) -> Result<()> {
        self.deferred.push(Deferred::RemoveAsset(blob));
        Ok(())
    }

    /// Sets a key of `pages` (`w`, `h`, …; `Value::Null` removes it).
    pub fn set_page_prop(&mut self, key: &str, value: Value) -> Result<()> {
        if key.is_empty() {
            return Err(ModelError::Invalid("empty page property".into()));
        }
        set_or_delete(&self.doc.get_map(root::PAGES), key, &value)
    }

    /// Sets an add-on's document-level data (`Value::Null` removes it).
    pub fn set_doc_ext(&mut self, addon: &str, value: Value) -> Result<()> {
        check_addon_id(addon)?;
        set_or_delete(&self.doc.get_map(root::EXT), addon, &value)
    }

    // ---- helpers -------------------------------------------------------------

    fn meta_map(&self) -> LoroMap {
        self.doc.get_map(root::META)
    }

    fn node_meta(&self, id: NodeId) -> Result<LoroMap> {
        Ok(self.tree.get_meta(self.live(id)?)?)
    }

    /// The node's text container, created if `create` and missing.
    fn text(&self, id: NodeId, create: bool) -> Result<Option<LoroText>> {
        let meta = self.node_meta(id)?;
        match meta.get(node::TEXT) {
            Some(ValueOrContainer::Container(Container::Text(text))) => Ok(Some(text)),
            Some(_) => Err(ModelError::Invalid(format!(
                "node {id} has a `t` that isn't a text"
            ))),
            None if create => Ok(Some(meta.ensure_mergeable_text(node::TEXT)?)),
            None => Ok(None),
        }
    }

    fn marked_text(&self, id: NodeId, range: &Range<usize>, key: &str) -> Result<LoroText> {
        let allowed = schema::mark(key).is_some_and(|m| !m.paragraph) || schema::is_addon_mark(key);
        if !allowed {
            return Err(ModelError::Invalid(format!(
                "`{key}` isn't a character mark"
            )));
        }
        let target = self
            .text(id, false)?
            .ok_or(ModelError::Invalid(format!("node {id} has no text")))?;
        let len = target.len_unicode();
        if range.start > range.end || range.end > len {
            return Err(ModelError::OutOfRange {
                what: "text range end",
                value: range.end,
                len,
            });
        }
        Ok(target)
    }
}

fn set_or_delete(map: &LoroMap, key: &str, value: &Value) -> Result<()> {
    match value {
        Value::Null => map.delete(key)?,
        value => map.insert(key, value.to_loro())?,
    }
    Ok(())
}

fn current_number(map: &LoroMap, key: &str) -> Option<f64> {
    match map.get(key)? {
        ValueOrContainer::Value(LoroValue::Double(v)) => Some(v),
        ValueOrContainer::Value(LoroValue::I64(v)) => Some(v as f64),
        _ => None,
    }
}

fn check_addon_id(addon: &str) -> Result<()> {
    if addon.contains('.') && !addon.starts_with('.') && !addon.ends_with('.') {
        Ok(())
    } else {
        Err(ModelError::Invalid(format!(
            "`{addon}` isn't a reverse-DNS add-on id"
        )))
    }
}

/// The position of the first `\n` at or after `pos`.
fn find_newline(text: &LoroText, pos: usize) -> Result<Option<usize>> {
    const CHUNK: usize = 256;
    let len = text.len_unicode();
    let mut start = pos;
    while start < len {
        let end = (start + CHUNK).min(len);
        if let Some(offset) = text.slice(start, end)?.chars().position(|c| c == '\n') {
            return Ok(Some(start + offset));
        }
        start = end;
    }
    Ok(None)
}

fn map_at(map: &LoroMap, key: &str) -> Option<LoroMap> {
    match map.get(key)? {
        ValueOrContainer::Container(Container::Map(map)) => Some(map),
        _ => None,
    }
}
