// SPDX-License-Identifier: AGPL-3.0-or-later

//! Identifiers of the schema (schema-v1.md §2) and actors (§11).

use std::fmt;
use std::sync::Arc;

use loro::TreeID;

use crate::error::{ModelError, Result};

/// The largest stored id: 2^53 − 1, the largest integer JavaScript represents
/// exactly.
pub(crate) const MAX_STORED_ID: u64 = (1 << 53) - 1;

/// A node's identity (schema-v1.md §5.5).
///
/// Normally the random id stored in the node's `id` key: an integer below
/// 2^53, so it is exact in JavaScript too, written as 16 lowercase hex
/// characters. It survives undo and redo, which re-create
/// deleted nodes under new CRDT tree ids. Nodes without a stored id, and
/// the losers of a duplicate id, are identified by their tree id instead,
/// written `counter@peer`.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum NodeId {
    /// The stored `id`.
    Stored(u64),
    /// The CRDT tree id, for nodes without a usable stored id.
    Tree(TreeID),
}

impl NodeId {
    /// A new random id.
    pub fn generate() -> Result<Self> {
        loop {
            let mut bytes = [0u8; 8];
            getrandom::fill(&mut bytes)
                .map_err(|err| ModelError::Crdt(format!("no randomness: {err}")))?;
            let id = u64::from_le_bytes(bytes) & MAX_STORED_ID;
            if id != 0 {
                return Ok(NodeId::Stored(id));
            }
        }
    }

    /// The value stored in a node's `id` key, if it is a valid stored id.
    pub(crate) fn from_stored(value: i64) -> Option<Self> {
        u64::try_from(value)
            .ok()
            .filter(|id| *id != 0 && *id <= MAX_STORED_ID)
            .map(NodeId::Stored)
    }

    /// Parses the text form: 16 hex characters, or `counter@peer`.
    pub fn parse(text: &str) -> Option<Self> {
        if let Some((counter, peer)) = text.split_once('@') {
            return Some(NodeId::Tree(TreeID::new(
                peer.parse().ok()?,
                counter.parse().ok()?,
            )));
        }
        Self::parse_stored(text)
    }

    /// Parses the text form of a stored id (16 lowercase hex characters).
    fn parse_stored(text: &str) -> Option<Self> {
        if text.len() != 16
            || !text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return None;
        }
        u64::from_str_radix(text, 16).ok().map(NodeId::Stored)
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodeId::Stored(id) => write!(f, "{id:016x}"),
            NodeId::Tree(tree) => write!(f, "{}@{}", tree.counter, tree.peer),
        }
    }
}

impl fmt::Debug for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodeId({self})")
    }
}

/// Where a node sits: at the root of the tree, or under another node.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Parent {
    /// A root node (a board's top level, or a deck's slides).
    Root,
    /// A child of this node.
    Node(NodeId),
}

/// A style of the `styles` map (schema-v1.md §8).
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct StyleId(Arc<str>);

impl StyleId {
    /// A new random id: 32 lowercase hex characters.
    pub fn generate() -> Result<Self> {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes)
            .map_err(|err| ModelError::Crdt(format!("no randomness: {err}")))?;
        Ok(Self(hex(&bytes).into()))
    }

    /// An id from its text (any non-empty string).
    pub fn new(id: impl Into<Arc<str>>) -> Result<Self> {
        let id = id.into();
        if id.is_empty() {
            return Err(ModelError::Invalid("empty style id".into()));
        }
        Ok(Self(id))
    }

    /// The id's text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StyleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for StyleId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "StyleId({})", self.0)
    }
}

/// A blob: the `ObjectId` of its `BLOB` record (spec §5.2).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlobId(pub [u8; 32]);

impl BlobId {
    /// Parses 64 hex characters.
    pub fn parse(text: &str) -> Option<Self> {
        if text.len() != 64 || !text.is_ascii() {
            return None;
        }
        let mut out = [0u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&text[2 * i..2 * i + 2], 16).ok()?;
        }
        Some(Self(out))
    }
}

impl fmt::Display for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex(&self.0))
    }
}

