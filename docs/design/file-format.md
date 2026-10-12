# The `.imag` file format: design

- Status: **Accepted design, defaults decided** (2026-10-12). The normative spec follows in M1.
- Scope: the on-disk *and* on-server representation of an Imaginarium document
- Normative spec: will be extracted into `docs/format/spec-v1.md` (RFC-style
  MUST/SHOULD, with test vectors) once this design is accepted.
- Reference implementation: crate `imaginarium-format` (no app dependencies,
  fuzzed, usable by third parties)

The file format is the hardest part of the system to change later. This
document explains *what* we store, *how* the bytes are laid out, and *why*,
so that later changes can be checked against the original reasoning.

---

## 1. Requirements

| #   | Requirement | Consequence |
|-----|-------------|-------------|
| R1  | **Small**: as compact as practical | Binary, zstd-compressed, columnar CRDT encoding, deduplicated media, optional history trimming |
| R2  | **Fast to open**: first content visible in under 500 ms even for big files | An index at the end of the file (random access). The document state loads before media. Media and raster tiles load lazily. |
| R3  | **Fast to save**: no full rewrite on every keystroke or autosave | Append-only incremental saves. Compaction happens only occasionally. |
| R4  | **Real-time multi-user editing** | The document *is* a CRDT. Edits are small update records that merge without conflicts. |
| R5  | **End-to-end encrypted**: the server can never read content | Encryption is part of the record layer. Local files and server storage use **the same encrypted records**. |
| R6  | **Encryption must not hurt performance** | Fast AEAD (XChaCha20-Poly1305) and BLAKE3, encryption per record, parallel and lazy decryption, never on the UI thread (§7) |
| R7  | **Open and documented** | A public spec with test vectors, a reference crate, and an `imag inspect` CLI. Only open, standardized building blocks. |
| R8  | **Extensible and forward-compatible** | Typed records with PNG-style critical/ancillary flags. Namespaced schema keys. Unknown data is preserved. Feature flags. Crypto-suite IDs. |
| R9  | **Robust**: a crash during save never corrupts the file | Append plus trailer. Each record is integrity-checked. Recovery by scanning backwards. Atomic rename on compaction. |
| R10 | **Same model for all modes** | One schema for boards, documents, slides, layouts, raster layers, comments and AI provenance |

Non-requirements: human readability (use `imag inspect`/`imag export-json`
for debugging) and editing with a text editor.

---

## 2. Overview: three layers

```
┌───────────────────────────────────────────────────────────────┐
│ 3. Logical layer   Document schema on a CRDT (Loro)            │
│                    nodes · text · styles · assets · comments   │
├───────────────────────────────────────────────────────────────┤
│ 2. Object layer    Immutable, typed objects                    │
│                    Snapshot · Update · Blob · Keyring · Meta … │
├───────────────────────────────────────────────────────────────┤
│ 1. Container layer Framing · compression · encryption · index  │
│                    = a "pack" of objects in one file           │
└───────────────────────────────────────────────────────────────┘
```

**The main idea:** a document is a set of immutable **objects**. A `.imag`
file is a *pack* of objects plus an index, similar to a git packfile. The
sync server stores **the same objects** (as the same encrypted bytes)
individually. Syncing, saving, exporting and backups all mean "move objects
around". There is no second format to keep consistent, and the server never
needs a key.

---

## 3. Logical layer: the document schema

### 3.1 Why a CRDT is the source of truth

Real-time multi-user editing (R4) with a server that cannot read the data
(R5) rules out server-side merging and operational transformation (OT).
Clients must merge on their own, so the document must be a **CRDT**.
Making the CRDT the persisted form from day one means there is no migration
when collaboration ships.

