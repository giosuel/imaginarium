// SPDX-License-Identifier: AGPL-3.0-or-later

//! Record signatures (spec §7): Ed25519 device keys for frequent records,
//! and hybrid Ed25519 + ML-DSA-65 identity keys for authority records.

use ed25519_dalek::Signer as _;
use imaginarium_format::{DocId, Record, RecordFlags, pb};
use ml_dsa::{Keypair as _, MlDsa65};
use prost::Message;
use zeroize::Zeroizing;

use crate::error::{CryptoError, Result};
use crate::keys::KeyId;
use crate::rng::{Prefilled, Randomness};

/// Ed25519 signature length.
pub const ED25519_SIGNATURE_LEN: usize = 64;
/// ML-DSA-65 signature length (FIPS 204).
pub const MLDSA65_SIGNATURE_LEN: usize = 3309;
/// ML-DSA-65 public key length (FIPS 204).
pub const MLDSA65_PUBLIC_KEY_LEN: usize = 1952;

/// A device's signing key (Ed25519). It signs `UPDT`, `SNAP`, `VERS`
/// (spec §7.3). Zeroized on drop by `ed25519-dalek`.
pub struct DeviceKey {
    key: ed25519_dalek::SigningKey,
}

impl DeviceKey {
    /// A new random device key.
    pub fn generate(rng: &mut dyn Randomness) -> Result<Self> {
        let mut seed = Zeroizing::new([0u8; 32]);
        rng.fill(seed.as_mut())?;
        Ok(Self::from_seed(&seed))
    }

    /// The key for a 32-byte seed (the Ed25519 secret key).
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self {
            key: ed25519_dalek::SigningKey::from_bytes(seed),
        }
    }

    /// The seed, for encrypted storage in the key bundle.
    pub fn seed(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(self.key.to_bytes())
    }

    /// The public key, as stored in `Device.ed25519_public_key`.
    pub fn public(&self) -> DevicePublic {
        DevicePublic(self.key.verifying_key().to_bytes())
    }
}

/// A user identity's signing keys (Ed25519 + ML-DSA-65). It signs `ACLC`
/// and `KEYR` (spec §7.3). Both parts are zeroized on drop by their crates.
pub struct IdentityKey {
    ed25519: ed25519_dalek::SigningKey,
    mldsa: ml_dsa::SigningKey<MlDsa65>,
}

impl IdentityKey {
    /// A new random identity.
    pub fn generate(rng: &mut dyn Randomness) -> Result<Self> {
        let mut seeds = Zeroizing::new([0u8; 64]);
        rng.fill(seeds.as_mut())?;
        let mut ed = [0u8; 32];
        let mut ml = [0u8; 32];
        ed.copy_from_slice(&seeds[..32]);
        ml.copy_from_slice(&seeds[32..]);
        let identity = Self::from_seeds(&ed, &ml);
        zeroize::Zeroize::zeroize(&mut ed);
        zeroize::Zeroize::zeroize(&mut ml);
        Ok(identity)
    }

    /// The identity for two 32-byte seeds (Ed25519 secret, ML-DSA ξ).
    pub fn from_seeds(ed25519_seed: &[u8; 32], mldsa_seed: &[u8; 32]) -> Self {
        Self {
            ed25519: ed25519_dalek::SigningKey::from_bytes(ed25519_seed),
            mldsa: ml_dsa::SigningKey::<MlDsa65>::from_seed(&ml_dsa::B32::from(*mldsa_seed)),
        }
    }

    /// Both seeds (Ed25519, ML-DSA), for encrypted storage in the key bundle.
    pub fn seeds(&self) -> Zeroizing<[u8; 64]> {
        let mut out = Zeroizing::new([0u8; 64]);
        out[..32].copy_from_slice(&self.ed25519.to_bytes());
        out[32..].copy_from_slice(self.mldsa.as_seed());
        out
    }

    /// The public keys, as stored in `Identity`.
    pub fn public(&self) -> IdentityPublic {
        IdentityPublic {
            ed25519: self.ed25519.verifying_key().to_bytes(),
            mldsa65: self.mldsa.verifying_key().encode().to_vec(),
        }
    }

