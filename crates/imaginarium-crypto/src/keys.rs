// SPDX-License-Identifier: AGPL-3.0-or-later

//! The key hierarchy of suite 1 (spec §6.2) and key ids (spec §7.1).

use std::collections::BTreeMap;

use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

use crate::error::{CryptoError, Result};
use crate::rng::Randomness;

/// BLAKE3 `derive_key` contexts (spec §6.2). Never change them.
pub(crate) const CONTEXT_CONTENT: &str = "imaginarium.imag v1 content key";
pub(crate) const CONTEXT_BLOB: &str = "imaginarium.imag v1 blob key";
pub(crate) const CONTEXT_PRESENCE: &str = "imaginarium.imag v1 presence key";

/// What a subkey is used for (spec §6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// `META`, `SNAP`, `UPDT`, `VERS`, `INDX`, extension records.
    Content,
    /// `BLOB`, `DERV`, `THMB`.
    Blob,
    /// Ephemeral live messages (sync protocol).
    Presence,
}

/// The random 32-byte key of one epoch, `DEK_e` (spec §6.2). Zeroized on drop.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct EpochKey {
    #[zeroize(skip)]
    epoch: u32,
    key: [u8; 32],
}

impl EpochKey {
    /// A new random key for `epoch` (≥ 1).
    pub fn generate(epoch: u32, rng: &mut dyn Randomness) -> Result<Self> {
        if epoch == 0 {
            return Err(CryptoError::InvalidInput("epochs start at 1".into()));
        }
        let mut key = [0u8; 32];
        rng.fill(&mut key)?;
        Ok(Self { epoch, key })
    }

    /// Wraps existing key bytes (e.g. just unwrapped from a keyring).
    pub fn from_bytes(epoch: u32, key: [u8; 32]) -> Self {
        Self { epoch, key }
    }

    /// The epoch number.
    pub fn epoch(&self) -> u32 {
        self.epoch
    }

    /// The raw key (for wrapping only).
    pub(crate) fn bytes(&self) -> &[u8; 32] {
        &self.key
    }

    /// `K_x = BLAKE3-derive_key(context, DEK_e)` (spec §6.2).
    pub fn subkey(&self, purpose: Purpose) -> Zeroizing<[u8; 32]> {
        let context = match purpose {
            Purpose::Content => CONTEXT_CONTENT,
            Purpose::Blob => CONTEXT_BLOB,
            Purpose::Presence => CONTEXT_PRESENCE,
        };
        Zeroizing::new(blake3::derive_key(context, &self.key))
    }
}

impl std::fmt::Debug for EpochKey {
    /// Never prints the key.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EpochKey")
            .field("epoch", &self.epoch)
            .finish_non_exhaustive()
    }
}

/// All epoch keys a reader holds (spec §6.2: old records stay readable).
#[derive(Clone, Debug, Default)]
pub struct EpochKeys {
    keys: BTreeMap<u32, EpochKey>,
}

impl EpochKeys {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a key (replacing one of the same epoch).
    pub fn insert(&mut self, key: EpochKey) {
        self.keys.insert(key.epoch(), key);
    }

    /// The key of `epoch`.
    pub fn get(&self, epoch: u32) -> Result<&EpochKey> {
        self.keys
            .get(&epoch)
            .ok_or(CryptoError::MissingEpoch(epoch))
    }

    /// The newest epoch's key: new content is encrypted under it.
    pub fn current(&self) -> Option<&EpochKey> {
        self.keys.values().next_back()
    }

    /// Number of epochs held.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// Whether no keys are held.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

/// A 16-byte key id: the first 16 bytes of BLAKE3 of the public key bytes
/// (spec §7.1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct KeyId(pub [u8; 16]);

impl KeyId {
    /// The id of a public key (or of the concatenation spec §7.1 prescribes).
    pub fn of(public_key_bytes: &[u8]) -> Self {
        let hash = blake3::hash(public_key_bytes);
        let mut id = [0u8; 16];
        id.copy_from_slice(&hash.as_bytes()[..16]);
        Self(id)
    }

    /// Parses 16 bytes.
    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        bytes.try_into().ok().map(Self)
    }
}

impl std::fmt::Debug for KeyId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KeyId(")?;
        self.0.iter().try_for_each(|b| write!(f, "{b:02x}"))?;
        f.write_str(")")
    }
}

/// The content-addressed id of a blob (spec §5.2):
/// `BLAKE3-keyed(blob_id_key, plaintext)`.
pub fn blob_id(blob_id_key: &[u8; 32], plaintext: &[u8]) -> imaginarium_format::ObjectId {
    imaginarium_format::ObjectId(*blake3::keyed_hash(blob_id_key, plaintext).as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subkeys_follow_the_spec_contexts() {
        let key = EpochKey::from_bytes(1, [7; 32]);
        assert_eq!(
            *key.subkey(Purpose::Content),
            blake3::derive_key("imaginarium.imag v1 content key", &[7; 32])
        );
        assert_ne!(*key.subkey(Purpose::Content), *key.subkey(Purpose::Blob));
        assert_ne!(*key.subkey(Purpose::Blob), *key.subkey(Purpose::Presence));
    }

    #[test]
    fn epoch_keys_track_the_current_epoch() {
        let mut keys = EpochKeys::new();
        keys.insert(EpochKey::from_bytes(2, [2; 32]));
        keys.insert(EpochKey::from_bytes(1, [1; 32]));
        assert_eq!(keys.current().unwrap().epoch(), 2);
        assert_eq!(keys.get(1).unwrap().epoch(), 1);
        assert_eq!(keys.get(3).unwrap_err(), CryptoError::MissingEpoch(3));
        assert!(EpochKey::generate(0, &mut crate::rng::OsRandomness).is_err());
    }

    #[test]
    fn debug_never_prints_keys() {
        let key = EpochKey::from_bytes(1, [0xAB; 32]);
        assert!(!format!("{key:?}").contains("ab"));
    }

    #[test]
    fn blob_ids_are_keyed() {
        let data = b"same file";
        assert_eq!(blob_id(&[1; 32], data), blob_id(&[1; 32], data));
        assert_ne!(
            blob_id(&[1; 32], data),
            blob_id(&[2; 32], data),
            "a different document → a different id"
        );
        assert_eq!(KeyId::of(b"pk").0, blake3::hash(b"pk").as_bytes()[..16]);
    }
}
