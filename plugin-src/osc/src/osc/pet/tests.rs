use super::*;
use std::collections::BTreeSet;

#[test]
fn quiet_pet_blinks_naps_and_wakes_to_listen_then_acknowledges_speech() {
    let start = Instant::now();
    let mut pet = Pet::default();
    let settings = PetSettings::default();
    let frames = (0..60)
        .map(|tick| {
            pet.frame(
                settings,
                Activity::default(),
                start + REFRESH_INTERVAL * tick,
                0,
            )
            .full
        })
        .collect::<BTreeSet<_>>();
    assert!(
        frames.len() >= 8,
        "Idle life should include more than a two-frame loop"
    );
    pet.frame(
        settings,
        Activity::default(),
        start + Duration::from_secs(75),
        0,
    );
    assert_eq!(pet.scene, Scene::Sleepy);
    let listening = Activity {
        speaking: true,
        ..Default::default()
    };
    pet.frame(settings, listening, start + Duration::from_secs(76), 0);
    assert_eq!(pet.scene, Scene::Waking);
    pet.frame(settings, listening, start + Duration::from_secs(79), 0);
    assert_eq!(pet.scene, Scene::Listening);
    let finished = Activity {
        latest_message: Some(start + Duration::from_secs(80)),
        ..Default::default()
    };
    pet.frame(settings, finished, start + Duration::from_secs(80), 0);
    assert_eq!(pet.scene, Scene::Happy);
    pet.frame(settings, finished, start + Duration::from_secs(88), 0);
    assert_eq!(
        pet.scene,
        Scene::Relaxed,
        "The same history entry must not retrigger happiness"
    );
}

#[test]
fn music_and_selected_moods_have_distinct_animated_personalities() {
    let now = Instant::now();
    let activity = Activity {
        music: true,
        ..Default::default()
    };
    let mut pet = Pet::default();
    pet.frame(PetSettings::default(), activity, now, 0);
    assert_eq!(pet.scene, Scene::Dancing);
    for (mood, expected) in [
        (PetMood::Happy, Scene::Happy),
        (PetMood::Shy, Scene::Shy),
        (PetMood::Sleepy, Scene::Sleepy),
    ] {
        let settings = PetSettings { mood };
        let first = pet.frame(settings, activity, now, 0).full;
        let frames = (1..12)
            .map(|tick| {
                pet.frame(settings, activity, now + REFRESH_INTERVAL * tick, 0)
                    .full
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(pet.scene, expected);
        assert!(frames.iter().any(|frame| frame != &first));
    }
}

#[test]
fn all_art_fits_a_single_line_and_small_budgets_use_whole_faces() {
    for scene in [
        Scene::Relaxed,
        Scene::Curious,
        Scene::Grooming,
        Scene::Stretching,
        Scene::Listening,
        Scene::Happy,
        Scene::Shy,
        Scene::Sleepy,
        Scene::Dancing,
        Scene::Waking,
    ] {
        for tick in 0..16 {
            let frame = poses::frame(scene, REFRESH_INTERVAL * tick);
            assert!(frame.full.chars().count() <= MAX_WIDTH);
            assert!(!frame.full.contains(['\n', '\r']));
            for budget in 0..=MAX_WIDTH {
                let text = frame.fit(budget);
                assert!(text.chars().count() <= budget);
                assert!(
                    text.is_empty() || [&frame.full, &frame.compact, &frame.tiny].contains(&&text)
                );
                assert_eq!(text.is_empty(), budget < 3);
            }
        }
    }
}
