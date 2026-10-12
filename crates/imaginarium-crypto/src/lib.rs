// SPDX-License-Identifier: AGPL-3.0-or-later

//! Suite 1 cryptography for `.imag` documents (spec §6–§8). Work in progress (M1.4).
#![forbid(unsafe_code)]

mod acl;
mod error;
mod keyring;
mod keys;
mod record;
mod rng;
mod sign;

pub use acl::*;
pub use error::{CryptoError, Result};
pub use keyring::*;
pub use keys::*;
pub use record::*;
pub use rng::*;
pub use sign::*;
