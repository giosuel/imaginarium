// SPDX-License-Identifier: AGPL-3.0-or-later

//! `imag verify`: every check this version can run on a file.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};
use imaginarium_crypto::{AclState, signer_key_id};
use imaginarium_format::{FormatError, ObjectId, Record, RecordFlags, RecordType};
use imaginarium_io_native::{IndexedRecords, NativeError, read_document};
use imaginarium_model::Mode;

use crate::keys::KeyArgs;
use crate::scan::{scan, size, type_name};

enum Outcome {
    Ok(String),
    Warn(String),
    Fail(String),
    Skipped(String),
}

struct Report<'w> {
    out: &'w mut dyn Write,
    failures: usize,
    warnings: usize,
    skipped: usize,
}

impl Report<'_> {
    fn check(&mut self, name: &str, outcome: Outcome) -> Result<()> {
        let (label, text) = match outcome {
            Outcome::Ok(text) => ("ok  ", text),
            Outcome::Warn(text) => {
                self.warnings += 1;
                ("warn", text)
            }
            Outcome::Fail(text) => {
                self.failures += 1;
                ("FAIL", text)
            }
            Outcome::Skipped(text) => {
                self.skipped += 1;
                ("skip", text)
            }
        };
        writeln!(self.out, "  {label}  {name:<12} {text}")?;
        Ok(())
    }
}

/// Runs every check. `Ok(true)` if nothing failed.
pub(crate) fn run(path: &Path, keys: &KeyArgs, out: &mut dyn Write) -> Result<bool> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    writeln!(out, "imag verify {}", path.display())?;
    let mut report = Report {
        out,
        failures: 0,
        warnings: 0,
        skipped: 0,
    };

    // 1. Framing and checksums of every record, without keys or the index.
    let scanned = scan(&bytes)?;
    let records = scanned.records.len();
    let outcome = match &scanned.stopped {
        None => Outcome::Ok(format!("{records} records, all checksums valid")),
        Some((offset, reason)) if scanned.trailers.last().is_some_and(|last| offset > last) => {
            Outcome::Warn(format!(
                "{records} records; {} after the last commit can't be read (an interrupted save?): {reason}",
                size(bytes.len() - offset)
            ))
        }
        Some((offset, reason)) => {
            Outcome::Fail(format!("unreadable from offset {offset}: {reason}"))
        }
    };
    report.check("records", outcome)?;

    // 2. The container: trailer and index (decrypted with the key).
    let file = std::fs::File::open(path)?;
    let mut reader = match imaginarium_io_native::open_reader(file, keys.credentials()?) {
        Ok(reader) => reader,
        Err(NativeError::Format(FormatError::NeedsKey)) => {
            let reason = "encrypted: give --password-stdin, --password-env or --key-file";
            report.check("container", Outcome::Skipped(reason.into()))?;
            for name in ["contents", "blobs", "signatures", "document"] {
                report.check(name, Outcome::Skipped("needs the key".into()))?;
            }
            return finish(report);
        }
        Err(err) => {
            report.check("container", Outcome::Fail(format!("can't open: {err}")))?;
            return finish(report);
        }
    };
    let entries = reader.index().entries().to_vec();
    let outcome = match reader.recovery() {
        Some(recovery) => Outcome::Warn(format!(
            "the last commit is unusable ({}); recovered with {:?}, {} entries",
            recovery.cause,
            recovery.method,
            entries.len()
        )),
        None => Outcome::Ok(format!(
            "trailer and index valid, {} entries",
            entries.len()
        )),
    };
    report.check("container", outcome)?;

    // 3. Every indexed record, its content decrypted and decompressed.
    let mut indexed: Vec<Record> = Vec::with_capacity(entries.len());
    let mut problems = Vec::new();
    for entry in &entries {
        match reader.read_record(entry) {
            Ok(record) => indexed.push(record),
            Err(err) => problems.push(format!("at {}: {err}", entry.offset)),
        }
    }
    let mut contents = 0;
    for (entry, record) in entries.iter().zip(&indexed) {
        if matches!(record.rtype(), RecordType::INDX) {
            continue;
        }
        match reader.read_content(entry) {
            Ok(_) => contents += 1,
            Err(err) => problems.push(format!(
                "{} at {}: {err}",
                type_name(record.rtype()),
                entry.offset
            )),
        }
    }
    let encrypted = indexed
        .iter()
        .filter(|r| r.flags().contains(RecordFlags::ENCRYPTED))
        .count();
    report.check(
        "contents",
        if problems.is_empty() {
            Outcome::Ok(format!("{contents} records read ({encrypted} decrypted)"))
        } else {
            Outcome::Fail(summarize(&problems))
        },
    )?;

    // 4. Blobs: every segment present, in order, authentic.
    let blob_ids: BTreeSet<Vec<u8>> = entries
        .iter()
        .filter(|e| e.rtype == RecordType::BLOB.0)
        .map(|e| e.object_id.clone())
        .collect();
    let mut blob_problems = Vec::new();
    for id in &blob_ids {
        let Some(object) = ObjectId::from_slice(id) else {
            continue;
        };
        if let Err(err) = reader.read_blob(&object) {
            blob_problems.push(format!("{object}: {err}"));
        }
    }
    report.check(
        "blobs",
        match (blob_ids.len(), blob_problems.is_empty()) {
            (0, _) => Outcome::Ok("none".into()),
            (n, true) => Outcome::Ok(format!("{n} blobs complete")),
            (_, false) => Outcome::Fail(summarize(&blob_problems)),
        },
    )?;

    // 5. Signatures and the ACL chain.
    let doc_id = reader.header().doc_id;
    let chain: Vec<Record> = indexed
        .iter()
        .filter(|r| r.rtype() == RecordType::ACLC)
        .cloned()
        .collect();
    let signed = indexed
        .iter()
        .filter(|r| r.signature_bytes().is_some())
        .count();
    if chain.is_empty() {
        report.check(
            "signatures",
            if signed == 0 {
                Outcome::Ok("no signed records, no ACL chain".into())
            } else {
                Outcome::Skipped(format!(
                    "{signed} signed records, but no ACL chain to check them against"
                ))
            },
        )?;
    } else {
        report.check(
            "signatures",
            check_acl(&doc_id, &chain, &indexed, keys.owner_unchecked()),
        )?;
    }

    // 6. The document itself.
    let records = IndexedRecords::new(&reader);
    let outcome = match read_document(&mut reader, &records) {
        Ok(document) => {
            let view = document.view();
            let version = view
                .meta()
                .schema_version()
                .map_or("?".into(), |v| v.to_string());
            match document.mode() {
                Mode::Editable => Outcome::Ok(format!(
                    "schema {version}, {} nodes, editable",
                    view.node_count()
                )),
                Mode::ReadOnly(reason) => Outcome::Warn(format!(
                    "schema {version}, {} nodes, read-only: {reason}",
                    view.node_count()
                )),
            }
        }
        Err(NativeError::NoDocument) => {
            Outcome::Warn("no main stream: the file holds no document".into())
        }
        Err(err) => Outcome::Fail(err.to_string()),
    };
    report.check("document", outcome)?;
    finish(report)
}

