# Context

## Files

- `crates/engine/src/smart_cmds.rs`: `Fit`, `replace_contents_with` (decode once, old boxes read
  first via `authored_boxes`), `stretch_placement`, `set_shared_source_fit` (level and stage
  scopes, hidden instances, `visible_clip`, `needs_whole_render`), `content_hash` (digest memoised
  per shared `Arc`), `plain_layer_surface` / `clear_transparent`, `flatten_banded`, source cache
  budget (`set_source_cache_budget`, `trim_sources`).
- `crates/engine/src/smart_cmds/stack_cache.rs`: stage budget (`set_budget`, `insert`, `trim`),
  per-command `reuse_stages` scope.
- `crates/engine/src/prefs.rs`: hidden prefs `performance.smartSourceCacheMb` (1024) and
  `performance.smartStageCacheMb` (768), applied in `apply_prefs` like `effectCacheMb`.
- `crates/algo/src/warp.rs`: reduced (mip) levels for strong shrinks (`Level`, `source_step`),
  clipped warps, `reuse_levels` scope and `reduced_levels_built`.
- `crates/compose/src/lib.rs`: the effect cache is least-recently-used (`touch_effect_entry`).
- `crates/automation/src/{rpc.rs,headless.rs}`: `doc.replaceContents`, `doc.save` `pngCompression`.
- `docs/control-protocol.md`, `apps/photocraft-cli/src/lib.rs` (serve help), UI catalogs (labels of
  the two prefs), `docs/scorecard.md`.
- Tests: `crates/engine/src/smart_cmds/tests/replace_fit.rs`, warp tests in `crates/algo/src/warp.rs`,
  `crates/compose/src/tests.rs`, `crates/automation/src/rpc.rs`.

## Design notes

- Stretch: `S` takes the new box onto the old; a warp normalised over box `B` becomes the same
  mesh over `S⁻¹(B)` (presets first become their exact mesh); otherwise `H·S` / `T·S`. The old box
  is the `soLD` stored size (cheap) or the decoded old contents' box, read once for all instances
  before the new file is decoded.
- `clipToCanvas`: warped instances render inside the canvas and their layer mask's revealed area
  (plus 3 × feather + 1); instances with layer effects or visible smart filters render whole;
  hidden instances (or in a hidden group) are left unrendered and show nothing until re-rendered.
- A plain one-layer RGB 8/16-bit design already in the document's format is its own source
  image (transparent pixels zeroed, as the composite writes them); anything else is flattened
  in ~2 MP bands.
- Mip levels: picked from the short axis of the inverse Jacobian; one level per Replace Contents
  serves every instance. The cache entry holds the source's tile `Arc`s, so equal tile pointers
  mean equal pixels.
- Process-wide cache budgets come from preferences; tests that change them use per-thread
  overrides and the `SOURCE_CACHE_TESTS` lock.
