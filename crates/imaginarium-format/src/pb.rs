// SPDX-License-Identifier: AGPL-3.0-or-later

//! Protobuf messages of the format, generated at build time from the
//! normative schema `docs/format/proto/imag_v1.proto`.

#![allow(missing_docs, clippy::all, clippy::pedantic, unreachable_pub)]

include!(concat!(env!("OUT_DIR"), "/imaginarium.imag.v1.rs"));
