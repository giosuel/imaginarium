// SPDX-License-Identifier: AGPL-3.0-or-later

//! The 24-byte trailer at the end of every file (spec §4.5).

use crate::{FormatError, Result};

/// Trailer magic: `IMGEND\0\0`.
pub const TRAILER_MAGIC: [u8; 8] = *b"IMGEND\0\0";

/// Trailer size in bytes.
pub const TRAILER_LEN: usize = 24;

/// Points to the latest index record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Trailer {
    /// Offset of the index record's first byte.
    pub index_offset: u64,
    /// Total length of the index record.
    pub index_len: u32,
}

impl Trailer {
    /// The 24 trailer bytes.
    pub fn encode(&self) -> [u8; TRAILER_LEN] {
        let mut out = [0u8; TRAILER_LEN];
        out[0..8].copy_from_slice(&self.index_offset.to_le_bytes());
        out[8..12].copy_from_slice(&self.index_len.to_le_bytes());
        let crc = crc32c::crc32c(&out[0..12]);
        out[12..16].copy_from_slice(&crc.to_le_bytes());
        out[16..24].copy_from_slice(&TRAILER_MAGIC);
        out
    }

    /// Parses a trailer and verifies its magic and CRC.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let bytes: &[u8; TRAILER_LEN] = bytes
            .try_into()
            .map_err(|_| FormatError::Truncated { what: "trailer" })?;
        if bytes[16..24] != TRAILER_MAGIC {
            return Err(FormatError::BadMagic { what: "trailer" });
        }
        let crc = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
        if crc != crc32c::crc32c(&bytes[0..12]) {
            return Err(FormatError::Crc { what: "trailer" });
        }
        let mut offset = [0u8; 8];
        offset.copy_from_slice(&bytes[0..8]);
        Ok(Self {
            index_offset: u64::from_le_bytes(offset),
            index_len: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_corruption() {
        let t = Trailer {
            index_offset: 123_456,
            index_len: 789,
        };
        let bytes = t.encode();
        assert_eq!(Trailer::decode(&bytes).unwrap(), t);
        let mut flipped = bytes;
        flipped[3] ^= 1;
        assert!(matches!(
            Trailer::decode(&flipped),
            Err(FormatError::Crc { .. })
        ));
        let mut magic = bytes;
        magic[20] = b'X';
        assert!(matches!(
            Trailer::decode(&magic),
            Err(FormatError::BadMagic { .. })
        ));
        assert!(Trailer::decode(&bytes[..23]).is_err());
    }
}
