use super::super::PetMood;
use super::*;
use actions::{Action, Effect, Step as ActionStep};
use world::{Face, Form, Place, Prop};

fn cast() -> Cast {
    [Some(PetSettings::default()), None]
}

fn frame(theater: &mut Theater, cast: Cast, budget: usize, now: Instant) -> Option<String> {
    theater.begin_frame(cast);
    let text = theater.render(budget, 9, true, now);
    theater.end_frame();
    text
}

#[test]
fn actions_require_real_contact_and_follow_a_moving_target() {
    let mut world = World::new(22, cast());
    world.spawn(Prop::Snack, Place::Right, Form::Normal);
    let start = world.actors[0].position.x;
    let mut action = Running::default();
    let step = ActionStep {
        action: Action::Use(Prop::Snack, Effect::Set(Form::Empty), 1.0),
        face: Face::Chew,
        signal: "",
    };
    // Time alone cannot consume a distant object: movement must actually run.
    for _ in 0..50 {
        assert_eq!(action.tick(step, &mut world, 0, STEP), Outcome::Running);
    }
    assert_eq!(world.form(Prop::Snack), Some(Form::Normal));
    assert_eq!(world.actors[0].position.x, start);
    for _ in 0..200 {
        let result = action.tick(step, &mut world, 0, STEP);
        world.advance(STEP);
        if result == Outcome::Complete {
            break;
        }
    }
    assert_eq!(world.form(Prop::Snack), Some(Form::Empty));
    assert!(world.actors[0].position.x > start + 1.0);

    world.spawn(Prop::Ball, Place::Left, Form::Normal);
    world.approach(0, Prop::Ball);
    let old_goal = world.actors[0].goal;
    world.body_mut(Prop::Ball).unwrap().position.x += 2.0;
    world.approach(0, Prop::Ball);
    assert_ne!(old_goal, world.actors[0].goal);
}

#[test]
fn motion_obeys_boundaries_gravity_and_object_attachment() {
    let mut world = World::new(22, cast());
    world.spawn(Prop::Ball, Place::Ahead, Form::Normal);
    world.body_mut(Prop::Ball).unwrap().velocity.x = 8.0;
    world.jump(0);
    let mut airborne = false;
    let mut bounced = false;
    for _ in 0..100 {
        world.advance(STEP);
        let actor = &world.actors[0];
        let ball = world.body(Prop::Ball).unwrap();
        airborne |= actor.position.y > 0.5;
        bounced |= ball.velocity.x < 0.0;
        assert!(
            actor.position.x >= actor.radius() && actor.position.x <= world.width - actor.radius()
        );
        assert!(
            ball.position.x >= Prop::Ball.radius()
                && ball.position.x <= world.width - Prop::Ball.radius()
        );
        assert!(actor.position.y >= 0.0 && ball.position.y >= 0.0);
    }
    assert!(airborne && bounced);
    assert!(world.actors[0].grounded);
    for _ in 0..200 {
        if world.attach(0, Prop::Ball) {
            break;
        }
        world.advance(STEP);
    }
    assert_eq!(world.body(Prop::Ball).unwrap().carried, Some(0));
    world.walk(0, 14.0);
    for _ in 0..20 {
        world.advance(STEP);
    }
    assert!(
        (world.body(Prop::Ball).unwrap().position.y - world.actors[0].position.y - 0.6).abs()
            < 0.01
    );
}

#[test]
fn world_facts_unlock_followups_and_recent_goals_do_not_repeat() {
    let mut world = World::new(22, cast());
    assert_eq!(Story::Water.weight(&world, false), 0);
    assert_eq!(Story::Tidy.weight(&world, false), 0);
    assert_eq!(Story::Gift.weight(&world, false), 0);
    world.spawn(Prop::Plant, Place::Left, Form::Sprout);
    world.spawn(Prop::Ball, Place::Right, Form::Normal);
    world.spawn(Prop::Star, Place::Right, Form::Normal);
    assert!(Story::Water.weight(&world, false) > 0);
    assert!(Story::Tidy.weight(&world, false) > 0);
    assert!(Story::Gift.weight(&world, false) > 0);
    let mut theater = Theater::seeded(42);
    theater.world = world;
    theater.cast = cast();
    for _ in 0..80 {
        let recent = theater.recent;
        theater.choose(true);
        assert!(!recent.contains(&Some(theater.goal.as_ref().unwrap().story)));
    }
}