    fn sign(&self, message: &[u8], rng: &mut dyn Randomness) -> Result<Vec<u8>> {
        let ed = self.ed25519.sign(message);
        // Hedged ML-DSA (FIPS 204 recommends randomized signing against
        // side-channel and fault attacks). It consumes 32 random bytes.
        let mut mldsa_rng = Prefilled::new(rng, 32)?;
        let ml = self
            .mldsa
            .expanded_key()
            .sign_randomized(message, b"", &mut mldsa_rng)
            .map_err(|_| CryptoError::Signature("ML-DSA signing failed".into()))?;
        let mut out = Vec::with_capacity(ED25519_SIGNATURE_LEN + MLDSA65_SIGNATURE_LEN);
        out.extend_from_slice(&ed.to_bytes());
        out.extend_from_slice(&ml.encode());
        Ok(out)
    }
}

/// A device's public key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DevicePublic(pub [u8; 32]);

impl DevicePublic {
    /// The key id (spec §7.1).
    pub fn key_id(&self) -> KeyId {
        KeyId::of(&self.0)
    }

    /// Parses `Device.ed25519_public_key`.
    pub fn from_slice(bytes: &[u8]) -> Result<Self> {
        bytes
            .try_into()
            .map(Self)
            .map_err(|_| CryptoError::InvalidKey("Ed25519 public key"))
    }

    fn verify(&self, message: &[u8], signature: &[u8]) -> Result<()> {
        let key = ed25519_dalek::VerifyingKey::from_bytes(&self.0)
            .map_err(|_| CryptoError::InvalidKey("Ed25519 public key"))?;
        let signature = ed25519_dalek::Signature::from_slice(signature)
            .map_err(|_| CryptoError::Signature("malformed Ed25519 signature".into()))?;
        // `verify_strict` also rejects non-canonical signatures and weak keys.
        key.verify_strict(message, &signature)
            .map_err(|_| CryptoError::Signature("Ed25519 verification failed".into()))
    }
}

/// An identity's public keys.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdentityPublic {
    /// Ed25519 public key.
    pub ed25519: [u8; 32],
    /// ML-DSA-65 public key (1952 bytes).
    pub mldsa65: Vec<u8>,
}

impl IdentityPublic {
    /// The key id: BLAKE3 of `ed25519 ‖ mldsa65`, first 16 bytes (spec §7.1).
    pub fn key_id(&self) -> KeyId {
        let mut bytes = self.ed25519.to_vec();
        bytes.extend_from_slice(&self.mldsa65);
        KeyId::of(&bytes)
    }

    /// From the signature keys of a protobuf `Identity`.
    pub fn from_pb(identity: &pb::Identity) -> Result<Self> {
        let ed25519 = identity
            .ed25519_public_key
            .as_slice()
            .try_into()
            .map_err(|_| CryptoError::InvalidKey("identity Ed25519 public key"))?;
        if identity.mldsa65_public_key.len() != MLDSA65_PUBLIC_KEY_LEN {
            return Err(CryptoError::InvalidKey("identity ML-DSA-65 public key"));
        }
        Ok(Self {
            ed25519,
            mldsa65: identity.mldsa65_public_key.clone(),
        })
    }

    fn verify(&self, message: &[u8], signature: &[u8]) -> Result<()> {
        if signature.len() != ED25519_SIGNATURE_LEN + MLDSA65_SIGNATURE_LEN {
            return Err(CryptoError::Signature(
                "hybrid signature has the wrong length".into(),
            ));
        }
        let (ed, ml) = signature.split_at(ED25519_SIGNATURE_LEN);
        DevicePublic(self.ed25519).verify(message, ed)?;
        let encoded_key = ml_dsa::EncodedVerifyingKey::<MlDsa65>::try_from(self.mldsa65.as_slice())
            .map_err(|_| CryptoError::InvalidKey("ML-DSA-65 public key"))?;
        let key = ml_dsa::VerifyingKey::<MlDsa65>::decode(&encoded_key);
        let encoded_sig = ml_dsa::EncodedSignature::<MlDsa65>::try_from(ml)
            .map_err(|_| CryptoError::Signature("malformed ML-DSA signature".into()))?;
        let sig = ml_dsa::Signature::<MlDsa65>::decode(&encoded_sig)
            .ok_or_else(|| CryptoError::Signature("malformed ML-DSA signature".into()))?;
        // Both halves must verify (spec §7.2).
        if key.verify_with_context(message, b"", &sig) {
            Ok(())
        } else {
            Err(CryptoError::Signature(
                "ML-DSA-65 verification failed".into(),
            ))
        }
    }
}

