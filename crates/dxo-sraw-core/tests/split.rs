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

#[test]
fn synthetic() {
    // 4-frame mini SuperRAW (thumbnail IFD0, raw+preview+3 raw SubIFDs, Exif ImageUniqueID)
    let buf = include_bytes!("data/synthetic.dxo");
    assert_eq!(SuperRaw::parse(buf).unwrap().frames.len(), 4);
    check_split(buf);
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
            Ok(buf) => check_split(&buf),
            Err(_) => eprintln!("skipping {name}: sample not present"),
        }
    }
}
