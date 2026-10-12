# Sync protocol: client ↔ Imaginarium Hub

- Status: **Draft** (2026-10-12, M1.2). It is implemented in M8 (Hub) and
  becomes normative then, as `docs/protocol/spec-v1.md` plus `.proto`.
- Builds on: [`.imag` spec v1](../format/spec-v1.md) (records, encryption,
  signatures, ACL chain), ADR-0011 (E2EE), ADR-0012 (Hub).
- Out of scope here: accounts, folders, sharing UI, billing (`hub.md`, M8).

## 1. Goals

1. **Zero-knowledge.** The Hub stores and relays the **same record bytes**
   as `.imag` files. It never holds a document key and never decrypts.
2. **Real-time.** Edits reach other open clients in under 100 ms plus
   network latency, and cursors and selections stream live.
3. **Offline-first.** Clients edit offline and sync on reconnect. Merging
   is the CRDT's job, so there is never a "conflict" dialog.
4. **Enforced permissions without plaintext.** The Hub rejects writes from
   members who may not write, using the plaintext record framing and
   signatures (spec §7, §8). Clients verify everything again.
5. **Efficient.** Binary framing, batching, resumable blob uploads,
   deduplication by object id, and catch-up from a sequence number.
6. **Robust against a malicious Hub** for confidentiality and integrity.
   Availability attacks (a Hub that drops or withholds data) can be
   detected in part but not prevented. That limitation is documented.

## 2. Transport

| Channel | Use |
|---|---|
| **HTTPS** (TLS 1.3, rustls), HTTP/2 | Login, catch-up downloads, blob uploads and downloads, snapshot uploads, administration |
| **WebSocket** over the same TLS connection origin | Live updates, presence, notifications |

- All payloads are binary. Control messages are protobuf
  (`imaginarium.sync.v1`). Records travel as raw spec-v1 record bytes,
  never re-encoded.
- One WebSocket per client session carries many documents (multiplexed by
  `doc_id`).
- Compression: none at the transport level. Records are already compressed
  and encrypted, and compressing on top would only cost CPU and risk
  CRIME-style leaks.

## 3. Sessions and authentication

1. **Login**: OPAQUE (RFC 9807) over HTTPS. The Hub never sees the password.
   The result is a **session token**: 32 random bytes, stored server-side
   only as a hash, with an expiry, and revocable.
2. The client downloads its **encrypted key bundle** (identity and device
   private keys, encrypted with a key derived from the OPAQUE export key).
   It decrypts the bundle locally.
3. **Device proof**: the WebSocket `Hello` carries the session token plus
   an Ed25519 signature by the device key over a Hub-issued nonce. The Hub
   then knows which device of which member is connected. That matters,
   because write permissions are per member, and devices sign records.
4. `Hello` also negotiates the protocol version
   (`min_version`/`max_version`). The Hub answers with the version it
   picked, or `UNSUPPORTED_VERSION`.

## 4. Per-document state on the Hub

For each `doc_id`, the Hub stores:

| State | Meaning |
|---|---|
| Records, by `object_id` | Every accepted record's bytes. `object_id` makes uploads idempotent. |
| **Server sequence** `seq` | A u64 per document, incremented by one for every accepted record. It gives a total order of acceptance, not a causal order, and the CRDT doesn't need one. |
| ACL head | Its own replay of the `ACLC` chain (spec §8): members, roles, devices, current epoch. |
| Latest `KEYR` | The keyrings, to hand to members. |
| Snapshot marks | Which `SNAP` covers which `seq` range (§7). |

The Hub parses only the plaintext parts: record framing, `RecordHeader`,
`ACLC`/`KEYR` payloads (spec §5.1: never encrypted) and signatures.

## 5. Writing (push)

```
client                                   hub
  │ Push{doc_id, records[]}                │
  │──────────────────────────────────────▶ │ validate each record (§5.1)
  │                                        │ persist (durably) → assign seq
  │ ◀──────────────────────────────────────│ PushResult{accepted[(object_id, seq)], rejected[(object_id, reason)]}
  │                                        │ fan out to subscribers: Records{doc_id, [(seq, bytes)]}
```

### 5.1 Validation by the Hub

For every record the Hub checks:

1. Framing: CRC, limits (spec §4.6), known `rtype` range. Unknown types
   are accepted only if they are not `CRITICAL` (forward compatibility).
