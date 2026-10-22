from __future__ import annotations

import struct
from pathlib import Path

import pytest

from dxo_sraw import SuperRawError, parse, split_file
from dxo_sraw import tiff as t

SAMPLE = Path(__file__).parent.parent / "assets" / "DXO_0202.DXO"


class _TiffBuilder:
    """Tiny little-endian TIFF writer: IFDs + out-of-line values, then pixel data."""

    def __init__(self) -> None:
        self.ifds: list[list[tuple[int, int, list[int] | bytes]]] = []

    def add(self, entries: list[tuple[int, int, list[int] | bytes]]) -> int:
        self.ifds.append(entries)
        return len(self.ifds) - 1

    def build(self, link: dict[tuple[int, int], list[int]], pixels: list[bytes]) -> bytes:
        """`link[(ifd, tag)]` = indices of IFDs (or pixel blocks for StripOffsets)."""
        # pass 1: compute IFD positions
        pos, ifd_pos = 8, []
        for entries in self.ifds:
            ifd_pos.append(pos)
            pos += 2 + 12 * len(entries) + 4
        ext_start = pos
        for entries in self.ifds:
            for _, typ, val in entries:
                n = len(val) * (1 if isinstance(val, bytes) else t.TYPE_SIZES[typ])
                if n > 4:
                    pos += n
        pix_pos = []
        for p in pixels:
            pix_pos.append(pos)
            pos += len(p)

        out = bytearray(b"II*\0" + struct.pack("<I", ifd_pos[0]))
        ext = bytearray()
        for i, entries in enumerate(self.ifds):
            out += struct.pack("<H", len(entries))
            for tag, typ, val in sorted(entries):
                if (i, tag) in link:
                    targets = link[(i, tag)]
                    val = (
                        [pix_pos[j] for j in targets]
                        if tag == t.STRIP_OFFSETS
                        else [ifd_pos[j] for j in targets]
                    )
                if isinstance(val, bytes):
                    raw, count = val, len(val)
                else:
                    fmt = {1: "B", 3: "H", 4: "I", 13: "I"}[typ]
                    raw, count = struct.pack(f"<{len(val)}{fmt}", *val), len(val)
                if len(raw) <= 4:
                    out += struct.pack("<HHI", tag, typ, count) + raw.ljust(4, b"\0")
                else:
                    out += struct.pack("<HHII", tag, typ, count, ext_start + len(ext))
                    ext += raw
            out += struct.pack("<I", 0)
        assert len(out) == ext_start
        return bytes(out + ext + b"".join(pixels))


W, H = 8, 4


def _raw_ifd(full: bool) -> list:
    e = [
        (t.NEW_SUBFILE_TYPE, 4, [0]),
        (t.IMAGE_WIDTH, 4, [W]),
        (t.IMAGE_LENGTH, 4, [H]),
        (t.BITS_PER_SAMPLE, 3, [8]),
        (t.COMPRESSION, 3, [1]),
        (t.PHOTOMETRIC, 3, [t.PHOTOMETRIC_CFA]),
        (t.STRIP_OFFSETS, 4, [0]),
        (t.STRIP_BYTE_COUNTS, 4, [W * H]),
    ]
    if full:
        e += [(0xC61D, 3, [255]), (0xC618, 3, list(range(256)))]  # WhiteLevel, LinearizationTable
    return e


