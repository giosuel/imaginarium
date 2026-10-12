// SPDX-License-Identifier: AGPL-3.0-or-later

//! Credentials from the command line. Secrets never come from arguments,
//! which other users on the machine can see in the process list.

use std::io::BufRead;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Args;
use imaginarium_crypto::XWingKeyPair;
use imaginarium_io_native::Credentials;
use zeroize::Zeroizing;

/// How to unlock an encrypted document.
#[derive(Args, Debug, Default)]
pub(crate) struct KeyArgs {
    /// Read the password from the first line of standard input.
    #[arg(long, conflicts_with_all = ["password_env", "key_file"])]
    pub password_stdin: bool,
    /// Read the password from this environment variable.
    #[arg(long, value_name = "VAR", conflicts_with = "key_file")]
    pub password_env: Option<String>,
    /// Use the X-Wing private key in this file (64 hex characters, as
    /// exported by Imaginarium).
    #[arg(long, value_name = "FILE")]
    pub key_file: Option<PathBuf>,
}

impl KeyArgs {
    /// Whether any credential was given.
    pub(crate) fn given(&self) -> bool {
        self.password_stdin || self.password_env.is_some() || self.key_file.is_some()
    }

    /// The credentials. Reads stdin or the key file once.
    pub(crate) fn credentials(&self) -> Result<Credentials> {
        if self.password_stdin {
            let mut line = Zeroizing::new(String::new());
            std::io::stdin()
                .lock()
                .read_line(&mut line)
                .context("reading the password from stdin")?;
            let password = line.trim_end_matches(['\r', '\n']);
            return Ok(Credentials::Password(Zeroizing::new(password.to_owned())));
        }
        if let Some(var) = &self.password_env {
            let password = std::env::var(var)
                .with_context(|| format!("environment variable {var} isn't set"))?;
            return Ok(Credentials::Password(Zeroizing::new(password)));
        }
        if let Some(path) = &self.key_file {
            let text = Zeroizing::new(
                std::fs::read_to_string(path)
                    .with_context(|| format!("reading {}", path.display()))?,
            );
            let bytes = Zeroizing::new(
                decode_hex(text.trim())
                    .with_context(|| format!("{} isn't a hex key", path.display()))?,
            );
            let pair =
                XWingKeyPair::from_private_bytes(&bytes).context("not an X-Wing private key")?;
            return Ok(Credentials::Recipient(pair));
        }
        Ok(Credentials::None)
    }
}

fn decode_hex(text: &str) -> Result<Vec<u8>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        bail!("odd length or non-ASCII characters");
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).context("not a hex digit"))
        .collect()
}
