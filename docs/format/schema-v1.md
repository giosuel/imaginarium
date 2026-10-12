# Imaginarium document schema, version 1

- Status: **Draft 1** (2026-10-12). It becomes normative together with
  [`spec-v1.md`](spec-v1.md) at tag `v0.0.2-m1`. Until then, implementation
  findings may change it.
- Scope: the **logical** document inside the `loro-1` codec (spec §3.1).
  These are the CRDT containers, keys, node kinds, text marks and ids. The
  container (records, encryption, index) is `spec-v1.md`.
- Reference implementation: crate `imaginarium-model`.

The key words MUST, SHOULD and MAY are used as in RFC 2119.

## 1. Principles

1. **One document model.** A board, a text document, a slide deck and a
   page layout are views of the same tree of nodes. Every view can show
   every node kind. A view that doesn't know a kind falls back to a
   placeholder (rule 4).
2. **The CRDT is the source of truth.** Everything users can see or change
   is in the containers below. Caches and derived data are not (spec
   `DERV` records).
3. **Additive evolution** within schema version 1 (§10). New keys, kinds,
   marks and style properties may be added, and existing ones never change
   meaning.
4. **Unknown data is preserved.** Readers MUST keep keys, kinds, marks and
   containers they don't understand. The CRDT does this automatically as
   long as the implementation never rewrites a whole map to "clean it up".
   Unknown node kinds are shown as a box from their geometry.
5. **Large binary data stays out of the CRDT.** Images, fonts and raster
   tiles are blobs (spec §5.3), referenced by their `BlobId`.

## 2. Values

| Type | Encoding in Loro |
|---|---|
| `string` | Loro string |
| `bool` | Loro bool |
| `int` | Loro `i64` |
| `number` | Loro `f64`. Readers MUST also accept `i64` where a number is expected. |
| `color` | `string`: `#RRGGBB` or `#RRGGBBAA`, sRGB, lowercase or uppercase hex |
| `length` | `number` in **points** (1 pt = 1/72 inch). Views choose the zoom. |
| `angle` | `number` in degrees, clockwise |
| `NodeId` | Loro `TreeID` of the node. Its text form is `counter@peer` (decimal), e.g. `12@1234567890`. |
| `StyleId` | `string`. Generated ids are 32 lowercase hex characters (128 random bits). Names starting with `builtin:` are reserved for built-in styles. |
| `BlobId` | `string`: the 64 lowercase hex characters of the blob's `ObjectId` (spec §5.2) |
| `Timestamp` | `int`: Unix time in milliseconds |

A key that is **absent** means "the default". Writers SHOULD delete a key
rather than store the default explicitly.

## 3. Root containers

A `main` stream document (spec §3.2) has these root containers:

| Name | Type | Content |
|---|---|---|
| `meta` | Map | Document metadata (§4) |
| `nodes` | Tree | All content nodes (§5). Fractional indexes MUST be enabled: sibling order is z-order on boards and reading order in documents. |
| `styles` | Map | `StyleId` → style map (§8) |
| `assets` | Map | `BlobId` → asset map (§9) |
| `pages` | Map | Page and slide setup (§7) |
| `ext` | Map | Document-level add-on data: add-on id (reverse DNS) → any value or container |

Other root containers MUST be preserved and MUST NOT be created, except by
a later schema version.

## 4. `meta`

| Key | Type | Meaning |
|---|---|---|
| `schema_version` | `int` | **Required.** The schema major version, `1` for this document. |
| `kind` | `string` | Which view opens by default: `board`, `document`, `deck` or `layout`. Unknown or absent values mean `board`. |
| `title` | `string` | The document title |
| `language` | `string` | Default language, BCP 47 (e.g. `de-CH`) |
| `created` | `Timestamp` | When the document was created |
| `required_features` | List of `string` | Features a reader MUST understand to **edit** the document (§10.2). Absent or empty means none. |
| `ai_policy` | Map | Reserved for the document's AI policy (M6) |

## 5. Nodes

Each node of the `nodes` tree has a meta map with these keys:

