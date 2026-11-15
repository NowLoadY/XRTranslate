//! A bounded side-view world. Positions come from locomotion, gravity, contacts
//! and attachment to a paw; story scripts never assign an actor's coordinates.
use super::super::PetSettings;

#[derive(Clone, Copy, Default, Debug)]
pub(super) struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Face {
    Calm,
    Curious,
    Happy,
    Startled,
    Focused,
    Chew,
    Sleep,
    Shy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Prop {
    Ball,
    Box,
    Plant,
    Can,
    Butterfly,
    Fishing,
    Snack,
    Cloud,
    Umbrella,
    Star,
    Boat,
    Basket,
    Pillow,
    Gift,
    Notes,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Form {
    #[default]
    Normal,
    Seed,
    Sprout,
    Flower,
    Crumbs,
    Empty,
    Peek,
    Fish,
    Paper,
    Packed,
}

impl Prop {
    pub fn radius(self) -> f32 {
        match self {
            Self::Box | Self::Pillow | Self::Cloud => 2.5,
            Self::Fishing => 2.0,
            Self::Ball | Self::Star => 0.7,
            _ => 1.5,
        }
    }
    pub fn persistent(self) -> bool {
        matches!(
            self,
            Self::Ball | Self::Box | Self::Plant | Self::Star | Self::Basket
        )
    }
}

#[derive(Clone, Debug)]
pub(super) struct Body {
    pub prop: Prop,
    pub position: Point,
    pub velocity: Point,
    pub form: Form,
    pub carried: Option<usize>,
    pub born: f32,
    pub dynamic: bool,
}

impl Body {
    pub fn present(&self) -> bool {
        (self.form != Form::Empty || self.prop == Prop::Cloud)
            && !(self.prop == Prop::Ball && self.form == Form::Packed)
    }
}

pub(super) struct Actor {
    pub settings: PetSettings,
    pub position: Point,
    pub velocity: Point,
    pub home: f32,
    pub facing: f32,
    pub face: Face,
    pub signal: &'static str,
    pub hidden: bool,
    pub goal: Option<f32>,
    pub grounded: bool,
}

impl Actor {
    const RADIUS: f32 = 3.5;

    pub fn hand(&self, radius: f32, width: f32) -> Point {
        Point {
            x: (self.position.x + self.facing * (self.radius() + radius))
                .clamp(radius, width - radius),
            y: self.position.y + 0.6,
        }
    }
    pub fn radius(&self) -> f32 {
        Self::RADIUS
    }
}

#[derive(Default)]
pub(super) struct World {
    pub width: f32,
    pub actors: Vec<Actor>,
    pub props: Vec<Body>,
    pub time: f32,
}

#[derive(Clone, Copy)]
pub(super) enum Place {
    Left,
    Right,
    Ahead,
    Sky,
}

impl World {
    pub fn new(width: usize, cast: super::Cast) -> Self {
        let width = width as f32;
        let actors = cast
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(index, settings)| {
                let x = if index == 0 {
                    Actor::RADIUS + 0.5
                } else {
                    width - Actor::RADIUS - 0.5
                };
                Actor {
                    settings,
                    position: Point { x, y: 0.0 },
                    velocity: Point::default(),
                    home: x,
                    facing: if index == 0 { 1.0 } else { -1.0 },
                    face: Face::Calm,
                    signal: "",
                    hidden: false,
                    goal: None,
                    grounded: true,
                }
            })
            .collect();
        Self {
            width,
            actors,
            ..Default::default()
        }
    }

    pub fn resize(&mut self, width: usize) {
        self.width = width as f32;
        for actor in &mut self.actors {
            let r = actor.radius();
            actor.position.x = actor.position.x.clamp(r, self.width - r);
            actor.home = actor.home.clamp(r, self.width - r);
        }
        for body in &mut self.props {
            let r = body.prop.radius();
            body.position.x = body.position.x.clamp(r, self.width - r);
        }
    }

    pub fn body(&self, prop: Prop) -> Option<&Body> {
        self.props.iter().find(|body| body.prop == prop)
    }
    pub fn body_mut(&mut self, prop: Prop) -> Option<&mut Body> {
        self.props.iter_mut().find(|body| body.prop == prop)
    }
    pub fn form(&self, prop: Prop) -> Option<Form> {
        self.body(prop).map(|body| body.form)
    }

    pub fn spawn(&mut self, prop: Prop, place: Place, form: Form) {
        if self.body(prop).is_some() {
            return;
        }
        let r = prop.radius();
        let actor = &self.actors[0];
        let preferred = match place {
            Place::Left => r + 0.5,
            Place::Right | Place::Sky => self.width - r - 1.0,
            Place::Ahead => actor.position.x + actor.facing * (actor.radius() + r + 1.0),
        }
        .clamp(r, self.width - r);
        // Choose the nearest free resting place to a semantic anchor.
        let x = if matches!(place, Place::Sky) {
            Some(preferred)
        } else {
            (0..=(self.width * 2.0) as usize)
                .map(|cell| cell as f32 / 2.0)
                .filter(|x| *x >= r && *x <= self.width - r)
                .filter(|x| {
                    self.props.iter().all(|body| {
                        body.position.y > 0.4
                            || !body.present()
                            || (*x - body.position.x).abs() >= r + body.prop.radius() + 0.2
                    })
                })
                .filter(|x| {
                    prop != Prop::Box
                        || self
                            .actors
                            .iter()
                            .all(|actor| (*x - actor.position.x).abs() >= r + actor.radius() + 0.2)
                })
                .min_by(|a, b| (a - preferred).abs().total_cmp(&(b - preferred).abs()))
        };
        let Some(x) = x else {
            return;
        };
        self.props.push(Body {
            prop,
            position: Point {
                x,
                y: if matches!(place, Place::Sky) {
                    2.0
                } else {
                    0.0
                },
            },
            velocity: Point::default(),
            form,
            carried: None,
            born: self.time,
            dynamic: matches!(prop, Prop::Ball | Prop::Boat),
        });
    }

    pub fn settle_episode(&mut self) {
        self.props.retain(|body| body.prop.persistent());
        for body in &mut self.props {
            body.carried = None;
        }
        for actor in &mut self.actors {
            actor.goal = None;
            actor.hidden = false;
            actor.signal = "";
        }
        if let Some(body) = self.body_mut(Prop::Box) {
            body.form = Form::Normal;
        }
    }

    pub fn look(&mut self, actor: usize, x: f32) {
        let delta = x - self.actors[actor].position.x;
        if delta.abs() > 0.1 {
            self.actors[actor].facing = delta.signum();
        }
    }

    pub fn walk(&mut self, actor: usize, x: f32) -> bool {
        let pet = &mut self.actors[actor];
        let x = x.clamp(pet.radius(), self.width - pet.radius());
        pet.goal = Some(x);
        (pet.position.x - x).abs() < 0.25 && pet.grounded
    }

    pub fn approach(&mut self, actor: usize, prop: Prop) -> bool {
        let Some(body) = self.body(prop) else {
            return false;
        };
        if body.carried == Some(actor) {
            return true;
        }
        let x = body.position.x;
        let pet = &self.actors[actor];
        let reach = pet.radius() + prop.radius() + 0.3;
        let side = if pet.position.x < x { -1.0 } else { 1.0 };
        self.look(actor, x);
        self.walk(actor, x + side * reach);
        (self.actors[actor].position.x - x).abs() <= reach + 0.3
            && (self.actors[actor].position.y - body_height(self.body(prop).unwrap())).abs() < 1.1
    }

    pub fn jump(&mut self, actor: usize) {
        let pet = &mut self.actors[actor];
        if pet.grounded {
            pet.velocity.y = 4.4;
            pet.grounded = false;
        }
    }

    pub fn attach(&mut self, actor: usize, prop: Prop) -> bool {
        if !self.approach(actor, prop) {
            return false;
        }
        if let Some(body) = self.body_mut(prop) {
            body.carried = Some(actor);
            body.velocity = Point::default();
        }
        true
    }

    pub fn drop(&mut self, prop: Prop) {
        let hand = self.body(prop).and_then(|body| {
            body.carried
                .map(|index| self.actors[index].hand(prop.radius(), self.width))
        });
        if let Some(body) = self.body_mut(prop) {
            if let Some(hand) = hand {
                body.position = hand;
            }
            body.carried = None;
            body.velocity.y = 0.0;
            body.dynamic = true;
        }
    }

    pub fn advance(&mut self, dt: f32) {
        super::motion::step(self, dt);
    }
}

fn body_height(body: &Body) -> f32 {
    if body.prop == Prop::Butterfly {
        0.8
    } else {
        body.position.y
    }
}
