// SPDX-License-Identifier: AGPL-3.0-or-later

//! Suite 1 record encryption (spec §6.3): XChaCha20-Poly1305 with the
//! format's AAD, and the STREAM nonce construction for blob segments.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use imaginarium_format::{
    DocId, FormatError, ObjectId, Record, RecordBuilder, RecordOpener, RecordSealer, RecordType,
    SEGMENT_LEN, SEGMENT_THRESHOLD, pb,
};

use crate::error::CryptoError;
use crate::keys::{EpochKey, EpochKeys, Purpose, blob_id};
use crate::rng::Randomness;

/// Suite id of suite 1 (spec §6.1).
pub const SUITE_1: u32 = 1;

/// Nonce length of XChaCha20-Poly1305.
const NONCE_LEN: usize = 24;
/// STREAM nonce prefix length (spec §6.3.2): 24 − 4 (counter) − 1 (last flag).
const PREFIX_LEN: usize = 19;

/// Which subkey encrypts a record type (spec §6.2).
fn purpose(rtype: RecordType) -> Purpose {
    match rtype {
        RecordType::BLOB | RecordType::DERV | RecordType::THMB => Purpose::Blob,
        _ => Purpose::Content,
    }
}

fn cipher(key: &EpochKey, rtype: RecordType) -> Result<XChaCha20Poly1305, CryptoError> {
    let subkey = key.subkey(purpose(rtype));
    XChaCha20Poly1305::new_from_slice(subkey.as_ref())
        .map_err(|_| CryptoError::InvalidKey("XChaCha20-Poly1305 key"))
}

/// The STREAM nonce of segment `index` of `count` (spec §6.3.2).
fn segment_nonce(segment: &pb::Segment) -> Result<XNonce, CryptoError> {
    if segment.nonce_prefix.len() != PREFIX_LEN {
        return Err(CryptoError::InvalidInput(format!(
            "STREAM nonce prefix must be {PREFIX_LEN} bytes, not {}",
            segment.nonce_prefix.len()
        )));
    }
    if segment.count == 0 || segment.index >= segment.count {
        return Err(CryptoError::InvalidInput(
            "segment index out of range".into(),
        ));
    }
    let mut nonce = [0u8; NONCE_LEN];
    nonce[..PREFIX_LEN].copy_from_slice(&segment.nonce_prefix);
    nonce[PREFIX_LEN..PREFIX_LEN + 4].copy_from_slice(&segment.index.to_be_bytes());
    nonce[NONCE_LEN - 1] = u8::from(segment.index + 1 == segment.count);
    Ok(XNonce::from(nonce))
}

/// Encrypts records under one epoch key (suite 1). Nonces come from the
/// given randomness: the OS in production, a seed for test vectors.
pub struct Suite1Sealer<'k, R: Randomness> {
    key: &'k EpochKey,
    rng: std::cell::RefCell<R>,
}

impl<'k, R: Randomness> Suite1Sealer<'k, R> {
    /// A sealer for new records under `key`, the current epoch's key.
    pub fn new(key: &'k EpochKey, rng: R) -> Self {
        Self {
            key,
            rng: std::cell::RefCell::new(rng),
        }
    }

    /// A fresh random STREAM nonce prefix for a new segmented blob.
    pub fn new_nonce_prefix(&self) -> Result<Vec<u8>, CryptoError> {
        let mut prefix = vec![0u8; PREFIX_LEN];
        self.rng.borrow_mut().fill(&mut prefix)?;
        Ok(prefix)
    }
}

impl<R: Randomness> RecordSealer for Suite1Sealer<'_, R> {
    fn suite(&self) -> u32 {
        SUITE_1
    }

    fn epoch(&self) -> u32 {
        self.key.epoch()
    }

    fn seal(
        &self,
        rtype: RecordType,
        header: &pb::RecordHeader,
        aad: &[u8],
        plaintext: &[u8],
    ) -> imaginarium_format::Result<Vec<u8>> {
        let cipher = cipher(self.key, rtype)?;
        let payload = Payload {
            msg: plaintext,
            aad,
        };
        match &header.segment {
            Some(segment) => {
                let nonce = segment_nonce(segment)?;
                cipher
                    .encrypt(&nonce, payload)
                    .map_err(|_| CryptoError::Authentication.into())
            }
            None => {
                let mut nonce = [0u8; NONCE_LEN];
                self.rng.borrow_mut().fill(&mut nonce)?;
                let ciphertext = cipher
                    .encrypt(&XNonce::from(nonce), payload)
                    .map_err(|_| FormatError::from(CryptoError::Authentication))?;
                let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
                out.extend_from_slice(&nonce);
                out.extend_from_slice(&ciphertext);
                Ok(out)
            }
        }
    }
}

/// Decrypts records with any of the epoch keys held (suite 1).
#[derive(Debug)]
pub struct Suite1Opener {
    keys: EpochKeys,
}