| Key | Type | Meaning |
|---|---|---|
| `k` | `string` | **Required.** The node kind (§5.1). |
| `g` | Map | Geometry (§5.2). Nodes that a view lays out itself, such as document paragraphs, omit it. |
| `s` | Map | Style: `id` (a `StyleId`) plus local overrides of style properties (§8). |
| `t` | Text | Rich text (§6), for kinds that hold text |
| `a` | Map | Asset references: role → `BlobId`. Roles: `src` (an image's pixels), `poster` (a video's frame). |
| `x` | Map | Node-level add-on data: add-on id → any value or container |
| `gen` | Map | AI provenance (§5.4) |
| `name` | `string` | A user-facing name, as in the layers panel |
| `hidden` | `bool` | Not shown and not printed. Default `false`. |
| `locked` | `bool` | Not selectable or editable on the canvas. Default `false`. |

### 5.1 Node kinds

| `k` | Meaning | Children |
|---|---|---|
| `group` | Groups its children. Its geometry is the group's frame. | any |
| `frame` | A board frame or artboard. It clips its children. | any |
| `page` | A page of a document or layout, in reading order | any |
| `slide` | A slide of a deck, in slide order | any |
| `shape` | A vector shape. `shape` (string) names the preset: `rect`, `ellipse`, `line`, `triangle`, `diamond`, … Unknown presets render as `rect`. | none |
| `text` | A text box. `t` holds the text. | none |
| `image` | An image. `a.src` is the blob. | none |
| `connector` | A connector. `from` and `to` are `NodeId` strings, both optional. The geometry holds the fallback endpoints. | none |
| `raster` | A paint layer: `tiles` is a Map `"x,y"` → `BlobId` of a 256×256 tile (spec §5.3, design §3.6). Last writer wins per tile. | none |

Further kinds (`table`, `chart`, `video`, `flow`, …) come in later minor
revisions. Readers MUST treat unknown kinds as leaf boxes (§1, rule 4).

### 5.2 Geometry (`g`)

| Key | Type | Meaning |
|---|---|---|
| `x`, `y` | `length` | Top-left corner, in the parent's coordinate space (the page, slide or frame; the board for root nodes) |
| `w`, `h` | `length` | Size, ≥ 0 |
| `r` | `angle` | Rotation around the box's center. Default `0`. |

Each coordinate is its own key, so concurrent edits of different
coordinates both survive. Concurrent edits of the same coordinate resolve
last-writer-wins (Loro map semantics).

### 5.3 Hierarchy and order

- The tree's parent-child relation is the containment relation.
  Concurrent moves never create cycles (Loro movable tree).
- **Sibling order** is the tree's fractional-index order: back to front on
  boards, slides, pages and frames; first to last for pages and slides
  under the root.
- Deleting a node deletes its subtree.

### 5.4 AI provenance (`gen`)

`gen` records how content was generated, so it can be regenerated, varied
and attributed (design §3.5).

| Key | Type |
|---|---|
| `provider`, `model` | `string` |
| `prompt`, `negative` | `string` |
| `seed` | `int` |
| `params` | Map |
| `inputs` | List of `BlobId` or `NodeId` strings |
| `mask` | `BlobId` |
| `created_by` | `string`: the actor (§11) |
| `created` | `Timestamp` |

## 6. Text

A text container holds **paragraphs**. Each paragraph ends with `\n`, and
the text of a non-empty container MUST end with `\n`. Writers keep that
final newline, and readers treat a missing one as if it were there.

