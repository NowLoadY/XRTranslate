//! Native TTS model packages.

mod audio8;
mod openvoice;

pub use audio8::AUDIO8_TTS_ONNX_FP16;
pub use openvoice::OPENVOICE_V3_ONNX_FP16;
