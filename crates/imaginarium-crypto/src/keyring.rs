// SPDX-License-Identifier: AGPL-3.0-or-later

//! Keyrings (spec §6.4): epoch keys wrapped for passwords, recipients
//! (HPKE with X-Wing), folders and share links, plus the
//! [`KeyProvider`]s that unlock documents with them.

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hpke::kem::XWing;
use hpke::{Deserializable, Kem as _, OpModeR, OpModeS, Serializable};
use imaginarium_format::{
    DocId, FileHeader, KeyProvider, Limits, ObjectId, Record, RecordBuilder, RecordFlags,
    RecordOpener, RecordType, pb,
};
use prost::Message;
use unicode_normalization::UnicodeNormalization;
use zeroize::Zeroizing;

use crate::acl::AclState;
use crate::error::{CryptoError, Result};
use crate::keys::{EpochKey, EpochKeys, KeyId};
use crate::record::Suite1Opener;
use crate::rng::{Prefilled, Randomness};

type HpkeAead = hpke::aead::ChaCha20Poly1305;
type HpkeKdf = hpke::kdf::HkdfSha256;

/// Randomness X-Wing encapsulation consumes (HPKE base mode).
const XWING_ENCAP_RANDOMNESS: usize = 64;

/// Argon2id parameters for password wraps (spec §6.4.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Argon2Config {
    /// Memory in KiB.
    pub memory_kib: u32,
    /// Iterations.
    pub iterations: u32,
    /// Parallelism.
    pub parallelism: u32,
}

impl Default for Argon2Config {
    /// Spec §6.4.1 recommendation: 64 MiB, 3 iterations, 1 lane.
    fn default() -> Self {
        Self {
            memory_kib: 65_536,
            iterations: 3,
            parallelism: 1,
        }
    }
}

/// Readers reject parameters above these (spec §6.4.1: DoS protection).
const MAX_MEMORY_KIB: u32 = 1_048_576;
const MAX_ITERATIONS: u32 = 16;
const MAX_PARALLELISM: u32 = 16;

fn wrap_aad(doc_id: &DocId, epoch: u32, kind: pb::WrapKind) -> Vec<u8> {
    let mut aad = b"imaginarium.imag v1 wrap".to_vec();
    aad.extend_from_slice(&doc_id.0);
    aad.extend_from_slice(&epoch.to_le_bytes());
    aad.push(u8::try_from(kind as i32).unwrap_or(0));
    aad
}

fn hpke_info(doc_id: &DocId, epoch: u32) -> Vec<u8> {
    let mut info = b"imaginarium.imag v1 keyring".to_vec();
    info.extend_from_slice(&doc_id.0);
    info.extend_from_slice(&epoch.to_le_bytes());
    info
}

/// AEAD-wraps `DEK_e` under a 32-byte KEK (password, folder and link wraps).
fn aead_wrap(
    kek: &[u8; 32],
    doc_id: &DocId,
    key: &EpochKey,
    kind: pb::WrapKind,
    rng: &mut dyn Randomness,
) -> Result<(Vec<u8>, Vec<u8>)> {
    let cipher =
        XChaCha20Poly1305::new_from_slice(kek).map_err(|_| CryptoError::InvalidKey("KEK"))?;
    let mut nonce = [0u8; 24];
    rng.fill(&mut nonce)?;
    let aad = wrap_aad(doc_id, key.epoch(), kind);
    let ciphertext = cipher
        .encrypt(
            &XNonce::from(nonce),
            Payload {
                msg: key.bytes(),
                aad: &aad,
            },
        )
        .map_err(|_| CryptoError::Authentication)?;
    Ok((nonce.to_vec(), ciphertext))
}