#[test]
fn dropped_objects_follow_the_current_paw_then_settle_and_missing_targets_fail() {
    let mut world = World::new(22, cast());
    world.spawn(Prop::Can, Place::Ahead, Form::Normal);
    for _ in 0..100 {
        if world.attach(0, Prop::Can) {
            break;
        }
        world.advance(STEP);
    }
    assert_eq!(world.body(Prop::Can).unwrap().carried, Some(0));
    world.actors[0].facing = -1.0;
    world.drop(Prop::Can);
    assert!(world.body(Prop::Can).unwrap().position.x < world.actors[0].position.x);
    for _ in 0..40 {
        world.advance(STEP);
    }
    assert_eq!(world.body(Prop::Can).unwrap().position.y, 0.0);
    let mut action = Running::default();
    assert_eq!(
        action.tick(
            ActionStep {
                action: Action::Pick(Prop::Snack),
                face: Face::Curious,
                signal: "?"
            },
            &mut world,
            0,
            STEP
        ),
        Outcome::Failed
    );
}

#[test]
fn interruption_preserves_the_world_and_repeated_frames_do_not_advance_time() {
    let mut theater = Theater::seeded(1);
    let start = Instant::now();
    frame(&mut theater, cast(), 144, start);
    let visible = frame(&mut theater, cast(), 144, start + Duration::from_secs(2));
    let clock = theater.world.time;
    assert_eq!(
        frame(&mut theater, cast(), 144, start + Duration::from_secs(2)),
        visible
    );
    assert_eq!(theater.world.time, clock);
    theater.begin_frame(cast());
    theater.end_frame(); // A caption owns this frame.
    assert_eq!(
        frame(&mut theater, cast(), 144, start + Duration::from_secs(300)),
        visible
    );
    assert_eq!(theater.world.time, clock);
    assert!(frame(&mut theater, cast(), 20, start + Duration::from_secs(301)).is_none());
    assert_eq!(
        frame(&mut theater, cast(), 144, start + Duration::from_secs(500)),
        visible
    );
}