def make_superraw(n_frames: int = 4, uid: bytes = b"ABCDEF0123456789ABCDEF0123456780\0") -> bytes:
    b = _TiffBuilder()
    ifd0 = b.add(
        [
            (t.NEW_SUBFILE_TYPE, 4, [1]),
            (t.DNG_VERSION, 1, [1, 4, 0, 0]),
            (t.SUB_IFDS, 4, [0] * (n_frames + 1)),
            (t.EXIF_IFD, 4, [0]),
            (t.RAW_DATA_UNIQUE_ID, 1, list(range(16))),
        ]
    )
    raw0 = b.add(_raw_ifd(full=True))
    preview = b.add([(t.NEW_SUBFILE_TYPE, 4, [1]), (t.PHOTOMETRIC, 3, [6])])
    others = [b.add(_raw_ifd(full=False)) for _ in range(n_frames - 1)]
    exif = b.add([(t.IMAGE_UNIQUE_ID, 2, uid)])
    link = {
        (ifd0, t.SUB_IFDS): [raw0, preview, *others],
        (ifd0, t.EXIF_IFD): [exif],
        (raw0, t.STRIP_OFFSETS): [0],
        **{(o, t.STRIP_OFFSETS): [k + 1] for k, o in enumerate(others)},
    }
    pixels = [bytes([k * 10 + 1]) * (W * H) for k in range(n_frames)]
    return b.build(link, pixels)


def _check_output(data: bytes, frame_index: int, expected_pixels: bytes) -> None:
    sr_tiff = t.Tiff(data)
    subs = sr_tiff.get_ints(sr_tiff.ifd0, t.SUB_IFDS)
    assert subs is not None and len(subs) == 2
    raw = sr_tiff.read_ifd(subs[0])
    assert 0xC618 in raw  # primary raw tags carried over
    [(off, n)] = sr_tiff.data_ranges(raw)
    assert data[off : off + n] == expected_pixels
    assert off + n == len(data)


def test_parse_synthetic():
    sr = parse(make_superraw())
    assert [f.index for f in sr.frames] == [1, 2, 3, 4]
    assert len(sr.other_subifds) == 1
    assert sr.width == W and sr.height == H


def test_split_synthetic(tmp_path: Path):
    src = tmp_path / "X.DXO"
    src.write_bytes(make_superraw())
    outs = split_file(src, tmp_path / "out")
    assert [p.name for p in outs] == ["X_1.dng", "X_2.dng", "X_3.dng", "X_4.dng"]
    uids, rids = set(), set()
    for k, p in enumerate(outs):
        data = p.read_bytes()
        _check_output(data, k + 1, bytes([k * 10 + 1]) * (W * H))
        tf = t.Tiff(data)
        exif = tf.read_ifd(tf.get_int(tf.ifd0, t.EXIF_IFD))
        uids.add(tf.raw_value(exif.entries[t.IMAGE_UNIQUE_ID]))
        rids.add(tf.raw_value(tf.ifd0.entries[t.RAW_DATA_UNIQUE_ID]))
    assert len(uids) == 4 and len(rids) == 4


def test_split_refuses_overwrite(tmp_path: Path):
    src = tmp_path / "X.DXO"
    src.write_bytes(make_superraw())
    split_file(src, tmp_path)
    with pytest.raises(FileExistsError):
        split_file(src, tmp_path)
    split_file(src, tmp_path, overwrite=True)


def test_two_frames_subifds_still_array():
    # 2 frames + preview -> output keeps [raw, preview]; must stay a valid array
    sr = parse(make_superraw(n_frames=2))
    assert len(sr.frames) == 2


def test_single_frame_rejected():
    with pytest.raises(SuperRawError, match="not a multi-frame"):
        parse(make_superraw(n_frames=1))


def test_not_tiff():
    with pytest.raises(t.TiffError):
        parse(b"\xff\xd8\xff\xe0 not a tiff")


@pytest.mark.skipif(not SAMPLE.exists(), reason="sample SuperRAW not present")
def test_split_real_sample(tmp_path: Path):
    src_bytes = SAMPLE.read_bytes()
    sr = parse(src_bytes)
    assert len(sr.frames) == 4
    outs = split_file(SAMPLE, tmp_path)
    for frame, p in zip(sr.frames, outs, strict=True):
        [(off, n)] = frame.data
        _check_output(p.read_bytes(), frame.index, src_bytes[off : off + n])
