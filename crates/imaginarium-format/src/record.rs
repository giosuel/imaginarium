// SPDX-License-Identifier: AGPL-3.0-or-later

//! Record framing (spec §4.2–§4.3, §5.1).

use std::io::Read as _;

use prost::Message;

use crate::pb;
use crate::varint;
use crate::{FormatError, ObjectId, Result};

/// A record type (spec §5.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RecordType(pub u32);

impl RecordType {
    /// Container-level document metadata (`DocumentMeta`).
    pub const META: Self = Self(0x01);
    /// Keyring (`Keyring`).
    pub const KEYR: Self = Self(0x02);
    /// CRDT snapshot.
    pub const SNAP: Self = Self(0x03);
    /// CRDT update batch.
    pub const UPDT: Self = Self(0x04);
    /// Media blob or blob segment.
    pub const BLOB: Self = Self(0x05);
    /// Thumbnail (WebP).
    pub const THMB: Self = Self(0x06);
    /// Named version (`NamedVersion`).
    pub const VERS: Self = Self(0x07);
    /// Access-control change (`AclChange`).
    pub const ACLC: Self = Self(0x08);
    /// Derived, regenerable data.
    pub const DERV: Self = Self(0x09);
    /// Index (`Index`).
    pub const INDX: Self = Self(0x3FFF);

    /// Whether this is an add-on extension type (`0x4000`–`0x7FFF`).
    pub fn is_extension(self) -> bool {
        (0x4000..=0x7FFF).contains(&self.0)
    }

    /// Whether this implementation knows the type (core types of v1.0).
    pub fn is_known(self) -> bool {
        (0x01..=0x09).contains(&self.0) || self == Self::INDX
    }

    /// The flags spec §5.1 requires for a known type (`SIGNED` excluded,
    /// because it depends on the document, §7.3).
    pub fn required_flags(self) -> RecordFlags {
        use RecordFlags as F;
        match self {
            Self::META | Self::KEYR | Self::BLOB | Self::ACLC => F::CRITICAL | F::SAFE_TO_COPY,
            Self::SNAP | Self::UPDT | Self::INDX => F::CRITICAL,
            Self::THMB | Self::VERS | Self::DERV => F::SAFE_TO_COPY,
            _ => F::empty(),
        }
    }
}

/// Record flags (spec §4.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct RecordFlags(pub u8);

impl RecordFlags {
    /// Unknown type ⇒ open read-only.
    pub const CRITICAL: Self = Self(1 << 0);
    /// May be copied unchanged when unknown.
    pub const SAFE_TO_COPY: Self = Self(1 << 1);
    /// Plaintext is a zstd frame.
    pub const ZSTD: Self = Self(1 << 2);
    /// Payload is AEAD ciphertext.
    pub const ENCRYPTED: Self = Self(1 << 3);
    /// One segment of a larger blob.
    pub const SEGMENTED: Self = Self(1 << 4);
    /// `slen` and `sig` follow the payload.
    pub const SIGNED: Self = Self(1 << 5);
    const RESERVED: u8 = 0b1100_0000;

    /// No flags.
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Whether all flags in `other` are set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Whether reserved bits 6–7 are set (spec §4.3: treat as unknown).
    pub const fn has_reserved(self) -> bool {
        self.0 & Self::RESERVED != 0
    }
}

impl std::ops::BitOr for RecordFlags {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl std::ops::BitOrAssign for RecordFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// Size limits a reader enforces (spec §4.6). The defaults are the spec's
/// minimum supported values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Maximum `hlen`.
    pub max_header: u64,
    /// Maximum `plen` and `plain_len`, and the largest segmented blob that
    /// [`crate::Reader::read_blob`] loads into memory.
    pub max_payload: u64,
    /// Maximum `slen`.
    pub max_signature: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_header: 65_536,
            max_payload: 1 << 30,
            max_signature: 16_384,
        }
    }
}

/// One record, with its header and signature kept **verbatim** (spec
/// §6.3.1): the AAD and signatures cover the stored bytes, and re-encoding
/// a decoded header could drop fields unknown to this version.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    rtype: RecordType,
    flags: RecordFlags,
    header_bytes: Vec<u8>,
    header: pb::RecordHeader,
    payload: Vec<u8>,
    signature_bytes: Option<Vec<u8>>,
}

