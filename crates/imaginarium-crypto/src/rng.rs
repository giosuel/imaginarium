// SPDX-License-Identifier: AGPL-3.0-or-later

//! Randomness. Production code uses only the OS CSPRNG (spec §12).

use rand_core::{TryCryptoRng, TryRng};

use crate::error::{CryptoError, Result};

/// Fills `buf` from the OS CSPRNG, or fails.
pub(crate) fn fill(buf: &mut [u8]) -> Result<()> {
    getrandom::fill(buf).map_err(|err| CryptoError::Random(err.to_string()))
}

/// A source of cryptographic randomness: the OS by default, or a seeded
/// deterministic stream for test vectors (feature `test-vectors`).
pub trait Randomness {
    /// Fills `buf` with random bytes.
    fn fill(&mut self, buf: &mut [u8]) -> Result<()>;
}

/// The OS CSPRNG.
#[derive(Clone, Copy, Debug, Default)]
pub struct OsRandomness;

impl Randomness for OsRandomness {
    fn fill(&mut self, buf: &mut [u8]) -> Result<()> {
        fill(buf)
    }
}

/// Adapts any [`Randomness`] to the `rand_core` traits the HPKE and ML-DSA
/// crates take. Their APIs need an infallible RNG, so the adapter
/// fills a buffer up front (fallibly) and serves from it. Callers pass the
/// exact number of bytes the operation consumes.
pub(crate) struct Prefilled {
    bytes: Vec<u8>,
    pos: usize,
}

impl Prefilled {
    /// Takes `len` bytes from `source`.
    pub(crate) fn new(source: &mut dyn Randomness, len: usize) -> Result<Self> {
        let mut bytes = vec![0u8; len];
        source.fill(&mut bytes)?;
        Ok(Self { bytes, pos: 0 })
    }

    /// Bytes not taken yet.
    pub(crate) fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    fn take(&mut self, dest: &mut [u8]) -> std::result::Result<(), RandomnessExhausted> {
        let end = self.pos + dest.len();
        let src = self.bytes.get(self.pos..end).ok_or(RandomnessExhausted)?;
        dest.copy_from_slice(src);
        self.pos = end;
        Ok(())
    }
}

impl Drop for Prefilled {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.bytes);
    }
}

/// The prefilled buffer ran out: a bug in how many bytes were requested.
#[derive(Debug)]
pub(crate) struct RandomnessExhausted;

impl core::fmt::Display for RandomnessExhausted {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("prefilled randomness exhausted")
    }
}

impl core::error::Error for RandomnessExhausted {}

impl TryRng for Prefilled {
    type Error = RandomnessExhausted;

    fn try_next_u32(&mut self) -> std::result::Result<u32, Self::Error> {
        let mut buf = [0u8; 4];
        self.take(&mut buf)?;
        Ok(u32::from_le_bytes(buf))
    }

    fn try_next_u64(&mut self) -> std::result::Result<u64, Self::Error> {
        let mut buf = [0u8; 8];
        self.take(&mut buf)?;
        Ok(u64::from_le_bytes(buf))
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> std::result::Result<(), Self::Error> {
        self.take(dest)
    }
}

impl TryCryptoRng for Prefilled {}

/// Deterministic randomness from a seed (BLAKE3 in XOF mode).
///
/// **Never use this outside tests and test vectors.** It exists so the
/// published vectors (spec §13) are reproducible. Only available with the
/// `test-vectors` feature.
#[cfg(feature = "test-vectors")]
pub struct SeededRandomness(blake3::OutputReader);

#[cfg(feature = "test-vectors")]
impl SeededRandomness {
    /// A stream derived from `label`. Equal labels give equal streams.
    pub fn new(label: &str) -> Self {
        let mut hasher = blake3::Hasher::new_derive_key("imaginarium.imag test vectors");
        hasher.update(label.as_bytes());
        Self(hasher.finalize_xof())
    }
}

#[cfg(feature = "test-vectors")]
impl Randomness for SeededRandomness {
    fn fill(&mut self, buf: &mut [u8]) -> Result<()> {
        self.0.fill(buf);
        Ok(())
    }
}
