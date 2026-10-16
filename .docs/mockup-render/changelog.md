# Changelog

- 2026-10-15: `fit: "stretch"` for Replace Contents (+10 tests): a print-size design no longer has
  to be pre-fitted to the contents' size (one mockup: 9–11 s → 2.6 s, same accuracy).
- 2026-10-15: `doc.replaceContents` serve method (read-root scoped), `doc.save` `pngCompression`;
  documented in docs/control-protocol.md and the serve help.
- 2026-10-15: Warp mip levels for shrinks ≥ 4× (3 tests, ignored bench: 246 → 84 ms).
- 2026-10-15: `clipToCanvas`: warped placements were rendered whole, far past the canvas (~100 MP
  for a 4 MP export); outputs byte-identical. Layers with effects or visible smart filters render
  whole; the mip level follows the short axis.
- 2026-10-15: On 20 real-world mockup PSDs: clip also to the layer mask's revealed area; the
  contents digest is memoised per shared `Arc` (N instances hashed a new file 2·N times). 0.51–1.0 s
  per design, byte-identical.
- 2026-10-16: Decode the new file once (−17 %); hidden instances unrendered under clipToCanvas
  (mockups usually keep a hidden "design here" layer); banded source flatten; one reduced level for
  all instances; LRU effect cache; source/stage cache budget prefs; a plain one-layer design used as
  its own source (−21 %). 20 PSDs: 0.27–0.75 s per design; peak RSS of a serve process 5.9 → 3.3 GB
  (2.8 GB with small cache budgets), CPU per request 3.95 → 2.94 s; every output byte-identical.
- 2026-10-16: Review fixes: the plain-layer source now equals the flatten byte for byte (opaque
  defaults and non-RGB documents excluded); a per-command stage scope (copies at one place and
  filter masks place the contents once even with no stage budget); old boxes read before the new
  file is decoded. UI labels for the two prefs in all catalogs; scorecard regenerated.
- 2026-10-16: Gate: 8915 workspace tests, panic_hunt, clippy, fmt, layers, scorecard, wasm green;
  `xtask test-corpus --changed`: 160 suites, 3691 passed, 0 failed.
