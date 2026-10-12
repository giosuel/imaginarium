# 0015. Distribution, website and documentation

- Status: Accepted
- Date: 2026-10-12

## Decision

### Distribution
- **Desktop builds** use `cargo-dist`, producing:
  - Windows: MSI/MSIX and winget
  - macOS: universal binary, signed and notarized DMG, and a Homebrew cask
  - Linux: AppImage, Flatpak (Flathub), `.deb` and `.rpm`
  - x86_64 and arm64 everywhere it applies
- **Auto-update** uses signed updates (evaluating Velopack) and can be
  disabled in settings.
- **Hub:** a static musl binary, a Docker image (amd64/arm64) and a Compose
  file.

### Website (`website/`, Astro Starlight)
Starlight brings search, i18n and versioned docs. The site contains:
- the landing page, downloads and the hosted plans (later),
- **user guides** for each mode, tutorials, AI setup (local and API), and
  self-hosting,
- **developer docs**: architecture overview, crate map, rendering engine,
  text engine, reactive UI, sync protocol, security whitepaper, `.imag`
  spec, add-on SDK, and contributing,
- generated API reference (rustdoc) linked from the developer docs,
- **diagrams** as code (D2/Mermaid). They will gradually be replaced by
  diagrams made in Imaginarium and exported to SVG.

Docs are part of the definition of done: a milestone isn't finished
until its user and developer docs are written. The site is built and
deployed in CI.
