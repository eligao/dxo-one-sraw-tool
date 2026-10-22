"""Split a DxO ONE SuperRAW (.DXO) file into one DNG per captured frame.

A SuperRAW file is a DNG 1.4 whose IFD0 (thumbnail) has several SubIFDs:

    SubIFD0   raw frame #1 - carries every DNG raw tag (linearization, black
              level, active area, gain-map opcodes, ...)
    SubIFD1   JPEG preview
    SubIFD2.. raw frames #2..#4 - only bare strip/geometry tags; they share
              frame #1's raw-processing tags.

All metadata (IFDs, MakerNote, previews) sits before the first raw frame's
pixel data. Lightroom and friends only read the first raw SubIFD.

Strategy for frame *k*: copy the metadata header verbatim (so MakerNote and
any other absolute offsets stay valid), append frame *k*'s pixel data right
after it, then patch in place:

  * the primary raw IFD's strip offsets/byte counts -> frame *k*'s data,
  * IFD0's SubIFDs list -> [primary raw IFD, non-raw SubIFDs...],
  * ImageUniqueID / RawDataUniqueID -> made unique per frame so catalog
    software does not treat the outputs as duplicates.

The unreferenced IFDs of the other frames stay in the header as dead bytes
(a few hundred bytes), which is harmless.
"""

from __future__ import annotations

import contextlib
import struct
from dataclasses import dataclass
from pathlib import Path

from . import tiff as t

# Tags that must match between a secondary frame and the primary frame for
# the primary's raw-processing tags to be valid for it.
GEOMETRY_TAGS = (
    t.IMAGE_WIDTH,
    t.IMAGE_LENGTH,
    t.BITS_PER_SAMPLE,
    t.COMPRESSION,
    t.PHOTOMETRIC,
    t.SAMPLES_PER_PIXEL,
    t.PLANAR_CONFIG,
    t.ROWS_PER_STRIP,
    t.TILE_WIDTH,
    t.TILE_LENGTH,
)


class SuperRawError(ValueError):
    pass


@dataclass(frozen=True)
class Frame:
    index: int  # 1-based capture order
    ifd: t.Ifd
    data: list[tuple[int, int]]  # (offset, length) blocks, in tag order

    @property
    def nbytes(self) -> int:
        return sum(n for _, n in self.data)


@dataclass
class SuperRaw:
    tiff: t.Tiff
    subifds_entry: t.Entry
    frames: list[Frame]
    other_subifds: list[int]  # offsets of non-raw SubIFDs (previews), in order
    data_start: int  # end of the metadata header == start of the first frame

    @property
    def primary(self) -> Frame:
        return self.frames[0]

    @property
    def width(self) -> int:
        return self.tiff.get_int(self.primary.ifd, t.IMAGE_WIDTH, 0) or 0

    @property
    def height(self) -> int:
        return self.tiff.get_int(self.primary.ifd, t.IMAGE_LENGTH, 0) or 0


def _is_raw(tf: t.Tiff, ifd: t.Ifd) -> bool:
    return tf.get_int(ifd, t.NEW_SUBFILE_TYPE, 0) == 0 and tf.get_int(ifd, t.PHOTOMETRIC) in (
        t.PHOTOMETRIC_CFA,
        t.PHOTOMETRIC_LINEAR_RAW,
    )


def _data_offsets_tag(ifd: t.Ifd) -> tuple[int, int]:
    for pair in ((t.STRIP_OFFSETS, t.STRIP_BYTE_COUNTS), (t.TILE_OFFSETS, t.TILE_BYTE_COUNTS)):
        if pair[0] in ifd and pair[1] in ifd:
            return pair
    raise SuperRawError(f"raw IFD at {ifd.offset} has no strip/tile data")


def _metadata_ranges(tf: t.Tiff, ifds: list[t.Ifd], raw_ifds: set[int]) -> list[tuple[int, int]]:
    """Every byte range referenced by metadata (not raw pixel data)."""
    ranges: list[tuple[int, int]] = []
    for ifd in ifds:
        ranges.append(ifd.span)
        data_tags = {tag for pair in t.DATA_TAG_PAIRS for tag in pair}
        for e in ifd.entries.values():
            if not e.inline:
                ranges.append((e.value_pos, e.size))
        if ifd.offset not in raw_ifds:
            ranges.extend(tf.data_ranges(ifd))  # thumbnails / previews
        elif data_tags.isdisjoint(ifd.entries):
            raise SuperRawError(f"raw IFD at {ifd.offset} has no image data")
    return ranges


