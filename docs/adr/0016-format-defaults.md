# 0016. Format defaults: local encryption, history, thumbnails, PQC

- Status: Accepted
- Date: 2026-10-12

## Decision (owner)
1. Local `.imag` files are **unencrypted by default**. Optional protection
   uses a **password** or **account keys**. Cloud documents are always
   end-to-end encrypted.
2. **History:** keep 90 days, and keep named versions forever. Configurable.
3. **Thumbnails:** plaintext thumbnails are on by default for unencrypted
   files (can be disabled). Encrypted files never contain a plaintext
   thumbnail.
4. **Post-quantum:** the hybrid post-quantum suite (X-Wing KEM, hybrid
   Ed25519 + ML-DSA-65 for authority records) is the default from the first
   release.

## Rationale
Plain local files cannot be lost through key loss and work with OS previews
and backups. The post-quantum protection costs only about 1 KB per
recipient, and nothing on the live editing path.
