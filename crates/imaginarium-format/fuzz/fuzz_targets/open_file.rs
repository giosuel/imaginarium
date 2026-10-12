// SPDX-License-Identifier: AGPL-3.0-or-later

//! Whole-file opening (trailer, index, recovery) and content reads on
//! arbitrary bytes. Must never panic or allocate in proportion to claimed
//! (rather than actual) sizes.
#![no_main]

use std::io::Cursor;

use imaginarium_format::Reader;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(mut reader) = Reader::open(Cursor::new(data)) {
        let entries = reader.index().entries().to_vec();
        for entry in entries.iter().take(32) {
            let _ = reader.read_content(entry);
        }
    }
});
