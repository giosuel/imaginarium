---
title: Download
description: Imaginarium downloads for Linux, Windows and macOS.
---

:::caution[No release yet]
Imaginarium is in early development (milestone M0: foundations). There is
nothing to install yet. Follow progress on
[GitHub](https://github.com/giosuel/imaginarium).
:::

When the first preview ships, builds will be available for:

| Platform | Packages |
|---|---|
| Linux | AppImage, Flatpak (Flathub), `.deb`, `.rpm` |
| Windows | MSI / MSIX, winget |
| macOS | Signed and notarized DMG (universal), Homebrew cask |

All builds will be x86_64 and arm64 where the platform supports it, with
optional signed auto-updates. The plan is in
[ADR-0015](/developers/reference/adr/0015-distribution-and-docs/).

## Building from source

You need [Rust](https://rustup.rs). The pinned toolchain installs itself.

```sh
git clone https://github.com/giosuel/imaginarium
cd imaginarium
cargo run -p imaginarium-app
```

Today this only prints the version. The editor arrives with the first UI
milestones.