impl Record {
    /// A new record from a header message and a finished payload. The
    /// header is encoded once, and those bytes become its verbatim form.
    /// Use [`crate::RecordBuilder`] to build payloads from plaintext.
    pub fn new(
        rtype: RecordType,
        flags: RecordFlags,
        header: pb::RecordHeader,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            rtype,
            flags,
            header_bytes: header.encode_to_vec(),
            header,
            payload,
            signature_bytes: None,
        }
    }

    /// Attaches a signature (an encoded `Signature` message) and sets `SIGNED`.
    /// The signature must cover [`Record::signed_body`] with `SIGNED`
    /// already set in the flags.
    pub fn with_signature(mut self, signature_bytes: Vec<u8>) -> Self {
        self.flags |= RecordFlags::SIGNED;
        self.signature_bytes = Some(signature_bytes);
        self
    }

    /// Record type.
    pub fn rtype(&self) -> RecordType {
        self.rtype
    }

    /// Flags.
    pub fn flags(&self) -> RecordFlags {
        self.flags
    }

    /// Decoded header (convenience view, never re-encoded).
    pub fn header(&self) -> &pb::RecordHeader {
        &self.header
    }

    /// Header bytes exactly as stored.
    pub fn header_bytes(&self) -> &[u8] {
        &self.header_bytes
    }

    /// The object id, if the header carries a valid 32-byte one.
    pub fn object_id(&self) -> Option<ObjectId> {
        ObjectId::from_slice(&self.header.object_id)
    }

    /// Payload bytes (possibly compressed and/or encrypted).
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// Signature bytes (an encoded `Signature`), if `SIGNED`.
    pub fn signature_bytes(&self) -> Option<&[u8]> {
        self.signature_bytes.as_deref()
    }

    /// Decoded signature, if `SIGNED`.
    pub fn signature(&self) -> Result<Option<pb::Signature>> {
        self.signature_bytes
            .as_deref()
            .map(|bytes| {
                pb::Signature::decode(bytes).map_err(|source| FormatError::Protobuf {
                    what: "Signature",
                    source,
                })
            })
            .transpose()
    }

    /// What a signature over this record covers (spec §7.2): the label,
    /// the document id, then the record body from `rtype` through `payload`,
    /// **with `SIGNED` set** in the flags, because that's how the record
    /// is stored once signed.
    pub fn signing_input(&self, doc_id: &crate::DocId) -> Vec<u8> {
        const LABEL: &[u8] = b"imaginarium.imag v1 record signature";
        let mut flagged = Self {
            flags: self.flags | RecordFlags::SIGNED,
            ..self.clone()
        };
        flagged.signature_bytes = None;
        let mut out = LABEL.to_vec();
        out.extend_from_slice(&doc_id.0);
        flagged.write_body(&mut out);
        out
    }

    /// Bytes from `rtype` through `payload`, with the flags as they are.
    pub fn signed_body(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.header_bytes.len() + self.payload.len() + 16);
        self.write_body(&mut out);
        out
    }

    /// The AEAD associated data of this record (spec §6.3.1):
    /// `"imaginarium.imag v1 record" ‖ doc_id ‖ varint(rtype) ‖ rflags ‖
    /// varint(hlen) ‖ header`.
    pub fn aad(&self, doc_id: &crate::DocId) -> Vec<u8> {
        aad(doc_id, self.rtype, self.flags, &self.header_bytes)
    }

    /// Encodes the complete record, including its CRC.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.encoded_len());
        self.write_body(&mut out);
        if let Some(signature) = &self.signature_bytes {
            varint::encode(signature.len() as u64, &mut out);
            out.extend_from_slice(signature);
        }
        let crc = crc32c::crc32c(&out);
        out.extend_from_slice(&crc.to_le_bytes());
        out
    }

    /// Length of [`Record::encode`]'s output.
    pub fn encoded_len(&self) -> usize {
        let body = varint::encoded_len(u64::from(self.rtype.0))
            + 1
            + varint::encoded_len(self.header_bytes.len() as u64)
            + self.header_bytes.len()
            + varint::encoded_len(self.payload.len() as u64)
            + self.payload.len();
        let signature = self
            .signature_bytes
            .as_ref()
            .map_or(0, |s| varint::encoded_len(s.len() as u64) + s.len());
        body + signature + 4
    }

    /// Decodes one record from the start of `buf`, verifying limits and
    /// the CRC. Returns the record and its encoded length.
    pub fn decode(buf: &[u8], limits: &Limits) -> Result<(Self, usize)> {
        let mut pos = 0usize;
        let rtype = read_varint(buf, &mut pos, "rtype")?;
        let rtype = u32::try_from(rtype)
            .map_err(|_| FormatError::Invalid(format!("rtype {rtype} too large")))?;
        let flags = RecordFlags(
            *buf.get(pos)
                .ok_or(FormatError::Truncated { what: "rflags" })?,
        );
        pos += 1;
        let header_bytes = read_bytes(buf, &mut pos, "header", limits.max_header)?;
        let payload = read_bytes(buf, &mut pos, "payload", limits.max_payload)?;
        let signature_bytes = if flags.contains(RecordFlags::SIGNED) {
            Some(read_bytes(
                buf,
                &mut pos,
                "signature",
                limits.max_signature,
            )?)
        } else {
            None
        };
        let crc_bytes = buf
            .get(pos..pos + 4)
            .ok_or(FormatError::Truncated { what: "record CRC" })?;
        let crc = u32::from_le_bytes([crc_bytes[0], crc_bytes[1], crc_bytes[2], crc_bytes[3]]);
        if crc != crc32c::crc32c(&buf[..pos]) {
            return Err(FormatError::Crc { what: "record" });
        }
        let header = pb::RecordHeader::decode(header_bytes.as_slice()).map_err(|source| {
            FormatError::Protobuf {
                what: "RecordHeader",
                source,
            }
        })?;
        Ok((
            Self {
                rtype: RecordType(rtype),
                flags,
                header_bytes,
                header,
                payload,
                signature_bytes,
            },
            pos + 4,
        ))
    }

    /// Reads one record from a stream (used by recovery's forward scan,
    /// where lengths aren't known in advance). Verifies limits and the CRC.
    pub fn read_from<R: std::io::Read>(reader: &mut R, limits: &Limits) -> Result<(Self, usize)> {
        let mut raw = Vec::new();
        stream_varint(reader, &mut raw, "rtype")?;
        let mut flag = [0u8; 1];
        read_exact(reader, &mut flag, "rflags")?;
        raw.push(flag[0]);
        let header_len = stream_varint(reader, &mut raw, "header")?;
        stream_bytes(reader, &mut raw, header_len, limits.max_header, "header")?;
        let payload_len = stream_varint(reader, &mut raw, "payload")?;
        stream_bytes(reader, &mut raw, payload_len, limits.max_payload, "payload")?;
        if RecordFlags(flag[0]).contains(RecordFlags::SIGNED) {
            let sig_len = stream_varint(reader, &mut raw, "signature")?;
            stream_bytes(reader, &mut raw, sig_len, limits.max_signature, "signature")?;
        }
        let mut crc = [0u8; 4];
        read_exact(reader, &mut crc, "record CRC")?;
        raw.extend_from_slice(&crc);
        Self::decode(&raw, limits)
    }

    fn write_body(&self, out: &mut Vec<u8>) {
        varint::encode(u64::from(self.rtype.0), out);
        out.push(self.flags.0);
        varint::encode(self.header_bytes.len() as u64, out);
        out.extend_from_slice(&self.header_bytes);
        varint::encode(self.payload.len() as u64, out);
        out.extend_from_slice(&self.payload);
    }
}

