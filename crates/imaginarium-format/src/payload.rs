// SPDX-License-Identifier: AGPL-3.0-or-later

//! Payload processing (spec §4.4): plaintext ⇄ payload, with optional zstd
//! compression and optional encryption through [`RecordSealer`] /
//! [`RecordOpener`].
//!
//! This crate defines *where* encryption happens and what the associated
//! data is (spec §6.3). The cryptography itself lives in
//! `imaginarium-crypto`, which implements these traits. That keeps the
//! container crate free of crypto dependencies, so third parties can
//! read unencrypted files with it alone.

use prost::Message;

use crate::pb;
use crate::record::{Limits, Record, RecordFlags, RecordType, aad};
use crate::{DocId, FormatError, ObjectId, Result};

/// Default zstd level: fast, good ratio (spec leaves the level to writers).
pub const DEFAULT_ZSTD_LEVEL: i32 = 3;

/// Encrypts record payloads (implemented by `imaginarium-crypto`).
pub trait RecordSealer {
    /// The suite id written to `RecordHeader.suite` (spec §6.1).
    fn suite(&self) -> u32;
    /// The epoch the sealer encrypts under (`RecordHeader.epoch`).
    fn epoch(&self) -> u32;
    /// Encrypts `plaintext`, returning the complete payload. For suite 1:
    /// `nonce ‖ ciphertext‖tag`, or the STREAM ciphertext for segments,
    /// where `header.segment` carries the nonce prefix (spec §6.3).
    fn seal(
        &self,
        rtype: RecordType,
        header: &pb::RecordHeader,
        aad: &[u8],
        plaintext: &[u8],
    ) -> Result<Vec<u8>>;
    /// Fills in header fields the sealer owns before the header is encoded
    /// (e.g. a fresh STREAM `nonce_prefix` for the first segment). The
    /// default does nothing.
    fn prepare_header(&self, _rtype: RecordType, _header: &mut pb::RecordHeader) -> Result<()> {
        Ok(())
    }
}

/// Decrypts record payloads (implemented by `imaginarium-crypto`).
pub trait RecordOpener {
    /// Decrypts `payload` of a record whose header names `suite`/`epoch`.
    /// Fails on unknown suites, missing epoch keys, or authentication
    /// failure.
    fn open(
        &self,
        rtype: RecordType,
        header: &pb::RecordHeader,
        aad: &[u8],
        payload: &[u8],
    ) -> Result<Vec<u8>>;
}

/// Builds a [`Record`] from plaintext content.
///
/// ```
/// use imaginarium_format::{pb, DocId, ObjectId, RecordBuilder, RecordType};
///
/// let doc = DocId([1; 16]);
/// let record = RecordBuilder::new(RecordType::UPDT, ObjectId([2; 32]))
///     .stream(pb::Stream::Main)
///     .codec("loro-1")
///     .compress(true)
///     .build(&doc, b"crdt update bytes".to_vec(), None)?;
/// assert_eq!(record.open_content(&doc, None, &Default::default())?, b"crdt update bytes");
/// # Ok::<(), imaginarium_format::FormatError>(())
/// ```
#[derive(Clone, Debug)]
pub struct RecordBuilder {
    rtype: RecordType,
    flags: RecordFlags,
    header: pb::RecordHeader,
    compress: Option<i32>,
}

impl RecordBuilder {
    /// Starts a record of `rtype` with the flags spec §5.1 requires for it.
    pub fn new(rtype: RecordType, object_id: ObjectId) -> Self {
        Self {
            rtype,
            flags: rtype.required_flags(),
            header: pb::RecordHeader {
                object_id: object_id.0.to_vec(),
                ..Default::default()
            },
            compress: None,
        }
    }

    /// Adds flags (e.g. `CRITICAL` for an extension type).
    pub fn flags(mut self, flags: RecordFlags) -> Self {
        self.flags |= flags;
        self
    }

    /// Sets the stream (SNAP/UPDT).
    pub fn stream(mut self, stream: pb::Stream) -> Self {
        self.header.stream = stream as i32;
        self
    }