impl Suite1Opener {
    /// An opener over the given keys.
    pub fn new(keys: EpochKeys) -> Self {
        Self { keys }
    }

    /// The keys held.
    pub fn keys(&self) -> &EpochKeys {
        &self.keys
    }
}

impl RecordOpener for Suite1Opener {
    fn open(
        &self,
        rtype: RecordType,
        header: &pb::RecordHeader,
        aad: &[u8],
        payload: &[u8],
    ) -> imaginarium_format::Result<Vec<u8>> {
        if header.suite != SUITE_1 {
            return Err(CryptoError::UnknownSuite(header.suite).into());
        }
        let key = self.keys.get(header.epoch)?;
        let cipher = cipher(key, rtype)?;
        let (nonce, ciphertext) = match &header.segment {
            Some(segment) => (segment_nonce(segment)?, payload),
            None => {
                if payload.len() < NONCE_LEN {
                    return Err(CryptoError::Authentication.into());
                }
                let (nonce, ciphertext) = payload.split_at(NONCE_LEN);
                let nonce: [u8; NONCE_LEN] =
                    nonce.try_into().map_err(|_| CryptoError::Authentication)?;
                (XNonce::from(nonce), ciphertext)
            }
        };
        cipher
            .decrypt(
                &nonce,
                Payload {
                    msg: ciphertext,
                    aad,
                },
            )
            .map_err(|_| CryptoError::Authentication.into())
    }
}

