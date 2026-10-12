// SPDX-License-Identifier: AGPL-3.0-or-later

//! Strict unsigned LEB128 varints (spec §1): at most 10 bytes, minimal
//! encoding only, no overflow beyond 2⁶⁴−1.

/// Appends `value` as a minimal varint.
pub(crate) fn encode(mut value: u64, out: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7F) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// Number of bytes `encode(value)` produces.
pub(crate) fn encoded_len(value: u64) -> usize {
    let bits = 64 - (value | 1).leading_zeros() as usize;
    bits.div_ceil(7)
}

/// Decodes a varint from the start of `buf`. Returns the value and the
/// number of bytes consumed, or `None` if it is truncated, longer than 10
/// bytes, overflows, or isn't minimally encoded.
pub(crate) fn decode(buf: &[u8]) -> Option<(u64, usize)> {
    let mut value: u64 = 0;
    for (i, &byte) in buf.iter().enumerate().take(10) {
        let payload = u64::from(byte & 0x7F);
        if i == 9 && payload > 1 {
            return None; // would overflow 64 bits
        }
        value |= payload << (7 * i);
        if byte & 0x80 == 0 {
            // Minimal: a multi-byte varint must not end in a zero group.
            if i > 0 && byte == 0 {
                return None;
            }
            return Some((value, i + 1));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn known_encodings() {
        for (value, bytes) in [
            (0u64, vec![0x00]),
            (1, vec![0x01]),
            (127, vec![0x7F]),
            (128, vec![0x80, 0x01]),
            (300, vec![0xAC, 0x02]),
            (
                u64::MAX,
                vec![0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x01],
            ),
        ] {
            let mut out = Vec::new();
            encode(value, &mut out);
            assert_eq!(out, bytes, "{value}");
            assert_eq!(encoded_len(value), bytes.len());
            assert_eq!(decode(&bytes), Some((value, bytes.len())));
        }
    }

    #[test]
    fn rejects_malformed() {
        assert_eq!(decode(&[]), None, "empty");
        assert_eq!(decode(&[0x80]), None, "truncated");
        assert_eq!(decode(&[0x80, 0x00]), None, "non-minimal zero");
        assert_eq!(decode(&[0x81, 0x80, 0x00]), None, "non-minimal 1");
        assert_eq!(decode(&[0xFF; 10]), None, "11+ bytes / overflow");
        let mut overflow = vec![0xFF; 9];
        overflow.push(0x02);
        assert_eq!(decode(&overflow), None, "bit 64 set");
    }

    proptest! {
        #[test]
        fn round_trip(value: u64, trailing in proptest::collection::vec(any::<u8>(), 0..4)) {
            let mut out = Vec::new();
            encode(value, &mut out);
            let len = out.len();
            out.extend(trailing);
            prop_assert_eq!(decode(&out), Some((value, len)));
            prop_assert_eq!(encoded_len(value), len);
        }

        #[test]
        fn decode_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..16)) {
            let _ = decode(&bytes);
        }
    }
}
