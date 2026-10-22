//! Minimal read-only TIFF/DNG structure parser.
//!
//! Only what the splitter needs: classic (non-Big) TIFF, IFD entries with their
//! file positions, so values can be patched in place in a copied header.
//! Every read is bounds-checked; malformed input yields [`Error`], never a panic.

use crate::Error;

pub const SHORT: u16 = 3;
pub const LONG: u16 = 4;
pub const IFD_TYPE: u16 = 13;

pub mod tag {
    pub const NEW_SUBFILE_TYPE: u16 = 0x00FE;
    pub const IMAGE_WIDTH: u16 = 0x0100;
    pub const IMAGE_LENGTH: u16 = 0x0101;
    pub const BITS_PER_SAMPLE: u16 = 0x0102;
    pub const COMPRESSION: u16 = 0x0103;
    pub const PHOTOMETRIC: u16 = 0x0106;
    pub const STRIP_OFFSETS: u16 = 0x0111;
    pub const SAMPLES_PER_PIXEL: u16 = 0x0115;
    pub const ROWS_PER_STRIP: u16 = 0x0116;
    pub const STRIP_BYTE_COUNTS: u16 = 0x0117;
    pub const PLANAR_CONFIG: u16 = 0x011C;
    pub const TILE_WIDTH: u16 = 0x0142;
    pub const TILE_LENGTH: u16 = 0x0143;
    pub const TILE_OFFSETS: u16 = 0x0144;
    pub const TILE_BYTE_COUNTS: u16 = 0x0145;
    pub const SUB_IFDS: u16 = 0x014A;
    pub const JPEG_IF_OFFSET: u16 = 0x0201;
    pub const JPEG_IF_LENGTH: u16 = 0x0202;
    pub const EXIF_IFD: u16 = 0x8769;
    pub const GPS_IFD: u16 = 0x8825;
    pub const INTEROP_IFD: u16 = 0xA005;
    pub const IMAGE_UNIQUE_ID: u16 = 0xA420;
    pub const DNG_VERSION: u16 = 0xC612;
    pub const RAW_DATA_UNIQUE_ID: u16 = 0xC65D;
}

pub const PHOTOMETRIC_CFA: u32 = 32803;
pub const PHOTOMETRIC_LINEAR_RAW: u32 = 34892;

/// (offsets tag, byte-counts tag) pairs that point at image data.
pub const DATA_TAG_PAIRS: [(u16, u16); 3] = [
    (tag::STRIP_OFFSETS, tag::STRIP_BYTE_COUNTS),
    (tag::TILE_OFFSETS, tag::TILE_BYTE_COUNTS),
    (tag::JPEG_IF_OFFSET, tag::JPEG_IF_LENGTH),
];

