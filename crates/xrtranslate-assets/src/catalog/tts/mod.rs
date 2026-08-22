//! Native TTS model packages.

mod audio8;
mod openvoice;

pub use audio8::AUDIO8_TTS_ONNX_FP16;
pub use openvoice::{OPENVOICE_V2_ONNX_FP16, OPENVOICE_V2_ZH_ONNX_FP16, OPENVOICE_V3_ONNX_FP16};
