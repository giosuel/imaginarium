# 0021. One shape model for presets and paths

- Status: Accepted (owner, 2026-10-16; the behaviour is theirs: "A
  rectangle stays a rectangle and it can be converted into a path if the
  user wants to do so")
- Date: 2026-10-16

## Context

M3 draws simple shapes: rectangles (also rounded), ellipses, lines,
triangles and diamonds. Later come path-based vector shapes as in
Inkscape, the draw tool's ink strokes (M4) and connectors. The owner wants
the simple shapes stored and handled the way path shapes will be, so the
later ones fit in without a second model, and a rectangle to stay a
rectangle (its parameters editable) until the user converts it to a path.

Schema v1 already has `shape` nodes with a `shape` preset name and a box
(`g`: x, y, w, h, rotation); unknown presets render as `rect`. Styles hold
`fill`, `stroke`, `stroke_w`, `opacity` and `radius`.

## Decision

1. **Every shape is a `shape` node.** Its preset says how its outline is
   made, from the box and the preset's parameters:
   - `rect` (the corner radius from the style's `radius`, or four radii in
     `radii`, a list of four lengths: top left, top right, bottom right,
     bottom left), `ellipse`, `triangle`, `diamond`;
   - `line`: from one corner of the box to the opposite one; `flip`
     (`"h"`, `"v"` or both) says which corners;
   - **`path`**: the outline is path data (below).

   Everything else about a shape (style, transform, name, lock, the
   selection, the inspector) is the same for every preset.
2. **Path data** is the key `d`: an SVG path string (`M`, `L`, `C`, `Q`,
   `Z`; absolute coordinates) in a coordinate space given by `vb`, a list
   `[x, y, w, h]`, which is mapped onto the box, as SVG's `viewBox` is.
   - Moving, resizing or rotating a path shape changes only `g`, as for any
     shape; the path follows its box.
   - Editing a path's nodes rewrites `d` (and `vb` and `g` when the
     outline grows), in one transaction.
   - Concurrent edits of the same path: the last writer wins for the whole
     path, as for any value. Edits of a path are usually one person's; if
     that turns out wrong, a later revision can store segments in a list
     container without changing the meaning of `d`.
3. **One pipeline.** For painting, hit testing, bounds and export, every
   preset is first turned into a `kurbo::BezPath` in the box's coordinates
   (`shape_path(node)`), then transformed by the box. Presets are just
   different ways to produce that path. New presets add a function, not a
   pipeline.
4. **Convert to path** (`shape.convert_to_path`, `Ctrl+Shift+C`) replaces
   the preset with `path`: it writes the preset's outline as `d` with
   `vb = [0, 0, w, h]` and removes the preset's parameters (`radii`,
   `flip`), in one transaction and one undo step. The style stays, and a
   rounded rectangle's corners become curves.
5. **Schema:** `path`, `d`, `vb`, `radii` and `flip` are added to schema v1
   (additive, §10.1). An older reader shows a path shape as a rectangle
   with its style and can still move and resize it without damage, so no
   `required_features` entry is needed.

M3 builds the presets and `shape_path`; `path` itself (storing, painting,
converting, editing nodes) comes when the path tools do (M4 with ink and
connectors, or the vector milestone). The schema keys are written into
the schema document now so nothing else takes them.

## Consequences

- Rectangles keep their meaning: changing the corner radius in the
  inspector works until the user converts, and only then do nodes appear.
- Ink strokes (M4) can be `path` shapes with a fill (the stroke's outline
  at its pressure widths), so they move, recolor and erase like any shape.
- A path's string is rewritten on every node edit; long paths (thousands
  of segments) make large updates. Ink strokes are simplified before they
  are stored.
- Hit testing a path is exact (fill containment, stroke distance) but
  slower than a box; candidates come from the canvas's grid first.

## Alternatives considered

- **Every shape stored as a path from the start.** One representation, but
  a rectangle would lose its parameters (radius, exact size), which the
  owner wants kept, and every resize would rewrite path data.
- **A separate node kind for paths.** Two kinds that behave the same in
  every view but painting; every view and command would need both.
- **Path segments as CRDT lists.** Merges concurrent node edits, but costs
  a container per shape and much more file size for the common case of a
  path edited by one person. Possible later without breaking `d`.
