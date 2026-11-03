use super::*;

#[test]
fn pauses_shorten_only_after_the_adaptive_onset_and_stay_fixed_during_silence() {
    // Frames are 32 ms. With a 4 s onset and an 8 s cap, 1 s and 3 s
    // utterances retain the full pause; only longer utterances shorten it.
    for (ordinary_pause, expected_pauses) in [
        (8, [8, 8, 8, 6, 5]),
        (23, [23, 23, 23, 14, 7]),
        (38, [38, 38, 38, 21, 9]),
    ] {
        for (speech_frames, expected_pause) in
            [32, 94, 125, 188, 235].into_iter().zip(expected_pauses)
        {
            let mut detector = EndpointDetector::new(EndpointConfig {
                silence_frames_to_finalize: ordinary_pause,
                adaptive_silence_after_frames: 125,
                adaptive_silence_frames_to_finalize: 4,
                max_active_frames: 250,
                ..Default::default()
            })
            .unwrap();
            for _ in 0..speech_frames {
                assert!(!matches!(
                    detector.push(&[1; FRAME_SAMPLES], 1.0).unwrap(),
                    EndpointEvent::Finalized(_)
                ));
            }
            for quiet_frames in 1..expected_pause {
                assert_eq!(
                    detector.push(&[0; FRAME_SAMPLES], 0.0).unwrap(),
                    EndpointEvent::SpeechContinues {
                        trailing_silence_frames: quiet_frames,
                    },
                    "speech={speech_frames}, pause={ordinary_pause}",
                );
            }
            let EndpointEvent::Finalized(utterance) =
                detector.push(&[0; FRAME_SAMPLES], 0.0).unwrap()
            else {
                panic!("speech={speech_frames}, pause={ordinary_pause} did not end on time");
            };
            assert_eq!(utterance.trailing_silence_frames, expected_pause);
            assert_eq!(
                utterance.end_reason,
                if expected_pause == ordinary_pause {
                    UtteranceEndReason::Silence
                } else {
                    UtteranceEndReason::AdaptiveSilence
                },
            );
        }
    }
}

#[test]
fn configured_duration_and_overlap_preserve_continuous_audio() {
    for (limit, overlap) in [(64, 2), (250, 8), (375, 10)] {
        let config = EndpointConfig {
            max_active_frames: limit,
            max_active_overlap_frames: overlap,
            adaptive_silence_after_frames: limit / 2,
            ..Default::default()
        };
        let mut detector = EndpointDetector::new(config).unwrap();
        let mut expected = Vec::new();
        let mut utterances = Vec::new();
        for index in 0..2 * limit + 7 {
            // Distinct frames detect shifted or incorrect overlap copies.
            let frame = [index as i16; FRAME_SAMPLES];
            expected.extend_from_slice(&frame);
            if let EndpointEvent::Finalized(utterance) = detector.push(&frame, 1.0).unwrap() {
                utterances.push(utterance);
            }
        }
        assert_eq!(utterances.len(), 2);
        assert!(utterances.iter().all(|utterance| {
            utterance.end_reason == UtteranceEndReason::MaxActiveFrames
                && utterance.frame_count() - utterance.overlap_frames == limit
        }));
        let tail = detector.flush().expect("retain the final partial window");
        assert_eq!(tail.end_reason, UtteranceEndReason::Flushed);
        assert_eq!(tail.frame_count() - tail.overlap_frames, 7);
        utterances.push(tail);

        let mut restored = Vec::new();
        for (index, utterance) in utterances.iter().enumerate() {
            let retained = utterance.overlap_frames * FRAME_SAMPLES;
            assert_eq!(
                utterance.overlap_frames,
                if index == 0 { 0 } else { overlap }
            );
            if index > 0 {
                let previous = &utterances[index - 1].samples;
                assert_eq!(
                    &utterance.samples[..retained],
                    &previous[previous.len() - retained..],
                );
            }
            restored.extend_from_slice(&utterance.samples[retained..]);
        }
        assert_eq!(restored, expected);
    }
}
