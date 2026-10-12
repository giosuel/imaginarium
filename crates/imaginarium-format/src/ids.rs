// SPDX-License-Identifier: AGPL-3.0-or-later

//! Document and object identifiers (spec §4.1, §5.2).

use std::fmt;

/// A document's id: 16 random bytes (a version-4 UUID), stable across copies
/// and the Hub (spec §4.1).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocId(pub [u8; 16]);

/// A record's object id: 32 bytes, random or content-addressed (spec §5.2).
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ObjectId(pub [u8; 32]);

impl DocId {
    /// A new random document id from the OS CSPRNG, formatted as a UUID
    /// version 4 (RFC 9562).
    pub fn random() -> crate::Result<Self> {
        let mut bytes = [0u8; 16];
        fill_random(&mut bytes)?;
        bytes[6] = (bytes[6] & 0x0F) | 0x40; // version 4
        bytes[8] = (bytes[8] & 0x3F) | 0x80; // RFC 4122 variant
        Ok(Self(bytes))
    }
}

impl ObjectId {
    /// A new random object id from the OS CSPRNG.
    pub fn random() -> crate::Result<Self> {
        let mut bytes = [0u8; 32];
        fill_random(&mut bytes)?;
        Ok(Self(bytes))
    }

    /// Parses a 32-byte slice.
    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        bytes.try_into().ok().map(Self)
    }
}

/// Fills `buf` from the OS CSPRNG (spec §12, "Randomness").
pub(crate) fn fill_random(buf: &mut [u8]) -> crate::Result<()> {
    getrandom::fill(buf)
        .map_err(|err| crate::FormatError::Crypto(format!("OS randomness unavailable: {err}")))
}

fn hex(bytes: &[u8], f: &mut fmt::Formatter<'_>) -> fmt::Result {
    bytes.iter().try_for_each(|b| write!(f, "{b:02x}"))
}

impl fmt::Display for DocId {
    /// Hyphenated UUID form.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let b = &self.0;
        hex(&b[0..4], f)?;
        f.write_str("-")?;
        hex(&b[4..6], f)?;
        f.write_str("-")?;
        hex(&b[6..8], f)?;
        f.write_str("-")?;
        hex(&b[8..10], f)?;
        f.write_str("-")?;
        hex(&b[10..16], f)
    }
}

impl fmt::Debug for DocId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DocId({self})")
    }
}

impl fmt::Display for ObjectId {
    /// Full lowercase hex.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        hex(&self.0, f)
    }
}

impl fmt::Debug for ObjectId {
    /// Abbreviated: the first 8 bytes.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ObjectId(")?;
        hex(&self.0[..8], f)?;
        f.write_str("…)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn random_doc_ids_are_uuid_v4() {
        let id = DocId::random().unwrap();
        assert_eq!(id.0[6] >> 4, 4);
        assert_eq!(id.0[8] >> 6, 0b10);
        assert_ne!(id, DocId::random().unwrap());
        assert_eq!(id.to_string().len(), 36);
    }

    #[test]
    fn object_ids_display_and_parse() {
        let id = ObjectId([0xAB; 32]);
        assert_eq!(id.to_string(), "ab".repeat(32));
        assert_eq!(ObjectId::from_slice(&[0xAB; 32]), Some(id));
        assert_eq!(ObjectId::from_slice(&[0; 31]), None);
    }
}
