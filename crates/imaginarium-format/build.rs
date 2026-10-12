// SPDX-License-Identifier: AGPL-3.0-or-later

//! Generates the protobuf types from the normative schema in
//! `docs/format/proto/`, so the code can never drift from the spec. Uses
//! `protox` (pure Rust), so no `protoc` is needed.

use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let proto_dir = manifest_dir.join("../../docs/format/proto");
    let proto = proto_dir.join("imag_v1.proto");
    println!("cargo:rerun-if-changed={}", proto.display());

    let descriptors = match protox::compile([&proto], [&proto_dir]) {
        Ok(descriptors) => descriptors,
        Err(err) => panic!("invalid protobuf schema {}: {err:?}", proto.display()),
    };
    if let Err(err) = prost_build::Config::new().compile_fds(descriptors) {
        panic!("protobuf code generation failed: {err}");
    }
}
