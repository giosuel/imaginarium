# Tafel

> Working codename. *Tafel* is German for "board/blackboard". It can be renamed
> cheaply until the first crates are published.

A cross-platform (Linux, Windows, macOS) office suite written in Rust that
puts **documents, presentations, page layouts, diagrams and drawings on
one whiteboard-like surface**:

| Mode         | Replaces                  | Formats                       |
|--------------|---------------------------|-------------------------------|
| Board        | draw.io, Visio, whiteboard | native, SVG, PNG, PDF         |
| Document     | Word, Writer              | DOCX, ODT, PDF export         |
| Presentation | PowerPoint, Impress       | PPTX, ODP, PDF export         |
| Layout       | Publisher                 | native, PDF export            |
| Paint        | Paint                     | PNG, JPEG, WebP, native layers|

Spreadsheets are out of scope for now, but the architecture leaves room
for them.

## Status

Planning. See [`docs/PLAN.md`](docs/PLAN.md) for the roadmap and
[`docs/adr/`](docs/adr/) for architecture decisions.
