//! Split a DxO ONE SuperRAW (.DXO) file into one DNG per captured frame.
//!
//! A SuperRAW file is a DNG 1.4 whose IFD0 (thumbnail) has several SubIFDs:
//!
//! ```text
//! SubIFD0   raw frame #1 - carries every DNG raw tag (linearization, black
//!           level, active area, gain-map opcodes, ...)
//! SubIFD1   JPEG preview
//! SubIFD2.. raw frames #2..#4 - only bare strip/geometry tags; they share
//!           frame #1's raw-processing tags.
//! ```
//!
//! All metadata (IFDs, MakerNote, previews) sits before the first raw frame's
//! pixel data. Lightroom and friends only read the first raw SubIFD.
//!
//! For frame *k*: copy the metadata header verbatim (so the MakerNote and any
//! other absolute offsets stay valid), append frame *k*'s pixel data right
//! after it, then patch in place:
//!
//! * the primary raw IFD's strip offsets/byte counts → frame *k*'s data,
//! * IFD0's SubIFDs list → `[primary raw IFD, non-raw SubIFDs...]`,
//! * ImageUniqueID / RawDataUniqueID → unique per frame, so catalog software
//!   does not treat the outputs as duplicates.
//!
//! The unreferenced IFDs of the other frames stay in the header as dead bytes
//! (a few hundred bytes), which is harmless.

use crate::Error;
use crate::tiff::{self, Ifd, Tiff, tag};

/// Tags that must match between a secondary frame and the primary frame for
/// the primary's raw-processing tags to be valid for it.
const GEOMETRY_TAGS: [u16; 10] = [
    tag::IMAGE_WIDTH,
    tag::IMAGE_LENGTH,
    tag::BITS_PER_SAMPLE,
    tag::COMPRESSION,
    tag::PHOTOMETRIC,
    tag::SAMPLES_PER_PIXEL,
    tag::PLANAR_CONFIG,
    tag::ROWS_PER_STRIP,
    tag::TILE_WIDTH,
    tag::TILE_LENGTH,
];

#[derive(Debug, Clone)]
pub struct Frame {
    /// 1-based capture order.
    pub index: u8,
    pub ifd: Ifd,
    /// `(offset, length)` pixel-data blocks, in tag order.
    pub data: Vec<(usize, usize)>,
}

impl Frame {
    #[must_use]
    pub fn nbytes(&self) -> usize {
        self.data.iter().map(|&(_, n)| n).sum()
    }
}

#[derive(Debug)]
pub struct SuperRaw<'a> {
    pub tiff: Tiff<'a>,
    subifds_entry: tiff::Entry,
    pub frames: Vec<Frame>,
    /// Offsets of non-raw SubIFDs (previews), in order.
    pub other_subifds: Vec<u32>,
    /// End of the metadata header == start of the first frame's pixel data.
    pub data_start: usize,
}

fn is_raw(tf: &Tiff, ifd: &Ifd) -> Result<bool, Error> {
    Ok(tf.get_int(ifd, tag::NEW_SUBFILE_TYPE)?.unwrap_or(0) == 0
        && matches!(
            tf.get_int(ifd, tag::PHOTOMETRIC)?,
            Some(tiff::PHOTOMETRIC_CFA | tiff::PHOTOMETRIC_LINEAR_RAW)
        ))
}

fn data_tags(ifd: &Ifd) -> Result<(u16, u16), Error> {
    [
        (tag::STRIP_OFFSETS, tag::STRIP_BYTE_COUNTS),
        (tag::TILE_OFFSETS, tag::TILE_BYTE_COUNTS),
    ]
    .into_iter()
    .find(|&(o, l)| ifd.contains(o) && ifd.contains(l))
    .ok_or_else(|| Error::Unsupported(format!("raw IFD at {} has no strip/tile data", ifd.offset)))
}