fn aead_unwrap(kek: &[u8; 32], doc_id: &DocId, epoch: u32, wrap: &pb::KeyWrap) -> Result<EpochKey> {
    let cipher =
        XChaCha20Poly1305::new_from_slice(kek).map_err(|_| CryptoError::InvalidKey("KEK"))?;
    let nonce: [u8; 24] = wrap
        .nonce
        .as_slice()
        .try_into()
        .map_err(|_| CryptoError::InvalidInput("wrap nonce".into()))?;
    let aad = wrap_aad(doc_id, epoch, wrap.kind());
    let key = Zeroizing::new(
        cipher
            .decrypt(
                &XNonce::from(nonce),
                Payload {
                    msg: &wrap.ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| CryptoError::Authentication)?,
    );
    let key: [u8; 32] = key
        .as_slice()
        .try_into()
        .map_err(|_| CryptoError::InvalidKey("unwrapped epoch key"))?;
    Ok(EpochKey::from_bytes(epoch, key))
}

/// `Argon2id(NFC(password), salt, params)` → 32-byte KEK (spec §6.4.1).
fn password_kek(password: &str, params: &pb::Argon2Params) -> Result<Zeroizing<[u8; 32]>> {
    if params.memory_kib > MAX_MEMORY_KIB
        || params.iterations > MAX_ITERATIONS
        || params.parallelism > MAX_PARALLELISM
    {
        return Err(CryptoError::Kdf(
            "Argon2 parameters exceed the allowed limits".into(),
        ));
    }
    let argon2_params = Params::new(
        params.memory_kib,
        params.iterations,
        params.parallelism,
        Some(32),
    )
    .map_err(|err| CryptoError::Kdf(err.to_string()))?;
    // NFC, so the same password typed on any OS gives the same key.
    let normalized: Zeroizing<String> = Zeroizing::new(password.nfc().collect());
    let mut kek = Zeroizing::new([0u8; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, argon2_params)
        .hash_password_into(normalized.as_bytes(), &params.salt, kek.as_mut())
        .map_err(|err| CryptoError::Kdf(err.to_string()))?;
    Ok(kek)
}

/// Wraps `key` for a password (spec §6.4.1).
pub fn wrap_password(
    doc_id: &DocId,
    key: &EpochKey,
    password: &str,
    config: Argon2Config,
    rng: &mut dyn Randomness,
) -> Result<pb::KeyWrap> {
    let mut salt = vec![0u8; 16];
    rng.fill(&mut salt)?;
    let params = pb::Argon2Params {
        memory_kib: config.memory_kib,
        iterations: config.iterations,
        parallelism: config.parallelism,
        salt,
    };
    let kek = password_kek(password, &params)?;
    let (nonce, ciphertext) = aead_wrap(&kek, doc_id, key, pb::WrapKind::Password, rng)?;
    Ok(pb::KeyWrap {
        kind: pb::WrapKind::Password as i32,
        recipient_id: Vec::new(),
        argon2: Some(params),
        hpke_enc: Vec::new(),
        nonce,
        ciphertext,
    })
}

/// Unwraps a password wrap of `epoch`.
pub fn unwrap_password(
    doc_id: &DocId,
    epoch: u32,
    wrap: &pb::KeyWrap,
    password: &str,
) -> Result<EpochKey> {
    let params = wrap.argon2.as_ref().ok_or_else(|| {
        CryptoError::InvalidInput("password wrap without Argon2 parameters".into())
    })?;
    let kek = password_kek(password, params)?;
    aead_unwrap(&kek, doc_id, epoch, wrap)
}

/// Wraps `key` under a folder or share-link key (spec §6.4.3).
pub fn wrap_with_key(
    doc_id: &DocId,
    key: &EpochKey,
    kek: &[u8; 32],
    kind: pb::WrapKind,
    recipient_id: Vec<u8>,
    rng: &mut dyn Randomness,
) -> Result<pb::KeyWrap> {
    if !matches!(kind, pb::WrapKind::FolderKey | pb::WrapKind::LinkKey) {
        return Err(CryptoError::InvalidInput(
            "wrap_with_key is for folder and link keys".into(),
        ));
    }
    let (nonce, ciphertext) = aead_wrap(kek, doc_id, key, kind, rng)?;
    Ok(pb::KeyWrap {
        kind: kind as i32,
        recipient_id,
        argon2: None,
        hpke_enc: Vec::new(),
        nonce,
        ciphertext,
    })
}

/// Unwraps a folder or link wrap of `epoch`.
pub fn unwrap_with_key(
    doc_id: &DocId,
    epoch: u32,
    wrap: &pb::KeyWrap,
    kek: &[u8; 32],
) -> Result<EpochKey> {
    aead_unwrap(kek, doc_id, epoch, wrap)
}

/// An X-Wing key pair for receiving wrapped keys (spec §6.4.2). The
/// private key is a 32-byte seed and is zeroized on drop by `x-wing`.
pub struct XWingKeyPair {
    private: <XWing as hpke::Kem>::PrivateKey,
    public: <XWing as hpke::Kem>::PublicKey,
}

impl XWingKeyPair {
    /// A new key pair from 32 bytes of seed randomness.
    pub fn generate(rng: &mut dyn Randomness) -> Result<Self> {
        let mut seed = Zeroizing::new([0u8; 32]);
        rng.fill(seed.as_mut())?;
        Ok(Self::from_seed(&seed))
    }

    /// The key pair derived from `seed` (deterministic; HPKE `DeriveKeyPair`).
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        let (private, public) = XWing::derive_keypair(seed);
        Self { private, public }
    }

    /// The private key's serialized form (32 bytes), for encrypted storage.
    pub fn private_bytes(&self) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(self.private.to_bytes().to_vec())
    }

    /// Restores a key pair from [`XWingKeyPair::private_bytes`].
    pub fn from_private_bytes(bytes: &[u8]) -> Result<Self> {
        let private = <XWing as hpke::Kem>::PrivateKey::from_bytes(bytes)
            .map_err(|_| CryptoError::InvalidKey("X-Wing private key"))?;
        let public = XWing::sk_to_pk(&private);
        Ok(Self { private, public })
    }

    /// The public key (1216 bytes), as stored in `Identity`/`Device`.
    pub fn public_bytes(&self) -> Vec<u8> {
        self.public.to_bytes().to_vec()
    }

    /// The recipient id: the key id of the public key.
    pub fn recipient_id(&self) -> KeyId {
        KeyId::of(&self.public_bytes())
    }
}

