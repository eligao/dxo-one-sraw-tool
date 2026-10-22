# dxo-sraw

Split **DxO ONE SuperRAW** (`.DXO`) files into one standard DNG per captured frame, so the
frames can be merged in Lightroom (Photo Merge → HDR), Photoshop, or any other stacking tool.

## Why

In SuperRAW mode the DxO ONE captures 4 frames at the same exposure and stores them in a
single DNG 1.4 file. Lightroom and most other raw tools only read the first frame.

## Usage

```sh
cargo build --release
target/release/dxo-sraw info  assets/DXO_0202.DXO
target/release/dxo-sraw split assets/DXO_0202.DXO            # -> assets/DXO_0202_1.dng .. _4.dng
target/release/dxo-sraw split -o out/ --force /card/DCIM/*.DXO
```

## Layout

- `crates/dxo-sraw-core` — parser + splitter. No file I/O (bytes in, bytes out), builds for
  `wasm32-unknown-unknown`.
- `crates/dxo-sraw` — the CLI.
- `crates/dxo-sraw-wasm` — wasm-bindgen wrapper (`info(bytes)`, `split(bytes) -> Uint8Array[]`).
- `web/` — Vite + React + TypeScript page: drop `.DXO` files, download the DNGs (or a zip).
  The split runs in a Web Worker; files never leave the browser.

## Web app

```sh
cd web
pnpm install
pnpm dev       # builds the wasm package (wasm-pack) then starts Vite
pnpm build     # static site in web/dist/, deployable anywhere (relative paths)
```

## File format (as observed, firmware V3.2.0)

```
IFD0        160x120 JPEG thumbnail + all DNG color tags, Exif IFD (with MakerNote)
 SubIFD0    frame 1: 5632x3710 8-bit CFA, LinearizationTable (8->12 bit), BlackLevel,
            ActiveArea, MaskedAreas, OpcodeList2 (GainMap lens shading), ...
 SubIFD1    2736x1824 JPEG preview
 SubIFD2-4  frames 2-4: same geometry, only bare strip tags
[metadata header ends] [frame 1 pixels][frame 2][frame 3][frame 4]
```

## How the split works

For frame *k*, the metadata header is copied byte for byte (so the MakerNote and its
absolute offsets stay valid), followed by frame *k*'s pixel data where frame 1's used to
be. Then these values are patched in place:

- the primary raw IFD's StripOffsets / StripByteCounts → frame *k*
- IFD0 SubIFDs → `[raw, preview]`, so frames 2–4 are no longer referenced
- Exif ImageUniqueID / RawDataUniqueID → a different value per frame, so catalog apps don't
  treat the outputs as duplicates

Before writing anything, the layout is validated (all metadata before the pixel data, every
frame matching frame 1's geometry). Files that don't fit are rejected and nothing is written.
Raw pixels are copied untouched.

## Development

```sh
cargo test     # synthetic fixture, plus assets/DXO_0202.DXO / DXO_0203.DXO when present
cargo clippy --all-targets && cargo fmt --check
```
