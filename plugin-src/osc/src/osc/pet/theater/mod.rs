//! Utility selection -> reusable actions -> fixed-step world -> bounded canvas.
//! Only visible time advances; captions suspend the same world without skipping
//! actions. Small world facts and recent goals replace a large scripted FSM.
mod actions;
mod motion;
mod sprites;
mod stage;
mod stories;
#[cfg(test)]
mod tests;
mod world;

use super::{PetSettings, REFRESH_INTERVAL};
use actions::{Outcome, Running};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use stories::Story;
use world::World;

pub(crate) type Cast = [Option<PetSettings>; 2];
const STEP: f32 = 0.1;

struct Goal {
    story: Story,
    step: usize,
    action: Running,
    variant: u8,
}

pub(crate) struct Theater {
    cast: Cast,
    world: World,
    recent: [Option<Story>; 4],
    goal: Option<Goal>,
    random: u64,
    accumulated: f32,
    last_visible: Option<Instant>,
    rendered: bool,
}

impl Default for Theater {
    fn default() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQUENCE: AtomicU64 = AtomicU64::new(1);
        let seed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64
            ^ SEQUENCE
                .fetch_add(1, Ordering::Relaxed)
                .wrapping_mul(0x9e3779b97f4a7c15);
        Self::seeded(seed)
    }
}

impl Theater {
    fn seeded(seed: u64) -> Self {
        Self {
            cast: [None; 2],
            world: World::default(),
            recent: [None; 4],
            goal: None,
            random: seed.max(1),
            accumulated: 0.0,
            last_visible: None,
            rendered: false,
        }
    }

    pub fn begin_frame(&mut self, cast: Cast) {
        if self.cast != cast {
            *self = Self {
                cast,
                ..Self::seeded(self.random)
            };
        }
        self.rendered = false;
    }

    pub fn end_frame(&mut self) {
        if !self.rendered {
            self.pause();
        }
    }
    pub fn pause(&mut self) {
        self.last_visible = None;
    }

    fn roll(&mut self) -> u64 {
        self.random ^= self.random << 13;
        self.random ^= self.random >> 7;
        self.random ^= self.random << 17;
        self.random
    }

    fn choose(&mut self, music: bool) {
        let weights = Story::ALL.map(|story| {
            if self.recent.contains(&Some(story)) {
                0
            } else {
                story.weight(&self.world, music)
            }
        });
        let mut ticket = self.roll() % u64::from(weights.iter().sum::<u32>());
        let story = Story::ALL
            .into_iter()
            .zip(weights)
            .find_map(|(story, weight)| {
                if ticket < u64::from(weight) {
                    Some(story)
                } else {
                    ticket -= u64::from(weight);
                    None
                }
            })
            .unwrap();
        self.recent.rotate_right(1);
        self.recent[0] = Some(story);
        story.prepare(&mut self.world);
        self.goal = Some(Goal {
            story,
            step: 0,
            action: Running::default(),
            variant: (self.roll() % 3) as u8,
        });
    }

    pub fn render(
        &mut self,
        budget: usize,
        lines: usize,
        music: bool,
        now: Instant,
    ) -> Option<String> {
        let width = stage::width(budget, lines)?;
        let actors = self.cast.iter().flatten().count();
        // Two full faces and an interaction need the full stage. Fall back to
        // compact banner pets rather than squeezing a scene into unreadable art.
        if actors == 0 || (actors > 1 && width < 24) {
            return None;
        }
        self.rendered = true;
        if self.world.actors.is_empty() {
            self.world = World::new(width - 2, self.cast);
        }
        self.world.resize(width - 2);
        let delta = self
            .last_visible
            .replace(now)
            .map_or(Duration::ZERO, |last| {
                now.saturating_duration_since(last)
                    .min(REFRESH_INTERVAL * 2)
            });
        self.accumulated += delta.as_secs_f32();
        if self.goal.is_none() {
            self.choose(music);
        }
        while self.accumulated + f32::EPSILON >= STEP {
            self.accumulated -= STEP;
            self.tick(music);
        }
        Some(stage::render(
            &self.world,
            self.goal.as_ref().unwrap().story,
        ))
    }

    fn tick(&mut self, music: bool) {
        let goal = self.goal.as_mut().unwrap();
        let step = goal.story.steps()[goal.step];
        let outcome = goal.action.tick(step, &mut self.world, goal.variant, STEP);
        if self.world.actors.len() == 2 {
            let x = self.world.actors[0].position.x;
            self.world.look(1, x);
            self.world.actors[1].face = if goal.story == Story::Share && goal.step >= 3 {
                world::Face::Chew
            } else if self.world.actors[0].face == world::Face::Sleep {
                world::Face::Sleep
            } else {
                world::Face::Curious
            };
        }
        self.world.advance(STEP);
        if outcome == Outcome::Complete {
            goal.step += 1;
            goal.action = Running::default();
        }
        if outcome == Outcome::Failed || goal.step == goal.story.steps().len() {
            self.choose(music);
        }
    }
}