/// Wraps `key` for a recipient's X-Wing public key (spec §6.4.2).
pub fn wrap_hpke(
    doc_id: &DocId,
    key: &EpochKey,
    recipient_public: &[u8],
    rng: &mut dyn Randomness,
) -> Result<pb::KeyWrap> {
    let public = <XWing as hpke::Kem>::PublicKey::from_bytes(recipient_public)
        .map_err(|_| CryptoError::InvalidKey("X-Wing public key"))?;
    let recipient_id = KeyId::of(recipient_public);
    let mut encap_rng = rand_core::UnwrapErr(Prefilled::new(rng, XWING_ENCAP_RANDOMNESS)?);
    let (enc, ciphertext) = hpke::single_shot_seal_with_rng::<HpkeAead, HpkeKdf, XWing>(
        &OpModeS::Base,
        &public,
        &hpke_info(doc_id, key.epoch()),
        key.bytes(),
        &recipient_id.0,
        &mut encap_rng,
    )
    .map_err(|err| CryptoError::InvalidInput(format!("HPKE seal: {err:?}")))?;
    // The encapsulation must use exactly the randomness prepared for it. If
    // an `hpke` update draws more, `UnwrapErr` stops it (a panic, never weak
    // randomness); if it draws less, this catches the changed assumption.
    if encap_rng.0.remaining() != 0 {
        return Err(CryptoError::InvalidInput(
            "HPKE encapsulation used less randomness than expected".into(),
        ));
    }
    Ok(pb::KeyWrap {
        kind: pb::WrapKind::Hpke as i32,
        recipient_id: recipient_id.0.to_vec(),
        argon2: None,
        hpke_enc: enc.to_bytes().to_vec(),
        nonce: Vec::new(),
        ciphertext,
    })
}

