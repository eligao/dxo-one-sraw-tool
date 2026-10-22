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
    /// IFDs referenced by the output (IFD0, primary raw, previews, Exif, ...).
    meta_ifds: Vec<Ifd>,
    /// Start of the first frame's pixel data (end of the leading metadata).
    pub data_start: usize,
    /// End of the last frame's pixel data (start of any trailing metadata).
    pub tail_start: usize,
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
        let tail_start = frames
            .iter()
            .flat_map(|f| f.data.iter().map(|&(o, n)| o + n))
            .max()
            .unwrap_or(data_start);
        for f in &frames {
            for &(off, n) in &f.data {
                tf.bytes(off, n).map_err(|_| {
                    Error::Malformed(format!("frame {} data is truncated", f.index))
                })?;
            }
        }

        // Metadata IFDs that stay referenced in the output. Metadata may live before the
        // pixel data (camera original) or after it (appended by e.g. Adobe software).
        let mut meta_ifds = vec![ifd0.clone(), frames[0].ifd.clone()];
        meta_ifds.extend(
            subifds
                .into_iter()
                .filter(|i| other_subifds.contains(&(i.offset as u32))),
        );
        for t in [tag::EXIF_IFD, tag::GPS_IFD] {
            if let Some(off) = tf.get_int(ifd0, t)?.filter(|&o| o != 0) {
                meta_ifds.push(tf.read_ifd(off as usize)?);
            }
        }
        let interop: Vec<u32> = meta_ifds
            .iter()
            .map(|ifd| tf.get_int(ifd, tag::INTEROP_IFD))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .filter(|&o| o != 0)
            .collect();
        for off in interop {
            meta_ifds.push(tf.read_ifd(off as usize)?);
        }
        if ifd0.next_offset != 0 {
            meta_ifds.push(tf.read_ifd(ifd0.next_offset)?);
        }

        let primary_offset = frames[0].ifd.offset;
        for ifd in &meta_ifds {
            let mut ranges = vec![ifd.span()];
            ranges.extend(
                ifd.entries
                    .iter()
                    .filter(|e| !e.is_inline())
                    .map(|e| (e.value_pos, e.size)),
            );
            if ifd.offset != primary_offset {
                ranges.extend(tf.data_ranges(ifd)?); // thumbnails / previews
            }
            let in_head = |pos: usize, len: usize| pos + len <= data_start;
            if let Some(&(pos, _)) = ranges
                .iter()
                .find(|&&(pos, len)| !in_head(pos, len) && pos < tail_start)
            {
                return Err(Error::Unsupported(format!(
                    "metadata at {pos} lies inside the raw frame data"
                )));
            }
            // MakerNote internals use absolute offsets: it must not move.
            if let Some(mn) = ifd
                .get(tag::MAKER_NOTE)
                .filter(|e| !in_head(e.value_pos, e.size))
            {
                return Err(Error::Unsupported(format!(
                    "MakerNote was moved by another app (now at {}); its internal offsets can't be fixed",
                    mn.value_pos
                )));
            }
        }

        Ok(SuperRaw {
            tiff: tf,
            subifds_entry,
            frames,
            other_subifds,
            meta_ifds,
            data_start,
            tail_start,
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

    /// The complete DNG for `frame`:
    /// `[leading metadata][frame pixels][trailing metadata]`, with pointers patched.
    pub fn frame_dng(&self, frame: &Frame) -> Result<Vec<u8>, Error> {
        let tf = &self.tiff;
        let buf = tf.buf();
        let new_tail = self.data_start + frame.nbytes();
        let mut out =
            Vec::with_capacity(self.data_start + frame.nbytes() + buf.len() - self.tail_start);
        out.extend_from_slice(&buf[..self.data_start]);
        for &(off, n) in &frame.data {
            out.extend_from_slice(&buf[off..off + n]);
        }
        out.extend_from_slice(&buf[self.tail_start..]);

        // Where a byte of the original file ended up in `out` (metadata is head or tail only).
        let moved = |p: usize| {
            if p >= self.tail_start {
                p - self.tail_start + new_tail
            } else {
                p
            }
        };
        let put = |out: &mut Vec<u8>, pos: usize, bytes: &[u8]| {
            let p = moved(pos);
            out[p..p + bytes.len()].copy_from_slice(bytes);
        };
        let offset_tags = [tag::STRIP_OFFSETS, tag::TILE_OFFSETS, tag::JPEG_IF_OFFSET];
        let pointer_tags = [tag::EXIF_IFD, tag::GPS_IFD, tag::INTEROP_IFD];

        // 1) pointers into the trailing metadata shift with it
        put(
            &mut out,
            4,
            &self.pack_ints(tiff::LONG, &[moved(tf.ifd0.offset)])?,
        );
        for ifd in &self.meta_ifds {
            let next_pos = ifd.offset + 2 + 12 * ifd.entries.len();
            put(
                &mut out,
                next_pos,
                &self.pack_ints(tiff::LONG, &[moved(ifd.next_offset)])?,
            );
            for e in &ifd.entries {
                if !e.is_inline() {
                    put(
                        &mut out,
                        e.pos + 8,
                        &self.pack_ints(tiff::LONG, &[moved(e.value_pos)])?,
                    );
                }
                let is_pointer = pointer_tags.contains(&e.tag)
                    || (offset_tags.contains(&e.tag) && ifd.offset != self.primary().ifd.offset);
                if is_pointer {
                    let vals: Vec<usize> =
                        tf.ints(e)?.into_iter().map(|v| moved(v as usize)).collect();
                    put(&mut out, e.value_pos, &self.pack_ints(e.typ, &vals)?);
                }
            }
        }

        // 2) primary raw IFD -> frame's pixel data, laid out right after the leading metadata
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
            put(
                &mut out,
                entry.value_pos,
                &self.pack_ints(entry.typ, values)?,
            );
        }

        // 3) SubIFDs -> [primary raw, previews...]
        let se = &self.subifds_entry;
        let new_subs: Vec<usize> = std::iter::once(self.primary().ifd.offset)
            .chain(self.other_subifds.iter().map(|&o| o as usize))
            .map(moved)
            .collect();
        put(
            &mut out,
            se.pos + 4,
            &self.pack_ints(tiff::LONG, &[new_subs.len()])?,
        );
        let mut packed = self.pack_ints(se.typ, &new_subs)?;
        if packed.len() <= 4 {
            // now fits inline in the entry
            packed.resize(4, 0);
            put(&mut out, se.pos + 8, &packed);
        } else {
            put(&mut out, se.value_pos, &packed);
        }

        // 4) unique IDs, so DAMs don't dedupe frames against each other / the source
        if let Some(exif_off) = tf.get_int(&tf.ifd0, tag::EXIF_IFD)?.filter(|&o| o != 0) {
            let exif = tf.read_ifd(exif_off as usize)?;
            if let Some(uid) = exif.get(tag::IMAGE_UNIQUE_ID).filter(|e| e.size >= 2) {
                let last = moved(uid.value_pos + uid.size - 2); // char before the NUL
                if let Some(d) = char::from(out[last]).to_digit(16) {
                    let x = (d ^ u32::from(frame.index)) & 0xF;
                    out[last] = char::from_digit(x, 16).expect("< 16").to_ascii_uppercase() as u8;
                }
            }
        }
        if let Some(rid) = tf.ifd0.get(tag::RAW_DATA_UNIQUE_ID).filter(|e| e.size > 0) {
            out[moved(rid.value_pos + rid.size - 1)] ^= frame.index;
        }

        Ok(out)
    }
}
