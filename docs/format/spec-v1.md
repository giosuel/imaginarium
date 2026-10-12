# The `.imag` file format, version 1

- **Status: Draft 1.** It becomes normative with tag `v0.0.2-m1`. Until then,
  changes are allowed and are recorded in §14.
- Media type: `application/vnd.imaginarium.imag`. Extension: `.imag`.
- Schema: [`proto/imag_v1.proto`](proto/imag_v1.proto) (normative).
- Background and rationale: [`../design/file-format.md`](../design/file-format.md).
  Crate choices: [`../design/m1-dependencies.md`](../design/m1-dependencies.md).
- License: this specification is part of Imaginarium (AGPL-3.0-or-later).
  Independent implementations of the format are explicitly welcome.

## 1. Conventions

The key words **MUST**, **MUST NOT**, **REQUIRED**, **SHOULD**, **SHOULD
NOT**, **MAY** and **OPTIONAL** are to be interpreted as described in BCP 14
(RFC 2119, RFC 8174) when, and only when, they appear in all capitals.

- `u8`, `u16`, `u32`, `u64`: unsigned integers, **little-endian**.
- `varint`: unsigned LEB128 (as in protobuf), at most 10 bytes. Writers
  MUST use the shortest encoding. Readers MUST reject non-minimal encodings
  and values above 2⁶⁴−1.
- `‖`: byte-string concatenation. `[n]`: exactly `n` bytes.
- "Protobuf message `X`" means the binary protobuf encoding (proto3) of
  message `X` from `imag_v1.proto`.
- `LABEL("…")`: the ASCII bytes of the string, without a terminator.
- Byte offsets are from the start of the file, starting at 0.

## 2. Overview

An `.imag` file is a **pack of immutable records** followed by an **index**
and a **trailer**. New records, a new index and a new trailer are
appended on every save. The file is never rewritten in place, except by
compaction (§9.4).

```
File    = Header Record* (IndexRecord Trailer)+     (appended over time)
Record  = rtype rflags hlen header plen payload [slen sig] crc
```

Records hold CRDT snapshots and updates (the document content), blobs
(media), the keyring, the access-control chain and metadata. The
**same record bytes** are used locally and by the sync server ("Hub").
Records MUST NOT contain file-specific data (offsets, file names), so they
can move between files and servers unchanged.

## 3. Logical layer

### 3.1 CRDT codecs

`SNAP` and `UPDT` payloads are encoded by a **codec** named in
`RecordHeader.codec`.

