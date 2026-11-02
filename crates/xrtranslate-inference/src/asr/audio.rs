//! Capture packet boundaries are independent of a provider's PCM wire frames.

use futures_util::{Stream, StreamExt, stream};

use crate::InferenceError;

/// Coalesce or split PCM16 packets without padding, dropping the tail, or
/// changing capture cadence. A short final frame is emitted only at input EOF.
pub(super) fn pcm16_frames<S>(
    audio: S,
    frame_bytes: usize,
) -> impl Stream<Item = Result<Vec<u8>, InferenceError>> + Send
where
    S: Stream<Item = Vec<u8>> + Send,
{
    debug_assert!(frame_bytes > 0 && frame_bytes.is_multiple_of(2));
    stream::try_unfold(
        (Box::pin(audio.fuse()), Vec::with_capacity(frame_bytes)),
        move |(mut audio, mut pending)| async move {
            while pending.len() < frame_bytes {
                let Some(packet) = audio.next().await else {
                    break;
                };
                if !packet.len().is_multiple_of(2) {
                    return Err(InferenceError::InvalidAudio {
                        message: "PCM16 input length must be even".into(),
                    });
                }
                pending.extend_from_slice(&packet);
            }
            if pending.is_empty() {
                return Ok(None);
            }
            let count = pending.len().min(frame_bytes);
            let frame = pending.drain(..count).collect();
            Ok(Some((frame, (audio, pending))))
        },
    )
}
