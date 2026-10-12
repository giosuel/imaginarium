// SPDX-License-Identifier: AGPL-3.0-or-later

//! Record framing parser (spec §4.2) on arbitrary bytes. Beyond "no
//! panics", it checks the verbatim invariant: a decoded record re-encodes
//! to exactly the bytes it was decoded from.
#![no_main]

use imaginarium_format::{Limits, Record};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let limits = Limits::default();
    if let Ok((record, len)) = Record::decode(data, &limits) {
        assert_eq!(record.encode(), &data[..len], "re-encoding must be byte-identical");
        // The streaming reader must agree with the buffer decoder.
        let (streamed, streamed_len) = Record::read_from(&mut &data[..], &limits).expect("streaming read");
        assert_eq!((streamed, streamed_len), (record, len));
    }
});
