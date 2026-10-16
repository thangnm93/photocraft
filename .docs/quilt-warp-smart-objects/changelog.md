# Changelog

- 2026-10-15: task created; root cause located (16-point-only warp parser).
- 2026-10-15: `blocks.rs`:
  - `parse_placed_warp` prefers `quiltWarp`;
  - `warp_of` reads multi-patch grids via the new `quilt_mesh`;
  - `placed_frame` takes the box of any valid custom mesh.
- 2026-10-15: Pixel diff against the reference renderer over the design area:

  | Mockup | before | fixed |
  |---|---|---|
  | sneaker, synthetic design | 14.4/255 | 0.66 |
  | jersey, design at the contents' pixel size | 13.0 | 0.64 |
  | sneaker, solid black (unchanged) | 0.58 | 0.58 |

  Timing per replace + render: sneaker 1.4 s; jersey 12.9 s (103 MP contents through 7 quilt
  warps, ~3 cores).
- 2026-10-15: Tests: 3 new unit tests; photocraft-io 587 pass; clippy clean; layers and wasm OK.
- 2026-10-15: Root cause posted on storytold/photocraft#4398 (text in `issue-4398-comment.md`).
- 2026-10-15: `cargo xtask test-corpus --changed` (psd, codecs, io, engine; release, corpus+heif):
  160 test binaries, 3665 passed, 0 failed, 27 ignored.
