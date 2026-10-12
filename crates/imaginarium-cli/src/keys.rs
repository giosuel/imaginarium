// SPDX-License-Identifier: AGPL-3.0-or-later

//! Credentials from the command line. Secrets never come from arguments,
//! which other users on the machine can see in the process list.

use std::io::BufRead;
use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use clap::Args;
use imaginarium_crypto::{KeyId, XWingKeyPair};
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
    /// The key id (32 hex characters) of the identity that must have
    /// started the document's ACL chain. Without it, any owner is accepted,
    /// and `verify` warns and shows who it was.
    #[arg(long, value_name = "KEYID")]
    pub owner: Option<String>,
}

impl KeyArgs {
    /// Whether any credential was given.
    pub(crate) fn given(&self) -> bool {
        self.password_stdin || self.password_env.is_some() || self.key_file.is_some()
    }

    /// The identity expected to have started the ACL chain (`--owner`).
    pub(crate) fn expected_owner(&self) -> Result<Option<KeyId>> {
        self.owner
            .as_deref()
            .map(|text| {
                decode_hex(text)
                    .ok()
                    .and_then(|bytes| KeyId::from_slice(&bytes))
                    .context("--owner takes a key id of 32 hex characters")
            })
            .transpose()
    }

    /// The credentials. Reads stdin or the key file once.
    pub(crate) fn credentials(&self) -> Result<Credentials> {
        if self.password_stdin {
            // Room for any sane password up front: growing the buffer would
            // leave copies of it behind that nobody zeroes. (std's own stdin
            // buffer keeps one anyway; that can't be helped.)
            let mut line = Zeroizing::new(String::with_capacity(1024));
            let read = std::io::stdin()
                .lock()
                .read_line(&mut line)
                .context("reading the password from stdin")?;
            if read == 0 {
                bail!("no password on standard input");
            }
            let password = line.trim_end_matches(['\r', '\n']);
            return Ok(Credentials::Password(Zeroizing::new(password.to_owned())));
        }
        if let Some(var) = &self.password_env {
            let password = std::env::var(var).map_err(|err| match err {
                std::env::VarError::NotPresent => anyhow!("environment variable {var} isn't set"),
                std::env::VarError::NotUnicode(_) => {
                    anyhow!("environment variable {var} isn't valid UTF-8")
                }
            })?;
            return Ok(Credentials::Password(Zeroizing::new(password)));
        }
        if let Some(path) = &self.key_file {
            let text = Zeroizing::new(
                std::fs::read_to_string(path)
                    .with_context(|| format!("reading {}", path.display()))?,
            );
            // Editors on Windows may start the file with a byte-order mark.
            let hex = text.trim_start_matches('\u{feff}').trim();
            let bytes =
                decode_hex(hex).with_context(|| format!("{} isn't a hex key", path.display()))?;
            let keypair =
                XWingKeyPair::from_private_bytes(&bytes).context("not an X-Wing private key")?;
            let owner = self.expected_owner()?;
            return Ok(Credentials::Recipient { keypair, owner });
        }
        Ok(Credentials::None)
    }
}

/// Decodes hex into a buffer of the final size (so no unzeroed copies of a
/// key are left behind by growing it).
fn decode_hex(text: &str) -> Result<Zeroizing<Vec<u8>>> {
    if !text.len().is_multiple_of(2) || !text.is_ascii() {
        bail!("odd length or non-ASCII characters");
    }
    let mut out = Zeroizing::new(Vec::with_capacity(text.len() / 2));
    for i in (0..text.len()).step_by(2) {
        out.push(u8::from_str_radix(&text[i..i + 2], 16).context("not a hex digit")?);
    }
    Ok(out)
}
