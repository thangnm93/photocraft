# Plan

1. ✅ Learned the quilt descriptor layout from public sources (ag-psd) and two real mockups.
2. ✅ Parse it into `BezierMesh` and generalise `placed_frame` (`crates/io/src/blocks.rs`).
3. ✅ A second, behavioural difference: Replace Contents keeps the new file's pixel size, while
   other renderers stretch it to the old contents' box (since added as `fit: "stretch"`, see
   `.docs/mockup-render/`).
4. ✅ Built `photocraft-cli` and compared with the reference renders.
5. ✅ Unit tests (3), `cargo test -p photocraft-io`, clippy `-D warnings`, `xtask layers`,
   `xtask wasm`, `xtask test-corpus --changed`.
6. ✅ Root cause posted on storytold/photocraft#4398 (`issue-4398-comment.md`).
7. Later: write `quiltWarp` back on PSD export (`smart_map::warp_desc`); open an upstream PR if
   wanted.