2. **Encryption rules** (spec §5.1): in encrypted documents, every record
   except `KEYR` and `ACLC` must have `ENCRYPTED` set and a known suite.
   `THMB` never travels in plaintext.
3. **Signature and role** (spec §7.3): the signer key must belong to a
   device (or identity) that the ACL head authorizes for this record type
   and stream. For example, a commenter may only push `UPDT` with
   `stream = DISCUSSION`.
4. **Epoch:** new content must use the current epoch (spec §8.3). The one
   exception is uploading *historical* records with `seq`-less provenance
   during first upload or migration. Those must be signed by an admin and
   are marked as historical.
5. `ACLC`: `prev_hash` must equal the current head (spec §8.2). A
   concurrent change by another admin is rejected with `ACL_CONFLICT`,
   and the client re-bases (re-signs the change on the new head).
6. Idempotency: a record whose `object_id` is already stored with
   identical bytes is acknowledged with its existing `seq` and not stored
   twice. Different bytes under the same `object_id` → `CONFLICT`. That
   can't happen with honest clients, because ids are random or
   content-derived.

The Hub acknowledges only after the record is durably stored (`fsync`, or
the database's commit), so an acknowledged record is never lost.

### 5.2 Batching on the client

The client groups CRDT changes into **update frames of about 50 ms** while
typing or dragging. Each frame is one `UPDT` record (one AEAD operation,
one signature). The pace adapts: during continuous edits the client sends
at most 20 frames per second, and it flushes immediately on blur, save
or close.

## 6. Reading

### 6.1 Subscribe and live updates

`Subscribe{doc_id, since_seq}` → the Hub sends every record with
`seq > since_seq` (catch-up), then streams new records live as
`Records{doc_id, [(seq, bytes)]}`. Clients store `last_seq` per document
and resume from it after a reconnect.

Large catch-ups (`> 4 MiB` or `> 1000` records) are redirected to HTTPS:
`GET /v1/docs/{doc_id}/records?since=<seq>`. The response is a raw stream
of spec-v1 records (framing only, no header or trailer), so clients parse
it with the same `imaginarium-format` code as files.

### 6.2 Opening a document that isn't cached

1. `GET …/keyring` → the latest `KEYR` records. Unwrap `DEK_e` (spec §6.4).
2. `GET …/acl` → the `ACLC` chain. Replay and verify it locally (spec §8).
3. `GET …/head` → the latest snapshot record per stream, plus the
   `seq` it covers.
4. Subscribe from that `seq`.
5. Fetch blobs lazily: `GET …/blobs/{object_id}/segments/{i}`.

The client keeps a local, encrypted cache (ADR-0011) in `.imag` format. The
cache *is* a `.imag` file that also stores `last_seq` (in an extension
record `org.imaginarium.sync-state`).

## 7. Snapshots and history without plaintext

The Hub can't merge updates into snapshots. **Clients do it.**

1. Any editor's client MAY create a new `SNAP` when more than N updates
   (default 500) have accumulated since the last snapshot. It encodes the
   current state, as a full snapshot or as a Loro shallow snapshot that
   respects the history policy (ADR-0016).
2. It pushes the `SNAP` together with a plaintext protocol field
   `covers_through_seq = S`, meaning "this snapshot includes every update
   with `seq ≤ S`".
3. The Hub records the mark. New clients start from the newest snapshot
   and fetch only updates with `seq > S`.
4. **Garbage collection:** a document's history policy (a document setting,
   stored *inside* the encrypted CRDT) is mirrored to the Hub as a
   plaintext retention value. Admins set it, and it is signed. The Hub
   deletes `UPDT` records covered by a snapshot only after the retention
   period has passed. Named versions (`VERS`) pin their snapshot forever.
   A malicious editor therefore can't erase history, because deletion
   follows only the signed retention policy plus a grace period
   (default 30 days).

The Hub can't verify that a `SNAP` really contains what its
`covers_through_seq` claims. Mitigation: clients that load a snapshot also
fetch the covered updates while still inside the grace period, check their
CRDT version vector, and **report a mismatch** to the user and the
admins. A forged snapshot then becomes visible before history is
collected.

## 8. Blobs

- `PUT /v1/docs/{doc_id}/blobs/{object_id}/segments/{i}`, one record per
  request. Resumable: `HEAD` tells which segments exist.
- Deduplication within a document comes for free through keyed BlobIds
  (spec §5.2). There is no deduplication across documents, by design: it
  would leak content equality.
