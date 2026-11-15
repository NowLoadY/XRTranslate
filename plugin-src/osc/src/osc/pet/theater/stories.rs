//! Small goal recipes. Selection uses world facts; execution is entirely shared.
//! New stories combine verbs and props, without adding locomotion/state machines.
use super::super::PetMood;
use super::{
    actions::{Action::*, Effect::*, Step, Target::*},
    world::{Face::*, Form, Place, Prop::*, World},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Story {
    Ball,
    Box,
    Plant,
    Water,
    Butterfly,
    Fish,
    Snack,
    Rain,
    Stars,
    Boat,
    Tidy,
    Nap,
    Gift,
    Music,
    Share,
}

const fn s(action: super::actions::Action, face: super::world::Face, signal: &'static str) -> Step {
    Step {
        action,
        face,
        signal,
    }
}

impl Story {
    pub const ALL: [Self; 15] = [
        Self::Ball,
        Self::Box,
        Self::Plant,
        Self::Water,
        Self::Butterfly,
        Self::Fish,
        Self::Snack,
        Self::Rain,
        Self::Stars,
        Self::Boat,
        Self::Tidy,
        Self::Nap,
        Self::Gift,
        Self::Music,
        Self::Share,
    ];

    pub fn weight(self, world: &World, music: bool) -> u32 {
        let available = match self {
            Self::Ball => world.form(Ball).is_none_or(|form| form == Form::Packed),
            Self::Plant => world.body(Plant).is_none(),
            Self::Water => world.form(Plant) == Some(Form::Sprout),
            Self::Tidy => world.form(Ball) == Some(Form::Normal),
            Self::Stars => world.form(Star).is_none_or(|form| form == Form::Empty),
            Self::Gift => world.form(Star) == Some(Form::Normal),
            Self::Music => music,
            Self::Share => world.actors.len() > 1,
            _ => true,
        };
        if !available {
            return 0;
        }
        let pet = world.actors[0].settings;
        let quiet = matches!(
            self,
            Self::Box | Self::Stars | Self::Nap | Self::Boat | Self::Rain
        );
        let mood = match pet.mood {
            PetMood::Sleepy if quiet => 9,
            PetMood::Sleepy => 1,
            PetMood::Shy
                if matches!(self, Self::Box | Self::Butterfly | Self::Gift | Self::Plant) =>
            {
                6
            }
            PetMood::Happy
                if matches!(
                    self,
                    Self::Ball | Self::Music | Self::Share | Self::Butterfly
                ) =>
            {
                6
            }
            _ => 3,
        };
        mood + u32::from(matches!(self, Self::Fish | Self::Ball | Self::Box)) * 2
    }

    pub fn props(self) -> &'static [super::world::Prop] {
        match self {
            Self::Ball => &[Ball],
            Self::Box => &[Box],
            Self::Plant | Self::Water => &[Plant, Can],
            Self::Butterfly => &[Butterfly],
            Self::Fish => &[Fishing],
            Self::Snack | Self::Share => &[Snack],
            Self::Rain => &[Cloud, Umbrella],
            Self::Stars => &[Star],
            Self::Boat => &[Boat],
            Self::Tidy => &[Ball, Basket],
            Self::Nap => &[Pillow],
            Self::Gift => &[Star, Gift],
            Self::Music => &[Notes],
        }
    }

    pub fn prepare(self, world: &mut World) {
        use Form::*;
        use Place::*;
        world.settle_episode();
        match self {
            Self::Ball => {
                world.props.retain(|body| body.prop != Ball);
                world.spawn(Ball, Ahead, Normal);
            }
            Self::Box => world.spawn(Box, Right, Normal),
            Self::Plant => {
                world.spawn(Plant, Left, Seed);
                world.spawn(Can, Ahead, Normal);
            }
            Self::Water => world.spawn(Can, Right, Normal),
            Self::Butterfly => world.spawn(Butterfly, Sky, Normal),
            Self::Fish => world.spawn(Fishing, Right, Normal),
            Self::Snack | Self::Share => world.spawn(Snack, Ahead, Normal),
            Self::Rain => {
                world.spawn(Cloud, Sky, Normal);
                world.spawn(Umbrella, Right, Normal);
            }
            Self::Stars => {
                world.props.retain(|body| body.prop != Star);
                world.spawn(Star, Sky, Packed);
            }
            Self::Boat => world.spawn(Boat, Ahead, Paper),
            Self::Tidy => world.spawn(Basket, Left, Normal),
            Self::Nap => world.spawn(Pillow, Right, Normal),
            Self::Gift => world.spawn(Gift, Left, Paper),
            Self::Music => world.spawn(Notes, Sky, Normal),
        }
    }

    pub fn steps(self) -> &'static [Step] {
        match self {
            Self::Ball => {
                const {
                    &[
                        s(Watch(Object(Ball), 3.0), Curious, "?"),
                        s(Go(Object(Ball)), Focused, ""),
                        s(Wait(2.0), Focused, "..."),
                        s(Push(Ball), Happy, "!"),
                        s(Watch(Object(Ball), 3.0), Startled, "!"),
                        s(Go(Object(Ball)), Focused, ""),
                        s(Push(Ball), Happy, ""),
                        s(Watch(Object(Ball), 4.0), Happy, "♡"),
                    ]
                }
            }
            Self::Box => {
                const {
                    &[
                        s(Watch(Object(Box), 3.0), Curious, "?"),
                        s(Go(Object(Box)), Focused, ""),
                        s(Wait(3.0), Curious, "..."),
                        s(EnterBox, Startled, "!"),
                        s(Wait(5.0), Curious, "?"),
                        s(Wait(9.0), Sleep, "z"),
                        s(LeaveBox, Curious, "?"),
                        s(Go(Home), Happy, "~"),
                    ]
                }
            }
            Self::Plant => {
                const {
                    &[
                        s(Go(Object(Plant)), Curious, "?"),
                        s(Use(Plant, Set(Form::Seed), 4.0), Focused, "..."),
                        s(Pick(Can), Focused, ""),
                        s(Deliver(Can, Object(Plant)), Focused, ""),
                        s(Use(Plant, Set(Form::Sprout), 4.0), Calm, "..."),
                        s(Watch(Object(Plant), 6.0), Curious, "..."),
                        s(Wait(4.0), Happy, "!"),
                        s(Go(Home), Happy, "♡"),
                    ]
                }
            }
            Self::Water => {
                const {
                    &[
                        s(Watch(Object(Plant), 3.0), Curious, "?"),
                        s(Pick(Can), Focused, ""),
                        s(Deliver(Can, Object(Plant)), Focused, ""),
                        s(Use(Plant, Set(Form::Flower), 5.0), Calm, "..."),
                        s(Watch(Object(Plant), 5.0), Happy, "!"),
                        s(Go(Home), Happy, "♡"),
                    ]
                }
            }
            Self::Butterfly => {
                const {
                    &[
                        s(Watch(Object(Butterfly), 4.0), Curious, "?"),
                        s(Go(Object(Butterfly)), Focused, ""),
                        s(Watch(Object(Butterfly), 5.0), Startled, "!"),
                        s(Go(Home), Calm, ""),
                        s(Watch(Object(Butterfly), 7.0), Calm, "..."),
                        s(Wait(4.0), Happy, "♡"),
                    ]
                }
            }
            Self::Fish => {
                const {
                    &[
                        s(Go(Object(Fishing)), Calm, ""),
                        s(Use(Fishing, Set(Form::Normal), 3.0), Focused, ""),
                        s(Watch(Object(Fishing), 10.0), Calm, "..."),
                        s(Use(Fishing, Catch, 3.0), Startled, "!"),
                        s(Watch(Object(Fishing), 5.0), Curious, "?"),
                        s(Use(Fishing, Set(Form::Empty), 3.0), Happy, "~"),
                    ]
                }
            }
            Self::Snack => {
                const {
                    &[
                        s(Watch(Object(Snack), 3.0), Curious, "?"),
                        s(Go(Object(Snack)), Focused, ""),
                        s(Use(Snack, Set(Form::Crumbs), 4.0), Chew, ""),
                        s(Use(Snack, Set(Form::Empty), 4.0), Chew, ""),
                        s(Wait(4.0), Happy, "♡"),
                        s(Wait(3.0), Focused, "~"),
                    ]
                }
            }
            Self::Rain => {
                const {
                    &[
                        s(Watch(Object(Cloud), 4.0), Curious, "?"),
                        s(Go(Object(Umbrella)), Startled, "!"),
                        s(Pick(Umbrella), Focused, ""),
                        s(Wait(9.0), Calm, "..."),
                        s(Use(Umbrella, ClearRain, 3.0), Curious, "?"),
                        s(Deliver(Umbrella, Home), Happy, "~"),
                    ]
                }
            }
            Self::Stars => {
                const {
                    &[
                        s(Watch(Object(Star), 8.0), Calm, "..."),
                        s(Wait(2.0), Curious, "?"),
                        s(Watch(Object(Star), 4.0), Startled, "!"),
                        s(Pick(Star), Focused, ""),
                        s(Deliver(Star, Home), Happy, ""),
                        s(Wait(5.0), Shy, "♡"),
                    ]
                }
            }
            Self::Boat => {
                const {
                    &[
                        s(Go(Object(Boat)), Curious, "?"),
                        s(Use(Boat, Set(Form::Normal), 6.0), Focused, "..."),
                        s(Wait(3.0), Happy, "!"),
                        s(Use(Boat, Launch, 2.0), Focused, ""),
                        s(Watch(Object(Boat), 8.0), Calm, "~"),
                        s(Wait(4.0), Happy, "♡"),
                    ]
                }
            }
            Self::Tidy => {
                const {
                    &[
                        s(Watch(Object(Ball), 3.0), Curious, "?"),
                        s(Pick(Ball), Focused, ""),
                        s(Deliver(Ball, Object(Basket)), Focused, ""),
                        s(Use(Basket, Store, 3.0), Happy, "!"),
                        s(Go(Home), Calm, "~"),
                        s(Wait(4.0), Happy, ""),
                    ]
                }
            }
            Self::Nap => {
                const {
                    &[
                        s(Go(Object(Pillow)), Sleep, "..."),
                        s(Wait(4.0), Focused, "~"),
                        s(Wait(14.0), Sleep, "z"),
                        s(Wait(10.0), Sleep, "zZ"),
                        s(Wait(4.0), Curious, "?"),
                        s(Go(Home), Happy, "~"),
                    ]
                }
            }
            Self::Gift => {
                const {
                    &[
                        s(Pick(Star), Curious, ""),
                        s(Deliver(Star, Object(Gift)), Focused, ""),
                        s(Use(Gift, Wrap, 5.0), Focused, "..."),
                        s(Wait(4.0), Happy, "!"),
                        s(Pick(Gift), Shy, ""),
                        s(Deliver(Gift, Home), Shy, "♡"),
                        s(Wait(6.0), Shy, "..."),
                    ]
                }
            }
            Self::Music => {
                const {
                    &[
                        s(Watch(Object(Notes), 4.0), Curious, "?"),
                        s(Wait(5.0), Calm, "♪"),
                        s(Go(Home), Happy, "♫"),
                        s(Wait(5.0), Happy, "♪"),
                        s(Wait(4.0), Shy, "♡"),
                    ]
                }
            }
            Self::Share => {
                const {
                    &[
                        s(Watch(Friend, 3.0), Curious, "?"),
                        s(Pick(Snack), Focused, ""),
                        s(Deliver(Snack, Friend), Shy, "♡"),
                        s(Use(Snack, Set(Form::Crumbs), 4.0), Happy, ""),
                        s(Use(Snack, Set(Form::Empty), 4.0), Chew, ""),
                        s(Watch(Friend, 5.0), Happy, "♡"),
                    ]
                }
            }
        }
    }
}