    /// Sets the add-on id (extension stream or extension record type).
    pub fn extension_id(mut self, id: impl Into<String>) -> Self {
        self.header.extension_id = id.into();
        self
    }

    /// Sets the CRDT codec (SNAP/UPDT).
    pub fn codec(mut self, codec: impl Into<String>) -> Self {
        self.header.codec = codec.into();
        self
    }

    /// Sets the derived-data kind (DERV).
    pub fn derived_kind(mut self, kind: impl Into<String>) -> Self {
        self.header.derived_kind = kind.into();
        self
    }

    /// Sets the writer-local sequence number.
    pub fn seq(mut self, seq: u64) -> Self {
        self.header.seq = seq;
        self
    }

    /// Marks the record as a segment of a larger blob (spec §5.3).
    ///
    /// In encrypted documents, `nonce_prefix` MUST be fresh random bytes for
    /// every blob, shared by its segments only: segment nonces are the
    /// prefix plus the segment index (spec §6.3.2), so reusing a prefix under
    /// the same epoch reuses nonces. Use `imaginarium_crypto::blob_records`,
    /// which does this, instead of building segments by hand.
    pub fn segment(
        mut self,
        index: u32,
        count: u32,
        total_len: u64,
        nonce_prefix: Vec<u8>,
    ) -> Self {
        self.flags |= RecordFlags::SEGMENTED;
        self.header.segment = Some(pb::Segment {
            index,
            count,
            total_len,
            nonce_prefix,
        });
        self
    }

    /// Sets `key_records` (INDX of encrypted documents, spec §9.1).
    pub fn key_records(mut self, refs: Vec<pb::RecordRef>) -> Self {
        self.header.key_records = refs;
        self
    }

    /// Sets `SIGNED` *before* encryption. Required for records that are
    /// both encrypted and signed: the AAD covers the flags (spec §6.3.1), so
    /// they must already be final when sealing. Attach the signature
    /// afterwards with [`Record::with_signature`].
    pub fn will_be_signed(mut self) -> Self {
        self.flags |= RecordFlags::SIGNED;
        self
    }

    /// Compresses the content with zstd at the default level.
    pub fn compress(mut self, enabled: bool) -> Self {
        self.compress = enabled.then_some(DEFAULT_ZSTD_LEVEL);
        self
    }

    /// Compresses with an explicit zstd level (e.g. 19 for archival compaction).
    pub fn compress_level(mut self, level: i32) -> Self {
        self.compress = Some(level);
        self
    }

    /// Finishes the record: compresses (if requested), encrypts with
    /// `sealer` (if given), and records `plain_len`, `suite` and `epoch`.
    pub fn build(
        self,
        doc_id: &DocId,
        content: Vec<u8>,
        sealer: Option<&dyn RecordSealer>,
    ) -> Result<Record> {
        let Self {
            rtype,
            mut flags,
            mut header,
            compress,
        } = self;
        header.plain_len = content.len() as u64;
        let plaintext = match compress {
            Some(level) => {
                flags |= RecordFlags::ZSTD;
                zstd::bulk::compress(&content, level)
                    .map_err(|err| FormatError::Invalid(format!("compression failed: {err}")))?
            }
            None => content,
        };
        let payload = match sealer {
            Some(sealer) => {
                flags |= RecordFlags::ENCRYPTED;
                header.suite = sealer.suite();
                header.epoch = sealer.epoch();
                sealer.prepare_header(rtype, &mut header)?;
                let header_bytes = header.encode_to_vec();
                let aad = aad(doc_id, rtype, flags, &header_bytes);
                sealer.seal(rtype, &header, &aad, &plaintext)?
            }
            None => plaintext,
        };
        Ok(Record::new(rtype, flags, header, payload))
    }
}

