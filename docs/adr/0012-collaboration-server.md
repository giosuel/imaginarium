# 0012. Self-hostable collaboration server ("Imaginarium Hub")

- Status: Accepted
- Date: 2026-10-12

## Decision
- `imaginarium-hub` is a single Rust binary built on axum, tokio and rustls.
  - Storage: **SQLite by default**, with PostgreSQL optional.
  - Objects go on the local filesystem or **S3-compatible** storage
    (`object_store`).
  - Distributed as a static binary, a Docker image and Docker Compose.
    Helm charts come later.
- It is a **dumb, zero-knowledge relay and store**:
  - stores encrypted objects per document,
  - orders updates with a server sequence number,
  - fans out live update frames and presence over WebSocket,
  - enforces roles via signatures and the ACL chain,
  - handles quotas.
- Features: accounts, workspaces/teams, encrypted folders, sharing (users,
  teams, links with the key in the URL fragment), roles (owner, admin,
  editor, commenter, viewer), comments and threads, version history, trash,
  and content-free notifications ("new comment in a document"; the name is
  resolved on the client).
- Admin is **UI-only**. On first start the hub prints a one-time setup code,
  and the admin claims the server from the desktop client. All server
  settings, users and quotas are then managed in the desktop client's
  admin panel.
- The sync protocol is specified in `docs/design/sync-protocol.md` (to be
  written in M1, alongside the format, because they share objects).

## Consequences
- The hub stays small and cheap to host, which suits the subscription
  business model.
- No server-side features need plaintext. Anything that does (e.g. a web
  viewer) runs on the client, as WASM in the browser, with keys taken from
  the URL fragment.
