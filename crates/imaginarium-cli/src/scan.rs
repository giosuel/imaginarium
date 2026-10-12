// SPDX-License-Identifier: AGPL-3.0-or-later

//! A raw scan of every record in a file, without keys and without the
//! index: record framing and headers are plaintext (spec §4).

use anyhow::{Context, Result};
use imaginarium_format::{
    FileHeader, HEADER_LEN, Limits, Record, RecordFlags, RecordType, TRAILER_LEN, Trailer, pb,
};
use prost::Message;

/// A record found by the scan.
pub(crate) struct Scanned {
    pub offset: usize,
    pub len: usize,
    pub record: Record,
}

/// What a scan found.
pub(crate) struct Scan {
    pub header: FileHeader,
    pub records: Vec<Scanned>,
    /// Offsets of trailers (one per commit).
    pub trailers: Vec<usize>,
    /// Where the scan stopped before the end of the file, and why.
    pub stopped: Option<(usize, String)>,
    pub file_len: usize,
}

/// Scans `bytes`: header, then records and trailers until the end.
pub(crate) fn scan(bytes: &[u8]) -> Result<Scan> {
    let header = FileHeader::decode(
        bytes
            .get(..HEADER_LEN)
            .context("file is shorter than an .imag header")?,
    )?;
    let limits = Limits::default();
    let mut scan = Scan {
        header,
        records: Vec::new(),
        trailers: Vec::new(),
        stopped: None,
        file_len: bytes.len(),
    };
    let mut offset = HEADER_LEN;
    while offset < bytes.len() {
        let rest = &bytes[offset..];
        match Record::decode(rest, &limits) {
            Ok((record, len)) => {
                scan.records.push(Scanned {
                    offset,
                    len,
                    record,
                });
                offset += len;
            }
            Err(record_err) => {
                if rest.len() >= TRAILER_LEN && Trailer::decode(&rest[..TRAILER_LEN]).is_ok() {
                    scan.trailers.push(offset);
                    offset += TRAILER_LEN;
                } else {
                    scan.stopped = Some((offset, record_err.to_string()));
                    break;
                }
            }
        }
    }
    Ok(scan)
}

/// A record type's name.
pub(crate) fn type_name(rtype: RecordType) -> String {
    match rtype {
        RecordType::META => "META".into(),
        RecordType::KEYR => "KEYR".into(),
        RecordType::SNAP => "SNAP".into(),
        RecordType::UPDT => "UPDT".into(),
        RecordType::BLOB => "BLOB".into(),
        RecordType::THMB => "THMB".into(),
        RecordType::VERS => "VERS".into(),
        RecordType::ACLC => "ACLC".into(),
        RecordType::DERV => "DERV".into(),
        RecordType::INDX => "INDX".into(),
        other => format!("{:#06x}", other.0),
    }
}

/// Flags as letters: Critical, Safe to copy, Zstd, Encrypted, seGmented,
/// sigNed; `-` for unset.
pub(crate) fn flag_letters(flags: RecordFlags) -> String {
    [
        (RecordFlags::CRITICAL, 'C'),
        (RecordFlags::SAFE_TO_COPY, 'S'),
        (RecordFlags::ZSTD, 'Z'),
        (RecordFlags::ENCRYPTED, 'E'),
        (RecordFlags::SEGMENTED, 'G'),
        (RecordFlags::SIGNED, 'N'),
    ]
    .iter()
    .map(|(flag, letter)| if flags.contains(*flag) { *letter } else { '-' })
    .collect()
}

/// The stream a record belongs to, if any.
pub(crate) fn stream_name(header: &pb::RecordHeader) -> String {
    match header.stream() {
        pb::Stream::Unspecified => "-".into(),
        pb::Stream::Main => "main".into(),
        pb::Stream::Discussion => "discussion".into(),
        pb::Stream::Extension => format!("ext:{}", header.extension_id),
    }
}

/// The signature algorithm and key id of a signed record.
pub(crate) fn signature_summary(record: &Record) -> Option<String> {
    let bytes = record.signature_bytes()?;
    let Ok(signature) = pb::Signature::decode(bytes) else {
        return Some("malformed signature".into());
    };
    let algorithm = match signature.algorithm() {
        pb::SignatureAlgorithm::Ed25519 => "ed25519",
        pb::SignatureAlgorithm::Ed25519Mldsa65 => "ed25519+ml-dsa-65",
        _ => "unknown algorithm",
    };
    Some(format!("{algorithm} by {}", short_hex(&signature.key_id)))
}

/// The first 4 bytes as hex.
pub(crate) fn short_hex(bytes: &[u8]) -> String {
    bytes.iter().take(4).map(|b| format!("{b:02x}")).collect()
}

/// Bytes, human-readable.
pub(crate) fn size(n: usize) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KiB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MiB", n as f64 / (1024.0 * 1024.0))
    }
}
