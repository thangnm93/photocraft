# Plan

1. Decode `LqMe` from a real file in a scratch script (outside the repo): header, mesh size,
   rects, run encoding, units. — done (context.md).
2. Fit the semantics against Photoshop's stored layer pixels; use Photoshop's own filter input
   (`FEid`) to remove our placement's edge differences from the fit. — done: inverse map in
   cells, nodes at cell centres, cell = frame / nodes, bilinear field (clamped) and source,
   frame at the bottom-left of the `FEid` rect.
3. `crates/algo/src/displacement_mesh.rs` (TDD): the resampler. — done.
4. `crates/io/src/liquify_map.rs` (TDD, synthetic blobs): decode, `psd.liquify` import with the
   item verbatim and the `FEid` rect as `params.frame`, export byte for byte, malformed →
   unsupported, caps, fuzz; frame trusted only when plausible. — done.
5. Engine glue + UI label + engine integration tests (pipeline, Replace Contents, PSD/.pcraft
   round trip, implausible frame). — done.
6. Measure on the real PSD (scratch probe, not committed). — done (context.md).
7. Gate: workspace tests, corpus, release build, 20-template byte-identical check, caps/polo,
   the Liquify template through the render front end OLD vs NEW, whole-document comparison with
   Photoshop / Photopea / the previous production renderer.