/// The AAD of spec §6.3.1, from its parts.
pub(crate) fn aad(
    doc_id: &crate::DocId,
    rtype: RecordType,
    flags: RecordFlags,
    header_bytes: &[u8],
) -> Vec<u8> {
    const LABEL: &[u8] = b"imaginarium.imag v1 record";
    let mut out = Vec::with_capacity(LABEL.len() + 16 + 8 + header_bytes.len());
    out.extend_from_slice(LABEL);
    out.extend_from_slice(&doc_id.0);
    varint::encode(u64::from(rtype.0), &mut out);
    out.push(flags.0);
    varint::encode(header_bytes.len() as u64, &mut out);
    out.extend_from_slice(header_bytes);
    out
}

fn read_exact<R: std::io::Read>(reader: &mut R, buf: &mut [u8], what: &'static str) -> Result<()> {
    reader.read_exact(buf).map_err(|err| match err.kind() {
        std::io::ErrorKind::UnexpectedEof => FormatError::Truncated { what },
        _ => FormatError::Io(err),
    })
}

/// Reads a varint byte by byte, appending its bytes to `raw`.
fn stream_varint<R: std::io::Read>(
    reader: &mut R,
    raw: &mut Vec<u8>,
    what: &'static str,
) -> Result<u64> {
    let start = raw.len();
    for _ in 0..10 {
        let mut byte = [0u8; 1];
        read_exact(reader, &mut byte, what)?;
        raw.push(byte[0]);
        if byte[0] & 0x80 == 0 {
            return varint::decode(&raw[start..])
                .map(|(value, _)| value)
                .ok_or(FormatError::Varint { what });
        }
    }
    Err(FormatError::Varint { what })
}