We use **[Loro](https://loro.dev)** (1.x, stable encoding since 1.0):

- **Movable tree** for the node hierarchy (groups, pages, slides, z-order).
  Concurrent moves don't create cycles.
- **Rich text** (Peritext-style marks on Fugue lists) for text flows.
  Concurrent formatting merges correctly, and text doesn't interleave.
- **Maps, lists and LWW registers** for properties.
- A **compact columnar encoding**, shallow snapshots (history trimming), a
  collaboration-aware undo manager, version vectors, and checkout (time
  travel for version history).
- Pure Rust and MIT licensed (compatible with AGPL).

The CRDT payload is tagged with a **codec id** (`loro-1`). If we ever replace
Loro, a new codec id is added and old files are migrated on open. The
container itself is unaffected.

### 3.2 Schema (top-level containers)

```
root (LoroDoc)
├── "meta"     Map   schema_version, kind hint (board|document|deck|layout),
│                    title, created, language, required_features[], ai_policy
├── "nodes"    Tree  movable tree of NodeIds; each tree node has a Map:
│                      "k"  kind  (frame|shape|connector|text|image|raster|group|table|…)
│                      "g"  geometry (x, y, w, h, rotation, transform)
│                      "s"  StyleId reference + local overrides
│                      "t"  Text container (if the node has text)
│                      "a"  asset references (BlobId)
│                      "x"  extension data, namespaced: {"org.example.addon": …}
│                      "gen" AI provenance / generation recipe (§3.5)
├── "styles"   Map   StyleId → style map (inheritance via "parent")
├── "assets"   Map   BlobId → {mime, size, codec, name, derived_from?}
├── "pages"    Map   page/slide/master setup (size, margins, masters, layouts)
└── "ext"      Map   document-level add-on data, namespaced by add-on id
```

Comments and discussions live in a **separate CRDT document** (`discussion`)
in the same container (§4.2). This allows:

- loading the document without its comments,
- "export without comments",
- a *commenter* role that can write discussions but not the document (§6.4).

Comments anchor to nodes or text ranges through Loro stable cursors.

### 3.3 Compact keys

Map keys inside nodes are short (`"k"`, `"g"`, …) because they are repeated
for every node. The spec documents the mapping. Loro's encoding
deduplicates repeated strings, so this mainly saves memory and CPU, not
much file size.

### 3.4 Schema evolution rules

1. **Additive only** within a major schema version. New keys or kinds are
   added, and existing ones are never repurposed.
2. **Unknown keys and kinds are preserved.** A CRDT keeps whatever it doesn't
   understand, and an older client renders an unknown node kind as a
   placeholder box using its geometry.
3. `meta.required_features` lists features a reader **must** understand to
   *edit* safely (for example `"linked-frames"`). A reader that lacks one
   opens the document **read-only** and says why.
4. Breaking changes bump `schema_version` and come with a migration that
   runs on open, written as a CRDT transaction so it syncs like any edit.

### 3.5 AI provenance

AI-generated or AI-edited content carries a `gen` map, for example
`{provider, model, prompt, negative, seed, params, inputs: [BlobId|NodeId],
mask: BlobId, created_by}`. This enables non-destructive regeneration and
variations, honest attribution, and an optional export to C2PA metadata.
Because this map is extensible, new AI tools don't need format changes.

### 3.6 Large binary content stays out of the CRDT

Images, fonts, raster tiles and video posters are **blobs** (§4.1),
referenced by `BlobId`. A raster layer is a CRDT map
`tile_coord → BlobId` (last writer wins per tile). Painting therefore
creates a few small CRDT entries plus tile blobs, and the CRDT state stays
small and fast.

---

## 4. Object layer

### 4.1 Object types

| Type id | Name   | Content | Encoding | Notes |
|---------|--------|---------|----------|-------|
| `0x01` | `META`  | File/document header: format version, doc id, created-with, feature flags | Protobuf | Small. Encrypted in encrypted files. |
| `0x02` | `KEYR`  | Keyring: wrapped content keys per epoch (§6) | Protobuf | **Never encrypted with the content key** (it holds the wrapped keys). Signed. |
| `0x03` | `SNAP`  | CRDT snapshot (`main` or `discussion` doc) | codec `loro-1` | Possibly shallow (history-trimmed) |
| `0x04` | `UPDT`  | CRDT update batch (one transaction group, ~50 ms window in real time) | codec `loro-1` | Signed by the author's device key (§6.3) |
| `0x05` | `BLOB`  | Media: image, font, raster tile, attachment | raw / segmented | Content-addressed (§4.3), deduplicated |
| `0x06` | `THMB`  | Preview thumbnail (WebP) | raw | *Ancillary.* Written by default in **unencrypted** files (OS previews, can be disabled in settings). In encrypted files it is always encrypted, so OS previews show a lock icon. |
| `0x07` | `VERS`  | Named version: label + CRDT frontier | Protobuf | For version history |
| `0x08` | `ACLC`  | Access-control change (signed membership/role chain) | Protobuf | §6.4 |
| `0x09` | `DERV`  | Derived data: image mip pyramids, layout caches, search index | raw | *Ancillary + droppable*: can always be regenerated |
| `0x0A`–`0x3FFF` | reserved for core | | | |
| `0x4000`–`0x7FFF` | **extension objects** | payload starts with an add-on id (reverse-DNS) | add-on defined | §8 |

Structured objects (`META`, `KEYR`, `VERS`, `ACLC`, the index) use
**Protocol Buffers**. Protobuf is compact, has an open schema language that
is easy to implement in any programming language, and supports field-number
based evolution (never reuse a field number; unknown fields are preserved).
The `.proto` files are part of the spec.

### 4.2 Object identity

- `ObjectId` = 32 bytes.
  - For `BLOB` and `DERV` it is **content-addressed**:
    `BLAKE3-keyed(doc_blob_key, plaintext)`. Using a keyed hash means the
    server cannot test whether a document contains a known file, which an
    unkeyed hash or convergent encryption would allow.
  - For all other types it is random (`UPDT`, `SNAP`, …).
- `stream`: `main` | `discussion` | `ext:<id>`. This says which CRDT document
  a `SNAP` or `UPDT` belongs to.

### 4.3 Blobs

- Blobs up to 1 MiB are stored as a single object. Larger blobs are
  **segmented** into 256 KiB segments. Each segment is compressed and
  encrypted separately, so we get random access (for example a region of a
  huge image), parallel decryption across cores, and resumable uploads.
- Each blob has a `codec`: `png`, `jpeg`, `webp`, `avif`, `qoi+zstd` (raster
  tiles), `font/otf`, … Data that is already compressed is **not** run
  through zstd again.
- Raster tiles are 256×256 px. Only painted tiles exist.

---

## 5. Container layer: byte layout

All integers are little-endian. `varint` means unsigned LEB128.

```
File     := Header  Record*  ( Index Trailer )+          (append-only)

Header   := magic[8]   = 89 49 4D 47 0D 0A 1A 0A          ("\x89IMG\r\n\x1a\n")
            major u16, minor u16
            flags u32                                     bit0 = encrypted file
            file_id [16]                                  random UUID
            reserved [4]                       → 36 bytes total

Record   := rtype varint                                  object type (§4.1)
            rflags u8                                     see below
            hlen varint, header[hlen]                     protobuf RecordHeader:
                                                            object_id, stream, epoch,
                                                            seq, suite, segment info
            plen varint, payload[plen]
            check [4]                                     CRC32C over all preceding record
                                                          bytes (catches torn writes even
                                                          in unencrypted files)

rflags   := bit0 CRITICAL     reader must understand rtype, else open read-only
            bit1 SAFE_TO_COPY may be copied unchanged when rewriting, even if unknown
            bit2 ZSTD         payload is zstd-compressed (before encryption)
            bit3 ENCRYPTED    payload is AEAD ciphertext (§6.2)
            bit4 SEGMENTED    payload is a segment of a larger blob
            bit5–7 reserved (must be 0)

Index    := an ordinary Record of rtype 0x3FFF ("INDX"), payload = protobuf:
              entries[]: {rtype, object_id, stream, offset, length, rflags, epoch}
              heads: latest SNAP per stream + UPDTs after it
              previous_index_offset
            (encrypted in encrypted files, so the record structure doesn't
             leak locally)

Trailer  := index_offset u64, index_len u32,
            crc32c u32 (of the trailer's first 12 bytes),
            magic[8] = "IMGEND\0\0"                → 24 bytes, always last
```

**Why the 8-byte magic follows PNG:** the high bit byte and `\r\n`/`\x1a`
detect transfer corruption (text-mode FTP, line-ending conversion) and stop
`type`/`cat` from dumping the rest of the file.

### 5.1 Operations

- **Open:** read the trailer, then the index, then the latest `SNAP`(s) and the
  following `UPDT`s of the `main` stream. The document is now interactive.
  Blobs are fetched by offset when they become visible. The `discussion`
  stream loads in the background.
- **Incremental save** (autosave, every edit batch): append new records, a new
  index and a new trailer, then `fsync`. The cost is proportional to the
  change, not to the document size.
- **Compaction** (when appended garbage is over 50 %, on "Save As", or on
  export): write a new snapshot and the live objects to a temporary file,
  `fsync`, then perform an atomic rename. Unknown `SAFE_TO_COPY` records are
  copied. Unknown non-safe ones are kept only if they are still referenced.
- **Recovery:** if the trailer is damaged, scan backwards for the previous
  `IMGEND` and fall back to that index. If no index survives, scan forward
  through the records (each one is self-delimiting and checksummed) and
  rebuild the index.
- **Streaming:** because records are self-delimiting, a file can be produced
  and consumed as a stream, which is used by sync and export.

### 5.2 Size budget (targets)

| Content | Target |
|---------|--------|
| Empty document | < 1 KiB |
| 100-shape diagram, no history | < 20 KiB |
| 300-page text document (no images), trimmed history | ≈ 0.5–1× the DOCX size |
| Overhead per record (encrypted) | ≈ 24 B nonce + 16 B tag + ~10 B header + 4 B CRC |

**History policy** is a document setting: *keep all*, *keep N days*, or
*none*. The default is **keep 90 days, and named versions forever**. It uses
Loro shallow snapshots. "Export clean copy" writes an
unencrypted, history-free, comment-free file for sending to others.

---

## 6. Encryption and integrity

### 6.1 Crypto suite (versioned)

Every encrypted record carries a `suite` id, so algorithms can be changed
later without a new format (crypto agility).

**Suite 1** (default from the first release: **hybrid post-quantum**):

| Purpose | Algorithm |
|---------|-----------|
| AEAD | **XChaCha20-Poly1305**. Its 192-bit random nonces are safe across many independent devices without any nonce coordination. It is fast in software and on ARM. 256-bit keys remain secure against quantum attacks. |
| Hash / KDF / MAC | **BLAKE3** (keyed and derive-key modes), HKDF-SHA256 where a standard requires it |
| Key wrapping to users/devices | **Hybrid KEM X-Wing** (X25519 + **ML-KEM-768**, FIPS 203) in HPKE-style sealing. An attacker must break *both* algorithms. This protects stored and recorded documents against "harvest now, decrypt later" attacks. |
| Signatures on frequent records (`UPDT`, presence) | **Ed25519** (batch verification). Forging a signature requires a quantum computer *at the time of the attack*, not later, so classical signatures are acceptable here and keep live frames small. |
| Signatures on long-lived authority records (identity keys, device certificates, `ACLC`, `KEYR`, `VERS`) | **Hybrid Ed25519 + ML-DSA-65** (FIPS 204). Both signatures must verify. These records are rare, so the ~3.3 KB ML-DSA signature size doesn't matter. |
| Password-based keys (local password files) | **Argon2id** (memory-hard; parameters stored in `KEYR`) |

Size cost of post-quantum: about 1.1 KB per recipient in the keyring and
about 5 KB per authority record, with no impact on live editing. Suite 2 is
reserved for ML-DSA on *all* records, should that become necessary.

All primitives come from audited, widely used crates (RustCrypto,
`ed25519-dalek`, `blake3`, `hpke`, `ml-kem`, `ml-dsa`). We never implement primitives ourselves.

### 6.2 Key hierarchy

```
Document epoch key  DEK_e (256-bit random, one per membership epoch)
  ├─ derive "imag/content"  → AEAD key for SNAP/UPDT/META/VERS/INDX
  ├─ derive "imag/blob"     → AEAD key for BLOB/DERV/THMB segments
  ├─ derive "imag/blob-id"  → keyed-hash key for content-addressed BlobIds
  └─ derive "imag/presence" → key for ephemeral real-time messages (cursors)

KEYR (keyring) holds DEK_e wrapped for each recipient:
  local password file : Argon2id(password, salt) → KEK → AEAD-wrap(DEK_e)
  local account file  : X-Wing-seal(each of the user's device keys, DEK_e)
  cloud document      : X-Wing-seal(recipient device/user key, DEK_e)
  folder share        : AEAD-wrap(folder key, DEK_e)  (folder key itself shared via HPKE)
  share link          : AEAD-wrap(link key, DEK_e); link key lives only in the
                        URL fragment (#…), which is never sent to the server
```

- **Per-record encryption:** `nonce = random 24 B`. The AAD (data that is
  authenticated but not encrypted) is
  `file_id ‖ rtype ‖ rflags ‖ object_id ‖ stream ‖ epoch ‖ seq`. This binds
  every record to its position and meaning, so records cannot be swapped,
  replayed into another document or relabelled.
- **Segmented blobs** use the STREAM construction: a per-blob nonce prefix,
  a segment counter and a last-segment flag. This prevents truncation and
  reordering while keeping random access.
- **Membership change** (someone is removed): a new epoch starts. New records
  use `DEK_{e+1}`, which is wrapped only for the remaining members. Old
  records stay readable under old epoch keys, which remain in the keyring
  for current members. Optionally, compaction can rewrite everything under
  the new key ("re-encrypt history").
- **Compression happens before encryption.** This leaks approximate sizes,
  which is acceptable for documents. Live real-time frames are padded to
  size buckets to reduce what traffic analysis can reveal.

### 6.3 Authenticity of edits

- Every `UPDT` (and `ACLC`, `KEYR`, `VERS`) is **signed with the author's
  device key**: `Ed25519(device_sk, record header ‖ ciphertext)`.
- Signing the **ciphertext** allows the server to check write permission
  *without being able to read*, so it rejects garbage from read-only members.
- Clients verify signatures *again* against the signed ACL chain (§6.4).
  A malicious server therefore cannot inject edits.
- Signatures are made per batch (~50 ms window), not per keystroke, which
  costs about 20 signatures per second per active user.

### 6.4 Roles and the ACL chain

Roles are `owner`, `admin`, `editor`, `commenter` and `viewer`.

- **Read** access is cryptographic: you can read if you hold the epoch key.
- **Write** access is enforced through signatures:
  - editors may sign `main` and `discussion` updates,
  - commenters may sign `discussion` updates only,
  - viewers may sign nothing.
- `ACLC` records form a hash-linked, signed chain of membership changes.
  Each change must be signed by someone whose role at that point in the
  chain allows it. Clients replay the chain to work out the current roles.
  The server enforces the same chain for transport.

### 6.5 What the server can see (documented honestly)

| The server can see | The server can never see |
|--------------------|--------------------------|
| Account identifiers, which users share which document ids, roles (needed for enforcement) | Document content, titles, file names, comments, thumbnails, AI prompts |
| Object sizes, counts, timestamps, edit frequency | Which known file a blob is (BlobIds are keyed hashes) |
| Public keys | Private keys, passwords (OPAQUE login, see PLAN §7) |

File and folder *names* are encrypted metadata, not plaintext.

---

## 7. Performance of encryption

Typical single-core throughput on a modern laptop (order of magnitude):

| Operation | Throughput / cost | What it means for us |
|-----------|-------------------|----------------------|
| XChaCha20-Poly1305 | ~1–2 GB/s per core (SIMD) | Decrypting a 50 MB document takes ~25–50 ms on one core, much less in parallel |
| BLAKE3 | several GB/s per core | Content addressing is effectively free |
| zstd level 3 compress / decompress | ~300–500 MB/s / >1 GB/s | Compression dominates over encryption, and less data means less to encrypt |
| Ed25519 sign / verify | ~15 µs / ~40 µs (batch verification cheaper) | ~20 signatures/s per user: negligible |
| HPKE unwrap | ~100 µs | Once per document open |
| Argon2id | intentionally ~250 ms | Only when unlocking a password file |

Rules:

1. Crypto **never runs on the UI thread**. It runs in the IO and worker pool.
2. Decryption is **lazy**: only the CRDT state plus visible blobs and tiles.
3. Segment decryption is **parallel** (rayon).
4. Real-time edits are **batched** into ~50 ms frames. Each frame costs one
   AEAD operation and one signature.
5. Decrypted plaintext is cached in memory and keyed by object id. Objects are
   immutable, so cache entries never go stale.

Benchmarks for all of the above are part of M1 (see PLAN), with
regression gates in CI.

---

## 8. Extensibility summary

| Kind of change | Mechanism | Old readers… |
|----------------|-----------|--------------|
| New property on a node | New CRDT key (additive) | ignore it and keep it |
| New node kind | New `k` value | show a placeholder and keep it |
| Feature that's unsafe to edit without understanding it | `meta.required_features` | open read-only |
| New object type, optional | New rtype, `CRITICAL=0`, `SAFE_TO_COPY=1` | skip and copy it |
| New object type, essential | New rtype, `CRITICAL=1` | open read-only with an explanation |
| Add-on data | `x`/`ext` namespaced keys, or `0x4000+` objects with an add-on id | keep it |
| New field in a structured object | New protobuf field number | ignore it |
| New crypto | New `suite` id | ask the user to update |
| Breaking container change | `major` version bump | refuse with a clear message |
| Replace the CRDT library | New codec id plus migration on open | can't read it (major bump) |

---

## 9. Tooling and specification deliverables

- `docs/format/spec-v1.md`: normative spec (byte layout, `.proto` files,
  schema keys, crypto procedures, conformance levels).
- `docs/format/test-vectors/`: sample files with known keys and expected
  decrypted output. An independent implementation can verify against these.
- `imaginarium-format`: the reference implementation, `#![forbid(unsafe_code)]`,
  continuously fuzzed (`cargo-fuzz`) for the parser.
- `imag` CLI: `inspect` (records, sizes, epochs), `verify` (CRCs, signatures,
  ACL chain), `export-json` (decrypted, readable dump for debugging),
  `compact`, `rekey`.
- OS integration: MIME type `application/vnd.imaginarium.imag` and
  thumbnail/QuickLook providers (they read only plaintext `THMB` records).

---

## 10. Alternatives considered

| Alternative | Why not |
|-------------|---------|
| **ZIP container (like ODF/OOXML)** | No good standard for per-entry encryption, rewriting the central directory forces full rewrites on save, and it doesn't fit an append-only real-time model. Its main benefit (unzip by hand) isn't a requirement. |
| **SQLite file** | Very robust, but tied to SQLite's page format, harder to specify as an *open format*, awkward to encrypt per record end-to-end, and it can't be streamed to the server as objects. |
| **Plain snapshots (no CRDT)** | Rules out E2EE real-time collaboration. We would have to migrate later, and changing the format later is exactly what we want to avoid. |
| **Own CRDT** | Years of work and subtle correctness issues. Loro is used behind a codec id, so it can be replaced. |
| **Yrs / Automerge** | Both are good. Loro was chosen for its movable tree (our scene graph), Peritext rich text, shallow snapshots and performance. Re-evaluated in M1 with benchmarks on our own workloads. |
| **AES-256-GCM** | Faster with AES-NI, but its 96-bit nonces are risky with many uncoordinated writers, and it is slower on devices without AES hardware. It can become a later suite if needed. |
| **MLS (RFC 9420) for group keys** | Excellent for messaging, but its forward secrecy works *against* documents, where new members must read history. We use epoch keys with HPKE wrapping instead. MLS may be used later for the ephemeral real-time channel. |
| **FlatBuffers / Cap'n Proto** | Zero-copy isn't needed for small control objects, and protobuf is simpler and more widely implemented. |

---

## 11. Decisions on defaults (2026-10-12, owner)

1. **Local files are unencrypted by default.** On demand they can be
   protected with a **password** (Argon2id) or with **account keys** (HPKE to
   the user's devices, no password prompt on the user's own devices). Cloud
   documents are always encrypted, and their local cache is encrypted with a
   device key from the OS keychain.
2. **Thumbnails:** a plaintext `THMB` record is written by default for
   unencrypted files and can be disabled in settings. Encrypted files never
   contain a plaintext thumbnail.
3. **History:** the default is keep 90 days plus named versions forever. It
   is configurable per document and per workspace.
4. **Post-quantum:** suite 1 is hybrid post-quantum from the first release
   (§6.1).
