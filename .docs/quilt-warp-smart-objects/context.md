# Context

- Root cause: `crates/io/src/blocks.rs` `warp_of` returns `None` unless the custom envelope has
  exactly 16 mesh points (one 4×4 Bezier patch). `placed_frame` has the same 16-point
  assumption.
- `photocraft_geom::warp::BezierMesh { us, vs, points }` already models multi-patch grids:
  `us`/`vs` are patch boundaries on the unit square; the points form a
  (3·(us-1)+1) × (3·(vs-1)+1) grid.
- Test data: two private mockup PSDs (not in the repo) and reference renders of the same replaced
  contents from Photopea. The unit tests use synthetic descriptors.
