# Mockup rendering through `photocraft-cli serve`

**What:** the engine support a long-running render server needs to put designs into PSD
mockups: Replace Contents that fills the old contents' box (`fit: "stretch"`), renders only what
a flattened export shows (`clipToCanvas`), a capability-scoped `doc.replaceContents` serve
method, `pngCompression` on `doc.save`, and the speed and memory work that makes one design per
request cheap.

**Why:** a server keeps a mockup PSD open in `photocraft-cli serve` and, per request, replaces the
smart object's contents, saves the flattened image and undoes. Every step of that loop has to be
fast and bounded in memory, and the output must not change.

**Correctness bar:** every optimisation keeps the flattened output byte-identical (checked on 20
real-world mockup PSDs with two print-size designs, 80/80 PNGs identical); anything that changed
pixels was rejected.

**Branch:** `fix/quilt-warp-smart-objects`, on top of the quilt warp fix (`.docs/quilt-warp-smart-objects/`).
All changes are generic and kept upstreamable: tests, clippy, `xtask layers`, `xtask wasm`,
`xtask scorecard`, corpus.
