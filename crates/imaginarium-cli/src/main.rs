// SPDX-License-Identifier: AGPL-3.0-or-later

//! `imag`: inspect, verify and export Imaginarium documents (`.imag`).
//!
//! # Responsibility
//! - `imag inspect FILE`: the records of a file (types, sizes, streams,
//!   epochs, signatures), read straight from the plaintext framing, so it
//!   works without keys. `--json` for scripts.
//! - `imag verify FILE`: every check this version can run: checksums, the
//!   index, decryption, blobs, keyrings, signatures, the ACL chain and the
//!   document itself. Exit code 0 if every check ran and passed, 1 if
//!   something failed, 3 if nothing failed but some checks couldn't run
//!   (no key, say).
//! - `imag export-json FILE`: the decrypted document as JSON.
//!
//! Keys come from `--password-stdin`, `--password-env VAR` or
//! `--key-file FILE`, never from an argument, which other users of the
//! machine could read from the process list.
//!
//! # Non-goals (yet)
//! `compact`, `rekey` and editing come later (file-format design §9).
#![forbid(unsafe_code)]

mod export;
mod inspect;
mod keys;
mod scan;
mod verify;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

use crate::keys::KeyArgs;

/// Inspect, verify and export Imaginarium documents (.imag).
#[derive(Parser)]
#[command(name = "imag", version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// List a file's records: types, sizes, streams, epochs, signatures.
    /// Works without keys.
    Inspect {
        /// The .imag file.
        file: PathBuf,
        /// Print JSON instead of a table.
        #[arg(long)]
        json: bool,
        #[command(flatten)]
        keys: KeyArgs,
    },
    /// Check a file: checksums, index, decryption, blobs, keyrings,
    /// signatures, ACL chain and the document. Exits with 1 on problems.
    Verify {
        /// The .imag file.
        file: PathBuf,
        #[command(flatten)]
        keys: KeyArgs,
    },
    /// Print the decrypted document as JSON.
    ExportJson {
        /// The .imag file.
        file: PathBuf,
        /// One line instead of indented JSON.
        #[arg(long)]
        compact: bool,
        #[command(flatten)]
        keys: KeyArgs,
    },
}

/// Exit code when the command couldn't run (bad arguments, unreadable file).
const ERROR: u8 = 2;
/// Exit code of `verify` when nothing failed but some checks couldn't run.
const INCOMPLETE: u8 = 3;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let result = match &cli.command {
        Command::Inspect { file, json, keys } => {
            inspect::run(file, keys, *json, &mut out).map(|()| ExitCode::SUCCESS)
        }
        Command::Verify { file, keys } => {
            verify::run(file, keys, &mut out).map(|verdict| match verdict {
                verify::Verdict::Ok => ExitCode::SUCCESS,
                verify::Verdict::Problems => ExitCode::from(1),
                verify::Verdict::Incomplete => ExitCode::from(INCOMPLETE),
            })
        }
        Command::ExportJson {
            file,
            compact,
            keys,
        } => export::run(file, keys, *compact, &mut out).map(|()| ExitCode::SUCCESS),
    };
    let verifying = matches!(cli.command, Command::Verify { .. });
    match result {
        Ok(code) => code,
        // The reader went away (`imag … | head`): stop quietly, like other
        // tools. A verify that didn't finish has no result, though.
        Err(err) if is_broken_pipe(&err) && verifying => ExitCode::from(ERROR),
        Err(err) if is_broken_pipe(&err) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("imag: {err:#}");
            ExitCode::from(ERROR)
        }
    }
}

fn is_broken_pipe(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        let kind = cause
            .downcast_ref::<std::io::Error>()
            .map(std::io::Error::kind)
            .or_else(|| {
                cause
                    .downcast_ref::<serde_json::Error>()
                    .and_then(serde_json::Error::io_error_kind)
            });
        kind == Some(std::io::ErrorKind::BrokenPipe)
    })
}
