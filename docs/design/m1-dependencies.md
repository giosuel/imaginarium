# M1 dependency review: crypto, protobuf, CRDT

- Status: **Accepted** (2026-10-12). It informs `docs/format/spec-v1.md` (M1.1).
- Scope: the third-party crates that implement the `.imag` format and its
  cryptography (`docs/design/file-format.md`, ADR-0009/0010/0011/0016).
  These are the hardest dependencies to replace later, because algorithm
  identifiers end up in stored files.

## 1. Policy for these dependencies

1. **Pure Rust where possible.** Constant-time code is reviewed upstream.
   We never implement primitives ourselves (MAINTAINERS.md, invariant 7).
2. **One ecosystem.** RustCrypto for every primitive it offers, so
   traits, `zeroize` support, MSRV and release cadence line up.
3. **Two-week rule:** a version is only adopted once it has been public for
   at least 14 days, transitive dependencies included. Exception: security
   fixes, documented in the commit. The exact version is pinned in
   `[workspace.dependencies]` when the crate is first added, not here.
4. **Monitoring:** `cargo deny` (RustSec advisories, licenses) runs in CI on
   every PR.
5. **Audit before the hosted service launches** (ADR-0011): an independent
   review of our crypto *use* plus the PQ crates, budgeted in M13.

## 2. Decisions