/// Who signs a record.
pub enum Signer<'a> {
    /// A device (Ed25519).
    Device(&'a DeviceKey),
    /// A user identity (Ed25519 + ML-DSA-65).
    Identity(&'a IdentityKey),
}

/// Whose signature a verifier expects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verifier {
    /// A device key.
    Device(DevicePublic),
    /// An identity key.
    Identity(IdentityPublic),
}

impl Verifier {
    /// The expected signer's key id.
    pub fn key_id(&self) -> KeyId {
        match self {
            Verifier::Device(public) => public.key_id(),
            Verifier::Identity(public) => public.key_id(),
        }
    }
}

/// Signs a record (spec §7.2). Encrypted records must have been built with
/// `RecordBuilder::will_be_signed`, because the AAD covers the flags.
pub fn sign_record(
    record: Record,
    doc_id: &DocId,
    signer: Signer<'_>,
    rng: &mut dyn Randomness,
) -> Result<Record> {
    if record.signature_bytes().is_some() {
        return Err(CryptoError::InvalidInput("record is already signed".into()));
    }
    if record.flags().contains(RecordFlags::ENCRYPTED)
        && !record.flags().contains(RecordFlags::SIGNED)
    {
        return Err(CryptoError::InvalidInput(
            "encrypted records must be built with will_be_signed() before encryption".into(),
        ));
    }
    let message = record.signing_input(doc_id);
    let (algorithm, key_id, signature) = match signer {
        Signer::Device(key) => (
            pb::SignatureAlgorithm::Ed25519,
            key.public().key_id(),
            key.key.sign(&message).to_bytes().to_vec(),
        ),
        Signer::Identity(key) => (
            pb::SignatureAlgorithm::Ed25519Mldsa65,
            key.public().key_id(),
            key.sign(&message, rng)?,
        ),
    };
    let signature = pb::Signature {
        algorithm: algorithm as i32,
        key_id: key_id.0.to_vec(),
        signature,
    };
    Ok(record.with_signature(signature.encode_to_vec()))
}

/// The key id a record claims to be signed by (to look up the signer).
pub fn signer_key_id(record: &Record) -> Result<KeyId> {
    let signature = decode_signature(record)?;
    KeyId::from_slice(&signature.key_id)
        .ok_or_else(|| CryptoError::Signature("malformed key id".into()))
}

/// Verifies that `record` is signed by `expected` (spec §7.2).
pub fn verify_record(record: &Record, doc_id: &DocId, expected: &Verifier) -> Result<()> {
    let signature = decode_signature(record)?;
    if signature.key_id != expected.key_id().0 {
        return Err(CryptoError::Signature("signed by a different key".into()));
    }
    let message = record.signing_input(doc_id);
    match (signature.algorithm(), expected) {
        (pb::SignatureAlgorithm::Ed25519, Verifier::Device(public)) => {
            public.verify(&message, &signature.signature)
        }
        (pb::SignatureAlgorithm::Ed25519Mldsa65, Verifier::Identity(public)) => {
            public.verify(&message, &signature.signature)
        }
        _ => Err(CryptoError::Signature(
            "unexpected signature algorithm for this signer".into(),
        )),
    }
}

