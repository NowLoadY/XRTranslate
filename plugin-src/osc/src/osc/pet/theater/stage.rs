//! A small, legible stage: faces and the current interaction take precedence.
//! Whole sprites may be occluded, but never overwritten into meaningless pieces.
use super::{
    sprites,
    stories::Story,
    world::{Face, Form, Prop, World},
};

const HEIGHT: usize = 3;
const ROW_HEIGHT: f32 = 0.8;
// Ordinary word spaces are much narrower than faces in proportional fonts.
// En spaces reduce that drift; this is a text stage, not a pixel-exact grid.
const SPACE: char = '\u{2002}';

pub(super) fn width(budget: usize, lines: usize) -> Option<usize> {
    let width = (budget.saturating_sub(HEIGHT - 1) / HEIGHT).min(24);
    (lines >= HEIGHT && width >= 18).then_some(width)
}

struct Canvas {
    rows: Vec<Vec<char>>,
    occupied: Vec<Vec<bool>>,
}

impl Canvas {
    fn new(width: usize) -> Self {
        Self {
            rows: vec![vec![SPACE; width]; HEIGHT],
            occupied: vec![vec![false; width]; HEIGHT],
        }
    }

    /// Bottom-anchored, atomic placement at the actual world position.
    fn put(&mut self, row: usize, center: f32, lines: &[&str]) -> bool {
        let width = lines
            .iter()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0);
        if width == 0 || lines.len() > row + 1 {
            return false;
        }
        let left = (center - width as f32 / 2.0).round();
        let top = row + 1 - lines.len();
        if left < 0.0 || left + width as f32 > self.rows[0].len() as f32 {
            return false;
        }
        let left = left as usize;
        if self.occupied[top..=row]
            .iter()
            .any(|cells| cells[left..left + width].iter().any(|cell| *cell))
        {
            return false;
        }
        for (y, line) in lines.iter().enumerate() {
            let x = left + (width - line.chars().count()) / 2;
            for (offset, ch) in line.chars().enumerate() {
                self.rows[top + y][x + offset] = if ch == ' ' { SPACE } else { ch };
            }
            self.occupied[top + y][left..left + width].fill(true);
        }
        true
    }

    fn finish(self) -> String {
        // Quiet side marks preserve leading/trailing space under line centering.
        // No large horizontal border: equal character counts do not imply equal
        // pixel widths in the game's font.
        self.rows
            .into_iter()
            .map(|row| format!("│{}│", row.into_iter().collect::<String>()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn row(height: f32) -> usize {
    HEIGHT
        - 1
        - (height / ROW_HEIGHT)
            .floor()
            .clamp(0.0, (HEIGHT - 1) as f32) as usize
}

fn face_row(height: f32) -> usize {
    // Actor positions are at their feet. Leave the ground row for objects near
    // their paws, so a companion cannot hide the entire subject of a story.
    row(height + ROW_HEIGHT)
}

pub(super) fn render(world: &World, story: Story) -> String {
    let mut canvas = Canvas::new(world.width as usize);
    for actor in &world.actors {
        if actor.hidden {
            if let Some(body) = world.body(Prop::Box) {
                canvas.put(
                    row(body.position.y),
                    body.position.x,
                    &[if actor.face == Face::Sleep {
                        "[ z ]"
                    } else {
                        "[･ω･]"
                    }],
                );
            }
        } else {
            canvas.put(
                face_row(actor.position.y),
                actor.position.x,
                &[&sprites::portrait(actor)],
            );
        }
    }
    // Held and story-relevant objects are laid down before scenery. There are at
    // most eight bodies; fixed passes avoid sorting or copying world state.
    for priority in 0..3 {
        for body in &world.props {
            let rank = if body.carried.is_some() {
                0
            } else if story.props().contains(&body.prop) {
                1
            } else {
                2
            };
            if rank == priority && body.present() {
                // The held umbrella is attached by its handle; its visible
                // canopy belongs above the paw, not behind a companion's face.
                let canopy = f32::from(body.prop == Prop::Umbrella && body.carried.is_some());
                canvas.put(
                    row(body.position.y + canopy),
                    body.position.x,
                    sprites::prop(body),
                );
            }
        }
    }
    for actor in &world.actors {
        if actor.hidden {
            continue;
        }
        let row = face_row(actor.position.y);
        if row == 0 {
            continue;
        }
        let signal = if actor.signal == "..." {
            "…"
        } else {
            actor.signal
        };
        if !signal.is_empty() {
            let beside = actor.position.x + actor.facing * (actor.radius() + 0.5);
            if !canvas.put(row - 1, beside, &[signal]) {
                canvas.put(row - 1, actor.position.x, &[signal]);
            }
        }
    }
    // A single contextual cue is enough; never tile waves/rain across the floor.
    if let Some(cloud) = world.body(Prop::Cloud)
        && cloud.form != Form::Empty
    {
        canvas.put(1, cloud.position.x, &["' '"]);
    }
    if let Some(boat) = world.body(Prop::Boat)
        && boat.form != Form::Paper
    {
        canvas.put(HEIGHT - 1, boat.position.x + 2.5, &["~"]);
    }
    canvas.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sprite_is_wholly_occluded_or_wholly_visible() {
        let mut canvas = Canvas::new(16);
        assert!(canvas.put(2, 5.0, &["(=^ω^=)"]));
        let before = canvas.rows.clone();
        assert!(!canvas.put(2, 8.0, &["✿", "[_]"]));
        assert_eq!(canvas.rows, before);
        assert!(!canvas.put(0, 13.0, &["|>", "\\_/"]));
        assert!(!canvas.put(2, 0.0, &["[+]"]));
        assert_eq!(canvas.rows, before);
        assert!(canvas.put(2, 12.0, &["✿", "[_]"]));
        assert!(canvas.finish().contains("(=^ω^=)"));
    }
}
