# 0014. Every setting is configurable in the UI (typed settings registry)

- Status: Accepted
- Date: 2026-10-12

## Decision
- `imaginarium-settings` is a typed registry. Each setting declares:
  - key, type, default, validation,
  - **scope**: app, user (synced and encrypted), workspace, document, or hub
    (server, admin only),
  - UI metadata: title, description, synonyms, group, and whether it is
    advanced.
- The settings UI is **generated from the registry**. It is searchable and
  integrated in the command palette ("Settings: AI provider…"). Add-ons
  declare settings the same way.
- Files (TOML for app scope) exist for portability and backup, but are
  **never required**.
- The hub server is also configured from the desktop client's admin panel.
  Its env/config file is optional and only for container deployments.
- A CI test ensures every setting read in code is registered with UI
  metadata, so no setting can exist only in a config file.
