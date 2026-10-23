# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

Splits DxO ONE SuperRAW (`.DXO`) files — one DNG holding 4 equal-exposure raw frames — into one
standard DNG per frame, so Lightroom etc. can merge them. Ships as a Rust CLI and an in-browser
page (wasm) deployed to https://eligao.github.io/dxo-one-sraw-tool/.

## Commands

```sh
cargo build --release                          # CLI -> target/release/dxo-sraw
cargo test                                     # all tests
cargo test -p dxo-sraw-core --test split synthetic   # single test (tests/split.rs)
cargo clippy --all-targets && cargo fmt --check

cd web
pnpm install          # pnpm 12 is pinned via "packageManager"
pnpm dev              # runs wasm-pack first (pnpm wasm), then Vite
pnpm build            # wasm-pack + tsc + vite -> web/dist
pnpm lint             # oxlint
pnpm fmt              # oxfmt (fmt:check in CI style)
```

`web/src/wasm/` is wasm-pack output (self-gitignored); regenerate with `pnpm wasm`, never edit it.

## Architecture

Cargo workspace (`crates/*`) + `web/`:

- `dxo-sraw-core` — all format logic. **No file I/O**: `SuperRaw::parse(&[u8])` and
  `frame_dng(&Frame) -> Vec<u8>`, so the same code runs natively and in wasm32. Keep it that way.
- `dxo-sraw` — clap CLI (`info`, `split [-o DIR] [-f]`), thin wrapper over core.
- `dxo-sraw-wasm` — wasm-bindgen wrapper (`info`, `split`). `wasm-opt` is disabled in its
  Cargo.toml on purpose (57 KB module; avoids requiring binaryen).
- `web/` — Vite + React + TS. `worker.ts` runs the wasm split in a Web Worker, serialized one file
  at a time to bound memory; `App.tsx` handles drop/download/zip (fflate, store-only).

### How the split works (superraw.rs)

A SuperRAW file: IFD0 (thumbnail + color tags + Exif/MakerNote) with SubIFDs
`[raw frame 1 (all DNG raw tags), JPEG preview, raw frames 2..4 (bare strip tags only)]`.
Frames 2–4 borrow frame 1's raw-processing tags.

Files are **patched, not rebuilt**. Output for frame k =
`[bytes before first frame][frame k pixels][bytes after last frame]`, then in place:
primary raw IFD strip offsets/counts → frame k; SubIFDs → `[primary, previews]`; every pointer
into the trailing region shifted; ImageUniqueID / RawDataUniqueID XOR'd with the frame index.

Constraints that drive this design:

- The DxO **MakerNote uses absolute file offsets**, so it must never move. A file where another
  app relocated it (e.g. an exiftool rewrite) is rejected rather than written broken.
- **Lightroom appends** XMP + a new IFD0 after the pixel data when it writes metadata into a
  file; the trailing-region handling exists for that (see `append_xmp` in tests/split.rs).
- `tiff.rs` is our own bounds-checked parser because patching needs each entry's file position
  (the `tiff` crate doesn't expose it). All reads must stay bounds-checked: untrusted input in
  the browser, and `usize` is 32-bit on wasm32 (use checked arithmetic for sizes).
- Max 15 frames, because the per-frame unique ID is XOR'd into one hex digit.

### Tests

`crates/dxo-sraw-core/tests/split.rs` checks that output pixels equal the source frame, that the
SubIFD layout is right and that IDs are unique, plus the Lightroom-appended case. It uses the 1.2 KB
`tests/data/synthetic.dxo` fixture, and also the real samples `assets/DXO_0202.DXO` /
`DXO_0203.DXO` when present (gitignored, ~85 MB each, not in CI). A change to the split
should keep camera-original outputs byte-identical unless a behavior change is intended.

## Deployment

Push to `master` → `.github/workflows/pages.yml` (cargo test, wasm-pack, `pnpm/setup` +
`pnpm build`, deploy `web/dist`). The `github-pages` environment only allows `master` to deploy.
Vite `base: './'` keeps the build working under the `/dxo-one-sraw-tool/` subpath.

## Scope decisions

- No frame-merging command: merging is left to Lightroom (Photo Merge → HDR). An earlier
  HDR+-based plan (hdrplus-python, AGPL) was dropped; don't reintroduce AGPL code — the project
  is MIT.
