//! Fixed-step locomotion, gravity and contact resolution; independent of goals.
use super::world::{Form, Prop, World};

pub(super) fn step(world: &mut World, dt: f32) {
    world.time += dt;
    let previous_heights = [
        world.actors[0].position.y,
        world.actors.get(1).map_or(0.0, |actor| actor.position.y),
    ];
    let box_x = world.body(Prop::Box).map(|body| body.position.x);
    for actor in &mut world.actors {
        let r = actor.radius();
        let difference = actor.goal.map_or(0.0, |goal| goal - actor.position.x);
        let wanted = difference.clamp(-1.0, 1.0) * 2.0;
        actor.velocity.x += (wanted - actor.velocity.x).clamp(-5.0 * dt, 5.0 * dt);
        if difference.abs() > 0.2 {
            actor.facing = difference.signum();
        }
        if let Some(x) = box_x
            && difference.abs() > 0.5
            && actor.grounded
            && (x - actor.position.x).signum() == difference.signum()
            && (x - actor.position.x).abs() < r + 3.0
        {
            actor.velocity.y = 4.4;
            actor.grounded = false;
        }
        let previous_y = actor.position.y;
        actor.velocity.y -= 7.0 * dt;
        actor.position.x = (actor.position.x + actor.velocity.x * dt).clamp(r, world.width - r);
        actor.position.y += actor.velocity.y * dt;
        actor.grounded = false;
        if let Some(x) = box_x
            && (actor.position.x - x).abs() < r + Prop::Box.radius()
            && actor.position.y < 0.7
        {
            if previous_y >= 0.7 && actor.velocity.y <= 0.0 {
                actor.position.y = 0.7;
                actor.velocity.y = 0.0;
                actor.grounded = true;
            } else {
                let side = if actor.position.x < x { -1.0 } else { 1.0 };
                actor.position.x = (x + side * (r + Prop::Box.radius())).clamp(r, world.width - r);
                actor.velocity.x = 0.0;
            }
        }
        if actor.position.y <= 0.0 {
            actor.position.y = 0.0;
            actor.velocity.y = 0.0;
            actor.grounded = true;
        }
    }
    // Actors cannot walk through one another. A moving actor can hop past a
    // companion; separation is solved only while their vertical spans meet.
    if world.actors.len() == 2 {
        let (left, right) = world.actors.split_at_mut(1);
        let (a, b) = (&mut left[0], &mut right[0]);
        let reach = a.radius() + b.radius() + 0.2;
        if (a.position.x - b.position.x).abs() < reach && (a.position.y - b.position.y).abs() < 0.8
        {
            if previous_heights[0] >= b.position.y + 0.8 && a.velocity.y <= 0.0 {
                a.position.y = b.position.y + 0.8;
                a.velocity.y = 0.0;
                a.grounded = true;
                advance_props(world, dt);
                return;
            }
            if a.goal.is_some() && a.grounded {
                a.velocity.y = 4.4;
                a.grounded = false;
            }
            let middle = (a.position.x + b.position.x) / 2.0;
            let side = if a.position.x < b.position.x {
                -1.0
            } else {
                1.0
            };
            a.position.x =
                (middle + side * reach / 2.0).clamp(a.radius(), world.width - a.radius());
            b.position.x =
                (middle - side * reach / 2.0).clamp(b.radius(), world.width - b.radius());
        }
    }
    advance_props(world, dt);
}

fn advance_props(world: &mut World, dt: f32) {
    for body in &mut world.props {
        let r = body.prop.radius();
        if body.prop == Prop::Star && body.form == Form::Packed && world.time - body.born > 8.0 {
            body.form = Form::Normal;
            body.dynamic = true;
        }
        if let Some(index) = body.carried {
            let actor = &world.actors[index];
            body.position = actor.hand(r, world.width);
        } else if body.prop == Prop::Butterfly {
            let target = world.actors[0].position.x + world.actors[0].facing * 3.0;
            body.velocity.x += ((target - body.position.x) * 0.6 - body.velocity.x) * dt;
            body.position.x = (body.position.x + body.velocity.x * dt).clamp(r, world.width - r);
            body.position.y = 1.8 + (world.time * 1.4).sin() * 0.35;
        } else if body.dynamic && !matches!(body.form, Form::Packed | Form::Empty) {
            body.velocity.y -= if body.prop == Prop::Star {
                0.7 * dt
            } else {
                7.0 * dt
            };
            body.position.x += body.velocity.x * dt;
            body.position.y += body.velocity.y * dt;
            if body.position.x < r || body.position.x > world.width - r {
                body.position.x = body.position.x.clamp(r, world.width - r);
                body.velocity.x *= -0.65;
            }
            if body.position.y <= 0.0 {
                body.position.y = 0.0;
                body.velocity.y = if body.velocity.y < -1.0 {
                    -body.velocity.y * 0.3
                } else {
                    0.0
                };
                body.velocity.x *= (1.0 - dt * 0.35).max(0.0);
            }
        }
    }
    // All released props share the same resting contacts and restitution.
    for i in 0..world.props.len() {
        if !world.props[i].dynamic
            || world.props[i].carried.is_some()
            || world.props[i].position.y > 0.1
            || !world.props[i].present()
        {
            continue;
        }
        for j in 0..world.props.len() {
            if i == j
                || world.props[j].carried.is_some()
                || world.props[j].position.y > 0.1
                || !world.props[j].present()
            {
                continue;
            }
            let reach = world.props[i].prop.radius() + world.props[j].prop.radius() + 0.1;
            let delta = world.props[i].position.x - world.props[j].position.x;
            if delta.abs() < reach {
                let side = if delta < 0.0 { -1.0 } else { 1.0 };
                let r = world.props[i].prop.radius();
                world.props[i].position.x =
                    (world.props[j].position.x + side * reach).clamp(r, world.width - r);
                world.props[i].velocity.x *= -0.4;
            }
        }
    }
}
