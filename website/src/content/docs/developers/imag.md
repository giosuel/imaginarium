---
title: The imag tool
description: Inspect, verify and export Imaginarium documents (.imag) from the command line.
---

`imag` reads `.imag` files without the app. Use it to look inside a
document, to check that it's intact and authentic, and to dump its content
for debugging. It is the reference reader of the
[file format](/developers/reference/format/spec-v1/).

```sh
cargo run --release -p imaginarium-cli -- --help    # from a checkout
imag inspect report.imag
imag verify --password-stdin report.imag
imag export-json --key-file device.key shared.imag > shared.json
```

## Commands

| Command | What it does | Needs a key? |
|---|---|---|
| `imag inspect FILE` | Lists the records: type, size, flags, stream, key epoch, signature. Add `--json` for scripts. With a key, it also shows the streams from the index. | No. Record headers are plaintext, the contents stay encrypted. |
| `imag verify FILE` | Runs every check (see below). Exits with code 0 if nothing failed, 1 if something did. | For everything past the structure checks |
| `imag export-json FILE` | Prints the decrypted document as JSON. Add `--compact` for one line. | For encrypted files |

Exit code 2 means the command couldn't run, for example because a file is
missing or the arguments conflict.

## What `verify` checks

1. **Records:** every record's framing and CRC, scanned without the index.
   Bytes after the last commit, left by an interrupted save, produce a
   warning, not a failure.
2. **Container:** the trailer and the index. If they are damaged, the file
   is recovered (spec §9.5) and you get a warning.
3. **Contents:** every indexed record is decrypted and decompressed
   (authentication tags, `plain_len`).
4. **Blobs:** every segment is present, in order and authentic.
5. **Signatures:** in documents with an ACL chain, the chain itself
   (linearity, forks, who may change what), every keyring, and every
   signed content record.
6. **Document:** the CRDT loads, the schema version and required features
   are understood, and the node count.

## Keys

Secrets are never passed as arguments, which other users of a machine can
read from the process list.

| Option | Use |
|---|---|
| `--password-stdin` | Reads the password from the first line of standard input, for example `imag verify --password-stdin doc.imag < pw.txt`. |
| `--password-env VAR` | Reads the password from the environment variable `VAR`. |
| `--key-file FILE` | A device's or user's X-Wing private key: 64 hex characters, as exported by Imaginarium. Used for shared documents. |

## Example

```text
$ imag verify --key-file owner.key 10a-acl-chain.imag
imag verify 10a-acl-chain.imag
  ok    records      11 records, all checksums valid
  ok    container    trailer and index valid, 10 entries
  ok    contents     10 records read (3 decrypted)
  ok    blobs        none
  ok    signatures   ACL chain valid (5 changes, 1 members, epoch 2); 4 records verified
  FAIL  document     can't decode the document: …
Result: 1 problem found
```

This is the published test vector 10a. Its records hold placeholder
bytes instead of a real document, so the last check fails by design.
