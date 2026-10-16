//! Direct interaction in the companion's shared tracking space, in metres.
use super::{
    input::Hand,
    voice_props::{self, ButtonVisual, MICROPHONE_GRIP_HALF, MICROPHONE_HEAD},
};
use crate::{
    ui::components::avatar::Model,
    voice_interaction::{VoiceAction, VoiceSnapshot},
};
use glam::{Mat4, Vec3};

#[derive(Clone, Copy)]
pub(super) enum ButtonId {
    Send,
    Revise,
}

pub(super) struct ButtonSpec {
    pub id: ButtonId,
    pub position: Vec3,
    pub half: Vec3,
}

// These are sibling objects in the shared space, never Avatar model coordinates.
// +Z faces the player. Add another direct action by adding its specification here.
pub(super) const BUTTONS: [ButtonSpec; 2] = [
    ButtonSpec {
        id: ButtonId::Send,
        position: Vec3::new(-0.18, 0.0, 0.0),
        half: Vec3::new(0.065, 0.048, 0.018),
    },
    ButtonSpec {
        id: ButtonId::Revise,
        position: Vec3::ZERO,
        half: Vec3::new(0.065, 0.048, 0.018),
    },
];
const DOCK: Vec3 = Vec3::new(0.20, 0.05, 0.0);
const MOUTH: Vec3 = Vec3::new(0.0, -0.10, -0.08);
const MIN_STRIKE_SPEED: f32 = 0.40;
const MAX_STRIKE_INTERVAL: f32 = 0.10;
const GRAB_MARGIN: f32 = 0.025;

#[derive(Clone, Copy, PartialEq, Eq)]
enum GrabTarget {
    Microphone,
    Button(usize),
}

// Radial projection gives a conservative surface gap for these rounded props,
// in metres. One small tolerance serves both shapes without a physics engine.
fn surface_gap(point: Vec3, half: Vec3) -> f32 {
    point.length() * (1.0 - 1.0 / (point / half).length().max(1.0))
}

fn on_face(point: Vec3, half: Vec3, margin: f32) -> bool {
    (point.x / (half.x + margin)).powi(2) + (point.y / (half.y + margin)).powi(2) <= 1.0
}

#[derive(Default)]
struct ButtonState {
    pose: Mat4,
    held: Option<(usize, Mat4)>,
    owner: Option<usize>,
    latched: bool,
    depth: f32,
    feedback: f32,
}
impl ButtonState {
    fn clear_contact(&mut self) {
        self.owner = None;
        self.latched = false;
        self.depth = 0.0;
        self.feedback = 0.0;
    }
}

pub(super) struct Interaction {
    pub microphone: Mat4,
    held: Option<(usize, Mat4)>,
    candidates: [Option<GrabTarget>; 2],
    previous: [Option<Vec3>; 2],
    buttons: [ButtonState; BUTTONS.len()],
    root: Option<Mat4>,
    gate: bool,
    gate_time: f32,
    pub freeze_controls: bool,
}

impl Default for Interaction {
    fn default() -> Self {
        Self {
            microphone: Mat4::IDENTITY,
            held: None,
            candidates: [None; 2],
            previous: [None; 2],
            buttons: std::array::from_fn(|i| ButtonState {
                pose: Mat4::from_translation(BUTTONS[i].position),
                ..Default::default()
            }),
            root: None,
            gate: false,
            gate_time: 0.0,
            freeze_controls: false,
        }
    }
}

