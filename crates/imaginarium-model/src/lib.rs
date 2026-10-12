// SPDX-License-Identifier: AGPL-3.0-or-later

//! The document model: schema v1 ([`docs/format/schema-v1.md`]) on the Loro
//! CRDT, with a materialized view model (ADR-0009).
//!
//! # Responsibility
//! - The **document**: [`Document`] holds the CRDT, which is the source of
//!   truth and syncs between peers. It creates, opens, exports and
//!   receives documents and updates.
//! - **One way to change it**: [`Document::transact`] runs typed edits
//!   ([`Transaction`]) as one CRDT transaction of an [`Actor`], who is
//!   recorded with the change. Commands, the AI assistant, add-ons and
//!   migrations all use it.
//! - The **view model**: [`DocumentView`], persistent `imbl` structures that
//!   readers on any thread use without locks. Each change updates it
//!   incrementally and reports a [`ChangeSet`] for fine-grained UI updates.
//! - **Undo per actor**: [`UndoScope`]s undo only their own changes. Other
//!   actors' and collaborators' edits stay.
//! - **Evolution**: [`Migration`]s and `required_features` decide whether
//!   a document opens editable or read-only ([`Mode`]).
//!
//! # Non-goals
//! - **No container format and no crypto.** `SNAP`/`UPDT` records, files and
//!   encryption are in `imaginarium-format` and `imaginarium-crypto`. This
//!   crate produces and consumes `loro-1` bytes ([`CODEC`]).
//! - **No commands, no UI.** Commands (`imaginarium-commands`) call
//!   [`Document::transact`]. The UI reads [`DocumentView`]s.
//! - **No threading.** A `Document` is used from one thread at a time
//!   (the model thread). Views can go anywhere.
//!
//! # Example
//! ```
//! use imaginarium_model::{Actor, Document, DocumentKind, Geometry, NodeKind, Parent, UndoScope};
//!
//! let mut doc = Document::new(DocumentKind::Board)?;
//! let (id, changes) = doc.transact(&Actor::User, |t| {
//!     let id = t.create_node(Parent::Root, None, &NodeKind::Text)?;
//!     t.set_geometry(id, Geometry::new(10.0, 10.0, 200.0, 40.0))?;
//!     t.insert_text(id, 0, "Hello")?;
//!     Ok(id)
//! })?;
//! assert_eq!(changes.created, vec![id]);
//! let node = doc.view().node(id).unwrap();
//! assert_eq!(node.text().unwrap().to_plain_string(), "Hello\n");
//!
//! doc.undo(UndoScope::User, &Actor::User)?;
//! assert!(doc.view().node(id).is_none());
//! # Ok::<(), imaginarium_model::ModelError>(())
//! ```
//!
//! [`docs/format/schema-v1.md`]: https://github.com/giosuel/imaginarium/blob/main/docs/format/schema-v1.md
#![forbid(unsafe_code)]

mod changes;
mod document;
mod error;
mod ids;
mod schema;
mod txn;
mod value;
pub mod view;

pub use changes::{ChangeOrigin, ChangeSet, NodeChanges};
pub use document::{Document, MIGRATIONS, Migration, Mode, OpenOptions, UndoConfig, Version};
pub use error::{ModelError, ReadOnlyReason, Result};
pub use ids::{Actor, BlobId, NodeId, Parent, StyleId, UndoScope};
pub use schema::{
    CODEC, DocumentKind, KNOWN_FEATURES, MARKS, MarkSpec, NodeKind, SCHEMA_VERSION, is_addon_mark,
    mark,
};
pub use txn::Transaction;
pub use value::{Geometry, Props, Value};
pub use view::DocumentView;