impl fmt::Debug for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BlobId({self})")
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Who makes a transaction (schema-v1.md §11). It is recorded as the
/// change's commit message, and it decides which undo stack a change
/// belongs to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Actor {
    /// The person at the keyboard.
    User,
    /// The AI assistant, in a session.
    Ai(String),
    /// An add-on acting on its own (not a user command).
    Addon(String),
    /// A schema migration on open.
    Migration,
}

impl Actor {
    /// The commit message: `user`, `ai:<session>`, `addon:<id>`, `migration`.
    pub fn message(&self) -> String {
        match self {
            Actor::User => "user".into(),
            Actor::Ai(session) => format!("ai:{session}"),
            Actor::Addon(id) => format!("addon:{id}"),
            Actor::Migration => "migration".into(),
        }
    }

    /// Parses a commit message. `None` for messages of no known actor.
    pub fn from_message(message: &str) -> Option<Self> {
        match message {
            "user" => Some(Actor::User),
            "migration" => Some(Actor::Migration),
            _ => {
                if let Some(session) = message.strip_prefix("ai:") {
                    Some(Actor::Ai(session.into()))
                } else {
                    message
                        .strip_prefix("addon:")
                        .map(|id| Actor::Addon(id.into()))
                }
            }
        }
    }

    /// The undo stack this actor's changes go to, if any.
    pub fn undo_scope(&self) -> Option<UndoScope> {
        match self {
            Actor::User => Some(UndoScope::User),
            Actor::Ai(_) => Some(UndoScope::Assistant),
            Actor::Addon(_) | Actor::Migration => None,
        }
    }
}

/// An undo stack. Each undoes only its own actor's changes, and treats the
/// others' changes (and remote ones) like a collaborator's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum UndoScope {
    /// The user's own edits.
    User,
    /// The AI assistant's edits (all its sessions on this device).
    Assistant,
}

impl UndoScope {
    /// The origin of this stack's undo and redo commits.
    pub(crate) fn undo_origin(self) -> &'static str {
        match self {
            UndoScope::User => "undo:user",
            UndoScope::Assistant => "undo:ai",
        }
    }

    /// Origin prefixes this stack must not record.
    pub(crate) fn excluded_origins(self) -> &'static [&'static str] {
        match self {
            UndoScope::User => &["ai:", "addon:", "migration", "undo:ai"],
            UndoScope::Assistant => &["user", "addon:", "migration", "undo:user"],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip_through_text() {
        let node = NodeId::Tree(TreeID::new(1234567890, 12));
        assert_eq!(node.to_string(), "12@1234567890");
        assert_eq!(NodeId::parse("12@1234567890"), Some(node));
        let stored = NodeId::Stored(0x000f_3a2c_9b1d_47e5);
        assert_eq!(NodeId::from_stored(0x000f_3a2c_9b1d_47e5), Some(stored));
        assert_eq!(NodeId::from_stored(0), None);
        assert_eq!(
            NodeId::from_stored(1 << 53),
            None,
            "beyond what JavaScript holds exactly"
        );
        assert_eq!(NodeId::from_stored(-1), None);
        assert_eq!(stored.to_string(), "000f3a2c9b1d47e5");
        assert_eq!(NodeId::parse("000f3a2c9b1d47e5"), Some(stored));
        assert_eq!(NodeId::parse("12"), None);
        assert_eq!(NodeId::parse("00ABCDEF01234567"), None, "lowercase only");
        assert!(matches!(NodeId::generate().unwrap(), NodeId::Stored(_)));
        let blob = BlobId([0xab; 32]);
        assert_eq!(BlobId::parse(&blob.to_string()), Some(blob));
        assert_eq!(BlobId::parse("ab"), None);
        let style = StyleId::generate().unwrap();
        assert_eq!(style.as_str().len(), 32);
        assert!(StyleId::new("").is_err());
    }

    #[test]
    fn actors_round_trip_through_messages() {
        for actor in [
            Actor::User,
            Actor::Ai("7f".into()),
            Actor::Addon("org.example".into()),
            Actor::Migration,
        ] {
            assert_eq!(Actor::from_message(&actor.message()), Some(actor));
        }
        assert_eq!(Actor::from_message(""), None);
    }
}
