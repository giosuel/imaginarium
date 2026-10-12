# `.imag` test vectors

Reproducible files for checking an implementation against
[`../spec-v1.md`](../spec-v1.md) (§13). All of them use the document id
`6d696e69-6d61-6c40-8976-6563746f7273`, and object ids made of a single
repeated byte (`0x01…01`, `0x02…02`, …), so they are easy to read in a hex
dump. Contents are opaque placeholder bytes, because these vectors test
the container, not the CRDT codec.

The reference implementation's test
(`crates/imaginarium-format/tests/vectors.rs`) checks that it writes these
files **byte for byte** and reads them with the results below.

| File | Content | Expected when reading |
|---|---|---|
| `01-minimal.imag` | Header, `META` (`DocumentMeta`: format 1.0, `blob_id_key` = 32 × `0xB1`), `SNAP` (stream main, codec `loro-1`), `INDX`, trailer | No recovery. Entries: `META` at offset 36, `SNAP`. Snapshot content `opaque snapshot bytes`. |
| `02-two-commits.imag` | Vector 1, then one incremental save: `UPDT` `0x03…`, new `INDX`, new trailer | Its first 427 bytes equal vector 1 (append-only). Three entries. Main head = `SNAP 0x02…` + update `0x03…`. |
| `03a-torn-append.imag` | Vector 2, plus a complete uncommitted `UPDT 0x04…` and the first half of `UPDT 0x05…` | Recovery `PreviousCommit` with 1 adopted record. Main head has updates `0x03…`, `0x04…`. |
| `03b-corrupt-trailer.imag` | Vector 1 with one flipped byte in the trailer's CRC | Recovery `PreviousCommit`: the index before the trailer is intact (spec §9.5, step 2). No adopted records, 24 discarded bytes. |
| `03c-no-index.imag` | Header and three records (`META`, `SNAP`, `UPDT`), no index or trailer | Recovery `ForwardScan` with 3 records. |
| `04-unknown-records.imag` | `META`, extension `0x4001` (`SAFE_TO_COPY`), extension `0x4002` (no flags) | Compaction keeps `0x4001` byte-for-byte, drops `0x4002`, and reports it. |

## Layout of `01-minimal.imag` (427 bytes)

| Offset | Length | Content |
|---|---|---|
| 0 | 36 | Header: magic `89 49 4D 47 0D 0A 1A 0A`, major `01 00`, minor `00 00`, flags `00 00 00 00`, doc id, 4 reserved zero bytes |
| 36 | 116 | `META` record: rtype `01`, rflags `03` (CRITICAL, SAFE_TO_COPY), hlen 36, header, plen 72, payload, CRC |
| 152 | 75 | `SNAP` record: rtype `03`, rflags `01` (CRITICAL), hlen 46, header, plen 21, payload, CRC |
| 227 | 176 | `INDX` record: rtype varint `FF 7F` (0x3FFF), rflags `01`, index payload |
| 403 | 24 | Trailer: index offset 227, length 176, CRC, `IMGEND\0\0` |

## Encrypted vectors (05–10)

`crates/imaginarium-crypto/tests/vectors.rs` writes these byte for byte
and checks the results below. Nonces, salts, STREAM prefixes, HPKE
encapsulations and hedged ML-DSA signatures come from a seeded generator
that only the reference implementation's test build has. Another
implementation can't reproduce the bytes, but it needs only the keys below
to run every reading check.

### Inputs

All keys are 32 bytes of one repeated value.

| Input | Value |
|---|---|
| `DEK_1`, `DEK_2` (epoch keys) | `0xD1…`, `0xD2…` |
| `DocumentMeta.blob_id_key` | `0xB1…` |
| Folder key (05, 06), folder id | `0xF0…`, UTF-8 `vector folder` |
| Password (07) | `café imaginarium`, NFC (é = U+00E9) |
| Argon2id (07), **vectors only** | 64 KiB, 1 iteration, parallelism 1 |
| X-Wing recipient seed (08) | `0xA1…` |
| Device Ed25519 seed (09) | `0xE1…` |
| Identity seeds (09): Ed25519, ML-DSA-65 ξ | `0xE2…`, `0xE3…` |
| ACL users (10): owner `n = 0x10`, editor `0x20`, viewer `0x30` | identity Ed25519 `n`, ML-DSA ξ `n+1`, identity X-Wing `n+2`, device Ed25519 `n+3`, device X-Wing `n+4` |

