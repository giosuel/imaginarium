---
title: Overview
description: Run your own Imaginarium Hub for private, real-time collaboration.
sidebar:
  order: 0
---

**Imaginarium Hub** is the small server for sharing and real-time
collaboration. It is planned for milestone M8 (see the
[roadmap](/imaginarium/developers/reference/plan/#12-roadmap)).

What it will be:

- **One binary or Docker image.** It uses SQLite and local files by default,
  or PostgreSQL and S3-compatible storage for larger installations.
- **Zero-knowledge.** Documents, file names, comments and cursors are
  end-to-end encrypted. The server stores and relays ciphertext and
  **can't read anything**. Passwords are never sent to it (OPAQUE login).
- **Administered from the desktop app.** On first start the Hub prints a
  one-time setup code, and you claim the server from the app. No config
  files are needed.
- **Features:** accounts and teams, encrypted folders, sharing with users,
  teams or links, roles (owner, admin, editor, commenter, viewer), comments,
  version history.

Details: [ADR-0012](/imaginarium/developers/reference/adr/0012-collaboration-server/)
and the [security model](/imaginarium/developers/reference/adr/0011-e2ee-security-model/).