impl Record {
    /// Recovers the plaintext content (spec §4.4): decrypts with `opener`
    /// if the record is encrypted, decompresses if `ZSTD`, and verifies
    /// that the result is exactly `plain_len` bytes. Decompression is
    /// bounded by `plain_len` (and `limits`), so a bomb fails early.
    pub fn open_content(
        &self,
        doc_id: &DocId,
        opener: Option<&dyn RecordOpener>,
        limits: &Limits,
    ) -> Result<Vec<u8>> {
        let plain_len = self.header().plain_len;
        if plain_len > limits.max_payload {
            return Err(FormatError::Limit {
                what: "plain_len",
                value: plain_len,
                limit: limits.max_payload,
            });
        }
        let data = if self.flags().contains(RecordFlags::ENCRYPTED) {
            let opener = opener.ok_or(FormatError::NeedsKey)?;
            opener.open(
                self.rtype(),
                self.header(),
                &self.aad(doc_id),
                self.payload(),
            )?
        } else {
            self.payload().to_vec()
        };
        let content = if self.flags().contains(RecordFlags::ZSTD) {
            decompress_bounded(&data, plain_len)?
        } else {
            data
        };
        if content.len() as u64 != plain_len {
            return Err(FormatError::PlainLen {
                expected: plain_len,
                actual: content.len() as u64,
            });
        }
        Ok(content)
    }
}

