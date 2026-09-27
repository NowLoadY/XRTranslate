pub use xrtranslate_engine::audio::PCM16_MONO_16KHZ_FORMAT;

pub fn pcm16_mono_16khz_to_wav(pcm: &[u8]) -> Result<Vec<u8>, crate::InferenceError> {
    xrtranslate_engine::audio::pcm16_mono_16khz_to_wav(pcm)
        .map_err(|message| crate::InferenceError::InvalidAudio { message })
}