/// Size in bytes of one value of a TIFF field type.
#[must_use]
pub fn type_size(typ: u16) -> usize {
    match typ {
        3 | 8 => 2,
        4 | 9 | 11 | 13 => 4,
        5 | 10 | 12 => 8,
        _ => 1, // BYTE, ASCII, SBYTE, UNDEFINED and unknown types
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub tag: u16,
    pub typ: u16,
    pub count: u32,
    /// File offset of the 12-byte entry.
    pub pos: usize,
    /// File offset of the value bytes (inline or out-of-line).
    pub value_pos: usize,
    /// Total size of the value in bytes.
    pub size: usize,
}

impl Entry {
    #[must_use]
    pub fn is_inline(&self) -> bool {
        self.size <= 4
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ifd {
    pub offset: usize,
    pub entries: Vec<Entry>,
    pub next_offset: usize,
}

impl Ifd {
    #[must_use]
    pub fn get(&self, tag: u16) -> Option<&Entry> {
        self.entries.iter().find(|e| e.tag == tag)
    }

    #[must_use]
    pub fn contains(&self, tag: u16) -> bool {
        self.get(tag).is_some()
    }

    /// `(start, length)` of the IFD structure itself.
    #[must_use]
    pub fn span(&self) -> (usize, usize) {
        (self.offset, 2 + 12 * self.entries.len() + 4)
    }
}

#[derive(Debug)]
pub struct Tiff<'a> {
    buf: &'a [u8],
    little_endian: bool,
    pub ifd0: Ifd,
}

impl<'a> Tiff<'a> {
    pub fn new(buf: &'a [u8]) -> Result<Self, Error> {
        let little_endian = match buf.get(..2) {
            Some(b"II") => true,
            Some(b"MM") => false,
            _ => return Err(Error::NotTiff),
        };
        let mut tiff = Tiff {
            buf,
            little_endian,
            ifd0: Ifd {
                offset: 0,
                entries: vec![],
                next_offset: 0,
            },
        };
        match tiff.u16_at(2)? {
            42 => {}
            43 => return Err(Error::BigTiff),
            _ => return Err(Error::NotTiff),
        }
        tiff.ifd0 = tiff.read_ifd(tiff.u32_at(4)? as usize)?;
        Ok(tiff)
    }

    #[must_use]
    pub fn buf(&self) -> &'a [u8] {
        self.buf
    }

    #[must_use]
    pub fn is_little_endian(&self) -> bool {
        self.little_endian
    }

    /// Bounds-checked slice of the file.
    pub fn bytes(&self, pos: usize, len: usize) -> Result<&'a [u8], Error> {
        pos.checked_add(len)
            .and_then(|end| self.buf.get(pos..end))
            .ok_or_else(|| {
                Error::Malformed(format!("read of {len} bytes at {pos} is out of range"))
            })
    }

    pub fn u16_at(&self, pos: usize) -> Result<u16, Error> {
        let b: [u8; 2] = self.bytes(pos, 2)?.try_into().expect("length checked");
        Ok(if self.little_endian {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }

    pub fn u32_at(&self, pos: usize) -> Result<u32, Error> {
        let b: [u8; 4] = self.bytes(pos, 4)?.try_into().expect("length checked");
        Ok(if self.little_endian {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }

    pub fn read_ifd(&self, offset: usize) -> Result<Ifd, Error> {
        if offset < 8 {
            return Err(Error::Malformed(format!(
                "IFD offset {offset} out of range"
            )));
        }
        let n = usize::from(self.u16_at(offset)?);
        let mut entries = Vec::with_capacity(n);
        for i in 0..n {
            let pos = offset + 2 + 12 * i;
            let tag = self.u16_at(pos)?;
            let typ = self.u16_at(pos + 2)?;
            let count = self.u32_at(pos + 4)?;
            let size = usize::try_from(count)
                .ok()
                .and_then(|c| c.checked_mul(type_size(typ)))
                .ok_or_else(|| Error::Malformed(format!("tag 0x{tag:04X} value too large")))?;
            let value_pos = if size <= 4 {
                pos + 8
            } else {
                self.u32_at(pos + 8)? as usize
            };
            self.bytes(value_pos, size)
                .map_err(|_| Error::Malformed(format!("tag 0x{tag:04X} value out of range")))?;
            entries.push(Entry {
                tag,
                typ,
                count,
                pos,
                value_pos,
                size,
            });
        }
        let next_offset = self.u32_at(offset + 2 + 12 * n)? as usize;
        Ok(Ifd {
            offset,
            entries,
            next_offset,
        })
    }

    /// Decode an unsigned integer-typed entry (BYTE, SHORT, LONG, UNDEFINED, IFD).
    pub fn ints(&self, entry: &Entry) -> Result<Vec<u32>, Error> {
        let raw = self.bytes(entry.value_pos, entry.size)?;
        match entry.typ {
            1 | 7 => Ok(raw.iter().map(|&b| u32::from(b)).collect()),
            3 => Ok(raw
                .chunks_exact(2)
                .map(|c| {
                    let b = [c[0], c[1]];
                    u32::from(if self.little_endian {
                        u16::from_le_bytes(b)
                    } else {
                        u16::from_be_bytes(b)
                    })
                })
                .collect()),
            4 | 13 => Ok(raw
                .chunks_exact(4)
                .map(|c| {
                    let b = [c[0], c[1], c[2], c[3]];
                    if self.little_endian {
                        u32::from_le_bytes(b)
                    } else {
                        u32::from_be_bytes(b)
                    }
                })
                .collect()),
            t => Err(Error::Malformed(format!(
                "tag 0x{:04X} has non-integer type {t}",
                entry.tag
            ))),
        }
    }

    pub fn get_ints(&self, ifd: &Ifd, tag: u16) -> Result<Option<Vec<u32>>, Error> {
        ifd.get(tag).map(|e| self.ints(e)).transpose()
    }

    pub fn get_int(&self, ifd: &Ifd, tag: u16) -> Result<Option<u32>, Error> {
        Ok(self.get_ints(ifd, tag)?.and_then(|v| v.first().copied()))
    }

    /// `(offset, length)` of every image-data block referenced by this IFD.
    pub fn data_ranges(&self, ifd: &Ifd) -> Result<Vec<(usize, usize)>, Error> {
        let mut out = Vec::new();
        for (off_tag, len_tag) in DATA_TAG_PAIRS {
            let (Some(offs), Some(lens)) =
                (self.get_ints(ifd, off_tag)?, self.get_ints(ifd, len_tag)?)
            else {
                continue;
            };
            if offs.len() != lens.len() {
                return Err(Error::Malformed(format!(
                    "tag 0x{off_tag:04X}/0x{len_tag:04X} count mismatch"
                )));
            }
            out.extend(
                offs.into_iter()
                    .zip(lens)
                    .map(|(o, l)| (o as usize, l as usize)),
            );
        }
        Ok(out)
    }
}
