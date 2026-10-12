# 0011. End-to-end encryption security model

- Status: Accepted
- Date: 2026-10-12

## Decision
- **The server cannot read anything.** Content, titles, file names, comments,
  thumbnails and presence (cursors) are encrypted on the client. Details are
  in `docs/design/file-format.md` §6.
- Suite 1: XChaCha20-Poly1305, BLAKE3, Ed25519 and HPKE (X25519), with Argon2id
  for password files. A post-quantum hybrid suite is reserved.
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