/// Unwraps an HPKE wrap of `epoch` with the recipient's key pair.
pub fn unwrap_hpke(
    doc_id: &DocId,
    epoch: u32,
    wrap: &pb::KeyWrap,
    keypair: &XWingKeyPair,
) -> Result<EpochKey> {
    let enc = <XWing as hpke::Kem>::EncappedKey::from_bytes(&wrap.hpke_enc)
        .map_err(|_| CryptoError::InvalidInput("HPKE encapsulated key".into()))?;
    let plaintext = Zeroizing::new(
        hpke::single_shot_open::<HpkeAead, HpkeKdf, XWing>(
            &OpModeR::Base,
            &keypair.private,
            &enc,
            &hpke_info(doc_id, epoch),
            &wrap.ciphertext,
            &wrap.recipient_id,
        )
        .map_err(|_| CryptoError::Authentication)?,
    );
    let key: [u8; 32] = plaintext
        .as_slice()
        .try_into()
        .map_err(|_| CryptoError::InvalidKey("unwrapped epoch key"))?;
    Ok(EpochKey::from_bytes(epoch, key))
}

/// Encodes a keyring as an unencrypted `KEYR` record (spec §5.1). In
/// documents with an ACL chain, sign it afterwards (spec §7.3).
pub fn keyring_record(
    doc_id: &DocId,
    object_id: ObjectId,
    keyring: &pb::Keyring,
    will_be_signed: bool,
) -> imaginarium_format::Result<Record> {
    let mut builder = RecordBuilder::new(RecordType::KEYR, object_id);
    if will_be_signed {
        builder = builder.will_be_signed();
    }
    builder.build(doc_id, keyring.encode_to_vec(), None)
}

/// Decodes the keyrings in a document's key records (`KEYR` only).
///
/// If the records include an ACL chain (`ACLC`), the chain is replayed and
/// every keyring must be signed by an owner or admin of its epoch (spec
/// §6.4, §7.3). Otherwise whoever can append to the file (a Hub, say) could
/// slip in a key it knows for a new epoch.
pub fn keyrings(doc_id: &DocId, key_records: &[Record]) -> Result<Vec<pb::Keyring>> {
    let acl = acl_of(doc_id, key_records)?;
    decode_keyrings(doc_id, key_records, acl.as_ref())
}

/// The replayed ACL chain among a document's key records, if it has one.
fn acl_of(doc_id: &DocId, key_records: &[Record]) -> Result<Option<AclState>> {
    let has_acl = key_records
        .iter()
        .any(|record| record.rtype() == RecordType::ACLC);
    has_acl
        .then(|| AclState::replay(doc_id, key_records))
        .transpose()
}

fn decode_keyrings(
    doc_id: &DocId,
    key_records: &[Record],
    acl: Option<&AclState>,
) -> Result<Vec<pb::Keyring>> {
    key_records
        .iter()
        .filter(|record| record.rtype() == RecordType::KEYR)
        .map(|record| {
            if let Some(acl) = acl {
                acl.verify_keyring(record)?;
            }
            if record.flags().contains(RecordFlags::ENCRYPTED) {
                return Err(CryptoError::InvalidInput(
                    "KEYR records must not be encrypted".into(),
                ));
            }
            let content = record
                .open_content(doc_id, None, &Limits::default())
                .map_err(|err| CryptoError::InvalidInput(err.to_string()))?;
            pb::Keyring::decode(content.as_slice())
                .map_err(|err| CryptoError::InvalidInput(format!("keyring: {err}")))
        })
        .collect()
}

/// How many different Argon2 keys one [`PasswordKeys`] derives at most.
/// Every password wrap may have its own salt and parameters, and each
/// derivation can cost up to 1 GiB and 16 passes (spec §6.4.1). Without a
/// cap, a crafted file with many wraps would take hours to reject a wrong
/// password. Real files need one, or one per epoch.
pub const MAX_PASSWORD_DERIVATIONS: usize = 16;

/// The salt and parameters a password KEK is derived with.
type KekId = (Vec<u8>, u32, u32, u32);

/// Unlocks documents with a password ([`KeyProvider`]).
///
/// Each key derived from the password is kept for the provider's lifetime,
/// so wraps that share a salt, and repeated opens (recovery may unlock more
/// than once), run Argon2 once. At most [`MAX_PASSWORD_DERIVATIONS`]
/// different keys are derived; wraps beyond that count as not unwrappable.
pub struct PasswordKeys {
    password: Zeroizing<String>,
    keks: std::collections::HashMap<KekId, Zeroizing<[u8; 32]>>,
}