fn stream_bytes<R: std::io::Read>(
    reader: &mut R,
    raw: &mut Vec<u8>,
    len: u64,
    limit: u64,
    what: &'static str,
) -> Result<()> {
    if len > limit {
        return Err(FormatError::Limit {
            what,
            value: len,
            limit,
        });
    }
    // Grow with the data actually read, never with the *claimed* length,
    // so a tiny file claiming a 1 GiB payload can't force a 1 GiB allocation.
    let start = raw.len();
    reader.take(len).read_to_end(raw)?;
    if (raw.len() - start) as u64 == len {
        Ok(())
    } else {
        Err(FormatError::Truncated { what })
    }
}

fn read_varint(buf: &[u8], pos: &mut usize, what: &'static str) -> Result<u64> {
    let rest = buf.get(*pos..).ok_or(FormatError::Truncated { what })?;
    let (value, len) = varint::decode(rest).ok_or_else(|| {
        if rest.len() < 10 && rest.iter().all(|b| b & 0x80 != 0) {
            FormatError::Truncated { what }
        } else {
            FormatError::Varint { what }
        }
    })?;
    *pos += len;
    Ok(value)
}

fn read_bytes(buf: &[u8], pos: &mut usize, what: &'static str, limit: u64) -> Result<Vec<u8>> {
    let len = read_varint(buf, pos, what)?;
    if len > limit {
        return Err(FormatError::Limit {
            what,
            value: len,
            limit,
        });
    }
    let len = usize::try_from(len).map_err(|_| FormatError::Limit {
        what,
        value: len,
        limit,
    })?;
    let end = pos
        .checked_add(len)
        .ok_or(FormatError::Truncated { what })?;
    let bytes = buf
        .get(*pos..end)
        .ok_or(FormatError::Truncated { what })?
        .to_vec();
    *pos = end;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn sample() -> Record {
        let header = pb::RecordHeader {
            object_id: vec![9; 32],
            stream: pb::Stream::Main as i32,
            plain_len: 5,
            codec: "loro-1".into(),
            ..Default::default()
        };
        Record::new(
            RecordType::UPDT,
            RecordFlags::CRITICAL,
            header,
            b"hello".to_vec(),
        )
    }

    #[test]
    fn round_trip_unsigned_and_signed() {
        let record = sample();
        let bytes = record.encode();
        assert_eq!(bytes.len(), record.encoded_len());
        let (decoded, len) = Record::decode(&bytes, &Limits::default()).unwrap();
        assert_eq!(len, bytes.len());
        assert_eq!(decoded, record);

        let signed = sample().with_signature(vec![1, 2, 3]);
        assert!(signed.flags().contains(RecordFlags::SIGNED));
        let bytes = signed.encode();
        let (decoded, _) = Record::decode(&bytes, &Limits::default()).unwrap();
        assert_eq!(decoded.signature_bytes(), Some(&[1u8, 2, 3][..]));
    }

    #[test]
    fn crc_detects_any_flipped_bit_in_a_record() {
        let bytes = sample().encode();
        for i in 0..bytes.len() {
            let mut corrupted = bytes.clone();
            corrupted[i] ^= 0x10;
            assert!(
                Record::decode(&corrupted, &Limits::default()).is_err(),
                "flip at {i} undetected"
            );
        }
    }

    #[test]
    fn limits_and_truncation() {
        let bytes = sample().encode();
        let tight = Limits {
            max_payload: 4,
            ..Limits::default()
        };
        assert!(matches!(
            Record::decode(&bytes, &tight),
            Err(FormatError::Limit {
                what: "payload",
                ..
            })
        ));
        for cut in 0..bytes.len() {
            assert!(
                Record::decode(&bytes[..cut], &Limits::default()).is_err(),
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn unknown_header_fields_survive_byte_for_byte() {
        // A newer writer's header with field 15 (unknown to v1).
        let mut header_bytes = sample().header().encode_to_vec();
        header_bytes.extend_from_slice(&[0x78, 0x2A]); // field 15, varint 42
        let mut raw = Vec::new();
        varint::encode(0x04, &mut raw);
        raw.push(RecordFlags::CRITICAL.0);
        varint::encode(header_bytes.len() as u64, &mut raw);
        raw.extend_from_slice(&header_bytes);
        varint::encode(5, &mut raw);
        raw.extend_from_slice(b"hello");
        let crc = crc32c::crc32c(&raw);
        raw.extend_from_slice(&crc.to_le_bytes());

        let (record, _) = Record::decode(&raw, &Limits::default()).unwrap();
        assert_eq!(
            record.header_bytes(),
            header_bytes.as_slice(),
            "kept verbatim"
        );
        assert_eq!(record.encode(), raw, "re-encodes byte-for-byte");
    }

    #[test]
    fn streaming_read_matches_buffer_decode() {
        let record = sample().with_signature(vec![4; 10]);
        let mut bytes = record.encode();
        let len = bytes.len();
        bytes.extend_from_slice(b"next record...");
        let mut cursor = std::io::Cursor::new(bytes);
        let (read, consumed) = Record::read_from(&mut cursor, &Limits::default()).unwrap();
        assert_eq!((read, consumed), (record, len));
        assert_eq!(cursor.position(), len as u64);
    }

    #[test]
    fn streaming_read_of_a_huge_claimed_length_fails_without_allocating_it() {
        // rtype, flags, hlen 0, then plen = 1 GiB, with only 3 bytes of data.
        let mut bytes = vec![0x04, 0x01, 0x00];
        varint::encode(1 << 30, &mut bytes);
        bytes.extend_from_slice(b"abc");
        let started = std::time::Instant::now();
        let result = Record::read_from(&mut std::io::Cursor::new(bytes), &Limits::default());
        assert!(matches!(result, Err(FormatError::Truncated { .. })));
        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "no 1 GiB allocation"
        );
    }

    #[test]
    fn signing_input_always_has_the_signed_flag() {
        let doc = crate::DocId([3; 16]);
        let input = sample().signing_input(&doc);
        let signed = sample().with_signature(vec![9; 4]);
        assert_eq!(
            input,
            signed.signing_input(&doc),
            "same input before and after attaching"
        );
        let label = b"imaginarium.imag v1 record signature";
        assert!(input.starts_with(label));
        assert_eq!(&input[label.len()..label.len() + 16], &[3; 16]);
        let flags_at = label.len() + 16 + 1; // after the 1-byte rtype varint
        assert!(RecordFlags(input[flags_at]).contains(RecordFlags::SIGNED));
    }

    #[test]
    fn aad_matches_the_spec_layout() {
        let record = sample();
        let doc = crate::DocId([1; 16]);
        let aad = record.aad(&doc);
        let mut expected = b"imaginarium.imag v1 record".to_vec();
        expected.extend_from_slice(&[1; 16]);
        expected.push(0x04); // rtype
        expected.push(0x01); // flags
        expected.push(record.header_bytes().len() as u8); // hlen (< 128)
        expected.extend_from_slice(record.header_bytes());
        assert_eq!(aad, expected);
    }

    #[test]
    fn flag_helpers() {
        assert!(
            RecordType::BLOB
                .required_flags()
                .contains(RecordFlags::SAFE_TO_COPY)
        );
        assert!(RecordType(0x4001).is_extension());
        assert!(!RecordType(0x0A).is_known());
        assert!(RecordFlags(0b0100_0000).has_reserved());
    }

    proptest! {
        #[test]
        fn decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
            let _ = Record::decode(&bytes, &Limits::default());
        }

        #[test]
        fn arbitrary_records_round_trip(
            rtype in 0u32..0x8000,
            flags in 0u8..0x20, // without SIGNED, set separately
            payload in proptest::collection::vec(any::<u8>(), 0..512),
            signature in proptest::option::of(proptest::collection::vec(any::<u8>(), 0..64)),
        ) {
            let header = pb::RecordHeader { object_id: vec![3; 32], plain_len: payload.len() as u64, ..Default::default() };
            let mut record = Record::new(RecordType(rtype), RecordFlags(flags), header, payload);
            if let Some(signature) = signature {
                record = record.with_signature(signature);
            }
            let bytes = record.encode();
            let (decoded, len) = Record::decode(&bytes, &Limits::default()).unwrap();
            prop_assert_eq!(len, bytes.len());
            prop_assert_eq!(decoded, record);
        }
    }
}
