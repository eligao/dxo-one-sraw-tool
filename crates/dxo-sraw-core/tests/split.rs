use dxo_sraw_core::tiff::{Tiff, tag};
use dxo_sraw_core::{Error, SuperRaw};

/// Check every output DNG: one raw SubIFD + preview, pixels == source frame, unique IDs.
fn check_split(buf: &[u8]) {
    let sr = SuperRaw::parse(buf).unwrap();
    let mut uids = Vec::new();
    for f in &sr.frames {
        let dng = sr.frame_dng(f).unwrap();
        let out = Tiff::new(&dng).unwrap();
        let subs = out.get_ints(&out.ifd0, tag::SUB_IFDS).unwrap().unwrap();
        assert_eq!(subs.len(), 1 + sr.other_subifds.len());
        let raw = out.read_ifd(subs[0] as usize).unwrap();
        let expected: Vec<u8> = f
            .data
            .iter()
            .flat_map(|&(o, n)| &buf[o..o + n])
            .copied()
            .collect();
        let got: Vec<u8> = out
            .data_ranges(&raw)
            .unwrap()
            .iter()
            .flat_map(|&(o, n)| &dng[o..o + n])
            .copied()
            .collect();
        assert_eq!(got, expected, "frame {} pixels", f.index);
        let exif = out
            .read_ifd(out.get_int(&out.ifd0, tag::EXIF_IFD).unwrap().unwrap() as usize)
            .unwrap();
        uids.push(
            out.bytes(exif.get(tag::IMAGE_UNIQUE_ID).unwrap().value_pos, 33)
                .unwrap()
                .to_vec(),
        );
    }
    uids.sort();
    uids.dedup();
    assert_eq!(
        uids.len(),
        sr.frames.len(),
        "ImageUniqueIDs must differ per frame"
    );
}

/// Simulate an Adobe-style metadata update (what Lightroom did to a user's file):
/// append an XMP packet + a new IFD0 (old entries + XMP tag) and repoint the header.
fn append_xmp(buf: &[u8], xmp: &[u8]) -> Vec<u8> {
    let tf = Tiff::new(buf).unwrap();
    let mut out = buf.to_vec();
    let xmp_pos = out.len() as u32;
    out.extend_from_slice(xmp);
    out.resize(out.len().next_multiple_of(2), 0);
    let ifd_pos = out.len() as u32;
    let mut entries: Vec<Vec<u8>> = tf
        .ifd0
        .entries
        .iter()
        .map(|e| buf[e.pos..e.pos + 12].to_vec())
        .collect();
    let mut x = 0x02BCu16.to_le_bytes().to_vec(); // XMP, BYTE[n]
    x.extend(1u16.to_le_bytes());
    x.extend((xmp.len() as u32).to_le_bytes());
    x.extend(xmp_pos.to_le_bytes());
    entries.push(x);
    entries.sort_by_key(|e| u16::from_le_bytes([e[0], e[1]]));
    out.extend((entries.len() as u16).to_le_bytes());
    entries.iter().for_each(|e| out.extend(e));
    out.extend((tf.ifd0.next_offset as u32).to_le_bytes());
    out[4..8].copy_from_slice(&ifd_pos.to_le_bytes());
    out
}

/// Split a file with appended metadata; outputs must keep the XMP and the right pixels.
fn check_appended_metadata(buf: &[u8]) {
    let xmp = b"<x:xmpmeta xmlns:x='adobe:ns:meta/'><rdf:RDF/></x:xmpmeta>";
    let edited = append_xmp(buf, xmp);
    check_split(&edited);
    let sr = SuperRaw::parse(&edited).unwrap();
    for f in &sr.frames {
        let dng = sr.frame_dng(f).unwrap();
        let out = Tiff::new(&dng).unwrap();
        let e = out.ifd0.get(0x02BC).expect("XMP tag kept");
        assert_eq!(out.bytes(e.value_pos, e.size).unwrap(), xmp);
    }
}

#[test]
fn synthetic() {
    // 4-frame mini SuperRAW (thumbnail IFD0, raw+preview+3 raw SubIFDs, Exif ImageUniqueID)
    let buf = include_bytes!("data/synthetic.dxo");
    assert_eq!(SuperRaw::parse(buf).unwrap().frames.len(), 4);
    check_split(buf);
    check_appended_metadata(buf);
}

#[test]
fn rejects_non_tiff_and_truncated() {
    assert!(matches!(
        SuperRaw::parse(b"\xff\xd8\xff\xe0 jpeg"),
        Err(Error::NotTiff)
    ));
    let buf = include_bytes!("data/synthetic.dxo");
    for len in [8, 64, 300, buf.len() - 1] {
        assert!(SuperRaw::parse(&buf[..len]).is_err(), "truncated to {len}");
    }
}

#[test]
fn real_samples() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets");
    for name in ["DXO_0202.DXO", "DXO_0203.DXO"] {
        match std::fs::read(format!("{dir}/{name}")) {
            Ok(buf) => {
                check_split(&buf);
                check_appended_metadata(&buf);
            }
            Err(_) => eprintln!("skipping {name}: sample not present"),
        }
    }
}