Indexes are Unicode scalar values (Loro's default).

### 6.1 Character marks

Loro marks over character ranges. The expand behaviour says whether text
typed at a mark's edge gets the mark. It is part of the schema, because
Loro requires that a key always uses the same behaviour.

| Key | Value | Expand | Meaning |
|---|---|---|---|
| `b` | `true` | after | Bold |
| `i` | `true` | after | Italic |
| `u` | `true` | after | Underline |
| `s` | `true` | after | Strikethrough |
| `va` | `"super"` or `"sub"` | after | Superscript, subscript |
| `font` | `string` | after | Font family |
| `size` | `length` | after | Font size |
| `color` | `color` | after | Text color |
| `bg` | `color` | after | Highlight |
| `link` | `string` (URL) | none | Hyperlink |
| `code` | `true` | none | Inline code |

### 6.2 Paragraph marks

Paragraph formatting is a mark on the paragraph's **terminating `\n`**
only, with expand `none`. Typing inside or at the end of a paragraph never
changes its formatting. Splitting a paragraph inserts a new `\n`, and the
editor copies the paragraph marks to it.

| Key | Value | Meaning |
|---|---|---|
| `ps` | `StyleId` | Paragraph style |
| `h` | `int` 1–6 | Heading level |
| `li` | `"bullet"` or `"ordered"` | List item |
| `lvl` | `int` ≥ 0 | List or outline level. Default `0`. |
| `al` | `"start"`, `"center"`, `"end"` or `"justify"` | Alignment |

Unknown mark keys MUST be preserved. Writers MUST NOT create marks with
keys that this table doesn't define. Add-ons namespace their keys as
`<add-on id>:<key>`, with expand `none`.

## 7. Pages (`pages`)

| Key | Type | Meaning |
|---|---|---|
| `w`, `h` | `length` | Default page or slide size. Defaults: A4 (595.28 × 841.89 pt) for documents and layouts, 960 × 540 pt for decks. |
| `margins` | Map: `t`, `r`, `b`, `l` (`length`) | Default page margins. Default 72 pt each. |

Masters and layouts come in a later minor revision.

## 8. Styles

`styles` maps a `StyleId` to a style map:

| Key | Type | Meaning |
|---|---|---|
| `name` | `string` | User-facing name |
| `parent` | `StyleId` | Inherits every property it doesn't set. Readers MUST stop at cycles and missing parents. |
| *property* | see below | A style property |

Style properties, valid in style maps and as local overrides in a node's
`s` map:

| Property | Type | Meaning |
|---|---|---|
| `fill` | `color` | Fill |
| `stroke` | `color` | Outline color |
| `stroke_w` | `length` | Outline width |
| `opacity` | `number` 0–1 | Opacity |
| `radius` | `length` | Corner radius |
| `font`, `size`, `color` | as the marks of §6.1 | Default text formatting |

The effective value of a property is the first one found in this order:
the node's local override, its style, the style's ancestors, and finally
the built-in default.

## 9. Assets

`assets` maps a `BlobId` to:

| Key | Type | Meaning |
|---|---|---|
| `mime` | `string` | Media type, e.g. `image/png` |
| `size` | `int` | Size in bytes |
| `name` | `string` | Original file name |
| `w`, `h` | `int` | Pixel size, for images |
| `derived_from` | `BlobId` | For derived variants, e.g. an AI edit of an image |

## 10. Evolution

### 10.1 Within version 1

Changes are additive only (design §3.4):
- new keys, kinds, marks, style properties and root containers in a later
  revision of this document;
- never a changed meaning for an existing key;
- never a reused name.

### 10.2 `required_features`

A feature that older readers would damage when *editing* (for example
linked text frames, whose text an old reader would duplicate) is listed in
`meta.required_features`. A reader that doesn't know every listed feature
MUST open the document **read-only** and tell the user why. Version 1
defines no features.

### 10.3 Schema migrations

A breaking change bumps `meta.schema_version`.
- A reader that knows a migration from the stored version runs it on open,
  as one CRDT transaction. The migration syncs like any other edit.
- Migrations MUST be deterministic, so peers that run the same migration
  concurrently converge to the same document.
- A reader that sees a newer version than it knows MUST open the document
  read-only.

## 11. Actors and attribution

Every transaction is made by an **actor**:
- `user` (the person at the keyboard);
- `ai:<session>` (the AI assistant);
- `addon:<add-on id>`;
- `migration`;
- `remote` (changes received from peers).

The actor is recorded as the Loro change's commit message, for example
`ai:7f3a`. Peers can therefore attribute edits after the fact, and the
editor undoes each local actor's changes separately (ADR-0009).

## 12. The discussion stream

Comments and threads live in the separate `discussion` stream (spec §3.2).
Its schema (threads anchored to nodes or to text ranges through Loro
cursors) is defined in a later revision, together with collaboration
(M8). Version 1 readers MUST preserve a discussion stream they can't
interpret.