X-Wing key pairs are derived from their seed with HPKE `DeriveKeyPair`.
Content records have object ids `0x01…` (`META`), `0x02…` (`SNAP`) and
`0x03…`, `0x04…` (`UPDT`). Their contents are those of vectors 1–4, and an
update `n` contains the text `opaque update n ` repeated 16 times.

### Files

| File | Content | Expected when reading |
|---|---|---|
| `05-encrypted.imag` | Encrypted header; `KEYR` (one folder-key wrap of `DEK_1`); encrypted `META`, `SNAP`, and `UPDT 0x03…` with `ZSTD`; encrypted `INDX` | Opens with the folder key, or with `DEK_1` directly. `UPDT` decrypts, then decompresses. A wrong folder key, folder id or DEK fails, and so does opening without a key. |
| `06a-segmented-blob.imag` | Like 05, with a blob of 1 MiB + 1000 bytes (byte `i` = `i × 7 mod 256`) in five `ZSTD` segments | `read_blob(blob_id)` returns the blob. `blob_id` = keyed BLAKE3 of the blob under `blob_id_key`. |
| `06b-segments-swapped.imag` | 06a with the payloads of segments 1 and 2 exchanged (headers unchanged, CRCs valid) | Opens, but reading the blob fails authentication. |
| `06c-segment-missing.imag` | 06a without the last segment | Opens, but reading the blob fails (4 of 5 segments). |
| `06d-last-flag-missing.imag` | 06a with the last segment encrypted under a nonce whose last flag is 0 | Opens, but reading the blob fails authentication. |
| `07-password.imag` | `KEYR` with one password wrap of `DEK_1`; encrypted `META` | Opens with the password, also when typed in NFD (e + U+0301). Fails with `cafe imaginarium`. |
| `08-hpke.imag` | `KEYR` with HPKE wraps of `DEK_1` and `DEK_2` to the recipient; `SNAP` under epoch 1, `UPDT` under epoch 2 | Both open with the recipient's key. Another X-Wing key fails. |
| `09-signatures.imag` | Unencrypted; `META`, `UPDT 0x03…` signed by the device (Ed25519), `UPDT 0x04…` signed by the identity (Ed25519 + ML-DSA-65) | Both signatures verify with their key. Neither verifies with the other key, and neither does a changed payload. |
| `10a-acl-chain.imag` | `ACLC` 0 genesis (owner), 1 add editor, 2 editor adds their device; `KEYR` (`DEK_1` to owner and editor, signed by the owner); `META`; `UPDT 0x03…` by the editor's device (epoch 1); `ACLC` 3 remove editor, 4 rotate to epoch 2; `KEYR` (`DEK_1`, `DEK_2` to the owner only); `UPDT 0x04…` by the owner's device (epoch 2); index under epoch 2 | The chain replays to: the owner as the only member, epoch 2, next index 5, no rotation pending. Every record is authorized (§8.4), including the editor's edit from before the removal. The owner reads both updates. The editor's identity key can't open the file, because the index is under epoch 2. |
| `10b-acl-fork.imag` | `ACLC` 0 genesis, 1 add editor, **another** 1 add viewer; `KEYR` for the owner; `META` | Opening fails: ACL fork at index 1. |
| `10c-acl-unauthorized.imag` | `ACLC` 0 genesis, 1 add editor, 2 add viewer **signed by the editor**; `KEYR` for the owner; `META` | Opening fails: the editor isn't allowed to add members. |

## Regenerating

Only when the spec changes deliberately:

```sh
IMAGINARIUM_UPDATE_VECTORS=1 cargo test -p imaginarium-format --test vectors
IMAGINARIUM_UPDATE_VECTORS=1 cargo test -p imaginarium-crypto --test vectors
```

Commit the changed files together with the spec change that caused them.
