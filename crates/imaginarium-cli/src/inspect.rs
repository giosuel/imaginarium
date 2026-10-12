// SPDX-License-Identifier: AGPL-3.0-or-later

//! `imag inspect`: the records of a file, without needing keys.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use imaginarium_format::RecordType;
use serde_json::json;

use crate::keys::KeyArgs;
use crate::scan::{
    Scan, flag_letters, scan, short_hex, signature_summary, size, stream_name, type_name,
};

pub(crate) fn run(path: &Path, keys: &KeyArgs, as_json: bool, out: &mut dyn Write) -> Result<()> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let scan = scan(&bytes)?;
    let heads = index_summary(path, keys, &scan);
    if as_json {
        write_json(path, &scan, heads.as_ref(), out)
    } else {
        write_text(path, &scan, heads.as_ref(), out)
    }
}

/// The streams of the index, if it can be read (unencrypted, or with keys).
fn index_summary(path: &Path, keys: &KeyArgs, scan: &Scan) -> Option<Result<Vec<String>, String>> {
    if scan.header.is_encrypted() && !keys.given() {
        return None;
    }
    let result = (|| -> Result<Vec<String>> {
        let file = std::fs::File::open(path)?;
        let reader = imaginarium_io_native::open_reader(file, keys.credentials()?)?;
        let mut lines: Vec<String> = reader
            .index()
            .heads()
            .iter()
            .map(|head| {
                let name = match head.stream() {
                    imaginarium_format::pb::Stream::Main => "main".to_owned(),
                    imaginarium_format::pb::Stream::Discussion => "discussion".to_owned(),
                    imaginarium_format::pb::Stream::Extension => {
                        format!("ext:{}", head.extension_id)
                    }
                    imaginarium_format::pb::Stream::Unspecified => "-".to_owned(),
                };
                let snapshot = if head.snapshot_id.is_empty() {
                    "no snapshot"
                } else {
                    "snapshot"
                };
                format!("{name}: {snapshot} + {} updates", head.update_ids.len())
            })
            .collect();
        if let Some(recovery) = reader.recovery() {
            lines.push(format!(
                "recovered ({:?}): {}",
                recovery.method, recovery.cause
            ));
        }
        Ok(lines)
    })();
    Some(result.map_err(|err| format!("{err:#}")))
}

fn write_text(
    path: &Path,
    scan: &Scan,
    heads: Option<&Result<Vec<String>, String>>,
    out: &mut dyn Write,
) -> Result<()> {
    let header = &scan.header;
    writeln!(
        out,
        "File      {} ({})",
        path.display(),
        size(scan.file_len)
    )?;
    writeln!(
        out,
        "Format    .imag {}.{}, document {}{}",
        header.major,
        header.minor,
        header.doc_id,
        if header.is_encrypted() {
            ", encrypted"
        } else {
            ""
        }
    )?;
    writeln!(out, "Commits   {}", scan.trailers.len())?;
    match heads {
        None => writeln!(out, "Streams   (encrypted index; give a key to read it)")?,
        Some(Ok(lines)) => {
            for (i, line) in lines.iter().enumerate() {
                writeln!(
                    out,
                    "{}{line}",
                    if i == 0 { "Streams   " } else { "          " }
                )?;
            }
        }
        Some(Err(err)) => writeln!(out, "Streams   can't read the index: {err}")?,
    }
    writeln!(out)?;
    writeln!(
        out,
        "{:>9} {:>9}  {:<6} {:<6} {:<10} {:>5}  {:<8}  Details",
        "Offset", "Size", "Type", "Flags", "Stream", "Epoch", "Object"
    )?;
    let mut totals: BTreeMap<String, (usize, usize)> = BTreeMap::new();
    for scanned in &scan.records {
        let record = &scanned.record;
        let h = record.header();
        let name = type_name(record.rtype());
        let total = totals.entry(name.clone()).or_default();
        total.0 += 1;
        total.1 += scanned.len;
        writeln!(
            out,
            "{:>9} {:>9}  {:<6} {:<6} {:<10} {:>5}  {:<8}  {}",
            scanned.offset,
            size(scanned.len),
            name,
            flag_letters(record.flags()),
            stream_name(h),
            if h.epoch == 0 {
                "-".into()
            } else {
                h.epoch.to_string()
            },
            short_hex(&h.object_id),
            details(record),
        )?;
    }
    writeln!(out)?;
    let totals: Vec<String> = totals
        .iter()
        .map(|(name, (n, bytes))| format!("{name} {n} ({})", size(*bytes)))
        .collect();
    writeln!(out, "Totals    {}", totals.join(" · "))?;
    writeln!(
        out,
        "Flags     C critical · S safe to copy · Z zstd · E encrypted · G segmented · N signed"
    )?;
    if let Some((offset, reason)) = &scan.stopped {
        writeln!(
            out,
            "Warning   the scan stopped at offset {offset} ({} unreadable): {reason}",
            size(scan.file_len - offset)
        )?;
    }
    Ok(())
}

fn details(record: &imaginarium_format::Record) -> String {
    let h = record.header();
    let mut parts = Vec::new();
    if h.plain_len > 0 && record.rtype() != RecordType::INDX {
        parts.push(format!(
            "{} plain",
            size(usize::try_from(h.plain_len).unwrap_or(usize::MAX))
        ));
    }
    if !h.codec.is_empty() {
        parts.push(h.codec.clone());
    }
    if let Some(segment) = &h.segment {
        // Untrusted: u32::MAX + 1 must not overflow.
        parts.push(format!(
            "segment {}/{}",
            u64::from(segment.index) + 1,
            segment.count
        ));
    }
    if !h.extension_id.is_empty() {
        parts.push(h.extension_id.clone());
    }
    if let Some(signature) = signature_summary(record) {
        parts.push(format!("signed {signature}"));
    }
    parts.join(", ")
}

fn write_json(
    path: &Path,
    scan: &Scan,
    heads: Option<&Result<Vec<String>, String>>,
    out: &mut dyn Write,
) -> Result<()> {
    let records: Vec<serde_json::Value> = scan
        .records
        .iter()
        .map(|scanned| {
            let record = &scanned.record;
            let h = record.header();
            json!({
                "offset": scanned.offset,
                "length": scanned.len,
                "type": type_name(record.rtype()),
                "rtype": record.rtype().0,
                "flags": flag_letters(record.flags()),
                "stream": stream_name(h),
                "epoch": h.epoch,
                "object_id": h.object_id.iter().map(|b| format!("{b:02x}")).collect::<String>(),
                "plain_len": h.plain_len,
                "codec": h.codec,
                "extension_id": h.extension_id,
                "segment": h.segment.as_ref().map(|s| json!({"index": s.index, "count": s.count})),
                "signature": signature_summary(record),
            })
        })
        .collect();
    let value = json!({
        "file": path.display().to_string(),
        "size": scan.file_len,
        "format": {"major": scan.header.major, "minor": scan.header.minor},
        "doc_id": scan.header.doc_id.to_string(),
        "encrypted": scan.header.is_encrypted(),
        "commits": scan.trailers.len(),
        "streams": match heads {
            None => serde_json::Value::Null,
            Some(Ok(lines)) => json!(lines),
            Some(Err(err)) => json!({"error": err}),
        },
        "records": records,
        "stopped": scan.stopped.as_ref().map(|(offset, reason)| json!({"offset": offset, "reason": reason})),
    });
    serde_json::to_writer_pretty(&mut *out, &value)?;
    writeln!(out)?;
    Ok(())
}
