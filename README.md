# dxo-sraw

Split **DxO ONE SuperRAW** (`.DXO`) files into one standard DNG per captured frame, so the
frames can be merged in Lightroom, Photoshop, or any other stacking / HDR tool.

## Why

In SuperRAW mode the DxO ONE captures 4 frames at the same exposure and stores them in a
single DNG 1.4 file. Lightroom and most other raw tools only read the first frame. Until now,
merging all four needed DxO Connect or PhotoLab.

## Usage

```sh
uv run dxo-sraw info  assets/DXO_0202.DXO
uv run dxo-sraw split assets/DXO_0202.DXO              # -> assets/DXO_0202_1.dng .. _4.dng
uv run dxo-sraw split -o out/ --force /card/DCIM/*.DXO
```

Or install it as a tool: `uv tool install .` then `dxo-sraw split ...`.

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

For frame *k*, the tool copies the metadata header byte for byte (so the MakerNote and its
absolute offsets stay valid), then writes frame *k*'s pixel data where frame 1's used to be.
Then it patches these values in place:

- the primary raw IFD's StripOffsets and StripByteCounts → frame *k*
- IFD0 SubIFDs → `[raw, preview]`, so frames 2–4 are no longer referenced
- Exif ImageUniqueID / RawDataUniqueID → a different value for each frame, so catalog apps
  don't treat the outputs as duplicates

Before writing anything, the tool checks that all metadata lies before the pixel data and
that every frame matches frame 1's geometry. If a file doesn't fit that layout, the tool
reports an error and writes nothing.

Each output is ~22 MB, and its raw pixels are identical to the source frame (no
re-encoding).

## Merging the frames

The frames have the same exposure but there is handheld shift between them (a few px), so
use a tool that aligns them first:

- **Photoshop**: *File → Scripts → Load Files into Stack* (tick *Attempt to Automatically
  Align*), then *Layer → Smart Objects → Stack Mode → Mean* or *Median* for noise reduction.
- **Lightroom**: *Photo Merge → HDR* produces a DNG and auto-aligns. (It is designed for
  bracketed exposures; how well it handles equal exposures has not been verified yet.)

## Development

```sh
uv sync
uv run pytest        # synthetic tests + a real-file test if assets/DXO_0202.DXO exists
uv run ruff check . && uv run ruff format .
```

Runtime has zero dependencies (stdlib only).

## Roadmap

- `merge`: align and average the frames into a single 16-bit DNG. This is the noise
  reduction SuperRAW was designed for, without needing DxO software.
- Native rewrite (Rust/Go) once the format handling is stable.
