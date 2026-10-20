# Changelog

- Task started from the production engine commit (Puppet Warp 898 fix).
- Decoded `LqMe`: header, run-length rows of (dx, dy) f32; every real blob decodes to its end.
- Semantics fitted: inverse map, displacement in cells (×4 first, then exactly `size / nodes`),
  bilinear; forward mesh reading rejected (mean 13.5 vs 0.24).
- Found Photoshop's filter input in the global `FEid` block; with it the fit is exact
  (mean 0.002–0.007): nodes at cell centres, frame at the bottom-left of the `FEid` rect.
- `feat(algo)`: `displacement_mesh` resampler (8 tests, 8/16/32-bit).
- `feat(io)`: `liquify_map` decoder, `psd.liquify` import/export, `FEid` rect → `params.frame`,
  export unions the frame into a rewritten `FEid` rect (13 tests incl. fuzz).
- `feat(engine)`: render `psd.liquify`, refuse `setParams`, Layers label; integration tests.
- `fix(io)`: security review — `params.frame` comes from the file; trusted only when it holds the
  mesh's frame, ≤ 1024 px larger, reaches the input and ≤ 16k² px; same check on export. Tests
  (unit + engine export) failed before, pass after.
- `fix(ui)`: the Layers fallback label used a new untranslated "Liquify" key (two i18n coverage
  tests failed in the gate); now the menu's translated "Liquify…" without the ellipsis.
- Gate: corpus 3803 / 0 (baseline 3787); 20 production templates × 2 fits byte-identical to the
  production engine (80/80), Puppet Warp caps and polos 5/5 identical; the Liquify template fails
  on the old engine (unsupported smart filter) and renders on the new one in ~1.3 s.
- Whole document, contents replaced by the smart object's own contents, vs Photoshop's stored
  composite over the print area: mean 0.00, max 1 (the previous renderers: 0.01 and 0.08).
  Black / white designs vs the old renderer: 0.01. A photo design vs the old renderer: 0.91
  (stretch; design edges only, mostly where the Liquify compresses; the bilinear vs Catmull-Rom
  field choice changes nothing there, JPEG decoding differs by 0.06) — the old renderer is not
  ground truth, and Photoshop's own render of a textured design isn't available.
- Open: the field between nodes is fixed by the measurements only along the silhouette (the
  template's contents are flat); interior interpolation (bilinear chosen) is unverified.