impl PasswordKeys {
    /// Uses `password` for all password wraps.
    pub fn new(password: &str) -> Self {
        Self {
            password: Zeroizing::new(password.to_owned()),
            keks: std::collections::HashMap::new(),
        }
    }

    fn unwrap(&mut self, doc_id: &DocId, epoch: u32, wrap: &pb::KeyWrap) -> Result<EpochKey> {
        let params = wrap.argon2.as_ref().ok_or_else(|| {
            CryptoError::InvalidInput("password wrap without Argon2 parameters".into())
        })?;
        let id = (
            params.salt.clone(),
            params.memory_kib,
            params.iterations,
            params.parallelism,
        );
        if !self.keks.contains_key(&id) {
            if self.keks.len() >= MAX_PASSWORD_DERIVATIONS {
                return Err(CryptoError::Kdf(format!(
                    "more than {MAX_PASSWORD_DERIVATIONS} different password keys in one document"
                )));
            }
            let kek = password_kek(&self.password, params)?;
            self.keks.insert(id.clone(), kek);
        }
        let kek = self
            .keks
            .get(&id)
            .ok_or_else(|| CryptoError::Kdf("password key missing".into()))?;
        aead_unwrap(kek, doc_id, epoch, wrap)
    }
}

impl KeyProvider for PasswordKeys {
    fn opener(
        &mut self,
        header: &FileHeader,
        key_records: &[Record],
    ) -> imaginarium_format::Result<Box<dyn RecordOpener>> {
        let keyrings = keyrings(&header.doc_id, key_records)?;
        unlock(keyrings, |epoch, wrap| {
            (wrap.kind() == pb::WrapKind::Password)
                .then(|| self.unwrap(&header.doc_id, epoch, wrap))
        })
    }
}

/// Unlocks documents with a folder or share-link key ([`KeyProvider`],
/// spec §6.4.3).
pub struct SharedKeys {
    kind: pb::WrapKind,
    recipient_id: Vec<u8>,
    key: Zeroizing<[u8; 32]>,
}

impl SharedKeys {
    /// Uses the key of folder `folder_id` for wraps addressed to it.
    pub fn folder(folder_id: Vec<u8>, key: [u8; 32]) -> Self {
        Self {
            kind: pb::WrapKind::FolderKey,
            recipient_id: folder_id,
            key: Zeroizing::new(key),
        }
    }

    /// Uses the key of share link `link_id` for wraps addressed to it.
    pub fn link(link_id: Vec<u8>, key: [u8; 32]) -> Self {
        Self {
            kind: pb::WrapKind::LinkKey,
            recipient_id: link_id,
            key: Zeroizing::new(key),
        }
    }
}

impl KeyProvider for SharedKeys {
    fn opener(
        &mut self,
        header: &FileHeader,
        key_records: &[Record],
    ) -> imaginarium_format::Result<Box<dyn RecordOpener>> {
        unlock(keyrings(&header.doc_id, key_records)?, |epoch, wrap| {
            (wrap.kind() == self.kind && wrap.recipient_id == self.recipient_id)
                .then(|| unwrap_with_key(&header.doc_id, epoch, wrap, &self.key))
        })
    }
}

/// Opens documents with epoch keys that are already known, for example
/// cached in the OS keychain after the first unlock, so the password's
/// Argon2 cost isn't paid on every open ([`KeyProvider`]).
pub struct KnownKeys {
    keys: EpochKeys,
}

impl KnownKeys {
    /// Uses `keys` as they are.
    pub fn new(keys: EpochKeys) -> Self {
        Self { keys }
    }
}

impl KeyProvider for KnownKeys {
    fn opener(
        &mut self,
        header: &FileHeader,
        key_records: &[Record],
    ) -> imaginarium_format::Result<Box<dyn RecordOpener>> {
        // Check the keyrings anyway: in documents with an ACL chain this
        // verifies that the key records are authentic.
        keyrings(&header.doc_id, key_records)?;
        if self.keys.is_empty() {
            return Err(CryptoError::NoMatchingWrap.into());
        }
        Ok(Box::new(Suite1Opener::new(self.keys.clone())))
    }
}

