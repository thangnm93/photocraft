# Context

## Relevant code

- `crates/io/src/liquify_map.rs`: the decoder (`LqMe` → `StoredLiquify`), the filter buffer
  (`params.frame`, `trusted_frame`), placement in the document (`placed_mesh`), import/export
  of the item. Mirrors `puppet_map.rs`.
- `crates/algo/src/displacement_mesh.rs`: `DisplacementMesh` + `warp_displacement` (inverse map,
  bilinear field clamped past the outer nodes, bilinear premultiplied source, area-limited).
- `crates/engine/src/distort_cmds.rs`: `PSD_LIQUIFY`, `psd_liquify_surface`, `apply_to_surface`.
- `crates/engine/src/smart_cmds.rs`: `set_filter_params` refuses editing it.
- `crates/io/src/psd_import.rs`: adds the smart object's `FEid` rect as `params.frame`.
- `crates/io/src/psd_export.rs`: a trusted frame is unioned into the rewritten `FEid` rect.
- `crates/ui-egui/src/smart_ui.rs`: Layers panel label (Photoshop's name without the ellipsis).
- A render front end refuses (422) a smart object whose visible instance has a visible
  `psd.unsupportedFilter`; Liquify now has its own command, which lifts that.

## The filter item

`filterFX` item: `Nm  ` "Liquify...", `filterID` 1282492025 (`LqFy` as a big-endian code),
`Fltr` = descriptor class `LqFy` with a single key `LqMe` of type `tdta` (raw bytes).

## LqMe format (decoded from four real meshes; every blob decodes to its last byte)

```text
offset  size  value
0       4     00 00 00 04            (big-endian 4)
4       8     "yfqL" "hseM"          (LqFy, Mesh as little-endian codes)
12      4     u32 LE 2               layout version
16      4     u32 cols               nodes per row  = width / 4 (integer division)
20      4     u32 rows               rows of nodes  = height / 4
24      4     u32 0                  (always 0)
28      4     u32 1                  (always 1)
32      16    u32 top, left, bottom, right   content bounds inside the frame
48      16    u32 0, 0, height, width        the frame (the filter buffer the mesh was made on)
64      ...   rows of nodes
```

- Each row is a run list: a count of undisplaced nodes (may be 0), then a count of displaced nodes
  followed by that many `(dx, dy)` f32 LE pairs, alternating, until the row holds `cols` nodes. A
  row ends right after the run that fills it. Undisplaced nodes are exactly (0, 0); displaced
  runs never hold a (0, 0) node; no empty run appears mid-row.
- Semantics (fitted against Photoshop's own filter input, see below, no free parameters left):
  - the frame is split into `cols × rows` cells of `width / cols` × `height / rows` px (a little
    over 4, e.g. 1730 / 432 = 4.0046);
  - node (i, j) sits at the centre of cell (i, j);
  - the mesh is an **inverse map in cells**: the output at a node shows the input at the node plus
    `(dx · cellw, dy · cellh)` px;
  - between nodes the displacement is bilinear (Catmull-Rom is no better: the field is smooth);
    past the outermost nodes it stays at theirs (clamp: very slightly better than zero);
  - the input is sampled bilinearly, premultiplied (bicubic a = −0.5 / −0.75 is 2× worse).
  - A forward reading (cells drawn onto displaced cells) is far off (mean 13.5 vs 0.24).
- Spacing exactly 4 instead of `size / nodes` leaves a 2–4 px drift across the frame (10× worse).

## Where the frame sits: Photoshop's filter buffer (`FEid`)

The global `FEid` block keeps, per smart object (`placed` id), Photoshop's unfiltered placed
pixels — the smart filters' input — over a rect. That rect is the filter buffer:

- its top-left = `floor(control bounds) − 3`, where the control bounds are the warp mesh's control
  points through the placement (not the warped outline: in one layer the control points reach
  10 px past the outline, and so does the buffer); right = `ceil(...) + 3`;
- its bottom sometimes extends further (by 45 px and 8 px in two of the four layers): the buffer
  is grown to keep the mesh's frame where it was made when the smart object was re-placed later;
- the mesh's frame sits at the buffer's **bottom-left** corner (shown by the layer whose buffer is
  8 px taller than the mesh; in the other three the sizes match).

Import stores the `FEid` rect in `params.frame`; the renderer anchors the mesh there, so Replace
Contents keeps the Liquify where Photoshop drew it. Without a frame: the input's bounds + 3 px.

Security: `params.frame` comes from the file. It is used only when it holds the mesh's frame, is
at most 1024 px larger each way, reaches the input + 3 px and is ≤ 16k × 16k px
(`trusted_frame`); else the fallback. Same check on export (where it is unioned into the FEid rect).

## Ground truth and numbers

The template's smart object contents are flat white with alpha, so only the silhouette (alpha)
constrains the map — but the antialiased edges give sub-pixel information.

Per layer, vs the pixels Photoshop stored for the layer (premultiplied RGBA, 0–255):

| layer | Liquify from our placement | skipped | Liquify from Photoshop's input | skipped |
|---|---|---|---|---|
| left leg back | mean 0.244, >2: 0.80 % | 27.7, 11.4 % | 0.0032, 0.021 % | 27.7 |
| left leg front | 0.158, 0.60 % | 14.0, 5.9 % | 0.0034, 0.036 % | 13.9 |
| right leg | 0.102, 0.32 % | 0.25, 0.40 % | 0.0067, 0.047 % | 0.16 |
| waistband | 0.110, 0.46 % | 2.11, 1.22 % | 0.0020, 0.021 % | 2.05 |

What is left with our placement is the placement's own edge rendering: our warped smart object vs
Photoshop's filter input differs by 0.10–0.16 mean, 0.30–0.44 % > 2 (Photoshop's coverage is
~0.1–0.2 px wider on every edge), independent of the Liquify.
