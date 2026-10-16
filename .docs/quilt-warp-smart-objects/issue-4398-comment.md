I think I found the root cause, and have a local fix with tests.

**Cause.** `crates/io/src/blocks.rs` › `warp_of()` only reads a `warpCustom` mesh with exactly 16 control points, i.e. a single 4 × 4 Bezier patch:

```rust
if xs.len() != 16 || ys.len() != 16 { return None; }
```

Mockups are usually warped with Photoshop's split warp (Edit › Transform › Warp with a grid). Photoshop stores that in `SoLd` under **`quiltWarp`** rather than `warp`. The descriptor holds:
- `deformNumCols` × `deformNumRows` control points, three per patch side plus one;
- the patch boundaries, in `customEnvelopeWarp.quiltSliceX` and `quiltSliceY`.

Those layers come in with `warp: None`. While nothing changes, the cached render hides it. Once Replace Contents re-renders them, the contents are drawn through the `Trnf` quad only. `placed_frame` then frames the source rect instead of the control-point box, so the result is flat and mis-scaled.

**Fix.** `BezierMesh { us, vs, points }` already models multi-patch grids, so the change is confined to the reader:
- read `quiltWarp`, preferring it over `warp`;
- turn the slices into `us` / `vs` knots;
- check the grid is consistent: 3n + 1 points per side, one slice per boundary, increasing, finite, bounded;
- let `placed_frame` take the box of all control points, not only 16.

**Results.** I tested the fix on two real mockups, which I can't share:
- a sneaker: two instances with 10 × 10 and 10 × 22 point grids and a perspective `Trnf`;
- a V-neck jersey: seven instances with 16 × 16 to 19 × 28 point grids.

The reference is a render of the same replaced contents by another PSD renderer (Photopea). Numbers are mean absolute difference over the design area on a 0–255 scale, and the share of samples off by more than 16.

| | before (0.9.0) | after |
|---|---|---|
| sneaker | 14.4, 10.8 % | 0.66, 0.02 % |
| jersey (contents at the original pixel size) | 13.0, 4.1 % | 0.64, 0.25 % |

The remaining difference is resampling: a solid fill already differs by about 0.6.

`cargo test -p photocraft-io`, clippy `-D warnings`, `cargo xtask layers` and `cargo xtask wasm` all pass.

The new tests cover three cases:
- a quilt reads as a multi-patch mesh, and wins over a `warpNone` `warp`;
- the quad is framed by all the control points;
- inconsistent or hostile quilts are refused without panicking.

I'm happy to open a PR.

Two things this doesn't cover:
- **Export.** PSD export still drops multi-patch meshes: `smart_map::warp_desc` writes `warpNone` with a warning instead of a `quiltWarp`.
- **Replace Contents with a file of a different pixel size.** The new contents keep their own pixel size, so a smaller file lands smaller. Photopea instead stretches it to the old contents' box. I haven't checked what Photoshop does.
