# Quilt-warp smart objects render wrong / not at all

**What:** PhotoCraft drops a placed layer's warp when it is a multi-patch ("quilt") custom warp,
the split-grid warp Photoshop writes for mockups. A smart object with stored pixels then keeps
them after Replace Contents (nothing changes); one without them re-renders with no warp (the
design lands in the wrong place).

**Why:** `photocraft-cli` is a fast headless mockup renderer (1–1.5 s per render against 9–16 s
for another PSD renderer, Photopea). Two real mockups fail today: a sneaker (10×10 and 10×22
point grids) and a V-neck jersey (16×16 up to 19×28).

**Done when:** both mockups render with the design where the reference renderer puts it,
measured by pixel diff; unit tests, clippy and the crate tests pass.
