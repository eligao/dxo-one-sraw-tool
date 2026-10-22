//! Parse DxO ONE SuperRAW (.DXO) files and split them into one DNG per frame.
//!
//! The library does no file I/O: it works on byte slices, so the same code runs
//! natively and in WebAssembly.
//!
//! ```no_run
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let buf = std::fs::read("DXO_0202.DXO")?;
//! let sr = dxo_sraw_core::SuperRaw::parse(&buf)?;
//! for frame in &sr.frames {
//!     let dng = sr.frame_dng(frame)?;
//!     std::fs::write(format!("DXO_0202_{}.dng", frame.index), dng)?;
//! }
//! # Ok(())
//! # }
//! ```

mod superraw;
pub mod tiff;

pub use superraw::{Frame, SuperRaw};

#[derive(Debug)]
pub enum Error {
    NotTiff,
    BigTiff,
    NotDng,
    NotSuperRaw(String),
    Unsupported(String),
    Malformed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotTiff => f.write_str("not a TIFF file"),
            Error::BigTiff => f.write_str("BigTIFF is not supported"),
            Error::NotDng => f.write_str("not a DNG file (no DNGVersion tag)"),
            Error::NotSuperRaw(m) => write!(f, "not a SuperRAW file: {m}"),
            Error::Unsupported(m) => write!(f, "unsupported layout: {m}"),
            Error::Malformed(m) => write!(f, "malformed file: {m}"),
        }
    }
}

impl std::error::Error for Error {}
