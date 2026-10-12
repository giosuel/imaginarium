# Architecture Decision Records

Each significant decision gets a short, numbered record. Once accepted, a
record is only changed in its status line. To change a decision, write a
new ADR that *supersedes* the old one.

Template: `0000-template.md`. Statuses: Proposed, Accepted, Superseded by NNNN.

| ADR  | Title                                                | Status |
|------|------------------------------------------------------|--------|
| 0001 | Rust workspace, toolchain and CI                     | Accepted |
| 0002 | iced as application shell                            | Superseded by 0007 |
| 0003 | Own render/text stack on wgpu + Vello + Parley       | Accepted |
| 0004 | Persistent document model with op log                | Superseded by 0009 |
| 0005 | File formats: native container, OOXML, ODF           | Partially superseded by 0010 |
| 0006 | Add-ons as WASM components + declarative packages    | Accepted (licensing per 0008) |
| 0007 | Own UI toolkit on the Linebender stack               | Accepted (prototype gate) |
| 0008 | AGPL-3.0-or-later, no proprietary add-ons            | Accepted |
| 0009 | CRDT (Loro) document model + view model              | Accepted (confirmed by M1.5) |
| 0010 | Native `.imag` encrypted object-pack format          | Accepted |
| 0011 | End-to-end encryption security model                 | Accepted |
| 0012 | Self-hostable zero-knowledge collaboration hub       | Accepted |
| 0013 | Provider-agnostic generative AI                      | Accepted |
| 0014 | All configuration via typed settings registry + UI   | Accepted |
| 0015 | Distribution, website and documentation              | Accepted |
| 0016 | Format defaults: local encryption, history, thumbnails, PQC | Accepted |
| 0017 | Dev build profile: optimized deps, unoptimized build-time crates | Accepted |
| 0018 | Recipient-key documents need a trusted ACL owner      | Accepted |
