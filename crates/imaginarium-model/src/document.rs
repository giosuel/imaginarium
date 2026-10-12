// SPDX-License-Identifier: AGPL-3.0-or-later

//! A document: the CRDT, its view model and its undo stacks.

use std::sync::{Arc, Mutex, MutexGuard};

use loro::{
    ExpandType, ExportMode, LoroDoc, StyleConfig, Subscription, UndoManager, VersionVector,
};

use crate::changes::{ChangeOrigin, ChangeSet};
use crate::error::{ModelError, ReadOnlyReason, Result};
use crate::ids::{Actor, UndoScope};
use crate::schema::{self, DocumentKind, node, root};
use crate::txn::Transaction;
use crate::value::Value;
use crate::view::{DocumentView, RawEvent, apply_events, build};

/// Whether the document can be edited (schema-v1.md §10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Edits are allowed.
    Editable,
    /// Only viewing (and receiving remote changes) is allowed.
    ReadOnly(ReadOnlyReason),
}

/// Undo behaviour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UndoConfig {
    /// Transactions of the same stack within this many milliseconds form
    /// one undo step, so typing a word undoes as one. `0` makes every
    /// transaction its own step.
    pub merge_interval_ms: i64,
    /// The maximum number of undo steps per stack.
    pub max_steps: usize,
}

impl Default for UndoConfig {
    fn default() -> Self {
        Self {
            merge_interval_ms: 500,
            max_steps: 1000,
        }
    }
}

/// A schema migration from version `from` to `from + 1` (schema-v1.md
/// §10.3). It runs as one transaction of [`Actor::Migration`] and must be
/// deterministic, so that peers migrating concurrently converge.
#[derive(Clone, Copy)]
pub struct Migration {
    /// The schema version it migrates from.
    pub from: i64,
    /// The edits.
    pub apply: fn(&mut Transaction<'_>) -> Result<()>,
}

/// The migrations this version knows. Schema version 1 is the first.
pub const MIGRATIONS: &[Migration] = &[];

/// Options for [`Document::open_with`].
#[derive(Clone, Copy)]
pub struct OpenOptions<'a> {
    /// Available migrations.
    pub migrations: &'a [Migration],
    /// Features this reader understands.
    pub known_features: &'a [&'a str],
    /// Undo behaviour.
    pub undo: UndoConfig,
}

impl Default for OpenOptions<'_> {
    fn default() -> Self {
        Self {
            migrations: MIGRATIONS,
            known_features: schema::KNOWN_FEATURES,
            undo: UndoConfig::default(),
        }
    }
}

/// A version of the document: what it contains, for exporting the changes
/// another peer is missing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version(VersionVector);

