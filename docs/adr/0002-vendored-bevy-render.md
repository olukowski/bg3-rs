# ADR 0002 — Vendored, patched `bevy_render`

Status: accepted · 2026-10-24

## Context

On macOS the viewer's frame is limited by Bevy's render thread on the CPU, not by the GPU
(`docs/performance.md`, "Where the Mac frame goes"). Every multidrawable batch set costs three
`queue.write_buffer` calls per view per frame. On Metal, wgpu backs each call with a freshly
allocated staging buffer that it frees after submission. On the Nautiloid that made about 2,100
Metal buffer allocations and frees per frame, roughly a quarter of the render thread's time.

The batch sets are this numerous because Bevy 0.20 caps a bindless slab at
`AUTO_BINDLESS_SLAB_RESOURCE_LIMIT` = **64 resources on macOS/iOS** (2048 elsewhere). A level
material binds ~8 textures, so a slab holds ~8 materials and the level's materials split into
dozens of bind groups, i.e. batch sets, in every shadow view. Our materials can't raise the limit
themselves. A `#[bindless(limit(N))]` on the extension wins in the slab allocator, but
`ExtendedMaterial` keeps `StandardMaterial`'s 64-entry binding arrays in the combined layout, and
wgpu rejects the bind groups.

Alternatives considered:

- **Upstream change** (a configurable limit, or `ExtendedMaterial` sizing the shared arrays from
  the combined limit): the right long-term fix, but not available for 0.20.
- **Our own base materials** instead of `StandardMaterial`: much more code, and we'd lose the
  PBR pipeline integration the extensions rely on.

## Decision

Vendor `bevy_render` 0.20.0 from crates.io into `vendor/bevy_render`, unchanged except for one
line: the macOS/iOS `AUTO_BINDLESS_SLAB_RESOURCE_LIMIT` is 512 instead of 64 (marked
`bg3-rs patch`). The root `Cargo.toml` points `[patch.crates-io]` at it and excludes `vendor/`
from the workspace, so `cargo fmt` and clippy leave it alone. The crate keeps its MIT/Apache-2.0
licence files.

## Consequences

- Nautiloid at 1600×900 on an M5 Pro: shadow batch sets 633 → 250, main view 66 → 30. Frames:
  combat 17.1 → 14.5 ms, wide 19.2 → 17.6 ms, start pod 14.6–15.0 → 13.2–13.4 ms. Images are
  unchanged. Other platforms are unaffected (the constant only applies to macOS/iOS).
- Bevy chose 64 because of Metal's sampler limits
  (<https://github.com/bevyengine/bevy/issues/16988>). On Apple silicon with tier 2 argument
  buffers, 512 passes wgpu's validation and Bevy's own `bindless_supported` check. Older Macs
  without tier 2 already fall back to non-bindless materials (`BG3_NO_BINDLESS`), so they don't
  depend on this limit.
- **Every Bevy upgrade** must re-vendor `bevy_render` at the new version and re-apply the patch,
  or drop it if upstream has fixed the limit. To re-vendor, copy the crate from
  `~/.cargo/registry/src/*/bevy_render-<version>`, delete `.cargo_vcs_info.json`, `.cargo-ok` and
  `Cargo.lock`, and re-apply the one-line change. `diff -r` against the registry copy must show
  only that change.
- Upstreaming the fix is the way to retire this ADR.