/// Unlocks documents with a device's or user's X-Wing key ([`KeyProvider`]).
///
/// Anyone can wrap a key to a public key. Recipient wraps therefore count
/// only in documents with an ACL chain whose `Genesis` was signed by the
/// expected owner, and only from keyrings that chain authorizes (spec
/// §6.4.2, ADR-0018). Otherwise whoever can write to the file could remove
/// or replace the chain and wrap a key they know.
pub struct RecipientKeys {
    keypair: XWingKeyPair,
    /// `None` accepts any owner (inspection tools only).
    owner: Option<KeyId>,
}

impl RecipientKeys {
    /// Uses `keypair` for HPKE wraps addressed to it, in documents whose ACL
    /// chain `owner` started: the user's own identity for files protected
    /// with account keys, or the owner named by the invitation for shared
    /// documents ([`AclState::genesis_owner`]).
    pub fn new(keypair: XWingKeyPair, owner: KeyId) -> Self {
        Self {
            keypair,
            owner: Some(owner),
        }
    }

    /// Like [`RecipientKeys::new`], but accepts a chain started by anyone.
    /// That proves nothing about who made the document, so this is only for
    /// tools that show the owner instead of trusting it, such as
    /// `imag verify`.
    pub fn trusting_any_owner(keypair: XWingKeyPair) -> Self {
        Self {
            keypair,
            owner: None,
        }
    }
}

impl KeyProvider for RecipientKeys {
    fn opener(
        &mut self,
        header: &FileHeader,
        key_records: &[Record],
    ) -> imaginarium_format::Result<Box<dyn RecordOpener>> {
        let acl = acl_of(&header.doc_id, key_records)?.ok_or_else(|| {
            CryptoError::Acl(
                "recipient keys need an ACL chain: without one, anyone could have \
                 wrapped the key (spec §6.4.2)"
                    .into(),
            )
        })?;
        if let Some(owner) = self.owner
            && acl.genesis_owner() != owner
        {
            return Err(CryptoError::Acl(format!(
                "the chain was started by {:?}, not by the expected owner {owner:?}",
                acl.genesis_owner()
            ))
            .into());
        }
        let own_id = self.keypair.recipient_id();
        let keyrings = decode_keyrings(&header.doc_id, key_records, Some(&acl))?;
        unlock(keyrings, |epoch, wrap| {
            (wrap.kind() == pb::WrapKind::Hpke && wrap.recipient_id == own_id.0)
                .then(|| unwrap_hpke(&header.doc_id, epoch, wrap, &self.keypair))
        })
    }
}

