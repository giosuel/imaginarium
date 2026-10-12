// SPDX-License-Identifier: AGPL-3.0-or-later

//! The 36-byte file header (spec §4.1).

use crate::record::{Record, RecordFlags, RecordType};
use crate::{DocId, FormatError, Result};

/// The file magic: `\x89IMG\r\n\x1a\n`.
pub const MAGIC: [u8; 8] = [0x89, b'I', b'M', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Header size in bytes. Records start at this offset.
pub const HEADER_LEN: usize = 36;

/// The major version this implementation reads and writes.
pub const MAJOR: u16 = 1;
/// The minor version this implementation writes.
pub const MINOR: u16 = 0;

/// Header flag bit 0: the document is encrypted.
pub const FLAG_ENCRYPTED: u32 = 1 << 0;
/// Reserved critical header flags (bits 1–15). Readers refuse files with any set.
const CRITICAL_RESERVED: u32 = 0x0000_FFFE;

/// The decoded file header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileHeader {
    /// Major version (always 1 for files this crate accepts).
    pub major: u16,
    /// Minor version. Higher minors are accepted (additive changes only).
    pub minor: u16,
    /// Header flags. Ignorable bits 16–31 are preserved as read.
    pub flags: u32,
    /// The document id.
    pub doc_id: DocId,
}

impl FileHeader {
    /// A header for a new document written by this implementation.
    pub fn new(doc_id: DocId, encrypted: bool) -> Self {
        Self {
            major: MAJOR,
            minor: MINOR,
            flags: if encrypted { FLAG_ENCRYPTED } else { 0 },
            doc_id,
        }
    }

    /// Whether the document is encrypted (flag bit 0).
    pub fn is_encrypted(&self) -> bool {
        self.flags & FLAG_ENCRYPTED != 0
    }

    /// Spec §5.1: in encrypted documents every record except `KEYR` and
    /// `ACLC` is `ENCRYPTED`, and those two never are. In unencrypted
    /// documents no record is. Returns what's wrong with `record`, if
    /// anything. Readers check this too: otherwise anyone holding an
    /// encrypted file could add plaintext records without the key.
    pub(crate) fn encryption_violation(&self, record: &Record) -> Option<String> {
        let encrypted = record.flags().contains(RecordFlags::ENCRYPTED);
        let key_record = matches!(record.rtype(), RecordType::KEYR | RecordType::ACLC);
        let must_be_encrypted = self.is_encrypted() && !key_record;
        (encrypted != must_be_encrypted).then(|| {
            format!(
                "record type {:#x} must {}be encrypted in {} documents",
                record.rtype().0,
                if must_be_encrypted { "" } else { "not " },
                if self.is_encrypted() {
                    "encrypted"
                } else {
                    "unencrypted"
                },
            )
        })
    }

    /// The 36 header bytes.
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut out = [0u8; HEADER_LEN];
        out[0..8].copy_from_slice(&MAGIC);
        out[8..10].copy_from_slice(&self.major.to_le_bytes());
        out[10..12].copy_from_slice(&self.minor.to_le_bytes());
        out[12..16].copy_from_slice(&self.flags.to_le_bytes());
        out[16..32].copy_from_slice(&self.doc_id.0);
        // bytes 32..36: reserved, zero
        out
    }

    /// Parses and validates a header.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let bytes: &[u8; HEADER_LEN] = bytes
            .get(..HEADER_LEN)
            .and_then(|b| b.try_into().ok())
            .ok_or(FormatError::Truncated {
                what: "file header",
            })?;
        if bytes[0..8] != MAGIC {
            return Err(FormatError::BadMagic { what: "header" });
        }
        let major = u16::from_le_bytes([bytes[8], bytes[9]]);
        let minor = u16::from_le_bytes([bytes[10], bytes[11]]);
        let flags = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
        if major != MAJOR {
            return Err(FormatError::UnsupportedMajor(major));
        }
        if flags & CRITICAL_RESERVED != 0 {
            return Err(FormatError::UnknownCriticalFlags(flags & CRITICAL_RESERVED));
        }
        let mut doc_id = [0u8; 16];
        doc_id.copy_from_slice(&bytes[16..32]);
        Ok(Self {
            major,
            minor,
            flags,
            doc_id: DocId(doc_id),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header() -> FileHeader {
        FileHeader::new(DocId([7; 16]), true)
    }

    #[test]
    fn round_trip() {
        let h = header();
        assert_eq!(FileHeader::decode(&h.encode()).unwrap(), h);
        assert!(h.is_encrypted());
        assert!(!FileHeader::new(DocId([0; 16]), false).is_encrypted());
    }

    #[test]
    fn rejects_bad_files() {
        let good = header().encode();
        assert!(matches!(
            FileHeader::decode(&good[..35]),
            Err(FormatError::Truncated { .. })
        ));
        let mut bad = good;
        bad[1] = b'X';
        assert!(matches!(
            FileHeader::decode(&bad),
            Err(FormatError::BadMagic { .. })
        ));
        let mut v2 = good;
        v2[8] = 2;
        assert!(matches!(
            FileHeader::decode(&v2),
            Err(FormatError::UnsupportedMajor(2))
        ));
        let mut critical = good;
        critical[12] |= 0b10;
        assert!(matches!(
            FileHeader::decode(&critical),
            Err(FormatError::UnknownCriticalFlags(2))
        ));
    }

    #[test]
    fn tolerates_newer_minor_and_ignorable_flags_and_reserved_bytes() {
        let mut bytes = header().encode();
        bytes[10] = 9; // minor 9
        bytes[14] = 0xFF; // ignorable flag bits 16..23
        bytes[33] = 0xEE; // reserved
        let h = FileHeader::decode(&bytes).unwrap();
        assert_eq!(h.minor, 9);
        assert_eq!(h.flags & 0x00FF_0000, 0x00FF_0000);
    }
}