impl Interaction {
    fn nearest_grabbable(&self, root: Mat4, point: Vec3, review: bool) -> Option<GrabTarget> {
        let mut nearest: Option<(GrabTarget, f32, f32)> = None;
        let mut consider = |target, pose: Mat4, half| {
            let local = pose.inverse().transform_point3(point);
            let gap = surface_gap(local, half);
            let distance = local.length_squared();
            if gap <= GRAB_MARGIN
                && nearest.is_none_or(|(_, best_gap, best_distance)| {
                    gap.total_cmp(&best_gap)
                        .then(distance.total_cmp(&best_distance))
                        .is_lt()
                })
            {
                nearest = Some((target, gap, distance));
            }
        };
        if self.held.is_none() {
            consider(
                GrabTarget::Microphone,
                self.microphone,
                MICROPHONE_GRIP_HALF,
            );
        }
        if review {
            for (i, (spec, state)) in BUTTONS.iter().zip(&self.buttons).enumerate() {
                if state.held.is_none() {
                    consider(GrabTarget::Button(i), root * state.pose, spec.half);
                }
            }
        }
        nearest.map(|(target, _, _)| target)
    }

    pub fn dock_microphone(&mut self) {
        if let Some((hand, _)) = self.held.take() {
            // Its contact point changes from the microphone head to the hand;
            // that jump must not become a button strike on the next frame.
            self.previous[hand] = None;
            for button in &mut self.buttons {
                if button.owner == Some(hand) {
                    button.clear_contact();
                }
            }
        }
        self.gate = false;
        self.gate_time = 0.0;
        if let Some(root) = self.root {
            self.microphone = root * Mat4::from_translation(DOCK);
        }
        // Grabbing requires a fresh pressed edge; an already held grip cannot
        // take the microphone back. Button poses, holds and the root are kept.
    }

    pub fn suspend(&mut self) -> Vec<VoiceAction> {
        let active = self.gate;
        self.held = None;
        self.candidates = [None; 2];
        self.previous = [None; 2];
        self.root = None;
        self.gate = false;
        self.gate_time = 0.0;
        self.freeze_controls = false;
        for state in &mut self.buttons {
            state.held = None;
            state.clear_contact();
        }
        if active {
            vec![VoiceAction::Microphone(false)]
        } else {
            Vec::new()
        }
    }

    pub fn update(
        &mut self,
        root: Mat4,
        head: Mat4,
        hands: [Hand; 2],
        snapshot: &VoiceSnapshot,
        elapsed: f32,
    ) -> (Vec<VoiceAction>, Vec<usize>) {
        let mut actions = Vec::new();
        let mut pulses = Vec::new();
        // Gesture speed uses real frame time; a stalled frame cannot become a hit.
        let strike_interval =
            elapsed.is_finite() && elapsed > 0.0 && elapsed <= MAX_STRIKE_INTERVAL;
        let dt = if elapsed.is_finite() {
            elapsed.clamp(0.0, 0.05)
        } else {
            0.0
        };
        let moved = self.root.is_none_or(|previous| {
            previous.w_axis.distance(root.w_axis) > 0.003
                || previous.z_axis.distance(root.z_axis) > 0.01
        });
        self.root = Some(root);
        let mut grab_changed = false;
        if let Some((hand, offset)) = self.held {
            if hands[hand].grab
                && let Some(pose) = hands[hand].pose
            {
                self.microphone = pose * offset;
            } else {
                self.held = None;
                grab_changed = true;
            }
        }
        if self.held.is_none() {
            self.microphone = root * Mat4::from_translation(DOCK);
        }
        let inverse_root = root.inverse();
        for state in &mut self.buttons {
            if let Some((hand, offset)) = state.held {
                if snapshot.review_enabled
                    && hands[hand].grab
                    && let Some(pose) = hands[hand].pose
                {
                    state.pose = inverse_root * pose * offset;
                } else {
                    // A placed button stays where it was released, in the shared space.
                    state.held = None;
                    grab_changed = true;
                }
            }
        }
        self.candidates = [None; 2];
        for (hand, input) in hands.iter().enumerate() {
            if (input.grab && !input.pressed)
                || self.held.is_some_and(|(owner, _)| owner == hand)
                || self
                    .buttons
                    .iter()
                    .any(|b| b.held.is_some_and(|(owner, _)| owner == hand))
            {
                continue;
            }
            let Some(pose) = input.pose else {
                continue;
            };
            let candidate =
                self.nearest_grabbable(root, pose.w_axis.truncate(), snapshot.review_enabled);
            self.candidates[hand] = candidate;
            if !input.pressed {
                continue;
            }
            match candidate {
                Some(GrabTarget::Microphone) => {
                    // Grip -Z runs along the held tube toward the thumb. Align
                    // the microphone's +Y with it for the same natural hold
                    // every time, centered in the palm instead of floating.
                    let offset = Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2);
                    self.microphone = pose * offset;
                    self.held = Some((hand, offset));
                }
                Some(GrabTarget::Button(i)) => {
                    let state = &mut self.buttons[i];
                    // Placement keeps the exact pickup pose, with no snap.
                    state.held = Some((hand, pose.inverse() * root * state.pose));
                }
                None => continue,
            }
            self.candidates[hand] = None;
            grab_changed = true;
            pulses.push(hand);
        }
        // A later hand may have taken an earlier hand's hovered object.
        for candidate in &mut self.candidates {
            let held = match *candidate {
                Some(GrabTarget::Microphone) => self.held.is_some(),
                Some(GrabTarget::Button(i)) => self.buttons[i].held.is_some(),
                None => false,
            };
            if held {
                *candidate = None;
            }
        }
        if moved || grab_changed {
            self.previous = [None; 2];
            for state in &mut self.buttons {
                state.clear_contact();
            }
        }