impl<'a> SuperRaw<'a> {
    /// Parse and validate a SuperRAW file.
    pub fn parse(buf: &'a [u8]) -> Result<Self, Error> {
        let tf = Tiff::new(buf)?;
        let ifd0 = &tf.ifd0;
        if !ifd0.contains(tag::DNG_VERSION) {
            return Err(Error::NotDng);
        }
        let subifds_entry = *ifd0
            .get(tag::SUB_IFDS)
            .ok_or_else(|| Error::NotSuperRaw("no SubIFDs".into()))?;

        let mut raw = Vec::new();
        let mut other_subifds = Vec::new();
        let mut subifds = Vec::new();
        for off in tf.ints(&subifds_entry)? {
            let ifd = tf.read_ifd(off as usize)?;
            if is_raw(&tf, &ifd)? {
                raw.push(ifd.clone());
            } else {
                other_subifds.push(off);
            }
            subifds.push(ifd);
        }
        if raw.len() < 2 {
            return Err(Error::NotSuperRaw(format!(
                "found {} raw frame(s), expected several",
                raw.len()
            )));
        }
        if raw.len() > 15 {
            return Err(Error::Unsupported(format!(
                "{} raw frames (at most 15 supported)",
                raw.len()
            )));
        }

        let primary = &raw[0];
        let primary_ranges = tf.data_ranges(primary)?;
        for ifd in &raw[1..] {
            for t in GEOMETRY_TAGS {
                let (a, b) = (tf.get_ints(primary, t)?, tf.get_ints(ifd, t)?);
                if b.is_some() && a != b {
                    return Err(Error::Unsupported(format!(
                        "frame IFD at {}: tag 0x{t:04X} = {b:?} differs from primary {a:?}",
                        ifd.offset
                    )));
                }
            }
            if data_tags(ifd)? != data_tags(primary)? {
                return Err(Error::Unsupported("frames mix strips and tiles".into()));
            }
            if tf.data_ranges(ifd)?.len() != primary_ranges.len() {
                return Err(Error::Unsupported(
                    "frames have different strip/tile counts".into(),
                ));
            }
        }

        let frames = raw
            .into_iter()
            .enumerate()
            .map(|(i, ifd)| {
                Ok(Frame {
                    index: i as u8 + 1,
                    data: tf.data_ranges(&ifd)?,
                    ifd,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        let data_start = frames
            .iter()
            .flat_map(|f| f.data.iter().map(|&(o, _)| o))
            .min()
            .ok_or_else(|| Error::Unsupported("raw frames have no data".into()))?;

        // Validate the layout assumption: all metadata lives before the pixel data.
        let mut ifds: Vec<Ifd> = std::iter::once(ifd0.clone()).chain(subifds).collect();
        for t in [tag::EXIF_IFD, tag::GPS_IFD] {
            if let Some(off) = tf.get_int(ifd0, t)?.filter(|&o| o != 0) {
                ifds.push(tf.read_ifd(off as usize)?);
            }
        }
        let interop: Vec<u32> = ifds
            .iter()
            .map(|ifd| tf.get_int(ifd, tag::INTEROP_IFD))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .filter(|&o| o != 0)
            .collect();
        for off in interop {
            ifds.push(tf.read_ifd(off as usize)?);
        }
        if ifd0.next_offset != 0 {
            ifds.push(tf.read_ifd(ifd0.next_offset)?);
        }

        let raw_offsets: Vec<usize> = frames.iter().map(|f| f.ifd.offset).collect();
        for ifd in &ifds {
            let mut ranges = vec![ifd.span()];
            ranges.extend(
                ifd.entries
                    .iter()
                    .filter(|e| !e.is_inline())
                    .map(|e| (e.value_pos, e.size)),
            );
            if !raw_offsets.contains(&ifd.offset) {
                ranges.extend(tf.data_ranges(ifd)?); // thumbnails / previews
            }
            if let Some(&(pos, _)) = ranges.iter().find(|&&(pos, len)| pos + len > data_start) {
                return Err(Error::Unsupported(format!(
                    "metadata at {pos} lies after raw data start {data_start}"
                )));
            }
        }
        for f in &frames {
            for &(off, n) in &f.data {
                tf.bytes(off, n).map_err(|_| {
                    Error::Malformed(format!("frame {} data is truncated", f.index))
                })?;
            }
        }

        Ok(SuperRaw {
            tiff: tf,
            subifds_entry,
            frames,
            other_subifds,
            data_start,
        })
    }

    #[must_use]
    pub fn primary(&self) -> &Frame {
        &self.frames[0]
    }

    fn primary_int(&self, t: u16) -> u32 {
        self.tiff
            .get_int(&self.primary().ifd, t)
            .ok()
            .flatten()
            .unwrap_or(0)
    }

    #[must_use]
    pub fn width(&self) -> u32 {
        self.primary_int(tag::IMAGE_WIDTH)
    }

    #[must_use]
    pub fn height(&self) -> u32 {
        self.primary_int(tag::IMAGE_LENGTH)
    }

    #[must_use]
    pub fn bits_per_sample(&self) -> u32 {
        self.primary_int(tag::BITS_PER_SAMPLE)
    }

    fn pack_ints(&self, typ: u16, values: &[usize]) -> Result<Vec<u8>, Error> {
        let le = self.tiff.is_little_endian();
        let mut out = Vec::with_capacity(values.len() * 4);
        for &v in values {
            match typ {
                tiff::SHORT => {
                    let v = u16::try_from(v)
                        .map_err(|_| Error::Unsupported(format!("{v} does not fit SHORT")))?;
                    out.extend(if le { v.to_le_bytes() } else { v.to_be_bytes() });
                }
                tiff::LONG | tiff::IFD_TYPE => {
                    let v = u32::try_from(v)
                        .map_err(|_| Error::Unsupported(format!("{v} does not fit LONG")))?;
                    out.extend(if le { v.to_le_bytes() } else { v.to_be_bytes() });
                }
                t => {
                    return Err(Error::Unsupported(format!(
                        "cannot write integers as TIFF type {t}"
                    )));
                }
            }
        }
        Ok(out)
    }

    /// Patched copy of the metadata header for `frame`.
    pub fn build_header(&self, frame: &Frame) -> Result<Vec<u8>, Error> {
        let tf = &self.tiff;
        let mut hdr = tf.buf()[..self.data_start].to_vec();

        // 1) primary raw IFD -> frame's pixel data, laid out right after the header
        let (off_tag, len_tag) = data_tags(&self.primary().ifd)?;
        let mut offsets = Vec::with_capacity(frame.data.len());
        let mut pos = self.data_start;
        for &(_, n) in &frame.data {
            offsets.push(pos);
            pos += n;
        }
        let counts: Vec<usize> = frame.data.iter().map(|&(_, n)| n).collect();
        for (t, values) in [(off_tag, &offsets), (len_tag, &counts)] {
            let entry = self.primary().ifd.get(t).expect("checked by data_tags");
            let packed = self.pack_ints(entry.typ, values)?;
            hdr[entry.value_pos..entry.value_pos + packed.len()].copy_from_slice(&packed);
        }

        // 2) SubIFDs -> [primary raw, previews...]
        let se = &self.subifds_entry;
        let new_subs: Vec<usize> = std::iter::once(self.primary().ifd.offset)
            .chain(self.other_subifds.iter().map(|&o| o as usize))
            .collect();
        let count = self.pack_ints(tiff::LONG, &[new_subs.len()])?;
        hdr[se.pos + 4..se.pos + 8].copy_from_slice(&count);
        let mut packed = self.pack_ints(se.typ, &new_subs)?;
        if packed.len() <= 4 {
            // now fits inline in the entry
            packed.resize(4, 0);
            hdr[se.pos + 8..se.pos + 12].copy_from_slice(&packed);
        } else {
            hdr[se.value_pos..se.value_pos + packed.len()].copy_from_slice(&packed);
        }

        // 3) unique IDs, so DAMs don't dedupe frames against each other / the source
        if let Some(exif_off) = tf.get_int(&tf.ifd0, tag::EXIF_IFD)?.filter(|&o| o != 0) {
            let exif = tf.read_ifd(exif_off as usize)?;
            if let Some(uid) = exif.get(tag::IMAGE_UNIQUE_ID).filter(|e| e.size >= 2) {
                let last = uid.value_pos + uid.size - 2; // char before the NUL
                if let Some(d) = char::from(hdr[last]).to_digit(16) {
                    let x = (d ^ u32::from(frame.index)) & 0xF;
                    hdr[last] = char::from_digit(x, 16).expect("< 16").to_ascii_uppercase() as u8;
                }
            }
        }
        if let Some(rid) = tf.ifd0.get(tag::RAW_DATA_UNIQUE_ID).filter(|e| e.size > 0) {
            hdr[rid.value_pos + rid.size - 1] ^= frame.index;
        }

        Ok(hdr)
    }

    /// The complete DNG for `frame`.
    pub fn frame_dng(&self, frame: &Frame) -> Result<Vec<u8>, Error> {
        let mut out = self.build_header(frame)?;
        out.reserve(frame.nbytes());
        for &(off, n) in &frame.data {
            out.extend_from_slice(&self.tiff.buf()[off..off + n]);
        }
        Ok(out)
    }
}
