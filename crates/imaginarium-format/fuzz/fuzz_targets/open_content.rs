// SPDX-License-Identifier: AGPL-3.0-or-later

//! Payload processing (spec §4.4) on arbitrary records: zstd frames and
//! `plain_len` bounds. Must never panic or over-allocate.
#![no_main]

use imaginarium_format::{DocId, Limits, Record};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = Limits::default();
    if let Ok((record, _)) = Record::decode(data, &limits) {
        if let Ok(content) = record.open_content(&DocId([0; 16]), None, &limits) {
            assert_eq!(content.len() as u64, record.header().plain_len);
        }
    }
});