/// Builds the record(s) of a blob (spec §5.2, §5.3): a content-addressed id
/// from `blob_id_key`, and segments of 256 KiB above 1 MiB, all sharing
/// one random STREAM prefix when encrypted.
///
/// Media that is already compressed (images, video) should pass
/// `compress = false`.
pub fn blob_records<R: Randomness>(
    doc_id: &DocId,
    blob_id_key: &[u8; 32],
    content: &[u8],
    compress: bool,
    sealer: Option<&Suite1Sealer<'_, R>>,
) -> imaginarium_format::Result<(ObjectId, Vec<Record>)> {
    let id = blob_id(blob_id_key, content);
    let builder = || RecordBuilder::new(RecordType::BLOB, id).compress(compress);
    let as_dyn = sealer.map(|s| s as &dyn RecordSealer);
    if content.len() <= SEGMENT_THRESHOLD {
        return Ok((id, vec![builder().build(doc_id, content.to_vec(), as_dyn)?]));
    }
    let prefix = match sealer {
        Some(sealer) => sealer.new_nonce_prefix()?,
        None => Vec::new(),
    };
    let chunks: Vec<&[u8]> = content.chunks(SEGMENT_LEN).collect();
    let count = u32::try_from(chunks.len())
        .map_err(|_| FormatError::Invalid("blob has too many segments".into()))?;
    let records = chunks
        .iter()
        .enumerate()
        .map(|(index, chunk)| {
            builder()
                .segment(index as u32, count, content.len() as u64, prefix.clone())
                .build(doc_id, chunk.to_vec(), as_dyn)
        })
        .collect::<imaginarium_format::Result<Vec<_>>>()?;
    Ok((id, records))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::OsRandomness;
    use imaginarium_format::{Limits, RecordFlags};

    const DOC: DocId = DocId([0x33; 16]);

    fn key(epoch: u32) -> EpochKey {
        EpochKey::from_bytes(epoch, [epoch as u8; 32])
    }

    fn opener(keys: &[EpochKey]) -> Suite1Opener {
        let mut set = EpochKeys::new();
        keys.iter().cloned().for_each(|k| set.insert(k));
        Suite1Opener::new(set)
    }

    fn encrypt(rtype: RecordType, content: &[u8], key: &EpochKey) -> Record {
        let sealer = Suite1Sealer::new(key, OsRandomness);
        RecordBuilder::new(rtype, ObjectId([1; 32]))
            .stream(pb::Stream::Main)
            .compress(true)
            .build(&DOC, content.to_vec(), Some(&sealer))
            .unwrap()
    }

    #[test]
    fn round_trip_with_the_right_key() {
        let k = key(1);
        let record = encrypt(RecordType::UPDT, b"secret edit", &k);
        assert!(record.flags().contains(RecordFlags::ENCRYPTED));
        assert_eq!((record.header().suite, record.header().epoch), (SUITE_1, 1));
        assert!(
            !record.encode().windows(6).any(|w| w == b"secret"),
            "no plaintext in the record"
        );
        let content = record
            .open_content(&DOC, Some(&opener(&[k])), &Limits::default())
            .unwrap();
        assert_eq!(content, b"secret edit");
    }

    #[test]
    fn wrong_key_epoch_document_or_tampering_fails() {
        let k = key(1);
        let record = encrypt(RecordType::SNAP, b"state", &k);
        let limits = Limits::default();
        assert!(
            record
                .open_content(&DOC, Some(&opener(&[key(2)])), &limits)
                .is_err(),
            "missing epoch"
        );
        assert!(
            record
                .open_content(
                    &DOC,
                    Some(&opener(&[EpochKey::from_bytes(1, [9; 32])])),
                    &limits
                )
                .is_err(),
            "wrong key"
        );
        assert!(
            record
                .open_content(
                    &DocId([0; 16]),
                    Some(&opener(std::slice::from_ref(&k))),
                    &limits
                )
                .is_err(),
            "other document"
        );

        let mut tampered_payload = record.payload().to_vec();
        let last = tampered_payload.len() - 1;
        tampered_payload[last] ^= 1;
        let tampered = Record::new(
            record.rtype(),
            record.flags(),
            record.header().clone(),
            tampered_payload,
        );
        assert!(
            tampered
                .open_content(&DOC, Some(&opener(std::slice::from_ref(&k))), &limits)
                .is_err(),
            "ciphertext"
        );

        let mut header = record.header().clone();
        header.seq = 99;
        let relabelled = Record::new(
            record.rtype(),
            record.flags(),
            header,
            record.payload().to_vec(),
        );
        assert!(
            relabelled
                .open_content(&DOC, Some(&opener(&[k])), &limits)
                .is_err(),
            "header is bound"
        );
    }

    #[test]
    fn content_and_blob_keys_are_separate() {
        let k = key(1);
        let blob = encrypt(RecordType::BLOB, b"image bytes", &k);
        // Re-labelling a blob as content must not decrypt (different subkey and AAD).
        let as_update = Record::new(
            RecordType::UPDT,
            blob.flags(),
            blob.header().clone(),
            blob.payload().to_vec(),
        );
        assert!(
            as_update
                .open_content(&DOC, Some(&opener(&[k])), &Limits::default())
                .is_err()
        );
    }

    #[test]
    fn unknown_suites_are_rejected() {
        let k = key(1);
        let record = encrypt(RecordType::UPDT, b"x", &k);
        let mut header = record.header().clone();
        header.suite = 2;
        let future = Record::new(
            record.rtype(),
            record.flags(),
            header,
            record.payload().to_vec(),
        );
        let err = future
            .open_content(&DOC, Some(&opener(&[k])), &Limits::default())
            .unwrap_err();
        assert!(err.to_string().contains("unsupported crypto suite 2"));
    }

    /// Just over the 1 MiB threshold: 5 full segments and a partial one.
    fn big_blob() -> Vec<u8> {
        (0..SEGMENT_THRESHOLD + SEGMENT_LEN + 123)
            .map(|i| (i * 7 % 256) as u8)
            .collect()
    }

    #[test]
    fn segmented_blobs_encrypt_with_stream_and_reassemble() {
        let k = key(1);
        let sealer = Suite1Sealer::new(&k, OsRandomness);
        let content = big_blob();
        let (id, records) = blob_records(&DOC, &[5; 32], &content, false, Some(&sealer)).unwrap();
        assert_eq!(records.len(), 6);
        assert_eq!(id, blob_id(&[5; 32], &content));
        let prefixes: Vec<_> = records
            .iter()
            .map(|r| r.header().segment.clone().unwrap().nonce_prefix)
            .collect();
        assert!(
            prefixes.iter().all(|p| p == &prefixes[0] && p.len() == 19),
            "one prefix per blob"
        );
        let o = opener(&[k]);
        let mut joined = Vec::new();
        for record in &records {
            joined.extend(
                record
                    .open_content(&DOC, Some(&o), &Limits::default())
                    .unwrap(),
            );
        }
        assert_eq!(joined, content);
    }

    #[test]
    fn swapped_or_truncated_segments_fail() {
        let k = key(1);
        let sealer = Suite1Sealer::new(&k, OsRandomness);
        let (_, records) = blob_records(&DOC, &[5; 32], &big_blob(), false, Some(&sealer)).unwrap();
        let o = opener(&[k]);
        let limits = Limits::default();
        // Segment 0's payload under segment 1's header: the nonce differs.
        let swapped = Record::new(
            RecordType::BLOB,
            records[1].flags(),
            records[1].header().clone(),
            records[0].payload().to_vec(),
        );
        assert!(swapped.open_content(&DOC, Some(&o), &limits).is_err());
        // Claiming the middle segment is the last one: the last flag differs.
        let mut header = records[1].header().clone();
        if let Some(segment) = header.segment.as_mut() {
            segment.count = 2;
        }
        let truncated = Record::new(
            RecordType::BLOB,
            records[1].flags(),
            header,
            records[1].payload().to_vec(),
        );
        assert!(truncated.open_content(&DOC, Some(&o), &limits).is_err());
    }

    #[test]
    fn small_and_unencrypted_blobs_are_single_records() {
        let (id, records) =
            blob_records::<OsRandomness>(&DOC, &[5; 32], b"tiny", true, None).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].object_id(), Some(id));
        assert_eq!(
            records[0]
                .open_content(&DOC, None, &Limits::default())
                .unwrap(),
            b"tiny"
        );
    }
}