/// Decompresses a zstd frame, producing at most `plain_len + 1` bytes.
/// Memory grows with the actual output, never with the claimed length,
/// so a lying header can't force a huge allocation, and a bomb stops one
/// byte past the limit.
fn decompress_bounded(data: &[u8], plain_len: u64) -> Result<Vec<u8>> {
    use std::io::Read;
    let decoder = zstd::stream::read::Decoder::new(data)
        .map_err(|err| FormatError::Decompress(err.to_string()))?;
    let mut out = Vec::with_capacity(usize::try_from(plain_len.min(1 << 20)).unwrap_or(0));
    decoder
        .take(plain_len.saturating_add(1))
        .read_to_end(&mut out)
        .map_err(|err| FormatError::Decompress(err.to_string()))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: DocId = DocId([5; 16]);

    /// Toy "cipher" for tests: XOR with a byte, and the AAD must match on
    /// open (tests the plumbing, not crypto).
    struct XorSealer(u8);

    impl RecordSealer for XorSealer {
        fn suite(&self) -> u32 {
            99
        }
        fn epoch(&self) -> u32 {
            7
        }
        fn seal(
            &self,
            _: RecordType,
            _: &pb::RecordHeader,
            aad: &[u8],
            plaintext: &[u8],
        ) -> Result<Vec<u8>> {
            let mut out = blake_tag(aad).to_vec();
            out.extend(plaintext.iter().map(|b| b ^ self.0));
            Ok(out)
        }
    }

    impl RecordOpener for XorSealer {
        fn open(
            &self,
            _: RecordType,
            header: &pb::RecordHeader,
            aad: &[u8],
            payload: &[u8],
        ) -> Result<Vec<u8>> {
            assert_eq!((header.suite, header.epoch), (99, 7));
            let (tag, body) = payload.split_at(4);
            if tag != blake_tag(aad) {
                return Err(FormatError::Crypto("AAD mismatch".into()));
            }
            Ok(body.iter().map(|b| b ^ self.0).collect())
        }
    }

    /// A 4-byte checksum of the AAD standing in for an AEAD tag.
    fn blake_tag(aad: &[u8]) -> [u8; 4] {
        crc32c::crc32c(aad).to_le_bytes()
    }

    fn content() -> Vec<u8> {
        b"the quick brown fox ".repeat(50)
    }

    #[test]
    fn plain_compressed_and_encrypted_round_trips() {
        let limits = Limits::default();
        for (compress, encrypt) in [(false, false), (true, false), (false, true), (true, true)] {
            let sealer = XorSealer(0x5A);
            let record = RecordBuilder::new(RecordType::SNAP, ObjectId([1; 32]))
                .stream(pb::Stream::Main)
                .codec("loro-1")
                .compress(compress)
                .build(
                    &DOC,
                    content(),
                    encrypt.then_some(&sealer as &dyn RecordSealer),
                )
                .unwrap();
            assert_eq!(record.flags().contains(RecordFlags::ZSTD), compress);
            assert_eq!(record.flags().contains(RecordFlags::ENCRYPTED), encrypt);
            assert_eq!(record.header().plain_len, content().len() as u64);
            if compress {
                assert!(record.payload().len() < content().len() / 4, "compressed");
            }
            let opener = encrypt.then_some(&sealer as &dyn RecordOpener);
            assert_eq!(
                record.open_content(&DOC, opener, &limits).unwrap(),
                content()
            );
        }
    }

    #[test]
    fn encrypted_records_need_a_key_and_bind_the_document() {
        let sealer = XorSealer(1);
        let record = RecordBuilder::new(RecordType::UPDT, ObjectId([1; 32]))
            .build(&DOC, content(), Some(&sealer))
            .unwrap();
        assert!(matches!(
            record.open_content(&DOC, None, &Limits::default()),
            Err(FormatError::NeedsKey)
        ));
        let other_doc = DocId([6; 16]);
        assert!(
            record
                .open_content(&other_doc, Some(&sealer), &Limits::default())
                .is_err(),
            "AAD binds doc_id"
        );
    }

    #[test]
    fn decompression_is_bounded_by_plain_len() {
        // A record whose header lies about plain_len (a "bomb").
        let honest = RecordBuilder::new(RecordType::BLOB, ObjectId([1; 32]))
            .compress(true)
            .build(&DOC, vec![0u8; 1_000_000], None)
            .unwrap();
        let mut header = honest.header().clone();
        header.plain_len = 1_000; // claims 1 KB, would inflate to 1 MB
        let bomb = Record::new(
            honest.rtype(),
            honest.flags(),
            header,
            honest.payload().to_vec(),
        );
        assert!(bomb.open_content(&DOC, None, &Limits::default()).is_err());

        let tight = Limits {
            max_payload: 999_999,
            ..Limits::default()
        };
        assert!(matches!(
            honest.open_content(&DOC, None, &tight),
            Err(FormatError::Limit { .. })
        ));
    }

    #[test]
    fn huge_claimed_plain_len_does_not_allocate_up_front() {
        // A tiny compressed payload claiming 1 GiB must fail fast and
        // without allocating 1 GiB (it would OOM otherwise).
        let tiny = RecordBuilder::new(RecordType::BLOB, ObjectId([1; 32]))
            .compress(true)
            .build(&DOC, vec![0u8; 100], None)
            .unwrap();
        let mut header = tiny.header().clone();
        header.plain_len = 1 << 30;
        let liar = Record::new(tiny.rtype(), tiny.flags(), header, tiny.payload().to_vec());
        let started = std::time::Instant::now();
        assert!(matches!(
            liar.open_content(&DOC, None, &Limits::default()),
            Err(FormatError::PlainLen { .. })
        ));
        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "no 1 GiB allocation"
        );
    }

    #[test]
    fn plain_len_mismatch_is_detected() {
        let mut header = pb::RecordHeader {
            object_id: vec![1; 32],
            plain_len: 10,
            ..Default::default()
        };
        let record = Record::new(
            RecordType::THMB,
            RecordFlags::SAFE_TO_COPY,
            header.clone(),
            vec![0; 9],
        );
        assert!(matches!(
            record.open_content(&DOC, None, &Limits::default()),
            Err(FormatError::PlainLen { .. })
        ));
        header.plain_len = 9;
        let ok = Record::new(
            RecordType::THMB,
            RecordFlags::SAFE_TO_COPY,
            header,
            vec![0; 9],
        );
        assert!(ok.open_content(&DOC, None, &Limits::default()).is_ok());
    }

    #[test]
    fn builder_sets_required_flags() {
        let record = RecordBuilder::new(RecordType::META, ObjectId([1; 32]))
            .build(&DOC, vec![], None)
            .unwrap();
        assert!(
            record
                .flags()
                .contains(RecordFlags::CRITICAL | RecordFlags::SAFE_TO_COPY)
        );
    }
}