| Purpose (suite 1) | Crate | Reviewed version | Notes |
|---|---|---|---|
| AEAD XChaCha20-Poly1305 | `chacha20poly1305` | 0.11.0 | RustCrypto. ~94M downloads. The crate was reviewed by NCC Group in 2020 ([public report](https://www.nccgroup.com/research/public-report-rustcrypto-aesgcm-and-chacha20pluspoly1305-implementation-review/)), on an earlier version. |
| Segmented-blob AEAD (STREAM) | `aead` (`stream` module) | with `chacha20poly1305` | The STREAM construction (nonce prefix, counter, last-block flag) is built in. The exact layout is specified in spec-v1. |
| Hash, keyed hash, KDF | `blake3` | 1.8.7 | The official implementation (BLAKE3 team), with SIMD. Keyed mode for BlobIds, `derive_key` with context strings for subkeys. |
| Signatures (frequent records) | `ed25519-dalek` | 3.0.0 | ~235M downloads. Quarkslab audited the dalek libraries in 2019 ([report](https://blog.quarkslab.com/security-audit-of-dalek-libraries.html)), with ed25519-dalek only lightly covered. Since 2.0 the API prevents the signing-oracle misuse of RUSTSEC-2022-0093. **3.0 is a new major (July 2026).** Review the changes when adopting it. |
| Signatures (authority records, hybrid) | `ml-dsa` (ML-DSA-65, FIPS 204) | 0.1.1 | RustCrypto. Two 2026 CVEs (a timing side channel, CVE-2026-22705, and accepted malformed hints, CVE-2026-24850), **both fixed before 0.1.0**. No independent audit found. **Pre-1.0.** |
| Key wrapping (hybrid PQ) | `hpke` with features `mlkem` + `chacha` | 0.14.1 | RFC 9180 HPKE. **Implements X-Wing as KEM `0x647A`** (IANA HPKE registry), with KATs. It pulls in `x-wing` and `ml-kem` (RustCrypto). Suite: KEM `0x647A` X-Wing, KDF `0x0001` HKDF-SHA256, AEAD `0x0003` ChaCha20-Poly1305. |
| ↳ X-Wing KEM | `x-wing` | 0.1.0 (0.1.1 when ≥ 14 days) | 0.1.1 (2026-10-01) only adds a fallible "reject non-contributory" decapsulation API. Not a security fix, so 0.1.0 is used until 0.1.1 passes the two-week rule. |
| ↳ ML-KEM-768 (FIPS 203) | `ml-kem` | 0.3.2 | RustCrypto. No CVEs found. No independent audit found. Pre-1.0. |
| Password KDF | `argon2` | 0.6.0 | RustCrypto. Argon2id parameters are stored in the keyring record. |
| Checksum CRC32C | `crc32c` | 0.6.8 | Hardware-accelerated (SSE4.2 / ARMv8 CRC). Last release 2024, but small, stable and widely used (~89M). |
| Compression | `zstd` | 0.14.2 | Bindings to the reference C libzstd (`zstd-sys`, compiled with `cc`). Its clean-build cost (~100 s CPU) is known (ADR-0017). Pure-Rust `ruzstd` only decodes well, so it's not suitable for writing. |
| Secret hygiene | `zeroize`, `subtle` | 1.9.x, 2.6.1 | Zeroize key material on drop. Constant-time comparisons. |
| OS randomness | `getrandom` (through `rand_core`) | 0.4.3 | The CSPRNG for keys and nonces. Never a seeded RNG outside tests. |
| Protobuf runtime | `prost` | 0.14.4 | The most widely used Rust protobuf crate (~632M). |
| Protobuf codegen | `prost-build` + `protox` | 0.14.4 + **0.9.1** | `protox` compiles `.proto` in pure Rust, so **`protoc` isn't needed** on any OS or in CI. 0.10.0 is only 5 days old, so we use 0.9.1, which targets prost 0.14. The `.proto` files in `docs/format/proto/` are the normative schema. `build.rs` generates the Rust code from exactly those files. |
| CRDT | `loro` | 1.16.2 | See §3. Confirmed by the M1.5 benchmark. |

All licenses are on the `deny.toml` allowlist (MIT, Apache-2.0, BSD-3-Clause,
CC0-1.0).

## 3. CRDT feature survey (benchmark follows in M1.5)

| Need (format design §3) | Loro 1.16 | Yrs 0.28 | Automerge 0.12 |
|---|---|---|---|
| **Movable tree** (groups, pages, z-order; concurrent moves without cycles) | ✅ `LoroTree` with move | ❌ (move = delete + reinsert, which loses identity and concurrent edits) | ❌ |
| Rich text with marks (Peritext semantics) | ✅ `mark` | ✅ formatting attributes | ✅ marks |
| **History trimming** (shallow snapshots, 90-day default from ADR-0016) | ✅ `ExportMode::ShallowSnapshot` | partial (GC of deleted content, no history cutoff) | ❌ (history is always complete) |
| Collaboration-aware undo | ✅ `UndoManager` | ✅ `UndoManager` | ⚠️ unclear from the API survey; verify in M1.5 |
| Compact encoding, stable since 1.0 | ✅ | ✅ | ✅ |
| License | MIT | MIT | MIT |

**Only Loro covers the movable tree and history trimming**, two hard
requirements of the format design. Yrs remains the fallback, but would need
a tree-move layer of our own. The M1.5 benchmark must confirm performance
and memory on our fixtures before ADR-0009 is final.

## 4. Standardization status of the PQ parts

- **X-Wing** is still an Internet-Draft: `draft-connolly-cfrg-xwing-kem-11`,
  September 2026, Informational, independent submission
  ([latest](https://www.ietf.org/archive/id/draft-connolly-cfrg-xwing-kem-11.html),
  [datatracker](https://datatracker.ietf.org/doc/draft-connolly-cfrg-xwing-kem/10/)).
  The core construction (X25519 + ML-KEM-768 with a SHA3 combiner) has been
  stable across revisions. Its HPKE KEM id **`0x647A`** is in the
  [IANA HPKE registry](https://www.iana.org/assignments/hpke/hpke.xhtml).
- **PQ HPKE** (`draft-ietf-hpke-pq-05`, July 2026,
  [datatracker](https://datatracker.ietf.org/doc/html/draft-ietf-hpke-pq-05))
  defines ML-KEM and hybrid KEMs for HPKE. It is not needed for X-Wing,
  which already has its own codepoint.
- **ML-KEM (FIPS 203)** and **ML-DSA (FIPS 204)** are final NIST standards.

## 5. Risks and mitigations

| Risk | Mitigation |
|---|---|
| **PQ crates are pre-1.0 and not independently audited. ML-DSA had CVEs in 2026.** | **Hybrid by design:** wrapping uses X-Wing, so it is at least as strong as X25519. Authority signatures require *both* Ed25519 and ML-DSA to verify, so an ML-DSA bug can't enable forgery. An ML-DSA bug that rejects valid signatures is only an availability problem. Pin exact versions, follow RustSec, audit before hosting. |
| X-Wing changes before an RFC | The draft is stable in substance, and we use the IANA codepoint. Our own test vectors pin the behavior in CI. A change becomes a new suite id (format §6.1); old files stay readable. |
| `ed25519-dalek` 3.0 is a fresh major | Review the changelog when adopting it. Use only the non-hazmat API, plus test vectors. |
| `zstd` is C code | Long-standing and fuzzed upstream (reference implementation). It only ever decompresses *authenticated* data, because decryption and the AEAD check come first. Unencrypted files are the exception, so the `imaginarium-format` fuzz targets cover zstd input as well. |
| Loro is a single-vendor project | The codec id `loro-1` in the format allows a migration (format §3.1). Yrs is the known fallback. |
| Protobuf codegen drift | `.proto` is normative, and the code is generated from it on every build (`protox`). There are no hand-written structs to drift. |

## 6. Not chosen

- **libcrux** (Cryspen; formally verified in parts with hax/F*). All crates
  are `0.0.x`, and Cryspen states that not all code is verified yet. In 2026
  independent researchers published proof-soundness gaps and bugs in
  ML-DSA, and those were since fixed
  ([Symbolic Software](https://symbolic.software/blog/2026-02-17-cryspen-mldsa/),
  [ePrint 2026/192](https://eprint.iacr.org/2026/192)). Worth re-evaluating
  later. The hybrid design already contains PQ implementation risk.
- **hpke-rs** (MPL-2.0): fine, but tied to the libcrux backend for PQ.
  `hpke` gives us X-Wing with the RustCrypto stack.
- **ring / aws-lc-rs**: C/assembly. Their ML-KEM may become interesting if
  FIPS-validated modules are ever required (enterprise hosting). It's a
  possible future suite, not suite 1.
- **Yrs / Automerge**: see §3.
- **rust-protobuf / hand-written codecs**: prost with `.proto` files is the
  most widely used combination and keeps the schema normative.

## 7. What the spec (M1.1) takes from this

- Suite 1 identifiers:
  - AEAD XChaCha20-Poly1305 (24-byte nonce, 16-byte tag);
  - BLAKE3 `derive_key` contexts of the form `"imaginarium.imag v1 <purpose>"`;
  - HPKE base mode `(0x647A, 0x0001, 0x0003)`;
  - Ed25519 (RFC 8032, pure);
  - ML-DSA-65 (FIPS 204, pure, empty context string);
  - Argon2id (RFC 9106) with stored parameters;
  - CRC32C (Castagnoli).
- Hybrid signature encoding: `ed25519_sig ‖ ml_dsa_sig`, fixed order. Both
  must verify.
- Protobuf schemas live in `docs/format/proto/` and are compiled with
  `protox` + `prost-build`.
