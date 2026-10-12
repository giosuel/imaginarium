# 0006. Add-ons as WASM components + declarative packages

- Status: Accepted
- Date: 2026-10-12

## Context
The app must be extensible: template packages, shape libraries, new tools,
new formats. Extensions must not crash or compromise the host, and the API
must stay stable while the internals evolve.

## Decision
- **Tier 1, packages (no code):** a zip with `package.toml` plus assets
  (SVG shapes with metadata, themes, styles, templates, brushes).
- **Tier 2, add-ons (code):** WebAssembly components run in wasmtime, with a
  versioned WIT world `tafel:addon@1`. Add-ons can register commands, tools,
  importers/exporters and declarative inspector panels, and can read the
  model and submit ops. Permissions are capability-based and declared in the
  manifest. Fuel and epoch limits prevent add-ons from hanging the app.
- First-party features use the same registries internally (commands, tools,
  formats), so the API is exercised from day one.
- Native dylib plugins are explicitly rejected (no stable ABI, unsafe).

## Consequences
- WASM adds call overhead, so hot-path rendering stays in the core and
  add-ons describe geometry rather than drawing per frame.
- We must maintain WIT compatibility through semver.