/// A document: the `loro-1` CRDT (the source of truth), its view model and
/// its undo stacks. Every change goes through [`Document::transact`],
/// [`Document::apply_remote`] or undo, and returns a [`ChangeSet`].
pub struct Document {
    doc: LoroDoc,
    view: DocumentView,
    inbox: Arc<Mutex<Vec<RawEvent>>>,
    _subscription: Subscription,
    undo_user: UndoManager,
    undo_ai: UndoManager,
    /// The actor message of the undo or redo in progress (see
    /// [`undo_manager`]).
    undo_message: Arc<Mutex<String>>,
    mode: Mode,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn new_loro() -> LoroDoc {
    let doc = LoroDoc::new();
    doc.config_text_style(schema::text_styles());
    // Marks the schema doesn't list (add-on marks) never expand.
    doc.config_default_text_style(Some(StyleConfig {
        expand: ExpandType::None,
    }));
    // Change timestamps, for time-based history trimming (ADR-0016).
    doc.set_record_timestamp(true);
    doc.get_tree(root::NODES).enable_fractional_index(0);
    doc
}

/// An undo stack for `scope`.
///
/// Loro commits every step it pops separately. When a step turns out to be
/// a no-op (its target is gone), its empty commit uses up the pending commit
/// message and origin, so the step that follows would be committed as
/// Loro's anonymous `undo`: unattributed, and recorded by the other stack.
/// `on_pop` therefore sets them again after every popped step.
fn undo_manager(
    doc: &LoroDoc,
    scope: UndoScope,
    config: UndoConfig,
    message: Arc<Mutex<String>>,
) -> UndoManager {
    let mut manager = UndoManager::new(doc);
    for prefix in scope.excluded_origins() {
        manager.add_exclude_origin_prefix(prefix);
    }
    manager.set_merge_interval(config.merge_interval_ms);
    manager.set_max_undo_steps(config.max_steps);
    let doc = doc.clone();
    manager.set_on_pop(Some(Box::new(move |_, _, _| {
        doc.set_next_commit_message(&lock(&message));
        doc.set_next_commit_origin(scope.undo_origin());
    })));
    manager
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

impl Document {
    /// A new, empty document.
    pub fn new(kind: DocumentKind) -> Result<Self> {
        Self::new_with(kind, UndoConfig::default())
    }

    /// A new, empty document with the given undo behaviour.
    pub fn new_with(kind: DocumentKind, undo: UndoConfig) -> Result<Self> {
        let mut document = Self::wrap(new_loro(), undo);
        // The schema skeleton isn't an undoable user edit.
        document.transact(&Actor::Migration, |t| t.init_meta(kind, now_ms()))?;
        Ok(document)
    }

    /// Opens a document from its `loro-1` parts: a snapshot, then updates.
    pub fn open(parts: &[&[u8]]) -> Result<Self> {
        Self::open_with(parts, &OpenOptions::default())
    }

    /// Like [`Document::open`], with options.
    ///
    /// A document with a newer schema, without a migration path, or with
    /// unknown required features opens read-only ([`Document::mode`]).
    pub fn open_with(parts: &[&[u8]], options: &OpenOptions<'_>) -> Result<Self> {
        let doc = new_loro();
        let result = match parts {
            [] => return Err(ModelError::Decode("no document data".into())),
            [single] => doc.import(single).map(|_| ()),
            parts => doc
                .import_batch(&parts.iter().map(|p| p.to_vec()).collect::<Vec<_>>())
                .map(|_| ()),
        };
        result.map_err(|err| ModelError::Decode(err.to_string()))?;
        let mut document = Self::wrap(doc, options.undo);
        document.check_schema(options)?;
        Ok(document)
    }

    fn wrap(doc: LoroDoc, undo: UndoConfig) -> Self {
        let inbox = Arc::new(Mutex::new(Vec::new()));
        let sink = inbox.clone();
        let subscription = doc.subscribe_root(Arc::new(move |event| {
            lock(&sink).push(RawEvent::from_event(&event));
        }));
        let view = build(&doc);
        let undo_message = Arc::new(Mutex::new(String::new()));
        Self {
            undo_user: undo_manager(&doc, UndoScope::User, undo, undo_message.clone()),
            undo_ai: undo_manager(&doc, UndoScope::Assistant, undo, undo_message.clone()),
            undo_message,
            doc,
            view,
            inbox,
            _subscription: subscription,
            mode: Mode::Editable,
        }
    }

    fn check_schema(&mut self, options: &OpenOptions<'_>) -> Result<()> {
        let mut version = self.view.meta.schema_version.ok_or_else(|| {
            ModelError::Decode("not an Imaginarium document: meta.schema_version is missing".into())
        })?;
        if version > schema::SCHEMA_VERSION {
            self.mode = Mode::ReadOnly(ReadOnlyReason::NewerSchema(version));
            return Ok(());
        }
        while version < schema::SCHEMA_VERSION {
            let Some(migration) = options.migrations.iter().find(|m| m.from == version) else {
                self.mode = Mode::ReadOnly(ReadOnlyReason::NoMigration(version));
                return Ok(());
            };
            let next = version + 1;
            self.transact(&Actor::Migration, |t| {
                (migration.apply)(t)?;
                t.set_schema_version(next)
            })?;
            version = next;
        }
        let missing: Vec<String> = self
            .view
            .meta
            .required_features
            .iter()
            .filter(|f| !options.known_features.contains(&f.as_str()))
            .cloned()
            .collect();
        if !missing.is_empty() {
            self.mode = Mode::ReadOnly(ReadOnlyReason::MissingFeatures(missing));
        }
        Ok(())
    }

    /// The current view: cheap to clone, safe to send to other threads.
    pub fn view(&self) -> &DocumentView {
        &self.view
    }

    /// Whether the document can be edited.
    pub fn mode(&self) -> &Mode {
        &self.mode
    }

    /// This session's CRDT peer id. A new one is chosen on every open.
    pub fn peer(&self) -> u64 {
        self.doc.peer_id()
    }

    /// Runs `edit` as one transaction of `actor` and returns its result and
    /// the changes. The actor is recorded as the change's commit message
    /// (schema-v1.md §11).
    ///
    /// If `edit` fails after making changes, they are reverted, and the
    /// undo stacks don't record anything.
    pub fn transact<R>(
        &mut self,
        actor: &Actor,
        edit: impl FnOnce(&mut Transaction<'_>) -> Result<R>,
    ) -> Result<(R, ChangeSet)> {
        if let Mode::ReadOnly(reason) = &self.mode
            && *actor != Actor::Migration
        {
            return Err(ModelError::ReadOnly(reason.clone()));
        }
        let before = self.doc.state_frontiers();
        let mut txn = Transaction::new(&self.doc, &self.view);
        let result = edit(&mut txn).and_then(|value| txn.finish().map(|()| value));
        match result {
            Ok(value) => {
                self.commit(actor);
                Ok((value, self.drain(ChangeOrigin::Local(actor.clone()))))
            }
            Err(err) => {
                self.rollback(actor, &before)?;
                Err(err)
            }
        }
    }

    fn commit(&self, actor: &Actor) {
        let message = actor.message();
        self.doc.set_next_commit_message(&message);
        self.doc.set_next_commit_origin(&message);
        self.doc.commit();
    }

    fn rollback(&mut self, actor: &Actor, before: &loro::Frontiers) -> Result<()> {
        if self.doc.get_pending_txn_len() == 0 {
            return Ok(());
        }
        self.undo_user.pause();
        self.undo_ai.pause();
        self.commit(actor);
        let reverted = self.doc.revert_to(before);
        self.commit(actor);
        self.undo_user.resume();
        self.undo_ai.resume();
        self.drain(ChangeOrigin::Local(actor.clone()));
        reverted.map_err(ModelError::from)
    }

    fn drain(&mut self, origin: ChangeOrigin) -> ChangeSet {
        let events = std::mem::take(&mut *lock(&self.inbox));
        if events.is_empty() {
            return ChangeSet {
                origin,
                ..ChangeSet::default()
            };
        }
        let mut changes = apply_events(&mut self.view, &self.doc, events);
        changes.origin = origin;
        changes
    }

    /// Applies updates (or snapshots) from other peers, in one batch.
    /// Allowed in read-only documents too.
    pub fn apply_remote(&mut self, updates: &[&[u8]]) -> Result<ChangeSet> {
        let result = match updates {
            [] => Ok(()),
            [single] => self.doc.import(single).map(|_| ()),
            updates => self
                .doc
                .import_batch(&updates.iter().map(|u| u.to_vec()).collect::<Vec<_>>())
                .map(|_| ()),
        };
        result.map_err(|err| ModelError::Decode(err.to_string()))?;
        Ok(self.drain(ChangeOrigin::Remote))
    }

    fn manager(&mut self, scope: UndoScope) -> &mut UndoManager {
        match scope {
            UndoScope::User => &mut self.undo_user,
            UndoScope::Assistant => &mut self.undo_ai,
        }
    }

    /// Undoes the last step of `scope`. Changes of other actors and peers
    /// stay. `by` is recorded as the undo change's actor. `None` if there
    /// was nothing to undo.
    pub fn undo(&mut self, scope: UndoScope, by: &Actor) -> Result<Option<ChangeSet>> {
        self.undo_or_redo(scope, by, true)
    }

    /// Redoes the last undone step of `scope`.
    pub fn redo(&mut self, scope: UndoScope, by: &Actor) -> Result<Option<ChangeSet>> {
        self.undo_or_redo(scope, by, false)
    }

    fn undo_or_redo(
        &mut self,
        scope: UndoScope,
        by: &Actor,
        undo: bool,
    ) -> Result<Option<ChangeSet>> {
        if let Mode::ReadOnly(reason) = &self.mode {
            return Err(ModelError::ReadOnly(reason.clone()));
        }
        let message = by.message();
        *lock(&self.undo_message) = message.clone();
        loop {
            self.doc.set_next_commit_message(&message);
            self.doc.set_next_commit_origin(scope.undo_origin());
            let manager = self.manager(scope);
            let done = if undo { manager.undo() } else { manager.redo() };
            // `on_pop` set them again after the last step, too.
            self.doc.clear_next_commit_options();
            let done = done?;
            let mut changes = self.drain(ChangeOrigin::Undo(scope));
            let ghosts = self.remove_ghosts(&changes, &message, scope)?;
            if !done {
                return Ok(None);
            }
            if ghosts.is_empty() {
                return Ok(Some(changes));
            }
            changes.forget(&ghosts);
            if !changes.is_empty() {
                return Ok(Some(changes));
            }
            // The step only brought back an empty node: it was stale, like
            // the no-op steps Loro skips itself. Go on with the next one.
        }
    }

    /// Deletes the nodes an undo or redo brought back without any content.
    ///
    /// Undoing a move of a node revives the node if it was deleted since.
    /// If another actor's undo removed it, its properties went with it, and
    /// what comes back is an empty node without kind or id. The deletion is
    /// committed with the undo's origin, which neither stack records.
    fn remove_ghosts(
        &mut self,
        changes: &ChangeSet,
        message: &str,
        scope: UndoScope,
    ) -> Result<Vec<crate::NodeId>> {
        let tree = self.doc.get_tree(root::NODES);
        let mut ghosts = Vec::new();
        for &id in &changes.created {
            let Some(tid) = self.view.handles.get(&id).copied() else {
                continue;
            };
            if tree.get_meta(tid)?.get(node::KIND).is_none() {
                tree.delete(tid)?;
                ghosts.push(id);
            }
        }
        if !ghosts.is_empty() {
            self.doc.set_next_commit_message(message);
            self.doc.set_next_commit_origin(scope.undo_origin());
            self.doc.commit();
            self.drain(ChangeOrigin::Undo(scope));
        }
        Ok(ghosts)
    }

    /// Whether `scope` has a step to undo.
    pub fn can_undo(&self, scope: UndoScope) -> bool {
        match scope {
            UndoScope::User => self.undo_user.can_undo(),
            UndoScope::Assistant => self.undo_ai.can_undo(),
        }
    }

    /// Whether `scope` has a step to redo.
    pub fn can_redo(&self, scope: UndoScope) -> bool {
        match scope {
            UndoScope::User => self.undo_user.can_redo(),
            UndoScope::Assistant => self.undo_ai.can_redo(),
        }
    }

    /// Starts grouping `scope`'s transactions into one undo step (a drag,
    /// or an AI action made of several commands). End it with
    /// [`Document::end_undo_group`].
    pub fn begin_undo_group(&mut self, scope: UndoScope) -> Result<()> {
        self.manager(scope).group_start()?;
        Ok(())
    }

    /// Ends the group started with [`Document::begin_undo_group`].
    pub fn end_undo_group(&mut self, scope: UndoScope) {
        self.manager(scope).group_end();
    }

    /// The current version.
    pub fn version(&self) -> Version {
        Version(self.doc.oplog_vv())
    }

    /// A full snapshot (state and history), the payload of a `SNAP` record.
    pub fn export_snapshot(&self) -> Result<Vec<u8>> {
        self.doc
            .export(ExportMode::Snapshot)
            .map_err(|err| ModelError::Crdt(err.to_string()))
    }

    /// A snapshot without history (Loro shallow snapshot), for compaction
    /// under the retention policy (ADR-0016).
    pub fn export_shallow(&self) -> Result<Vec<u8>> {
        let frontiers = self.doc.oplog_frontiers();
        self.doc
            .export(ExportMode::shallow_snapshot(&frontiers))
            .map_err(|err| ModelError::Crdt(err.to_string()))
    }

    /// The changes since `version`, the payload of an `UPDT` record.
    pub fn export_updates_since(&self, version: &Version) -> Result<Vec<u8>> {
        self.doc
            .export(ExportMode::updates(&version.0))
            .map_err(|err| ModelError::Crdt(err.to_string()))
    }

    /// The whole document as one value (for debugging and `imag
    /// export-json`).
    pub fn to_value(&self) -> Value {
        Value::from_loro(&self.doc.get_deep_value())
    }

    /// Frees memory held for history operations (undo of old steps and
    /// time travel get slower until it's rebuilt). Useful after large
    /// imports.
    pub fn compact_memory(&self) {
        self.doc.compact_change_store();
        self.doc.free_history_cache();
    }

    /// The actor recorded for this session's newest change (schema-v1.md
    /// §11), for tests of attribution only.
    #[doc(hidden)]
    pub fn last_local_actor_for_tests(&self) -> Option<Actor> {
        let peer = self.doc.peer_id();
        let id = self
            .doc
            .oplog_frontiers()
            .iter()
            .find(|id| id.peer == peer)?;
        let message = self.doc.get_change(id)?.message?;
        Actor::from_message(&message)
    }

    /// Writes `meta.schema_version`, for tests of migrations only. The model
    /// itself never writes an older version.
    #[doc(hidden)]
    pub fn set_schema_version_for_tests(&mut self, version: i64) -> Result<()> {
        self.transact(&Actor::Migration, |t| t.set_schema_version(version))?;
        Ok(())
    }

    /// Builds the view from scratch, O(document size). The incremental view
    /// must always equal it; tests and debug builds use this to check.
    pub fn rebuild_view(&self) -> DocumentView {
        build(&self.doc)
    }
}
