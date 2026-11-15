//! Deliberately small visual vocabulary. Shape and state must survive tiny type;
//! physics and behavior do not depend on the glyphs used to represent them.
use super::world::{Actor, Body, Face, Form, Prop};

pub(super) fn portrait(actor: &Actor) -> String {
    let face = match actor.face {
        Face::Calm => "･ω･",
        Face::Curious => "oωo",
        Face::Happy => "^ω^",
        Face::Startled => ">ω<",
        Face::Focused => "･ω-",
        Face::Chew => "^～^",
        Face::Sleep => "-ω-",
        Face::Shy => "〃ω〃",
    };
    format!("(={face}=)")
}

pub(super) fn prop(body: &Body) -> &'static [&'static str] {
    use Form::*;
    use Prop::*;
    if !body.present() {
        return &[];
    }
    match (body.prop, body.form) {
        (Ball, _) => &["○"],
        (Box, Peek) => &["[･ω･]"],
        (Box, _) => &["[__]"],
        (Plant, Seed) => &["[_]"],
        (Plant, Flower) => &["✿", "[_]"],
        (Plant, _) => &["♧", "[_]"],
        (Can, _) => &["c[]"],
        (Butterfly, _) => &["εïз"],
        (Fishing, Fish) => &["><>"],
        (Fishing, _) => &["╱┐", " J"],
        (Snack, Crumbs) => &["· ·"],
        (Snack, _) => &["(#)"],
        (Cloud, _) => &["☁"],
        (Umbrella, _) => &["☂"],
        (Star, _) => &["☆"],
        (Boat, Paper) | (Gift, Paper) => &["__"],
        (Boat, _) => &[" |>", "\\_/"],
        (Basket, _) => &["[○]"],
        (Pillow, _) => &["___"],
        (Gift, _) => &["[+]"],
        (Notes, _) => &["♫"],
    }
}