        let mouth = head.transform_point3(MOUTH);
        let microphone_head = self.microphone.transform_point3(MICROPHONE_HEAD);
        let distance = microphone_head.distance(mouth);
        let wants_gate = self.held.is_some() && distance < if self.gate { 0.28 } else { 0.18 };
        if wants_gate != self.gate {
            self.gate_time += dt;
            if self.held.is_none() || self.gate_time >= if wants_gate { 0.08 } else { 0.16 } {
                self.gate = wants_gate;
                self.gate_time = 0.0;
                actions.push(VoiceAction::Microphone(self.gate));
            }
        } else {
            self.gate_time = 0.0;
        }

        self.freeze_controls = hands
            .iter()
            .filter_map(|hand| hand.pose)
            .any(|pose| pose.w_axis.truncate().distance(root.transform_point3(DOCK)) < 0.14);
        if !snapshot.review_enabled {
            self.previous = [None; 2];
            for state in &mut self.buttons {
                state.clear_contact();
            }
            return (actions, pulses);
        }
        self.freeze_controls |= self.buttons.iter().any(|state| state.held.is_some());
        let points = std::array::from_fn::<_, 2, _>(|hand| {
            let pose = hands[hand].pose?;
            if self.held.is_some_and(|(owner, _)| owner == hand) {
                Some(microphone_head)
            } else if hands[hand].grab
                || self
                    .buttons
                    .iter()
                    .any(|state| state.held.is_some_and(|(owner, _)| owner == hand))
            {
                // Grip is manipulation intent. A held button cannot activate another.
                None
            } else {
                // The same grip center serves both touch and pickup, with no
                // controller-specific tip offset to learn.
                Some(pose.w_axis.truncate())
            }
        });
        let mut used = [false; 2];
        for (spec, state) in BUTTONS.iter().zip(&mut self.buttons) {
            if let Some(hand) = state.owner {
                let inverse = (root * state.pose).inverse();
                let retained = state.held.is_none()
                    && points[hand]
                        .map(|p| inverse.transform_point3(p))
                        .is_some_and(|p| on_face(p, spec.half, 0.04) && p.z > -0.09 && p.z < 0.16);
                if retained {
                    used[hand] = true;
                } else {
                    state.owner = None;
                    state.latched = false;
                }
            }
        }
        for (spec, state) in BUTTONS.iter().zip(&mut self.buttons) {
            state.feedback = (state.feedback - dt).max(0.0);
            state.depth = 0.012 * (state.feedback / 0.22).clamp(0.0, 1.0);
            if state.held.is_some() {
                continue;
            }
            let inverse = (root * state.pose).inverse();
            let local = points.map(|point| point.map(|p| inverse.transform_point3(p)));
            let inside = |p: Vec3, margin: f32| {
                on_face(p, spec.half, margin) && p.z > -0.09 && p.z < 0.12 + margin
            };
            let press_z = |hand| {
                spec.half.z
                    + if self.held.is_some_and(|(owner, _)| owner == hand) {
                        0.035
                    } else {
                        0.018
                    }
                    - 0.010
            };
            // Grip positions also hold the anchor still during slow approach.
            self.freeze_controls |= hands
                .iter()
                .filter_map(|hand| hand.pose)
                .any(|pose| inside(inverse.transform_point3(pose.w_axis.truncate()), 0.09));
            let crossings = std::array::from_fn::<_, 2, _>(|hand| {
                let radius = if self.held.is_some_and(|(owner, _)| owner == hand) {
                    0.035
                } else {
                    0.018
                };
                let previous = inverse.transform_point3(self.previous[hand]?);
                let current = local[hand]?;
                let press_z = press_z(hand);
                let travel = previous.z - current.z;
                if moved || previous.z <= press_z || current.z > press_z {
                    return None;
                }
                let point = previous.lerp(current, (previous.z - press_z) / travel);
                on_face(point, spec.half, radius * 0.6).then_some(
                    strike_interval
                        && travel / elapsed >= MIN_STRIKE_SPEED
                        && travel >= previous.distance(current) * 0.5,
                )
            });
            if state.owner.is_none() {
                state.owner = (0..2)
                    .filter(|hand| !used[*hand])
                    .filter(|hand| {
                        crossings[*hand].is_some()
                            || local[*hand]
                                .is_some_and(|p| inside(p, 0.025) && p.z <= press_z(*hand))
                    })
                    .min_by(|a, b| {
                        local[*a]
                            .unwrap()
                            .length_squared()
                            .total_cmp(&local[*b].unwrap().length_squared())
                    });
            }
            let Some(hand) = state.owner else {
                continue;
            };
            used[hand] = true;
            let enabled = match spec.id {
                ButtonId::Send => snapshot.can_send,
                ButtonId::Revise => snapshot.can_revise,
            };
            let strike = !state.latched && crossings[hand] == Some(true);
            // Slow contact also consumes this approach. Withdraw before striking;
            // resting a hand on the face cannot turn into a delayed activation.
            state.latched = true;
            if enabled && strike {
                state.feedback = 0.22;
                state.depth = 0.012;
                actions.push(match spec.id {
                    ButtonId::Send => VoiceAction::Send,
                    ButtonId::Revise => VoiceAction::Revise,
                });
                pulses.push(hand);
            }
        }
        self.previous = points;
        (actions, pulses)
    }

    pub fn model(&self, root: Mat4, snapshot: &VoiceSnapshot) -> Model {
        let buttons = std::array::from_fn::<_, { BUTTONS.len() }, _>(|i| {
            let spec = &BUTTONS[i];
            let state = &self.buttons[i];
            ButtonVisual {
                kind: spec.id,
                pose: root * state.pose,
                half: spec.half,
                enabled: match spec.id {
                    ButtonId::Send => snapshot.can_send,
                    ButtonId::Revise => snapshot.can_revise,
                },
                held: state.held.is_some(),
                hover: self.candidates.contains(&Some(GrabTarget::Button(i))),
                depth: state.depth,
                feedback: state.feedback,
            }
        });
        voice_props::render(
            root * Mat4::from_translation(DOCK),
            self.microphone,
            self.held.is_some(),
            self.candidates.contains(&Some(GrabTarget::Microphone)),
            snapshot,
            &buttons,
        )
    }
}