fn decode_signature(record: &Record) -> Result<pb::Signature> {
    record
        .signature()
        .map_err(|err| CryptoError::Signature(err.to_string()))?
        .ok_or_else(|| CryptoError::Signature("record is not signed".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::Suite1Sealer;
    use crate::rng::OsRandomness;
    use crate::{EpochKey, Suite1Opener};
    use imaginarium_format::{Limits, ObjectId, RecordBuilder, RecordType};

    const DOC: DocId = DocId([0x55; 16]);

    fn update() -> Record {
        RecordBuilder::new(RecordType::UPDT, ObjectId([1; 32]))
            .stream(pb::Stream::Main)
            .build(&DOC, b"edit".to_vec(), None)
            .unwrap()
    }

    #[test]
    fn device_signatures_verify_and_bind_everything() {
        let device = DeviceKey::generate(&mut OsRandomness).unwrap();
        let signed =
            sign_record(update(), &DOC, Signer::Device(&device), &mut OsRandomness).unwrap();
        let verifier = Verifier::Device(device.public());
        verify_record(&signed, &DOC, &verifier).unwrap();
        assert_eq!(signer_key_id(&signed).unwrap(), device.public().key_id());
        assert!(
            verify_record(&signed, &DocId([0; 16]), &verifier).is_err(),
            "document is bound"
        );
        let other = DeviceKey::generate(&mut OsRandomness).unwrap();
        assert!(verify_record(&signed, &DOC, &Verifier::Device(other.public())).is_err());

        // Tamper with the payload, keep the signature.
        let tampered = Record::new(
            signed.rtype(),
            signed.flags(),
            signed.header().clone(),
            b"evil".to_vec(),
        )
        .with_signature(signed.signature_bytes().unwrap().to_vec());
        assert!(verify_record(&tampered, &DOC, &verifier).is_err());
        assert!(
            verify_record(&update(), &DOC, &verifier).is_err(),
            "unsigned"
        );
    }

    #[test]
    fn hybrid_signatures_need_both_halves() {
        let identity = IdentityKey::generate(&mut OsRandomness).unwrap();
        let signed = sign_record(
            update(),
            &DOC,
            Signer::Identity(&identity),
            &mut OsRandomness,
        )
        .unwrap();
        let verifier = Verifier::Identity(identity.public());
        verify_record(&signed, &DOC, &verifier).unwrap();
        assert_eq!(identity.public().mldsa65.len(), MLDSA65_PUBLIC_KEY_LEN);

        let mut signature = pb::Signature::decode(signed.signature_bytes().unwrap()).unwrap();
        assert_eq!(
            signature.signature.len(),
            ED25519_SIGNATURE_LEN + MLDSA65_SIGNATURE_LEN
        );
        for (what, index) in [("Ed25519 half", 10), ("ML-DSA half", 64 + 100)] {
            signature.signature[index] ^= 1;
            let broken = Record::new(
                signed.rtype(),
                signed.flags(),
                signed.header().clone(),
                signed.payload().to_vec(),
            )
            .with_signature(signature.encode_to_vec());
            assert!(
                verify_record(&broken, &DOC, &verifier).is_err(),
                "{what} must be checked"
            );
            signature.signature[index] ^= 1;
        }
    }

    #[test]
    fn identity_and_device_algorithms_are_not_interchangeable() {
        let device = DeviceKey::from_seed(&[1; 32]);
        let signed =
            sign_record(update(), &DOC, Signer::Device(&device), &mut OsRandomness).unwrap();
        let identity = IdentityKey::from_seeds(&[1; 32], &[2; 32]);
        // Same Ed25519 key, but an identity verifier requires the hybrid algorithm.
        assert!(verify_record(&signed, &DOC, &Verifier::Identity(identity.public())).is_err());
    }

    #[test]
    fn encrypted_records_must_be_prepared_for_signing() {
        let key = EpochKey::from_bytes(1, [1; 32]);
        let sealer = Suite1Sealer::new(&key, OsRandomness);
        let device = DeviceKey::generate(&mut OsRandomness).unwrap();
        let unprepared = RecordBuilder::new(RecordType::UPDT, ObjectId([2; 32]))
            .build(&DOC, b"x".to_vec(), Some(&sealer))
            .unwrap();
        assert!(sign_record(unprepared, &DOC, Signer::Device(&device), &mut OsRandomness).is_err());

        let prepared = RecordBuilder::new(RecordType::UPDT, ObjectId([2; 32]))
            .will_be_signed()
            .build(&DOC, b"secret".to_vec(), Some(&sealer))
            .unwrap();
        let signed =
            sign_record(prepared, &DOC, Signer::Device(&device), &mut OsRandomness).unwrap();
        verify_record(&signed, &DOC, &Verifier::Device(device.public())).unwrap();
        let mut keys = crate::EpochKeys::new();
        keys.insert(key);
        let opener = Suite1Opener::new(keys);
        assert_eq!(
            signed
                .open_content(&DOC, Some(&opener), &Limits::default())
                .unwrap(),
            b"secret",
            "still decrypts"
        );
    }

    #[test]
    fn keys_round_trip_through_their_seeds() {
        let identity = IdentityKey::generate(&mut OsRandomness).unwrap();
        let seeds = identity.seeds();
        let mut ed = [0u8; 32];
        let mut ml = [0u8; 32];
        ed.copy_from_slice(&seeds[..32]);
        ml.copy_from_slice(&seeds[32..]);
        assert_eq!(
            IdentityKey::from_seeds(&ed, &ml).public(),
            identity.public()
        );
        let device = DeviceKey::generate(&mut OsRandomness).unwrap();
        assert_eq!(
            DeviceKey::from_seed(&device.seed()).public(),
            device.public()
        );
    }
}
