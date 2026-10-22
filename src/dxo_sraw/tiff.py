"""Minimal read-only TIFF/DNG structure parser.

Only what the splitter needs: classic (non-Big) TIFF, IFD entries with their
file positions, so values can be patched in place in a copied header.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass, field

# TIFF field type -> size in bytes of one value
TYPE_SIZES = {
    1: 1,  # BYTE
    2: 1,  # ASCII
    3: 2,  # SHORT
    4: 4,  # LONG
    5: 8,  # RATIONAL
    6: 1,  # SBYTE
    7: 1,  # UNDEFINED
    8: 2,  # SSHORT
    9: 4,  # SLONG
    10: 8,  # SRATIONAL
    11: 4,  # FLOAT
    12: 8,  # DOUBLE
    13: 4,  # IFD
}
SHORT, LONG, IFD_TYPE = 3, 4, 13

# Tags we care about
NEW_SUBFILE_TYPE = 0x00FE
IMAGE_WIDTH = 0x0100
IMAGE_LENGTH = 0x0101
BITS_PER_SAMPLE = 0x0102
COMPRESSION = 0x0103
PHOTOMETRIC = 0x0106
STRIP_OFFSETS = 0x0111
SAMPLES_PER_PIXEL = 0x0115
ROWS_PER_STRIP = 0x0116
STRIP_BYTE_COUNTS = 0x0117
PLANAR_CONFIG = 0x011C
TILE_WIDTH = 0x0142
TILE_LENGTH = 0x0143
TILE_OFFSETS = 0x0144
TILE_BYTE_COUNTS = 0x0145
SUB_IFDS = 0x014A
JPEG_IF_OFFSET = 0x0201
JPEG_IF_LENGTH = 0x0202
EXIF_IFD = 0x8769
GPS_IFD = 0x8825
INTEROP_IFD = 0xA005
IMAGE_UNIQUE_ID = 0xA420
DNG_VERSION = 0xC612
RAW_DATA_UNIQUE_ID = 0xC65D

PHOTOMETRIC_CFA = 32803
PHOTOMETRIC_LINEAR_RAW = 34892

# (offsets tag, byte-counts tag) pairs that point at image data
DATA_TAG_PAIRS = (
    (STRIP_OFFSETS, STRIP_BYTE_COUNTS),
    (TILE_OFFSETS, TILE_BYTE_COUNTS),
    (JPEG_IF_OFFSET, JPEG_IF_LENGTH),
)


class TiffError(ValueError):
    pass


@dataclass(frozen=True)
class Entry:
    tag: int
    type: int
    count: int
    pos: int  # file offset of the 12-byte entry
    value_pos: int  # file offset of the value bytes (inline or out-of-line)

    @property
    def size(self) -> int:
        return TYPE_SIZES.get(self.type, 1) * self.count

    @property
    def inline(self) -> bool:
        return self.size <= 4


@dataclass
class Ifd:
    offset: int
    entries: dict[int, Entry] = field(default_factory=dict)
    next_offset: int = 0

    @property
    def span(self) -> tuple[int, int]:
        """(start, length) of the IFD structure itself."""
        return self.offset, 2 + 12 * len(self.entries) + 4

    def __contains__(self, tag: int) -> bool:
        return tag in self.entries


class Tiff:
    def __init__(self, buf: bytes | memoryview) -> None:
        self.buf = memoryview(buf)
        order = bytes(self.buf[:2])
        if order == b"II":
            self.endian = "<"
        elif order == b"MM":
            self.endian = ">"
        else:
            raise TiffError("not a TIFF file (bad byte order mark)")
        if self.u16(2) != 42:
            raise TiffError("not a classic TIFF file (BigTIFF is unsupported)")
        self.ifd0 = self.read_ifd(self.u32(4))

    # -- primitive access ---------------------------------------------------
    def u16(self, pos: int) -> int:
        return struct.unpack_from(self.endian + "H", self.buf, pos)[0]

    def u32(self, pos: int) -> int:
        return struct.unpack_from(self.endian + "I", self.buf, pos)[0]

    def read_ifd(self, offset: int) -> Ifd:
        if not 8 <= offset < len(self.buf) - 2:
            raise TiffError(f"IFD offset {offset} out of range")
        n = self.u16(offset)
        ifd = Ifd(offset)
        for i in range(n):
            pos = offset + 2 + 12 * i
            tag, typ, count = struct.unpack_from(self.endian + "HHI", self.buf, pos)
            size = TYPE_SIZES.get(typ, 1) * count
            value_pos = pos + 8 if size <= 4 else self.u32(pos + 8)
            if value_pos + size > len(self.buf):
                raise TiffError(f"tag 0x{tag:04X} value out of range")
            ifd.entries[tag] = Entry(tag, typ, count, pos, value_pos)
        ifd.next_offset = self.u32(offset + 2 + 12 * n)
        return ifd

    # -- value decoding -----------------------------------------------------
    def ints(self, entry: Entry) -> list[int]:
        fmt = {1: "B", 3: "H", 4: "I", 6: "b", 8: "h", 9: "i", 13: "I", 7: "B"}.get(entry.type)
        if fmt is None:
            raise TiffError(f"tag 0x{entry.tag:04X} is not an integer type")
        return list(struct.unpack_from(f"{self.endian}{entry.count}{fmt}", self.buf, entry.value_pos))

    def get_ints(self, ifd: Ifd, tag: int) -> list[int] | None:
        e = ifd.entries.get(tag)
        return self.ints(e) if e else None

    def get_int(self, ifd: Ifd, tag: int, default: int | None = None) -> int | None:
        v = self.get_ints(ifd, tag)
        return v[0] if v else default

    def raw_value(self, entry: Entry) -> bytes:
        return bytes(self.buf[entry.value_pos : entry.value_pos + entry.size])

    def data_ranges(self, ifd: Ifd) -> list[tuple[int, int]]:
        """(offset, length) of every image-data block referenced by this IFD."""
        out: list[tuple[int, int]] = []
        for off_tag, len_tag in DATA_TAG_PAIRS:
            offs, lens = self.get_ints(ifd, off_tag), self.get_ints(ifd, len_tag)
            if offs is None or lens is None:
                continue
            if len(offs) != len(lens):
                raise TiffError(f"tag 0x{off_tag:04X}/0x{len_tag:04X} count mismatch")
            out.extend(zip(offs, lens, strict=True))
        return out
