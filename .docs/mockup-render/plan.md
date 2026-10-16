# Plan

1. ✅ `fit: "stretch"`, `doc.replaceContents`, `doc.save` `pngCompression`, mip levels.
2. ✅ `clipToCanvas` (canvas, then the layer mask's revealed area); digest memoised per buffer.
3. ✅ Decode the new file once; reduced level and placements shared by the instances of one
   Replace Contents; hidden instances unrendered under `clipToCanvas`.
4. ✅ Memory: banded flatten, plain one-layer sources used as is, cache budget prefs, LRU effect
   cache.
5. ✅ Review fixes, gate (workspace tests, panic_hunt, clippy, fmt, layers, scorecard, wasm,
   corpus).
6. Later, if wanted: decode PNG designs straight into tiles (one full-size buffer less at peak);
   `malloc_trim` when a serve process is idle; open an upstream PR.

Rejected (measured): a lower mip threshold (≤ 1.5 % faster, 0.375/255 off), destination-sized warp
cells (1–3 % CPU, not byte-identical), 16-bit mip storage (not byte-identical).
