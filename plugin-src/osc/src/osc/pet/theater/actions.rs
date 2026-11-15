//! Shared, feedback-driven actions. Arrival and contact are tested against the
//! live world each tick. No action can complete by merely waiting at a distance.
use super::world::{Face, Form, Prop, World};

#[derive(Clone, Copy)]
pub(super) enum Target {
    Object(Prop),
    Home,
    Friend,
}

#[derive(Clone, Copy)]
pub(super) enum Effect {
    Set(Form),
    Catch,
    Launch,
    ClearRain,
    Store,
    Wrap,
}

#[derive(Clone, Copy)]
pub(super) enum Action {
    Go(Target),
    Watch(Target, f32),
    Wait(f32),
    Push(Prop),
    Pick(Prop),
    Deliver(Prop, Target),
    Use(Prop, Effect, f32),
    EnterBox,
    LeaveBox,
}

#[derive(Clone, Copy)]
pub(super) struct Step {
    pub action: Action,
    pub face: Face,
    pub signal: &'static str,
}

#[derive(Default)]
pub(super) struct Running {
    pub elapsed: f32,
    contact: f32,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Running,
    Complete,
    Failed,
}

impl Running {
    pub fn tick(&mut self, step: Step, world: &mut World, variant: u8, dt: f32) -> Outcome {
        let present = match step.action {
            Action::Go(target) | Action::Watch(target, _) => target_x(world, target).is_some(),
            Action::Push(prop)
            | Action::Pick(prop)
            | Action::Deliver(prop, _)
            | Action::Use(prop, _, _) => world.body(prop).is_some(),
            Action::EnterBox | Action::LeaveBox => world.body(Prop::Box).is_some(),
            Action::Wait(_) => true,
        };
        if !present {
            return Outcome::Failed;
        }
        self.elapsed += dt;
        world.actors[0].face = step.face;
        world.actors[0].signal = step.signal;
        let complete = match step.action {
            Action::Go(target) => approach(world, target),
            Action::Watch(target, seconds) => {
                if let Some(x) = target_x(world, target) {
                    world.look(0, x);
                }
                world.actors[0].goal = None;
                self.elapsed >= seconds
            }
            Action::Wait(seconds) => {
                world.actors[0].goal = None;
                self.elapsed >= seconds
            }
            Action::Push(prop) => {
                if world.approach(0, prop) {
                    let direction = world.actors[0].facing;
                    if let Some(body) = world.body_mut(prop) {
                        body.velocity.x = direction * 5.0;
                        body.velocity.y = 1.4;
                    }
                    true
                } else {
                    false
                }
            }
            Action::Pick(prop) => world.attach(0, prop),
            Action::Deliver(prop, target) => {
                if world.body(prop).is_none() {
                    return Outcome::Failed;
                }
                if world.body(prop).unwrap().carried != Some(0) && !world.attach(0, prop) {
                    false
                } else if approach(world, target) {
                    world.drop(prop);
                    true
                } else {
                    false
                }
            }
            Action::Use(prop, effect, seconds) => {
                if world.body(prop).is_none() {
                    return Outcome::Failed;
                }
                if world.approach(0, prop) {
                    world.actors[0].goal = None;
                    self.contact += dt;
                    if self.contact >= seconds {
                        apply(world, prop, effect, variant);
                        true
                    } else {
                        false
                    }
                } else {
                    self.contact = 0.0;
                    false
                }
            }
            Action::EnterBox => {
                let Some(x) = target_x(world, Target::Object(Prop::Box)) else {
                    return Outcome::Failed;
                };
                world.walk(0, x);
                if world.actors[0].grounded && (world.actors[0].position.x - x).abs() > 0.3 {
                    world.jump(0);
                }
                if (world.actors[0].position.x - x).abs() < 0.35 && world.actors[0].grounded {
                    world.actors[0].hidden = true;
                    world.actors[0].goal = None;
                    world.body_mut(Prop::Box).unwrap().form = Form::Peek;
                    true
                } else {
                    false
                }
            }
            Action::LeaveBox => {
                world.actors[0].hidden = false;
                if let Some(body) = world.body_mut(Prop::Box) {
                    body.form = Form::Normal;
                }
                world.jump(0);
                true
            }
        };
        if complete {
            world.actors[0].goal = None;
            Outcome::Complete
        } else if self.elapsed > 24.0 {
            // Replan instead of teleporting a stuck actor or inventing success.
            Outcome::Failed
        } else {
            Outcome::Running
        }
    }
}

fn target_x(world: &World, target: Target) -> Option<f32> {
    match target {
        Target::Object(prop) => world.body(prop).map(|body| body.position.x),
        Target::Home => Some(world.actors[0].home),
        Target::Friend => world.actors.get(1).map(|actor| actor.position.x),
    }
}

fn approach(world: &mut World, target: Target) -> bool {
    match target {
        Target::Object(prop) => world.approach(0, prop),
        Target::Home => world.walk(0, world.actors[0].home),
        Target::Friend => {
            let Some(friend) = world.actors.get(1) else {
                return false;
            };
            let side = if world.actors[0].position.x < friend.position.x {
                -1.0
            } else {
                1.0
            };
            world.walk(
                0,
                friend.position.x + side * (friend.radius() + world.actors[0].radius() + 0.4),
            )
        }
    }
}

fn apply(world: &mut World, prop: Prop, effect: Effect, variant: u8) {
    use Effect::*;
    match effect {
        Set(form) => world.body_mut(prop).unwrap().form = form,
        Catch => {
            // An empty hook is legible at chatbox size; a tiny ASCII boot is not.
            world.body_mut(prop).unwrap().form = if variant == 0 {
                Form::Normal
            } else {
                Form::Fish
            }
        }
        Launch => {
            let direction = world.actors[0].facing;
            world.body_mut(prop).unwrap().velocity.x = direction * 2.0;
        }
        ClearRain => {
            if let Some(cloud) = world.body_mut(Prop::Cloud) {
                cloud.form = Form::Empty;
            }
        }
        Store => {
            if let Some(ball) = world.body_mut(Prop::Ball) {
                ball.form = Form::Packed;
            }
        }
        Wrap => {
            if let Some(star) = world.body_mut(Prop::Star) {
                star.form = Form::Empty;
            }
            world.body_mut(prop).unwrap().form = Form::Packed;
        }
    }
}
