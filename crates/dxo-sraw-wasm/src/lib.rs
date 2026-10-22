use dxo_sraw_core::SuperRaw;
use js_sys::{Array, Uint8Array};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct Info {
    pub frames: u32,
    pub width: u32,
    pub height: u32,
    pub bits: u32,
}

#[wasm_bindgen]
pub fn info(bytes: &[u8]) -> Result<Info, JsError> {
    let sr = SuperRaw::parse(bytes)?;
    Ok(Info {
        frames: sr.frames.len() as u32,
        width: sr.width(),
        height: sr.height(),
        bits: sr.bits_per_sample(),
    })
}

/// One DNG (`Uint8Array`) per frame, in capture order.
#[wasm_bindgen]
pub fn split(bytes: &[u8]) -> Result<Array, JsError> {
    let sr = SuperRaw::parse(bytes)?;
    sr.frames
        .iter()
        .map(|f| Ok(JsValue::from(Uint8Array::from(sr.frame_dng(f)?.as_slice()))))
        .collect()
}
