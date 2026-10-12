# 0008. Licensing: AGPL-3.0-or-later, no proprietary add-ons

- Status: Accepted
- Date: 2026-10-12

## Decision
- The whole project, client and server alike, is licensed
  **AGPL-3.0-or-later**.
- There is **no add-on exception**. Add-ons that link against or use the add-on
  API must be AGPL-compatible. Declarative packages (templates, shapes) may
  carry their own content licenses (e.g. CC-BY), declared in `package.toml`.
- Every dependency must be AGPL-compatible. This is enforced with a
  `cargo deny` license allowlist in CI.
- External contributions are accepted under the **DCO** (Signed-off-by). A
  CLA is not required unless the owner decides otherwise (see open question
  in PLAN).
- The name "Imaginarium" and its logo are to be protected as trademarks, so
  forks may reuse the code but not the brand.

## Rationale
The business model is open source plus paid hosting. With AGPL, anyone who
offers a modified hosted service must publish their modifications, which
protects the hosted offering from closed forks.