- The blob counts as complete when all `count` segments are stored. Only
  then may an `UPDT` referencing it be relied on. Clients upload blobs
  **before** the update that references them.

## 9. Presence (live cursors, selections, viewports)

- `Presence{doc_id, ciphertext}` over the WebSocket. **Not stored, only
  relayed.**
- Encrypted with `K_presence` (spec §6.2), using XChaCha20-Poly1305 with a
  random nonce. The plaintext is a small protobuf (cursor position, selection
  node ids, viewport, color, name hint).
- **Padded** to size buckets (64, 256 or 1024 bytes) before encryption, to
  limit what message sizes reveal.
- Rate limit: up to 30 messages per second per client per document.
  Clients coalesce presence to one message per frame.
- Presence isn't signed (it's ephemeral and low-value). The Hub tells
  receivers which device sent it, from the authenticated session.

## 10. Notifications

The Hub sends content-free events: "new comment in document `doc_id`",
"you were added to a document". Names and comment text are resolved on the
client after decryption. Push notifications to phones (later) carry only
ids.

## 11. Offline and reconnect

1. While offline, records are written to the local cache file as usual and
   also queued as unacknowledged (`sync-state` extension record).
2. On reconnect: `Hello`, then for each document push the queued records
   (idempotent), then `Subscribe{since_seq = last_seq}`.
3. The order of steps 2 and 3 doesn't matter for correctness. CRDT merging
   is commutative and idempotent. Pushing first just gets local changes to
   others sooner.
4. If an `ACLC` was rejected with `ACL_CONFLICT`, the client fetches the
   new head and re-applies its change if it's still allowed. For example,
   it doesn't retry adding a member whom another admin just removed.

## 12. Errors and limits

| Code | Meaning | Client action |
|---|---|---|
| `UNAUTHENTICATED` | Session invalid or expired | Log in again |
| `FORBIDDEN` | Role doesn't allow this record or stream | Show the reason. Mark local changes as "not synced". |
| `INVALID_RECORD` | Framing, limit or encryption-rule violation | Bug: report it and don't retry |
| `STALE_EPOCH` | Written under an old epoch after a rotation | Fetch the keyring, re-encrypt under the new epoch, re-push |
| `ACL_CONFLICT` | `prev_hash` isn't the head | Re-base (§11) |
| `QUOTA_EXCEEDED` | Storage or rate quota | Show it. Keep the data local. |
| `UNSUPPORTED_VERSION` | Protocol version mismatch | Ask the user to update |

Rate limits (per client, adjustable by the Hub admin): 100 pushed records per
second, 30 presence messages per second, and 10 concurrent blob
uploads.

## 13. Security analysis (summary)

| Threat | Protection |
|---|---|
| Hub reads content | E2EE. The Hub has no keys (spec §6). |
| Hub forges or alters edits | Signatures, verified by clients against the client-verified ACL chain (spec §7–8) |
| Hub adds a member to gain access | Impossible: membership changes need a hybrid signature by an owner or admin, and keys are wrapped only by clients |
| Hub swaps a user's public key when sharing | TOFU plus verification UI (ADR-0011); key transparency is planned |
| Hub replays old records | Idempotent CRDT merges, plus epoch rules for new content |
| Hub withholds records (availability) | **Detectable, not preventable.** Clients compare version vectors with peers when they meet (later: peer-to-peer on the LAN). Snapshot cross-checks (§7). |
| Removed member keeps reading | `RotateEpoch` plus new keys only for the remaining members (spec §8.3). Content they already had stays known to them. |
| Traffic analysis | Partially mitigated: presence padding, batching. Edit timing and sizes stay visible. |
| Malicious member floods the document | Rate limits, quotas, and admin removal |

## 14. Message overview (to be specified as `.proto` in M8)

`Hello`, `HelloOk`, `Subscribe`, `Unsubscribe`, `Push`, `PushResult`,
`Records`, `Presence`, `PresenceRelay`, `Notification`, `Error`,
`SnapshotMark` (`covers_through_seq`), `Ping`/`Pong` (keep-alive every 30 s).

## 15. Open questions

1. **Peer-to-peer on the LAN** (no Hub) for offline offices: later, with
   the same records and protocol over mDNS + QUIC.
2. **Key transparency** for public keys: needed before the hosted launch,
   with a design in M8.
3. **Server-side search** is impossible by design. Client-side search over
   the local cache covers it.
