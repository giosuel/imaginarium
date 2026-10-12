# 0011. End-to-end encryption security model

- Status: Accepted
- Date: 2026-10-12

## Decision
- **The server cannot read anything.** Content, titles, file names, comments,
  thumbnails and presence (cursors) are encrypted on the client. Details are
  in `docs/design/file-format.md` §6.
- Suite 1 is **hybrid post-quantum from the first release**:
  - XChaCha20-Poly1305 and BLAKE3,
  - X-Wing (X25519 + ML-KEM-768) for key wrapping,
  - Ed25519 for live edits,
  - Ed25519 + ML-DSA-65 for long-lived authority records,
  - Argon2id for password files.

  This was amended per the owner's decision on 2026-10-12; see format
  design §6.1.
- **Accounts:** login uses **OPAQUE** (RFC 9807, `opaque-ke`), so the server
  never sees the password. The OPAQUE export key encrypts the user's private
  key bundle stored on the server. A **recovery key** (shown once, printable)
  is required, because a forgotten password otherwise means lost data.
- **Devices:** each device has its own keys, signed by the user identity key.
  Lost devices can be revoked.
- **Key authenticity:** public keys are trusted on first use, with a
  verification UI (safety numbers / QR) for sharing. A key-transparency log
  is planned so the server cannot swap keys unnoticed.
- **Transport:** TLS 1.3 (rustls) in addition to E2EE.
- **Local:** cloud-document caches are encrypted with a device key kept in
  the OS keychain. API keys for AI providers are also stored in the keychain.
- The crypto design and implementation get an **independent security audit
  before the hosted service launches**.

## Consequences
- The server cannot search, preview or merge. Search is client-side over the
  local cache, and compaction is done by clients.
- Password reset without the recovery key cannot restore data. The UX must
  make this clear.
