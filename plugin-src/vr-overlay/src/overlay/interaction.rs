//! Left-wrist review buttons, pressed with the right controller.
use super::{
    input::Hand,
    voice_props::{self, ButtonVisual},
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
// Tray -Y runs toward the forearm in the button plane; +Z is its outward normal.
pub(super) const BUTTONS: [ButtonSpec; 2] = [
    ButtonSpec {
        id: ButtonId::Send,
        position: Vec3::new(0.075, -0.07, 0.0),
        half: Vec3::new(0.032, 0.032, 0.018),
    },
    ButtonSpec {
        id: ButtonId::Revise,
        position: Vec3::new(0.0, -0.07, 0.0),
        half: Vec3::new(0.032, 0.032, 0.018),
    },
];
pub(super) const CONTROL_HAND: usize = 0;
pub(super) fn controller_root(grip: Mat4) -> Mat4 {
    // Touch Plus: native button travel is (0, -0.927, 0.375), while the
    // OpenXR grip is pitched +20.6 degrees in the render-model frame.
    let panel_pitch = -0.375_f32.atan2(0.927) - 20.6_f32.to_radians();
    grip * Mat4::from_rotation_x(panel_pitch)
        * Mat4::from_translation(Vec3::new(0.0, 0.04, -0.04))
        * Mat4::from_rotation_x(-std::f32::consts::FRAC_PI_2)
}

fn on_face(point: Vec3, half: Vec3, margin: f32) -> bool {
    (point.x / (half.x + margin)).powi(2) + (point.y / (half.y + margin)).powi(2) <= 1.0
}

#[derive(Default)]
struct ButtonState {
    previous: Option<Vec3>,
    latched: bool,
    hover: bool,
    depth: f32,
    feedback: f32,
}

#[derive(Default)]
pub(super) struct Interaction {
    buttons: [ButtonState; BUTTONS.len()],
}

impl Interaction {
    pub fn suspend(&mut self) -> Vec<VoiceAction> {
        self.buttons = Default::default();
        Vec::new()
    }

    pub fn update(
        &mut self,
        root: Mat4,
        hands: [Hand; 2],
        snapshot: &VoiceSnapshot,
        elapsed: f32,
    ) -> (Vec<VoiceAction>, Vec<usize>) {
        let mut actions = Vec::new();
        let mut pulses = Vec::new();
        let dt = if elapsed.is_finite() {
            elapsed.clamp(0.0, 0.05)
        } else {
            0.0
        };
        if !snapshot.review_enabled {
            self.buttons = Default::default();
            return (actions, pulses);
        }
        // Only the right controller presses the left-arm buttons. Grip is not an action.
        for (spec, state) in BUTTONS.iter().zip(&mut self.buttons) {
            let pose = root * Mat4::from_translation(spec.position);
            let local = hands[1]
                .pose
                .map(|hand| pose.inverse().transform_point3(hand.w_axis.truncate()));
            let previous = state.previous;
            state.previous = local;
            state.feedback = (state.feedback - dt).max(0.0);
            state.hover = false;
            let enabled = match spec.id {
                ButtonId::Send => snapshot.can_send,
                ButtonId::Revise => snapshot.can_revise || snapshot.revise_cancels,
            };
            let press_z = spec.half.z + 0.018 - 0.010;
            let contact = local
                .is_some_and(|p| on_face(p, spec.half, 0.018) && p.z > -0.05 && p.z <= press_z);
            let separated = local.is_none_or(|p| {
                !on_face(p, spec.half, 0.025) || p.z > press_z + 0.025 || p.z <= -0.05
            });
            if separated {
                state.latched = false;
            }
            // A normal, arbitrarily slow front press crosses this plane. New tracking
            // already inside a button is consumed until withdrawal, never a fresh press.
            let pressed = contact
                && !state.latched
                && previous.is_some_and(|p| p.z > press_z && on_face(p, spec.half, 0.025));
            if contact {
                state.latched = true;
                if enabled && pressed {
                    actions.push(match spec.id {
                        ButtonId::Send => VoiceAction::Send,
                        ButtonId::Revise => VoiceAction::Revise,
                    });
                    pulses.push(1);
                    state.feedback = 0.22;
                }
            }
            if let Some(p) = local {
                state.hover = on_face(p, spec.half, 0.025) && p.z > -0.05 && p.z < 0.09;
            }
            let pressure = if contact {
                local.map_or(0.0, |p| (press_z - p.z).clamp(0.0, 0.012))
            } else {
                0.0
            };
            state.depth = if enabled {
                pressure.max(0.012 * (state.feedback / 0.22).clamp(0.0, 1.0))
            } else {
                0.0
            };
        }
        (actions, pulses)
    }

    pub fn model(&self, root: Mat4, snapshot: &VoiceSnapshot) -> Model {
        let mut model = Model::default();
        if !snapshot.review_enabled {
            return model;
        }
        let buttons = std::array::from_fn::<_, { BUTTONS.len() }, _>(|i| {
            let spec = &BUTTONS[i];
            let state = &self.buttons[i];
            ButtonVisual {
                kind: spec.id,
                pose: root * Mat4::from_translation(spec.position),
                half: spec.half,
                enabled: match spec.id {
                    ButtonId::Send => snapshot.can_send,
                    ButtonId::Revise => snapshot.can_revise || snapshot.revise_cancels,
                },
                hover: state.hover,
                depth: state.depth,
                feedback: state.feedback,
            }
        });
        model
            .parts
            .extend(voice_props::render(&buttons, snapshot.revise_cancels).parts);
        model
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Quat;
    fn snapshot() -> VoiceSnapshot {
        VoiceSnapshot {
            enabled: true,
            review_enabled: true,
            can_send: true,
            can_revise: true,
            ..Default::default()
        }
    }
    fn touch(root: Mat4, z: f32) -> [Hand; 2] {
        [
            Hand::default(),
            Hand {
                pose: Some(root * Mat4::from_translation(BUTTONS[0].position + Vec3::Z * z)),
            },
        ]
    }
    #[test]
    fn buttons_follow_native_touch_panel_when_the_arm_rotates() {
        let native = Mat4::from_rotation_translation(
            Quat::from_euler(glam::EulerRot::XYZ, 0.4, -0.6, 0.8),
            Vec3::new(-0.3, 1.2, -0.5),
        );
        let grip = native * Mat4::from_rotation_x(20.6_f32.to_radians());
        let root = controller_root(grip);
        let normal = native.transform_vector3(Vec3::new(0.0, 0.927, -0.375).normalize());
        assert!(root.transform_vector3(Vec3::Z).abs_diff_eq(normal, 1e-5));
        let direction = root
            .transform_vector3(BUTTONS[1].position - BUTTONS[0].position)
            .normalize();
        let forearm = native.transform_vector3(Vec3::new(0.0, 0.375, 0.927).normalize());
        assert!(direction.dot(forearm).abs() < 1e-5);
        assert!(direction.abs_diff_eq(-native.x_axis.truncate(), 1e-5));
        assert_eq!(BUTTONS[0].half.x, BUTTONS[0].half.y);
        assert_eq!(BUTTONS[1].half.x, BUTTONS[1].half.y);
        assert!(direction.dot(normal).abs() < 1e-5);
        assert!((root.determinant() - 1.0).abs() < 1e-5);
    }
    #[test]
    fn slow_press_fires_once_and_rearms_after_withdrawal_while_arm_moves() {
        let mut interaction = Interaction::default();
        for (i, z) in [
            0.080, 0.050, 0.030, 0.025, 0.020, 0.025, 0.080, 0.030, 0.025,
        ]
        .into_iter()
        .enumerate()
        {
            let root = Mat4::from_translation(Vec3::X * i as f32 * 0.01);
            let (actions, _) = interaction.update(root, touch(root, z), &snapshot(), 0.05);
            assert_eq!(
                actions,
                if i == 3 || i == 8 {
                    vec![VoiceAction::Send]
                } else {
                    vec![]
                }
            );
        }
    }
    #[test]
    fn tracking_recovery_inside_button_requires_withdrawal() {
        let mut interaction = Interaction::default();
        for z in [0.025, 0.020, 0.025] {
            assert!(
                interaction
                    .update(Mat4::IDENTITY, touch(Mat4::IDENTITY, z), &snapshot(), 0.05)
                    .0
                    .is_empty()
            );
        }
    }
    #[test]
    fn second_edit_press_finishes_editing_and_keeps_result() {
        use crate::voice_interaction::{VoiceController, VoiceEffect};
        let mut controller = VoiceController::default();
        controller.set_enabled(true, true);
        controller.set_edit_available(true);
        controller.observe_phrase("原句", "original", "English");
        controller.apply(VoiceAction::Revise);
        let effects = controller.observe_phrase("改得简洁一些", "", "");
        let VoiceEffect::Ask { operation, .. } = effects[0] else {
            panic!("Expected edit request");
        };
        controller.ask_done(operation, Ok("edited result".into()));
        let effects = controller.observe_phrase("再改一次", "", "");
        let VoiceEffect::Ask { operation, .. } = effects[0] else {
            panic!("Expected pending edit");
        };
        let mut interaction = Interaction::default();
        let root = Mat4::IDENTITY;
        let edit = |z| {
            [
                Hand::default(),
                Hand {
                    pose: Some(Mat4::from_translation(BUTTONS[1].position + Vec3::Z * z)),
                },
            ]
        };
        interaction.update(root, edit(0.05), &controller.snapshot(), 0.05);
        let (actions, _) = interaction.update(root, edit(0.025), &controller.snapshot(), 0.05);
        assert_eq!(actions, vec![VoiceAction::Revise]);
        controller.apply(actions[0]);
        assert!(!controller.is_editing());
        controller.ask_done(operation, Ok("late result".into()));
        assert_eq!(controller.snapshot().text, "edited result");
    }
}