/// Replays the chain and checks every keyring and content signature. With
/// `owner_unchecked` (a recipient key without `--owner`), nothing showed
/// that the chain is the document's real one, so a valid chain is a warning
/// that names its owner (ADR-0018).
fn check_acl(
    doc_id: &imaginarium_format::DocId,
    chain: &[Record],
    records: &[Record],
    owner_unchecked: bool,
) -> Outcome {
    let acl = match AclState::replay(doc_id, chain) {
        Ok(acl) => acl,
        Err(err) => return Outcome::Fail(format!("ACL chain: {err}")),
    };
    let mut problems = Vec::new();
    let mut verified = 0;
    for record in records {
        let result = match record.rtype() {
            RecordType::KEYR => acl.verify_keyring(record),
            RecordType::UPDT | RecordType::SNAP | RecordType::VERS => acl.verify_content(record),
            _ => continue,
        };
        match result {
            Ok(()) => verified += 1,
            Err(err) => {
                let by = signer_key_id(record)
                    .map_or_else(|_| "unsigned".into(), |id| format!("{id:?}"));
                problems.push(format!("{} ({by}): {err}", type_name(record.rtype())));
            }
        }
    }
    let owner = hex(&acl.genesis_owner().0);
    let summary = format!(
        "ACL chain valid ({} changes, {} members, epoch {}, started by {owner}); \
         {verified} records verified",
        acl.next_index(),
        acl.members().len(),
        acl.epoch()
    );
    if !problems.is_empty() {
        Outcome::Fail(summarize(&problems))
    } else if owner_unchecked {
        Outcome::Warn(format!(
            "{summary}, but the owner wasn't checked: pass --owner {owner} if you trust it"
        ))
    } else {
        Outcome::Ok(summary)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn summarize(problems: &[String]) -> String {
    let mut text = problems
        .iter()
        .take(3)
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
    if problems.len() > 3 {
        text.push_str(&format!("; and {} more", problems.len() - 3));
    }
    text
}

fn finish(report: Report<'_>) -> Result<bool> {
    let plural = |n: usize, word: &str| format!("{n} {word}{}", if n == 1 { "" } else { "s" });
    let mut notes = Vec::new();
    if report.warnings > 0 {
        notes.push(plural(report.warnings, "warning"));
    }
    if report.skipped > 0 {
        notes.push(format!("{} skipped", plural(report.skipped, "check")));
    }
    let notes = if notes.is_empty() {
        String::new()
    } else {
        format!(" ({})", notes.join(", "))
    };
    let summary = if report.failures == 0 {
        format!("Result: ok{notes}")
    } else {
        format!(
            "Result: {} found{notes}",
            plural(report.failures, "problem")
        )
    };
    writeln!(report.out, "{summary}")?;
    Ok(report.failures == 0)
}