| Codec id | Meaning |
|---|---|
| `loro-1` | [Loro](https://loro.dev) 1.x snapshot (`ExportMode::Snapshot` or `ShallowSnapshot`) or update encoding |

A reader that doesn't support a record's codec MUST treat the stream as
unreadable (§10.2). The document schema on top of the codec (containers,
keys, node kinds) is defined in `schema-v1.md`, which is written in M1.6.

### 3.2 Streams

| `Stream` | Content |
|---|---|
| `STREAM_MAIN` | The document. |
| `STREAM_DISCUSSION` | Comments and threads, a separate CRDT document. It can be omitted from exports, and commenters may write it (§8.3). |
| `STREAM_EXTENSION` | Add-on data. `extension_id` names the add-on (reverse DNS). |

## 4. Container

### 4.1 Header (36 bytes)

| Offset | Size | Field | Value |
|---|---|---|---|
| 0 | 8 | magic | `89 49 4D 47 0D 0A 1A 0A` (`\x89IMG\r\n\x1a\n`) |
| 8 | 2 | major | `1` |
| 10 | 2 | minor | `0` |
| 12 | 4 | flags | see below |
| 16 | 16 | doc_id | random UUID (version 4) of the **document** |
| 32 | 4 | reserved | zero |

- `flags` bit 0: **ENCRYPTED**, the document is encrypted (§6). Bits 1–15:
  reserved and critical. Readers MUST refuse the file if any of them is
  set. Bits 16–31: reserved and ignorable. Readers MUST ignore them, and
  writers MUST write 0.
- `doc_id` identifies the document across copies, the Hub and exports. It
  is part of every record's authenticated data (§6.3). "Save as a new
  document" MUST create a new `doc_id` and therefore new records.
- A reader MUST refuse a `major` it doesn't know. It SHOULD accept a higher
  `minor` (additive changes only, §10).
- `reserved` MUST be written as zero and MUST be ignored when read.

### 4.2 Record framing

| Field | Encoding | Meaning |
|---|---|---|
| `rtype` | varint | Record type (§5.1). |
| `rflags` | u8 | Flags (§4.3). |
| `hlen` | varint | Length of `header`, at most 65,536. |
| `header` | `[hlen]` | Protobuf message `RecordHeader` (plaintext). |
| `plen` | varint | Length of `payload`. |
| `payload` | `[plen]` | See §4.4. |
| `slen` | varint | Only if `rflags.SIGNED`: length of `sig`, at most 16,384. |
| `sig` | `[slen]` | Only if `rflags.SIGNED`: protobuf message `Signature` (§7). |
| `crc` | u32 | **CRC32C** (Castagnoli) of all preceding bytes of this record, from `rtype` through `sig`. |

The CRC detects torn writes and corruption. It is **not** an integrity
mechanism against attackers. Encrypted records rely on the AEAD for that
(§6), and signed records on their signature (§7).

### 4.3 Record flags (`rflags`)

| Bit | Name | Meaning |
|---|---|---|
| 0 | `CRITICAL` | A reader that doesn't understand `rtype` MUST open the document **read-only** (§10.2). |
| 1 | `SAFE_TO_COPY` | A writer that doesn't understand `rtype` MAY copy the record unchanged during compaction. |
| 2 | `ZSTD` | The plaintext is a zstd frame (§4.4). |
| 3 | `ENCRYPTED` | The payload is AEAD ciphertext (§6.3). |
| 4 | `SEGMENTED` | The record is one segment of a larger blob (§5.3). |
| 5 | `SIGNED` | `slen` and `sig` follow the payload (§7). |
| 6–7 | reserved | MUST be 0. A record with a reserved bit set MUST be treated as a record of an unknown type (§10.2). |

### 4.4 Payload processing

Writing:

1. `P` = the record's plaintext content (§5).
2. If `ZSTD`: `P` = zstd-compress(`P`), as one zstd frame. Writers SHOULD
   NOT compress data that is already compressed (PNG, JPEG, WebP, AVIF,
   video).
3. If `ENCRYPTED`: `payload` = `nonce ‖ AEAD-encrypt(P)` (§6.3).
   Otherwise `payload` = `P`.
4. `RecordHeader.plain_len` = the length of the content from step 1
   (before compression).

Reading reverses the steps. Readers MUST verify that the decompressed
length equals `plain_len`. They MUST stop decompressing as soon as output
would exceed `plain_len`, and MUST reject records whose `plain_len` exceeds
their resource limits. This protects against decompression bombs.

### 4.5 Trailer (24 bytes, always the last bytes of a file)

| Offset (from the trailer start) | Size | Field |
|---|---|---|
| 0 | 8 | `index_offset`: u64 offset of the latest index record's first byte |
| 8 | 4 | `index_len`: u32 total length of that record |
| 12 | 4 | `crc`: u32 CRC32C of the trailer's bytes 0–11 |
| 16 | 8 | magic `49 4D 47 45 4E 44 00 00` (`IMGEND\0\0`) |

### 4.6 Limits

Readers MUST support at least these limits, and MAY reject larger values.
Writers MUST NOT exceed them.

| Item | Minimum supported |
|---|---|
| `hlen` | 65,536 |
| `plen` and `plain_len` of non-segmented records | 1 GiB |
| Segment plaintext size | 256 KiB (exactly, except the last segment) |
| `slen` | 16,384 |
| Records per file | 2³² |

## 5. Records

### 5.1 Record types

| `rtype` | Name | Flags | Content (plaintext) |
|---|---|---|---|
| `0x01` | `META` | `CRITICAL`, `SAFE_TO_COPY` | protobuf `DocumentMeta` |
| `0x02` | `KEYR` | `CRITICAL`, `SAFE_TO_COPY`, `SIGNED` if an ACL exists | protobuf `Keyring` (§6.4) |
| `0x03` | `SNAP` | `CRITICAL` | CRDT snapshot (codec, §3.1) |
| `0x04` | `UPDT` | `CRITICAL` | CRDT update batch (codec, §3.1) |
| `0x05` | `BLOB` | `CRITICAL`, `SAFE_TO_COPY` | Media bytes, or one segment of them (§5.3) |
| `0x06` | `THMB` | `SAFE_TO_COPY` | WebP thumbnail (for OS previews and the app's file browser) |
| `0x07` | `VERS` | `SAFE_TO_COPY` | protobuf `NamedVersion` |
| `0x08` | `ACLC` | `CRITICAL`, `SAFE_TO_COPY`, `SIGNED` | protobuf `AclChange` (§8) |
| `0x09` | `DERV` | `SAFE_TO_COPY` | Derived, regenerable data. `derived_kind` says what kind. |
| `0x0A`–`0x3FFE` | reserved | | Future core types |
| `0x3FFF` | `INDX` | `CRITICAL` | protobuf `Index` (§9) |
| `0x4000`–`0x7FFF` | extension | add-on defined | `extension_id` in the header is REQUIRED |
| `0x8000`+ | reserved | | |

The flags in this table are REQUIRED. Writers MUST set them as listed.
In addition, `SIGNED` is REQUIRED wherever §7.3 requires a signature.

- `META`: exactly one current `META` per document. A newer one supersedes
  older ones (the last in the index wins).
- `THMB` is plaintext in unencrypted documents, so OS previews can read
  it. In encrypted documents it is encrypted like every other record, so
  only the app shows it and OS previews don't (ADR-0016). Writers SHOULD
  write one for unencrypted documents unless the user disabled
  thumbnails.
- In encrypted documents, every record except `KEYR` and `ACLC` MUST be
  `ENCRYPTED`. `KEYR` and `ACLC` MUST NOT be `ENCRYPTED`: they are
  needed to obtain keys and to check permissions, and they are protected
  by wrapping and signatures instead.
- In unencrypted documents, no record is `ENCRYPTED`, and `KEYR` MAY be
  absent.

### 5.2 Object identity

`RecordHeader.object_id` is 32 bytes.

- For `BLOB` and `DERV`: content-addressed:
  `object_id = BLAKE3-keyed(blob_id_key, blob plaintext)`. Here
  `blob_id_key` comes from `DocumentMeta`, and the blob plaintext is the
  whole blob before compression and segmentation. Using a keyed hash means
  a party without the key (the Hub) can't test whether a document contains
  a known file. Equal blobs in one document are stored once.
- For every other type: 32 bytes from the OS CSPRNG.
- `blob_id_key` is 32 random bytes generated once per document. It is
  **independent of the epoch keys**, so BlobIds stay stable across key
  rotations and when encryption is added or removed.

### 5.3 Segmented blobs

Blobs larger than 1 MiB MUST be split into segments of exactly 256 KiB of
plaintext. The last segment holds the remainder.

- Every segment is its own `BLOB` record with `SEGMENTED` set. Its
  `object_id` is the blob's id, and `RecordHeader.segment` carries `index`,
  `count`, `total_len` and (if encrypted) `nonce_prefix`.
- Each segment is compressed independently, if `ZSTD` is set.
- Encryption uses the STREAM construction (§6.3.2).
- Readers MUST verify that all `count` segments are present, that
  `total_len` matches, and that segment indices are unique.

### 5.4 Metadata records

- `DocumentMeta.required_features` lists logical features that a reader MUST
  understand to *edit* safely. A reader missing one MUST open the document
  read-only (§10.3).
- `NamedVersion.frontier` is codec-specific (for `loro-1`: Loro's encoded
  `Frontiers`).

## 6. Encryption

### 6.1 Suites

`RecordHeader.suite` names the algorithms of an encrypted record.

| Suite | AEAD | KDF, hash | Key wrapping (HPKE) | Signatures |
|---|---|---|---|---|
| `0` | none (unencrypted) | | | |
| `1` | XChaCha20-Poly1305 (24-byte nonce, 16-byte tag) | BLAKE3 | HPKE (RFC 9180) base mode, KEM `0x647A` X-Wing, KDF `0x0001` HKDF-SHA256, AEAD `0x0003` ChaCha20-Poly1305 | Ed25519 / Ed25519 + ML-DSA-65 |

Readers MUST reject records with unknown suites (they treat them as
unreadable, §10.2). Suite 2 is reserved for a future fully post-quantum
signature profile.

### 6.2 Key hierarchy (suite 1)

- Each **epoch** `e` (1, 2, …) has a random 32-byte **epoch key** `DEK_e`.
  A new epoch starts when a member is removed (§8). Records carry
  the epoch they were encrypted under.
- Subkeys are derived with BLAKE3's `derive_key` mode:

| Subkey | Context string | Used for |
|---|---|---|
| `K_content` | `imaginarium.imag v1 content key` | `META`, `SNAP`, `UPDT`, `VERS`, `INDX`, extension records |
| `K_blob` | `imaginarium.imag v1 blob key` | `BLOB`, `DERV`, `THMB` |
| `K_presence` | `imaginarium.imag v1 presence key` | Ephemeral live messages (sync protocol) |

`K_x = BLAKE3-derive_key(context, DEK_e)`, 32 bytes.

### 6.3 Record encryption (suite 1)

#### 6.3.1 Non-segmented records

- `nonce`: 24 bytes from the OS CSPRNG, unique per record.
- `key`: `K_content` or `K_blob` of the record's epoch (§6.2).
- `aad = LABEL("imaginarium.imag v1 record") ‖ doc_id[16] ‖ varint(rtype) ‖ rflags[1] ‖ varint(hlen) ‖ header[hlen]`

  `rflags` has its final value here: `ENCRYPTED`, and `SIGNED` if the
  record will be signed.
- `payload = nonce ‖ XChaCha20-Poly1305-Encrypt(key, nonce, plaintext, aad)`.
  The ciphertext includes the 16-byte tag.

This binds every record to its document, type, flags and complete header
(object id, stream, epoch, suite, `plain_len`, codec, segment).

The AAD and signatures (§7.2) use the **header bytes exactly as stored**.
Implementations MUST keep `header` (and `sig`) verbatim. They MUST NOT
re-encode a decoded `RecordHeader`, because a reader that doesn't know a
newer field would drop it, and the AAD and signature would no longer
match. Copying a record (compaction, sync) MUST copy its bytes unchanged. Records
therefore can't be moved between documents, relabelled, or have their
header altered without decryption failing.

#### 6.3.2 Segments (STREAM)

The construction is the RustCrypto `aead::stream::StreamBE32` construction
over XChaCha20-Poly1305:

- `nonce_prefix`: 19 random bytes per blob (stored in every segment's header).
- Nonce for segment `i` of `n`: `nonce_prefix[19] ‖ u32_be(i) ‖ last[1]`,
  where `last = 0x01` if `i = n − 1`, else `0x00`.
- AAD as in §6.3.1 (it covers `segment.index` and `segment.count`).
- `payload` = ciphertext with tag (no nonce prefix in the payload).

Truncation, reordering and swapping of segments therefore fail
authentication.

### 6.4 Keyring (`KEYR`)

A `Keyring` contains, for each epoch, `DEK_e` wrapped for every recipient.
The `KEYR` record itself is not encrypted. In documents with an ACL chain,
it MUST be signed (§7) by a member whose role allows changing membership
(§8.3), and readers MUST reject keyrings that fail this check (§8.4)
instead of using their keys. Otherwise anyone who can append to the file,
such as a Hub, could wrap a key it knows for a new epoch.

All wraps of an epoch MUST unwrap to the same `DEK_e`.

#### 6.4.1 Password (`WRAP_KIND_PASSWORD`)

- `KEK = Argon2id(password UTF-8 NFC, salt, memory_kib, iterations,
  parallelism, output 32 bytes)` (RFC 9106, version 0x13).
- `ciphertext = XChaCha20-Poly1305-Encrypt(KEK, nonce, DEK_e, aad)` with
  `aad = LABEL("imaginarium.imag v1 wrap") ‖ doc_id ‖ u32(epoch) ‖ u8(kind)`.
- Writers SHOULD use `memory_kib ≥ 65536`, `iterations ≥ 3` and
  `parallelism = 1`. Readers MUST reject `memory_kib > 1048576`,
  `iterations > 16` or `parallelism > 16`, which would otherwise make a
  malicious file a denial-of-service vector.

#### 6.4.2 Recipient keys (`WRAP_KIND_HPKE`)

- HPKE base mode, suite `(0x647A, 0x0001, 0x0003)`, to the recipient's
  X-Wing public key (user identity or device, §8).
- `info = LABEL("imaginarium.imag v1 keyring") ‖ doc_id ‖ u32(epoch)`,
  `aad = recipient_id`.
- `hpke_enc` = encapsulated key, `ciphertext` = sealed `DEK_e`.

#### 6.4.3 Folder and link keys (`WRAP_KIND_FOLDER_KEY`, `WRAP_KIND_LINK_KEY`)

These work like §6.4.1, with `KEK` = the 32-byte folder or link key instead
of an Argon2id output. Link keys MUST only be transported in URL fragments,
which browsers don't send to servers.

## 7. Signatures

### 7.1 Keys and key ids

- Device keys: Ed25519. Identity (authority) keys: Ed25519 + ML-DSA-65.
- `key_id = first 16 bytes of BLAKE3(public key bytes)`. For identities,
  the public key bytes are `ed25519_public_key ‖ mldsa65_public_key`.

### 7.2 Signed message

```
msg = LABEL("imaginarium.imag v1 record signature") ‖ doc_id[16]
      ‖ record bytes from rtype through payload   (excluding slen, sig, crc)
```

- `SIGNATURE_ALGORITHM_ED25519`: `signature = Ed25519-Sign(sk, msg)`
  (RFC 8032, pure).
- `SIGNATURE_ALGORITHM_ED25519_MLDSA65`:
  `signature = Ed25519-Sign(sk_ed, msg) ‖ ML-DSA-65-Sign(sk_mldsa, msg, ctx = "")`
  (64 + 3309 bytes). Verifiers MUST check **both** and reject if either fails.
  Signers SHOULD use the hedged (randomized) variant of FIPS 204, which
  resists side-channel and fault attacks. Verification is the same for
  hedged and deterministic signatures.
- The ML-DSA-65 key is generated from a 32-byte seed ξ (FIPS 204
  `ML-DSA.KeyGen_internal`), and the seed is what key bundles store.

### 7.3 When signatures are required

In a document with an ACL chain (any `ACLC` record):

| Record | Required signature |
|---|---|
| `ACLC` | Hybrid, by an identity permitted to make that change (§8.3) |
| `KEYR` | Hybrid, by an identity permitted to change membership |
| `UPDT` (main), `VERS` | Ed25519, by a device of a member with role ≥ editor |
| `UPDT` (discussion) | Ed25519, by a device of a member with role ≥ commenter |
| `SNAP` | Ed25519, by a device of a member with role ≥ editor |

Readers MUST reject unsigned or invalidly signed records of these types,
and records signed by keys that the ACL chain (§8) doesn't authorize
*at the time of the change*. Documents without an ACL chain (local files)
MAY contain signed records. Readers SHOULD verify them but MUST NOT
require them.

## 8. Access-control chain (`ACLC`)

### 8.1 Model

Members are **identities** (hybrid keys, §7.1) with a **role**, and each
member has **devices** (Ed25519 for signing frequent records, X-Wing for
receiving keys). The current roles follow from replaying the chain.

### 8.2 Chain integrity

- The first `ACLC` (`index = 0`) MUST be a `Genesis`, with
  `prev_hash` = 32 zero bytes. It is signed by the owner identity.
- Each later change has `index = previous index + 1`, and
  `prev_hash = BLAKE3(complete previous ACLC record bytes, rtype through crc)`.
- The chain MUST be linear. A reader that finds two changes with the same
  `index` (a fork) MUST stop accepting changes from that source and report
  tampering. Hubs MUST reject changes whose `prev_hash` isn't the current
  head (sync protocol).

### 8.3 Permissions

| Change | Who may sign |
|---|---|
| `Genesis` | the new owner (self) |
| `AddMember`, `RemoveMember`, `SetRole` | `owner`, or `admin` for members whose old and new role are both below `admin` |
| `AddDevice`, `RemoveDevice` | the member themselves, or anyone who may remove that member |
| `RotateEpoch` | `owner`, `admin` |

A `RemoveMember` or `RemoveDevice` MUST be followed by a `RotateEpoch`
before any new content is written. (Every device has an X-Wing key and may
have received keys, so the chain treats every removed device as having
held them.) Writers
MUST NOT encrypt new records under an epoch older than the latest
`RotateEpoch`. There MUST be exactly one owner. Transfer of ownership
(`SetRole` to `owner`) MUST be signed by the current owner, who then
becomes `admin`. The owner's role changes only through such a transfer,
and the owner can't be removed. `AddMember` with role `owner` is invalid.
The `Genesis` epoch is 1, and each `RotateEpoch` names the previous epoch
plus one. A device belongs to exactly one member.

### 8.4 Authorizing records

Records don't say at which point of the chain they were written, and a
file has no trusted clock. Readers therefore authorize them per **epoch**:

- For each key, a reader tracks the highest role its member held at any
  point of the chain while each epoch was current (a device inherits its
  member's role while it is registered).
- A `UPDT`, `SNAP` or `VERS` (§7.3) is valid if its signer is a device whose
  role *in the record's epoch* is high enough for the record type and
  stream. Its `epoch` MUST be one the chain has reached (not 0).
- A `KEYR` is valid if its signer is an identity that was `owner` or
  `admin` in the newest epoch the keyring contains.

Edits made before a removal therefore stay valid after it. The limit: a
removed member who kept the old epoch key can still write records under
that old epoch that look as if they were made before the removal. Hubs
close this gap for shared documents by checking every pushed record against
the *current* ACL head (sync protocol §5.1). After the `RotateEpoch`, a
removed member can't write under the new epoch at all.

## 9. Index, opening, appending, compaction, recovery

### 9.1 Index (`INDX`)

The index lists every live record of the file with its offset and length,
the per-stream heads (latest snapshot plus subsequent updates), and the
offset of the previous index. In encrypted documents, the index MUST be
encrypted (`K_content`) and is not readable without the key.

Keys come from `KEYR` records, and the index is encrypted. To break that
cycle, the `RecordHeader` of an encrypted document's `INDX` MUST list all
`KEYR` and `ACLC` records of the file in `key_records` (offset and length).
These records are plaintext anyway. The list is authenticated as part of
the AAD (§6.3.1). Readers MUST verify that each listed record has type
`KEYR` or `ACLC`, and, after decrypting the index, that the list matches
the index entries. Unencrypted documents MUST leave `key_records` empty.

### 9.2 Opening

1. Read the 24-byte trailer and verify its CRC and magic. Otherwise go to
   §9.5.
2. Read the index record at `index_offset` and verify `index_len` and the
   CRC. If the document is encrypted, first load the `KEYR` and `ACLC`
   records listed in the index header's `key_records`, obtain the epoch
   key, then decrypt the index.
3. Load `META`, then, for
   `STREAM_MAIN`, the head snapshot followed by its updates in order. At
   this point the document can be shown.
4. Load blobs on demand, by offset. Load the discussion and extension
   streams in the background.

Readers MUST verify the CRC of every record they read.

### 9.3 Appending (incremental save)

A writer appends: new records, then a new `INDX` (with
`previous_index_offset` set to the old one), then a new trailer. It then
syncs the file to stable storage (e.g. `fsync`). The old trailer stays in
the file and is ignored. A crash at any point leaves the previous trailer
valid (§9.5).

### 9.4 Compaction

A writer MAY compact when more than half the file is superseded records,
and SHOULD compact on "Save as" and export:

1. Write a new file next to the original (temporary name) with the header,
   a current snapshot per stream, all live records, and records of
   unknown type that have `SAFE_TO_COPY` set (copied byte-for-byte), then
   an index and trailer.
2. Sync it to stable storage, then atomically rename it over the original.

Unknown records without `SAFE_TO_COPY` MUST be dropped during compaction,
and the user SHOULD be warned when that happens.

### 9.5 Recovery

If the trailer is invalid:

1. Search backwards from the end for the trailer magic `IMGEND\0\0`. For
   each candidate whose CRC and index record verify, use that index (the
   last good save). Readers SHOULD then also adopt every complete,
   CRC-valid record after that commit. For example, an autosave that
   appended records but crashed before committing holds the user's newest
   edits, and CRDT records are safe to apply even if they turn out to be
   duplicates.
2. If none is found, scan forward from offset 36. Parse records and verify
   each CRC, stopping at the first invalid record. Then rebuild the heads
   from the records found. For CRDT streams, applying all found `SNAP` and
   `UPDT` records is safe, because CRDT merges are idempotent.

Recovered files SHOULD be compacted before further appends.

Recovery is for torn writes and corruption, which the CRCs detect. If an
index record has a valid CRC but fails to decrypt, the key is missing or
wrong, or the ciphertext was tampered with. Readers MUST report that and
MUST NOT fall back to recovery, which would present a wrong password as a
damaged file, or let a removed member read around the index of an epoch
they no longer have.

## 10. Extensibility and compatibility

### 10.1 Rules for future versions

- New record types, fields, flags (ignorable header bits 16–31) and
  suites are added in a new **minor** version. Readers ignore what they
  don't understand, as described below.
- Field numbers, record type ids, suite ids and codec ids MUST NOT be
  reused for something else.
- Incompatible changes require a new **major** version.

### 10.2 Unknown records

| Unknown record (type, codec, suite or reserved flag) | Reader behavior |
|---|---|
| `CRITICAL` | Open **read-only**, and explain why |
| not `CRITICAL` | Ignore |
| On compaction | Copy if `SAFE_TO_COPY`, otherwise drop |

### 10.3 Required features

If `DocumentMeta.required_features` names a feature the reader doesn't
implement, it MUST open the document read-only and name the missing
features.

## 11. Conformance levels

| Level | Must implement |
|---|---|
| **Reader (plain)** | §4, §5, §9.2, §9.5, §10. Unencrypted documents, read-only. |
| **Reader (encrypted)** | Reader (plain) + §6 + §7 verification + §8 replay |
| **Writer** | Reader (encrypted) + §9.3, §9.4 and every writer MUST of this spec |
| **Sync participant** | Writer + the sync protocol (`../design/sync-protocol.md`, M1.2) |

## 12. Security considerations

- **What isn't encrypted.**
  - The record framing (`rtype`, `rflags`, `RecordHeader`) is plaintext, so
    anyone holding the file (or the Hub) sees record **types, sizes,
    counts, streams, epochs, codecs and signer key ids**. The Hub needs these
    to enforce permissions.
  - Content, titles, file names (which live in the CRDT), comments,
    thumbnails and the index are encrypted.
- **Compression before encryption** reveals approximate content size. That
  is accepted for documents. Live sync frames are padded (sync protocol).
- **Nonces** are 192-bit random values, so collisions are negligible even
  across many uncoordinated devices.
- **Malicious files.** Readers enforce the limits in §4.6 and §6.4.1, check
  `plain_len` before and during decompression, reject non-minimal varints,
  and never trust offsets that point outside the file or overlap.
- **Hybrid post-quantum.** Key wrapping is at least as strong as X25519,
  and authority signatures are at least as strong as Ed25519, even if the
  post-quantum part is broken.
- **Removed members** keep any content they already decrypted. New content
  after `RotateEpoch` is out of their reach. Re-encrypting history is
  optional (compaction under the new epoch).
- **Randomness:** keys, nonces, `doc_id`, `object_id`s and salts MUST come
  from the OS CSPRNG.

## 13. Test vectors

Published in `docs/format/test-vectors/` (M1.3/M1.4). Each vector comes
with its inputs (keys, nonces, salts fixed for reproducibility) and the
expected bytes:

1. A minimal unencrypted file (header, `META`, empty `SNAP`, index, trailer).
2. The same file after one incremental save, so it has two trailers.
3. Recovery cases: a torn last append, a corrupted trailer, a file with no
   valid index.
4. Unknown records: critical, non-critical, and `SAFE_TO_COPY` on
   compaction.
5. An encrypted record (suite 1) with fixed `DEK`, nonce and header.
6. A segmented encrypted blob (five segments: the smallest segmented blob
   is just over 1 MiB, §5.3), plus tampering cases (swapped, truncated,
   last flag missing).
7. A password keyring (small Argon2 parameters, allowed only for vectors).
8. An HPKE keyring wrap with a deterministic seed.
9. Ed25519 and hybrid signatures over a record.
10. An ACL chain: genesis, add member, remove member, rotate epoch. Also a
    fork case and an unauthorized-signer case.

## 14. Changes from the design document

These refine `../design/file-format.md` while writing the normative text:

1. **`doc_id` instead of `file_id`** in the header and the AAD. Records must
   be portable between files and the Hub. A per-file id would make copies
   undecryptable.
2. **BlobId key is per document and independent of epochs**
   (`DocumentMeta.blob_id_key`), instead of being derived from the epoch
   key. Otherwise key rotation, or adding or removing encryption, would
   change every BlobId referenced by the CRDT.
3. **Signatures are a framing section** (`SIGNED` flag, `slen`, `sig`) after
   the payload, not part of the header, because a signature can't cover
   the header that contains it.
4. **`plain_len`** in `RecordHeader`, for decompression-bomb protection and
   allocation.
5. **`codec` per record**, not only per document, so a codec migration can
   happen record by record.
6. **Overhead**: a typical `RecordHeader` is about 50 bytes (the 32-byte object
   id dominates), so an encrypted record has about 100 bytes of overhead
   (header, nonce 24, tag 16, CRC 4, varints). The design estimated ~54.
   Per edit batch (~50 ms) this is negligible.
7. **ACL chain** is specified concretely: linear, index plus `prev_hash`,
   fork means tampering, and a permission table.
8. *(Found while implementing M1.3)* **Verbatim header bytes** (§6.3.1):
   re-encoding a decoded header would silently drop unknown fields of
   newer versions, and break the AAD and signatures.
9. *(Found while implementing M1.3)* **`key_records` in the `INDX`
   header** of encrypted documents. Without it, a reader would need the
   key to decrypt the index that tells it where the keys are, and would
   have to scan the whole file instead.
10. *(Found while implementing M1.3)* **Recovery adopts complete records
    after the last good commit** (§9.5). Otherwise the most common crash
    (autosave interrupted before its commit) would lose the newest edits.
11. *(Found while implementing M1.4)* **Records are authorized per epoch**
    (§8.4), because records carry no position in the ACL chain. The design
    said "at the time of the change", which a reader can't determine.
12. *(Found while implementing M1.4)* **Keyrings are checked against the
    ACL chain** before their keys are used (§6.4). Without it, whoever can
    append to a file could wrap a key it knows for a new epoch.
13. *(Found while implementing M1.4)* **No recovery after a decryption
    failure** of a CRC-valid index (§9.5).
14. *(Found while implementing M1.4)* **Every `RemoveDevice` requires a
    rotation** (§8.3), because the chain can't know which devices received
    keys. Signers SHOULD use hedged ML-DSA (§7.2).
15. *(Found while implementing M1.4)* Test vector 6 has **five segments**,
    not three: §5.3 only segments blobs above 1 MiB, in 256 KiB pieces.