def parse(buf: bytes | memoryview) -> SuperRaw:
    tf = t.Tiff(buf)
    ifd0 = tf.ifd0
    if t.DNG_VERSION not in ifd0:
        raise SuperRawError("not a DNG file (no DNGVersion tag)")
    sub_entry = ifd0.entries.get(t.SUB_IFDS)
    if sub_entry is None:
        raise SuperRawError("no SubIFDs - not a SuperRAW file")

    sub_offsets = tf.ints(sub_entry)
    subifds = [tf.read_ifd(off) for off in sub_offsets]
    raw = [ifd for ifd in subifds if _is_raw(tf, ifd)]
    if len(raw) < 2:
        raise SuperRawError(f"found {len(raw)} raw frame(s) - not a multi-frame SuperRAW file")
    others = [ifd.offset for ifd in subifds if not _is_raw(tf, ifd)]

    primary = raw[0]
    for ifd in raw[1:]:
        for tag in GEOMETRY_TAGS:
            a, b = tf.get_ints(primary, tag), tf.get_ints(ifd, tag)
            if b is not None and a != b:
                raise SuperRawError(
                    f"frame IFD at {ifd.offset}: tag 0x{tag:04X} = {b} differs from primary {a}"
                )
        if _data_offsets_tag(ifd) != _data_offsets_tag(primary):
            raise SuperRawError("frames mix strips and tiles")
        if len(tf.data_ranges(ifd)) != len(tf.data_ranges(primary)):
            raise SuperRawError("frames have different strip/tile counts")

    frames = [Frame(i + 1, ifd, tf.data_ranges(ifd)) for i, ifd in enumerate(raw)]
    data_start = min(off for f in frames for off, _ in f.data)

    # Validate the layout assumption: all metadata lives before the pixel data.
    ifds = [ifd0, *subifds]
    for tag in (t.EXIF_IFD, t.GPS_IFD):
        off = tf.get_int(ifd0, tag)
        if off:
            ifds.append(tf.read_ifd(off))
    for ifd in list(ifds):
        off = tf.get_int(ifd, t.INTEROP_IFD)
        if off:
            ifds.append(tf.read_ifd(off))
    if ifd0.next_offset:
        ifds.append(tf.read_ifd(ifd0.next_offset))

    meta = _metadata_ranges(tf, ifds, {f.ifd.offset for f in frames})
    bad = [r for r in meta if r[0] + r[1] > data_start]
    if bad:
        raise SuperRawError(
            f"unsupported layout: metadata at {bad[0][0]} lies after raw data start {data_start}"
        )
    for f in frames:
        for off, n in f.data:
            if off + n > len(tf.buf):
                raise SuperRawError(f"frame {f.index} data is truncated")

    return SuperRaw(tf, sub_entry, frames, others, data_start)


# -- writing -----------------------------------------------------------------


def _pack_ints(endian: str, typ: int, values: list[int]) -> bytes:
    fmt = {t.SHORT: "H", t.LONG: "I", t.IFD_TYPE: "I"}.get(typ)
    if fmt is None:
        raise SuperRawError(f"cannot write integer values as TIFF type {typ}")
    try:
        return struct.pack(f"{endian}{len(values)}{fmt}", *values)
    except struct.error as exc:
        raise SuperRawError(f"value does not fit TIFF type {typ}: {exc}") from exc


def build_header(sr: SuperRaw, frame: Frame) -> bytearray:
    """Patched copy of the metadata header for `frame`."""
    tf = sr.tiff
    tf_endian = tf.endian
    hdr = bytearray(tf.buf[: sr.data_start])

    # 1) primary raw IFD -> frame's pixel data, laid out right after the header
    off_tag, len_tag = _data_offsets_tag(sr.primary.ifd)
    offs, pos = [], sr.data_start
    for _, n in frame.data:
        offs.append(pos)
        pos += n
    for tag, values in ((off_tag, offs), (len_tag, [n for _, n in frame.data])):
        entry = sr.primary.ifd.entries[tag]
        packed = _pack_ints(tf_endian, entry.type, values)
        hdr[entry.value_pos : entry.value_pos + len(packed)] = packed

    # 2) SubIFDs -> [primary raw, previews...]
    new_subs = [sr.primary.ifd.offset, *sr.other_subifds]
    se = sr.subifds_entry
    struct.pack_into(tf_endian + "I", hdr, se.pos + 4, len(new_subs))
    packed = _pack_ints(tf_endian, se.type, new_subs)
    if len(packed) <= 4:  # now fits inline in the entry
        hdr[se.pos + 8 : se.pos + 12] = packed.ljust(4, b"\0")
    else:
        hdr[se.value_pos : se.value_pos + len(packed)] = packed

    # 3) unique IDs, so DAMs don't dedupe the frames against each other / the source
    exif_off = tf.get_int(tf.ifd0, t.EXIF_IFD)
    if exif_off:
        uid = tf.read_ifd(exif_off).entries.get(t.IMAGE_UNIQUE_ID)
        if uid and uid.count >= 2:
            last = uid.value_pos + uid.count - 2  # char before the NUL
            with contextlib.suppress(ValueError):  # not a hex ID: leave it
                hdr[last] = ord(f"{(int(chr(hdr[last]), 16) ^ frame.index) & 0xF:X}")
    rid = tf.ifd0.entries.get(t.RAW_DATA_UNIQUE_ID)
    if rid and rid.count:
        p = rid.value_pos + rid.count - 1
        hdr[p] ^= frame.index

    return hdr


def write_frame(sr: SuperRaw, frame: Frame, out: Path) -> None:
    hdr = build_header(sr, frame)
    with out.open("wb") as fh:
        fh.write(hdr)
        for off, n in frame.data:
            fh.write(sr.tiff.buf[off : off + n])


def split_file(src: Path, out_dir: Path, *, overwrite: bool = False) -> list[Path]:
    """Split `src` into `<out_dir>/<stem>_<k>.dng`, one per frame."""
    sr = parse(src.read_bytes())
    targets = [out_dir / f"{src.stem}_{f.index}.dng" for f in sr.frames]
    if not overwrite:
        existing = [p for p in targets if p.exists()]
        if existing:
            raise FileExistsError(f"{existing[0]} exists (use --force to overwrite)")
    out_dir.mkdir(parents=True, exist_ok=True)
    for frame, target in zip(sr.frames, targets, strict=True):
        write_frame(sr, frame, target)
    return targets