/// Collects every epoch key that `try_unwrap` can open. It must open at
/// least one. Wraps the credential doesn't address return `None`. Failures
/// of addressed wraps (wrong password) are errors.
fn unlock(
    keyrings: Vec<pb::Keyring>,
    mut try_unwrap: impl FnMut(u32, &pb::KeyWrap) -> Option<Result<EpochKey>>,
) -> imaginarium_format::Result<Box<dyn RecordOpener>> {
    let mut keys = EpochKeys::new();
    let mut last_error = None;
    for keyring in keyrings {
        for epoch in &keyring.epochs {
            for wrap in &epoch.wraps {
                match try_unwrap(epoch.epoch, wrap) {
                    Some(Ok(key)) => {
                        // Spec §6.4: all wraps of an epoch hold the same key.
                        // Otherwise which one counts would depend on order.
                        if let Ok(known) = keys.get(key.epoch())
                            && known.bytes() != key.bytes()
                        {
                            return Err(CryptoError::InvalidInput(format!(
                                "two wraps of epoch {} hold different keys",
                                key.epoch()
                            ))
                            .into());
                        }
                        keys.insert(key);
                    }
                    Some(Err(err)) => last_error = Some(err),
                    None => {}
                }
            }
        }
    }
    if keys.is_empty() {
        return Err(last_error.unwrap_or(CryptoError::NoMatchingWrap).into());
    }
    Ok(Box::new(Suite1Opener::new(keys)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::OsRandomness;

    const DOC: DocId = DocId([0x44; 16]);

    /// Fast parameters for tests only. Production uses the default.
    const FAST: Argon2Config = Argon2Config {
        memory_kib: 64,
        iterations: 1,
        parallelism: 1,
    };

    #[test]
    fn password_wrap_round_trip_and_wrong_password() {
        let key = EpochKey::from_bytes(1, [9; 32]);
        let wrap = wrap_password(&DOC, &key, "correct horse", FAST, &mut OsRandomness).unwrap();
        assert_eq!(
            unwrap_password(&DOC, 1, &wrap, "correct horse")
                .unwrap()
                .bytes(),
            &[9; 32]
        );
        assert_eq!(
            unwrap_password(&DOC, 1, &wrap, "wrong").unwrap_err(),
            CryptoError::Authentication
        );
        assert!(
            unwrap_password(&DOC, 2, &wrap, "correct horse").is_err(),
            "epoch is bound"
        );
        assert!(
            unwrap_password(&DocId([0; 16]), 1, &wrap, "correct horse").is_err(),
            "document is bound"
        );
    }

    #[test]
    fn passwords_are_nfc_normalized() {
        let key = EpochKey::from_bytes(1, [1; 32]);
        let composed = "caf\u{e9}"; // é as one code point
        let decomposed = "cafe\u{301}"; // e + combining acute
        let wrap = wrap_password(&DOC, &key, composed, FAST, &mut OsRandomness).unwrap();
        assert!(unwrap_password(&DOC, 1, &wrap, decomposed).is_ok());
    }

    #[test]
    fn hostile_argon2_parameters_are_rejected() {
        let key = EpochKey::from_bytes(1, [1; 32]);
        let mut wrap = wrap_password(&DOC, &key, "pw", FAST, &mut OsRandomness).unwrap();
        if let Some(params) = wrap.argon2.as_mut() {
            params.memory_kib = 4_000_000; // ~4 GiB
        }
        assert!(matches!(
            unwrap_password(&DOC, 1, &wrap, "pw"),
            Err(CryptoError::Kdf(_))
        ));
    }

    #[test]
    fn hpke_wrap_round_trip_and_wrong_recipient() {
        let alice = XWingKeyPair::generate(&mut OsRandomness).unwrap();
        let bob = XWingKeyPair::generate(&mut OsRandomness).unwrap();
        assert_eq!(alice.public_bytes().len(), 1216);
        let key = EpochKey::from_bytes(3, [3; 32]);
        let wrap = wrap_hpke(&DOC, &key, &alice.public_bytes(), &mut OsRandomness).unwrap();
        assert_eq!(wrap.recipient_id, alice.recipient_id().0.to_vec());
        assert_eq!(wrap.hpke_enc.len(), 1120);
        assert_eq!(
            unwrap_hpke(&DOC, 3, &wrap, &alice).unwrap().bytes(),
            &[3; 32]
        );
        assert!(unwrap_hpke(&DOC, 3, &wrap, &bob).is_err());
        assert!(
            unwrap_hpke(&DOC, 4, &wrap, &alice).is_err(),
            "epoch is bound via info"
        );
    }

    #[test]
    fn x_wing_private_keys_round_trip() {
        let pair = XWingKeyPair::from_seed(&[7; 32]);
        let restored = XWingKeyPair::from_private_bytes(&pair.private_bytes()).unwrap();
        assert_eq!(restored.public_bytes(), pair.public_bytes());
        assert_eq!(
            XWingKeyPair::from_seed(&[7; 32]).public_bytes(),
            pair.public_bytes(),
            "deterministic"
        );
    }

    #[test]
    fn folder_and_link_wraps() {
        let key = EpochKey::from_bytes(1, [5; 32]);
        let link_key = [0x11; 32];
        let wrap = wrap_with_key(
            &DOC,
            &key,
            &link_key,
            pb::WrapKind::LinkKey,
            b"link-1".to_vec(),
            &mut OsRandomness,
        )
        .unwrap();
        assert_eq!(
            unwrap_with_key(&DOC, 1, &wrap, &link_key).unwrap().bytes(),
            &[5; 32]
        );
        assert!(unwrap_with_key(&DOC, 1, &wrap, &[0x12; 32]).is_err());
        assert!(
            wrap_with_key(
                &DOC,
                &key,
                &link_key,
                pb::WrapKind::Password,
                vec![],
                &mut OsRandomness
            )
            .is_err()
        );
    }

    #[test]
    fn password_keys_derive_each_kek_once_and_only_a_few() {
        let key = EpochKey::from_bytes(1, [9; 32]);
        let header = FileHeader::new(DOC, true);
        let keyring = |wraps: Vec<pb::KeyWrap>| {
            let keyring = pb::Keyring {
                epochs: vec![pb::EpochKeys { epoch: 1, wraps }],
            };
            keyring_record(&DOC, ObjectId([1; 32]), &keyring, false).unwrap()
        };
        // The same salt twice, and two opens (recovery may unlock more than
        // once): Argon2 runs once.
        let wrap = wrap_password(&DOC, &key, "pw", FAST, &mut OsRandomness).unwrap();
        let record = keyring(vec![wrap.clone(), wrap]);
        let mut keys = PasswordKeys::new("pw");
        for _ in 0..2 {
            assert!(keys.opener(&header, std::slice::from_ref(&record)).is_ok());
        }
        assert_eq!(keys.keks.len(), 1);

        // A hostile file with many salts: a wrong password runs Argon2 a
        // bounded number of times, not once per wrap.
        let wraps = (0..40)
            .map(|_| wrap_password(&DOC, &key, "pw", FAST, &mut OsRandomness).unwrap())
            .collect();
        let mut wrong = PasswordKeys::new("nope");
        assert!(wrong.opener(&header, &[keyring(wraps)]).is_err());
        assert_eq!(wrong.keks.len(), MAX_PASSWORD_DERIVATIONS);
    }

    #[test]
    fn all_wraps_of_an_epoch_must_hold_the_same_key() {
        // Spec §6.4. Otherwise which key wins depends on keyring order.
        let header = FileHeader::new(DOC, true);
        let wraps = [[1; 32], [2; 32]]
            .map(|bytes| {
                let key = EpochKey::from_bytes(1, bytes);
                wrap_password(&DOC, &key, "pw", FAST, &mut OsRandomness).unwrap()
            })
            .to_vec();
        let keyring = pb::Keyring {
            epochs: vec![pb::EpochKeys { epoch: 1, wraps }],
        };
        let record = keyring_record(&DOC, ObjectId([1; 32]), &keyring, false).unwrap();
        let err = PasswordKeys::new("pw")
            .opener(&header, &[record])
            .err()
            .unwrap();
        assert!(err.to_string().contains("different keys"), "{err}");
    }

    #[test]
    fn providers_unlock_from_key_records() {
        let k1 = EpochKey::from_bytes(1, [1; 32]);
        let k2 = EpochKey::from_bytes(2, [2; 32]);
        let device = XWingKeyPair::generate(&mut OsRandomness).unwrap();
        let keyring = pb::Keyring {
            epochs: vec![
                pb::EpochKeys {
                    epoch: 1,
                    wraps: vec![wrap_password(&DOC, &k1, "pw", FAST, &mut OsRandomness).unwrap()],
                },
                pb::EpochKeys {
                    epoch: 2,
                    wraps: vec![
                        wrap_password(&DOC, &k2, "pw", FAST, &mut OsRandomness).unwrap(),
                        wrap_hpke(&DOC, &k2, &device.public_bytes(), &mut OsRandomness).unwrap(),
                    ],
                },
            ],
        };
        let record = keyring_record(&DOC, ObjectId([1; 32]), &keyring, false).unwrap();
        let header = FileHeader::new(DOC, true);
        assert!(
            PasswordKeys::new("pw")
                .opener(&header, std::slice::from_ref(&record))
                .is_ok()
        );
        assert!(
            PasswordKeys::new("nope")
                .opener(&header, std::slice::from_ref(&record))
                .is_err()
        );
        // Recipient wraps need an ACL chain (ADR-0018). This keyring has
        // none, so even the addressed device doesn't open it. The ACL tests
        // cover the accepted case.
        assert!(
            RecipientKeys::trusting_any_owner(device)
                .opener(&header, &[record])
                .err()
                .unwrap()
                .to_string()
                .contains("need an ACL chain")
        );
    }
}