#[test]
fn scene_canvas_remains_bounded_through_long_runs_and_budget_changes() {
    let now = Instant::now();
    for mood in [
        PetMood::Natural,
        PetMood::Sleepy,
        PetMood::Happy,
        PetMood::Shy,
    ] {
        let mut theater = Theater::seeded(7);
        let cast = [Some(PetSettings { mood }), Some(PetSettings::default())];
        for tick in 0..900 {
            let budget = [144, 119, 94, 50][tick % 4];
            if let Some(text) = frame(
                &mut theater,
                cast,
                budget,
                now + REFRESH_INTERVAL * tick as u32,
            ) {
                assert!(text.chars().count() <= budget);
                assert_eq!(text.lines().count(), 3);
                let lengths = text
                    .lines()
                    .map(|line| line.chars().count())
                    .collect::<Vec<_>>();
                assert!(lengths.iter().all(|length| *length == lengths[0]));
                assert!(theater.world.props.len() <= 8);
                for actor in &theater.world.actors {
                    if !actor.hidden {
                        assert!(
                            text.contains(&sprites::portrait(actor)),
                            "a complete face must remain visible: {text}"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn crowded_stages_fall_back_without_advancing_or_squeezing_the_world() {
    let now = Instant::now();
    let mut theater = Theater::seeded(42);
    let cast = [Some(PetSettings::default()); 2];
    let visible = frame(&mut theater, cast, 144, now).unwrap();
    assert_eq!(visible.chars().count(), 74);
    assert!(frame(&mut theater, cast, 70, now + Duration::from_secs(5)).is_none());
    assert_eq!(
        frame(&mut theater, cast, 144, now + Duration::from_secs(300)),
        Some(visible)
    );
    assert!(frame(&mut theater, [cast[0], None], 56, now).is_some());
    assert!(frame(&mut theater, [cast[0], None], 55, now).is_none());
}

#[test]
fn cues_and_scenery_cannot_overwrite_a_face_or_a_carried_object() {
    let mut world = World::new(22, cast());
    world.spawn(Prop::Plant, Place::Right, Form::Flower);
    world.spawn(Prop::Umbrella, Place::Ahead, Form::Normal);
    world.actors[0].signal = "...";
    let face = sprites::portrait(&world.actors[0]);
    let umbrella = world.body_mut(Prop::Umbrella).unwrap();
    umbrella.carried = Some(0);
    umbrella.position = world::Point { x: 8.0, y: 0.0 };
    let flower = world.body_mut(Prop::Plant).unwrap();
    flower.position.x = 8.0; // Its crown would overlap the umbrella.
    let text = stage::render(&world, Story::Rain);
    assert!(text.contains(&face) && text.contains('☂'));
    assert!(!text.contains('✿') && !text.contains("[_]"));
    assert!(!text.contains("...") && !text.contains('ฅ'));
}

#[test]
fn a_companion_does_not_hide_the_story_object_at_its_feet() {
    let mut world = World::new(22, [Some(PetSettings::default()); 2]);
    world.spawn(Prop::Fishing, Place::Right, Form::Fish);
    let text = stage::render(&world, Story::Fish);
    assert!(text.contains("><>"));
    for actor in &world.actors {
        assert!(text.contains(&sprites::portrait(actor)));
    }
    world.spawn(Prop::Umbrella, Place::Ahead, Form::Normal);
    let umbrella = world.body_mut(Prop::Umbrella).unwrap();
    umbrella.carried = Some(0);
    umbrella.position = world::Point { x: 18.0, y: 1.4 };
    assert!(stage::render(&world, Story::Rain).contains('☂'));
}

#[test]
fn every_goal_can_finish_without_teleporting_or_timing_out() {
    for story in Story::ALL {
        let mut world = World::new(22, [Some(PetSettings::default()); 2]);
        match story {
            Story::Water => world.spawn(Prop::Plant, Place::Left, Form::Sprout),
            Story::Tidy => world.spawn(Prop::Ball, Place::Right, Form::Normal),
            Story::Gift => world.spawn(Prop::Star, Place::Left, Form::Normal),
            _ => {}
        }
        story.prepare(&mut world);
        for (index, step) in story.steps().iter().enumerate() {
            let mut action = Running::default();
            let result = loop {
                let result = action.tick(*step, &mut world, 1, STEP);
                world.advance(STEP);
                if result != Outcome::Running {
                    break result;
                }
            };
            assert_eq!(
                result,
                Outcome::Complete,
                "{story:?} step {index}, x={} goal={:?}",
                world.actors[0].position.x,
                world.actors[0].goal
            );
        }
    }
}

#[test]
fn continuous_world_keeps_finishing_varied_goals_as_props_accumulate() {
    for company in [false, true] {
        let mut theater = Theater::seeded(71);
        let cast = [
            Some(PetSettings::default()),
            company.then_some(PetSettings::default()),
        ];
        frame(&mut theater, cast, 144, Instant::now());
        let mut completed = Vec::new();
        let mut failures = 0;
        for _ in 0..18_000 {
            let goal = theater.goal.as_ref().unwrap();
            let (story, step) = (goal.story, goal.step);
            theater.tick(true);
            if theater.goal.as_ref().unwrap().story != story {
                if step + 1 == story.steps().len() {
                    completed.push(story);
                } else {
                    failures += 1;
                }
            }
        }
        let unique = Story::ALL
            .into_iter()
            .filter(|story| completed.contains(story))
            .count();
        assert!(
            unique >= 10 && failures < completed.len() / 4,
            "company={company}: {} complete, {unique} unique, {failures} failures: {completed:?}",
            completed.len()
        );
    }
}
