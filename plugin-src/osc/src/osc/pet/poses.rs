//! Small, deliberately paced gestures. Keep art separate from state transitions.
use super::{PetFrame, Scene};
use std::time::Duration;

struct Pose {
    hold_ms: u64,
    face: &'static str,
    left: &'static str,
    right: &'static str,
}

const fn p(hold_ms: u64, face: &'static str, left: &'static str, right: &'static str) -> Pose {
    Pose {
        hold_ms,
        face,
        left,
        right,
    }
}

pub(super) fn frame(scene: Scene, elapsed: Duration) -> PetFrame {
    let poses: &[Pose] = match scene {
        Scene::Relaxed => &[
            p(4800, "･ω･", "", ""),
            p(1200, "-ω-", "", ""),
            p(3600, "･ω･", "", ""),
            p(1200, "･ω<", "", "~"),
        ],
        Scene::Curious => &[
            p(2400, "･ω･", "", "?"),
            p(2400, "･ω･", "?", ""),
            p(1200, "oωo", "", "!"),
            p(3600, "･ω･", "", ""),
        ],
        Scene::Grooming => &[
            p(2400, "-ω-", "ฅ", ""),
            p(1200, "･ω-", "ฅ", ""),
            p(2400, "-ω-", "", "ฅ"),
            p(3600, "^ω^", "", "~"),
        ],
        Scene::Stretching => &[
            p(2400, "-ω-", "", ""),
            p(2400, ">ω<", "ヽ", "ﾉ"),
            p(1200, "^ω^", "ฅ", "ฅ"),
            p(3600, "-ω-", "", "~"),
        ],
        Scene::Listening => &[
            p(3600, "oωo", "", "?"),
            p(2400, "･ω･", "ฅ", ""),
            p(1200, "-ω-", "", ""),
            p(3600, "･ω･", "", ""),
        ],
        Scene::Happy => &[
            p(1200, "^ω^", "ฅ", "ฅ"),
            p(1200, "^ω^", "", "♡"),
            p(2400, ">ω<", "ฅ", "ฅ"),
            p(1200, "^ω^", "♡", ""),
        ],
        Scene::Shy => &[
            p(3600, "//ω//", "", ""),
            p(2400, "-ω-", "ฅ", "ฅ"),
            p(2400, "･ω･", "", "♡"),
            p(1200, "//ω//", "", ""),
        ],
        Scene::Sleepy => &[
            p(4800, "-ω-", "", "z"),
            p(3600, "uωu", "", "zZ"),
            p(2400, "-ω-", "", "zZz"),
            p(1200, "-ω-", "", "｡"),
        ],
        Scene::Dancing => &[
            p(1200, "^ω^", "♪", ""),
            p(1200, "^ω^", "", "♪"),
            p(1200, ">ω<", "ヽ", "ﾉ"),
            p(1200, "^ω^", "", "♫"),
        ],
        Scene::Waking => &[p(1200, "-ω-", "", "?"), p(1200, "oωo", "", "!")],
    };
    let mut position =
        (elapsed.as_millis() % poses.iter().map(|p| p.hold_ms as u128).sum::<u128>()) as u64;
    let pose = poses
        .iter()
        .find(|pose| {
            if position < pose.hold_ms {
                true
            } else {
                position -= pose.hold_ms;
                false
            }
        })
        .unwrap();
    let compact = format!("(={}=)", pose.face);
    PetFrame {
        full: format!("{}{compact}{}", pose.left, pose.right),
        compact,
        tiny: match scene {
            Scene::Sleepy | Scene::Grooming => "-ω-",
            Scene::Listening | Scene::Waking | Scene::Curious => "oωo",
            Scene::Shy => ">ω<",
            _ => "^ω^",
        }
        .into(),
    }
}
