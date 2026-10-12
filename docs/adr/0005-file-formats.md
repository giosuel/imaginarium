# 0005. File formats: native container, OOXML, ODF

- Status: Accepted
- Date: 2026-10-12

## Context
The tool must read and write DOCX, PPTX, ODT and ODP, and needs its own format
for features those formats cannot express (boards, mixed content).

## Decision
- **Native `.imag`:** a zip containing a JSON manifest, a postcard-encoded
  model with a schema version and migrations, the op log, media and
  thumbnails.
- **OOXML:** use `ooxmlsdk` (pure Rust, typed, round-trip tested) for
  (de)serialization. Our `imaginarium-io-ooxml` crate maps it to and from our model.
- **ODF:** no mature Rust crate exists. Write `imaginarium-io-odf` on `quick-xml`
  and `zip`, scoped to what our model supports.
- **Fidelity:** unknown elements are preserved as opaque XML on the nearest
  node and written back unchanged. A corpus of round-trip tests runs in CI.
- **PDF export:** use `krilla` (fallback: ooxmlsdk-pdf). SVG/PNG via
  resvg and `image`.

## Consequences
- Our own ODF crate is a maintenance cost, but it gives full control.
- Our model, not the file format, is the source of truth, so some exotic
  features degrade gracefully instead of being lost.
