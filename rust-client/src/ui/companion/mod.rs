//! One persistent companion for the whole application.
mod attention;
mod dialogue;
mod feedback;
mod inbox;
mod parking;
mod placement;

pub(crate) use inbox::Inbox;

use crate::{
    i18n::{self, UiLanguage},
    ui::components::avatar::{Classic, Expression, Gaze, Pose, Presentation, Speech},
};
use attention::{Attention, Target};
use dialogue::{Cue, Route};
use eframe::egui::{self, Id, Pos2, Rect, Vec2};
use std::{
    f32::consts::{PI, TAU},
    time::Duration,
};

pub(crate) struct OnboardingLayout {
    pub features: Option<[Rect; 3]>,
    pub header: Rect,
    pub content: Option<(Rect, egui::LayerId)>,
    pub next: Rect,
    pub footer: Rect,
    pub requirement: Option<&'static str>,
}

pub(crate) enum Layout {
    Onboarding(OnboardingLayout),
    Page { bounds: Rect, layer: egui::LayerId },
}

impl Default for OnboardingLayout {
    fn default() -> Self {
        Self {
            features: None,
            header: Rect::NOTHING,
            content: None,
            next: Rect::NOTHING,
            footer: Rect::NOTHING,
            requirement: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Peek,
    Greet,
    TurnAway,
    WalkAway,
    TurnBack,
    Ready,
}

#[derive(Clone)]
struct Guide {
    stage: Stage,
    stage_started: f64,
    clock: f64,
    last_wall: f64,
    paused: bool,
    background_tick: Option<std::time::Instant>,
    language: UiLanguage,
    route: Route,
    cue: Cue,
    pending: Option<(Cue, f64)>,
    announced: u32,
    center: Pos2,
    velocity: Vec2,
    opacity: f32,
    placement: placement::Placement,
    parking: parking::Parking,
    candidate: Option<Attention>,
    candidate_since: f64,
    focus: Option<Target>,
    resume_at: f64,
    last_activity: f64,
    last_spoken: f64,
    mentioned: u8,
    line: &'static str,
    speech: Speech,
    mouth: f32,
    feedback: feedback::Feedback,
}

impl Guide {
    fn new(
        now: f64,
        language: UiLanguage,
        start: Pos2,
        scene: &dialogue::Context,
        intro: bool,
    ) -> Self {
        Self {
            stage: if intro { Stage::Peek } else { Stage::Ready },
            stage_started: 0.0,
            clock: 0.0,
            last_wall: now,
            paused: true,
            background_tick: None,
            language,
            route: scene.route,
            cue: scene.cue,
            pending: (!intro && matches!(scene.route, Route::Onboarding(_)))
                .then_some((scene.cue, 0.0)),
            announced: 0,
            center: start,
            velocity: Vec2::ZERO,
            opacity: 1.0,
            placement: placement::Placement::default(),
            parking: parking::Parking::default(),
            candidate: None,
            candidate_since: 0.0,
            focus: None,
            resume_at: 0.0,
            last_activity: 0.0,
            last_spoken: 0.0,
            mentioned: 0,
            line: scene.cue.text(),
            speech: Speech::default(),
            mouth: 0.0,
            feedback: feedback::Feedback::default(),
        }
    }

    fn enter(&mut self, stage: Stage) {
        self.stage = stage;
        self.stage_started = self.clock;
    }

    fn say(&mut self, line: &'static str) {
        self.line = line;
        self.last_spoken = self.clock;
        self.speech.say(i18n::tr(self.language, line), self.clock);
    }

    fn follow_scene(&mut self, scene: &dialogue::Context, start: Pos2, intro: bool) {
        let changed_page = self.route != scene.route;
        if changed_page && scene.route == Route::Onboarding(0) {
            // A new welcome visit restarts the whole entrance, including drag
            // placement, speech and announcement history.
            *self = Self::new(self.last_wall, self.language, start, scene, intro);
            return;
        }
        if changed_page {
            self.route = scene.route;
            self.velocity = Vec2::ZERO;
            self.parking = parking::Parking::default();
            self.enter(Stage::Ready);
            self.focus = None;
            self.candidate = None;
            self.candidate_since = self.clock;
            self.mentioned = 0;
        }
        if changed_page || self.cue != scene.cue {
            self.cue = scene.cue;
            self.speech = Speech::default();
            self.mouth = 0.0;
            self.pending = (matches!(scene.route, Route::Onboarding(_))
                && self.announced & scene.cue.bit() == 0)
                .then_some((scene.cue, self.clock));
        }
    }

    fn announce(&mut self, cue: Cue) {
        self.announced |= cue.bit();
        self.say(cue.text());
    }

    fn ready_to_speak(&self) -> bool {
        self.pending.is_none()
            && self.speech.finished(self.clock)
            && self.clock - self.last_spoken >= 3.0
    }

    fn advance_background(&mut self, now: std::time::Instant) -> f32 {
        let dt = self
            .background_tick
            .replace(now)
            .map_or(0.0, |last| now.duration_since(last).as_secs_f64().min(0.5));
        self.clock += dt;
        // A resumed paint pass must not count the hidden time again.
        self.paused = true;
        dt as f32
    }

    fn read_mail(&mut self, inbox: &mut Inbox) {
        if self.stage == Stage::Ready
            && self.ready_to_speak()
            && let Some(message) = inbox.read()
        {
            self.say(message);
        }
    }

    fn attend(&mut self, target: Option<Target>, clicked: bool, scene: &dialogue::Context) {
        let hovered = target.map(|target| target.attention);
        if hovered != self.candidate {
            self.candidate = hovered;
            self.candidate_since = self.clock;
        }
        let dwell = self.clock - self.candidate_since;
        if dwell >= 0.65 {
            self.focus = target;
        }
        if clicked && hovered == Some(Attention::Avatar) {
            self.focus = target;
        }
        if !self.ready_to_speak() {
            return;
        }
        if clicked && hovered == Some(Attention::Avatar) {
            self.say(scene.reply(Attention::Avatar));
        } else if let Some(topic) = hovered
            && dwell >= 1.15
            && topic.bit() != 0
            && self.mentioned & topic.bit() == 0
        {
            self.mentioned |= topic.bit();
            self.say(scene.reply(topic));
        }
    }
}

fn state_id() -> Id {
    Id::new("application_companion")
}

pub(crate) fn show(ctx: &egui::Context, app: &mut crate::XRTranslateApp, layout: Layout) {
    // Navigation can change while the old page is still being drawn. Wait for
    // the matching layout before consuming its companion entry transition.
    if app.first_run != matches!(&layout, Layout::Onboarding(_)) {
        ctx.request_repaint();
        return;
    }
    let (layout, mut page) = match layout {
        Layout::Onboarding(layout) => (Some(layout), None),
        Layout::Page { bounds, layer } => (None, Some((bounds, layer))),
    };
    let scene = dialogue::Context::read(app, layout.as_ref().and_then(|layout| layout.requirement));
    let language = app.ui_language;
    let screen = ctx.viewport_rect();
    // The entrance has its own stage; settled companions share the page's gaps.
    let welcome = layout
        .as_ref()
        .and_then(|layout| {
            layout.features.map(|cards| {
                Rect::from_min_max(
                    egui::pos2(
                        screen.left() + 20.0,
                        cards.iter().map(Rect::bottom).fold(screen.top(), f32::max) + 14.0,
                    ),
                    egui::pos2(screen.right() - 20.0, layout.footer.top() - 10.0),
                )
            })
        })
        .filter(|rect| rect.height() >= 180.0);
    let bounds = welcome
        .or_else(|| {
            layout
                .as_ref()
                .map(|layout| layout.header)
                .filter(|rect| rect.is_finite() && rect.is_positive())
        })
        .unwrap_or(screen.shrink(12.0));
    if welcome.is_none() {
        page = page.or_else(|| layout.as_ref().and_then(|layout| layout.content));
    }
    let (wall, pointer, activity, focused, pressed) = ctx.input(|input| {
        (
            input.time,
            input.pointer.hover_pos(),
            input.pointer.delta().length_sq() > 0.5 || !input.events.is_empty(),
            input.viewport().focused.unwrap_or(true),
            input.pointer.any_down(),
        )
    });
    let radius = (screen.width().min(screen.height()) * 0.067).clamp(28.0, 48.0);
    let small = radius * 0.58;
    let intro = egui::pos2(
        bounds.right() - radius * 2.6,
        bounds.bottom() - radius * 1.6,
    );
    let home = egui::pos2(
        bounds.right() - small * 1.6,
        if welcome.is_some() {
            bounds.top() + small * 1.8
        } else {
            bounds.bottom() - small * 2.0
        },
    );
    let start = egui::pos2(screen.right() + radius * 1.2, intro.y + 12.0);
    let mut state = ctx
        .data(|data| data.get_temp::<Guide>(state_id()))
        .unwrap_or_else(|| {
            Guide::new(
                wall,
                language,
                if welcome.is_some() { start } else { home },
                &scene,
                welcome.is_some(),
            )
        });
    let hidden = app.modal_dialog.open
        || ctx.memory(|memory| memory.top_modal_layer().is_some())
        || screen.height() < radius * 4.0;
    let vr_active = app.vr_overlay_plugin.manager().companion_active();
    let interactive = !hidden && (focused || vr_active) && !ctx.any_popup_open();
    let paused = !interactive || pressed;
    let visual_dt = if interactive {
        (wall - state.last_wall).clamp(0.0, 0.05) as f32
    } else {
        0.0
    };
    let elapsed_wall = if paused || state.paused {
        0.0
    } else {
        (wall - state.last_wall).clamp(0.0, 1.0)
    };
    let dt = (elapsed_wall as f32).min(0.05);
    state.last_wall = wall;
    state.background_tick = None;
    state.paused = paused;
    state.clock += elapsed_wall;
    state.follow_scene(
        &scene,
        if welcome.is_some() { start } else { home },
        welcome.is_some(),
    );
    if welcome.is_none() && state.stage != Stage::Ready {
        state.enter(Stage::Ready);
        state.center = home;
    }
    if state.language != language {
        state.language = language;
        if !state.speech.finished(state.clock) {
            state.say(state.line);
        }
    }
    if hidden {
        app.vr_overlay_plugin
            .manager()
            .present_companion(Presentation::default());
        ctx.data_mut(|data| data.insert_temp(state_id(), state));
        return;
    }
    let dock = page
        .and_then(|(bounds, layer)| state.parking.locate(ctx, layer, bounds, small, state.clock));
    if page.is_some() && dock.is_none() && !vr_active {
        state.velocity = Vec2::ZERO;
        ctx.request_repaint_after(Duration::from_millis(350));
        ctx.data_mut(|data| data.insert_temp(state_id(), state));
        return;
    }
    let small = dock.map_or(small, |spot| spot.radius);
    if activity && !paused {
        state.last_activity = state.clock;
    }
    if !paused
        && !ctx.text_edit_focused()
        && let Some((cue, since)) = state.pending
        && state.clock - since >= 0.55
    {
        state.pending = None;
        state.announce(cue);
    }
    // Read mail on the companion's own clock, without interrupting dialogue,
    // entrance, dragging, hidden/modal states, or its existing speech cooldown.
    if !paused {
        state.read_mail(&mut app.companion_inbox);
    }
    let elapsed = (state.clock - state.stage_started) as f32;
    let previous_center = state.center;
    let mut size = radius;
    let mut yaw = None;
    let mut roll = 0.0;
    let mut offset = Vec2::ZERO;
    let target = match state.stage {
        Stage::Peek => {
            let t = smooth((elapsed - 0.25) / 1.15);
            yaw = Some(0.0);
            roll = -0.60 * (1.0 - smooth((t - 0.4) / 0.6));
            state.center = start.lerp(intro, t);
            if elapsed >= 1.4 {
                state.enter(Stage::Greet);
                state.announce(Cue::Hello);
            }
            state.center
        }
        Stage::Greet => {
            if state.speech.finished(state.clock) {
                state.enter(Stage::TurnAway);
            }
            intro
        }
        Stage::TurnAway => {
            yaw = Some(-PI * smooth(elapsed / 0.7));
            if elapsed >= 0.7 {
                state.enter(Stage::WalkAway);
            }
            intro
        }
        Stage::WalkAway => {
            let t = (elapsed / 1.65).clamp(0.0, 1.0);
            yaw = Some(-PI);
            size = egui::lerp(radius..=small, smooth(t));
            // Three small steps, tapered at both ends. No idle bobbing.
            let envelope = (PI * t).sin();
            roll = (t * TAU * 3.0).sin() * 0.045 * envelope;
            offset.y = -(t * PI * 3.0).sin().abs() * 3.0 * envelope;
            state.center = intro.lerp(home, smooth(t));
            if t >= 1.0 {
                state.enter(Stage::TurnBack);
            }
            state.center
        }
        Stage::TurnBack => {
            size = small;
            yaw = Some(-PI - PI * smooth(elapsed / 0.75));
            if elapsed >= 0.75 {
                state.enter(Stage::Ready);
            }
            home
        }
        Stage::Ready => {
            size = small;
            if state.clock < state.resume_at {
                state.center
            } else if let Some(spot) = dock {
                spot.center
            } else if !layout
                .as_ref()
                .is_some_and(|layout| layout.features.is_some())
            {
                home
            } else {
                state.focus.map_or(state.center, |focus| {
                    focus.destination(state.center, small, screen)
                })
            }
        }
    };
    size = state.placement.radius(size, small, visual_dt);
    // Clamp the destination as well as the body: an unreachable target otherwise
    // keeps walking into the same window edge on every frame.
    let target = if state.stage == Stage::Ready {
        let center = placement::constrain(state.center, size, screen);
        if center != state.center {
            state.velocity = Vec2::ZERO;
        }
        state.center = center;
        placement::constrain(target, size, screen)
    } else {
        target
    };
    if state.stage == Stage::Ready {
        placement::approach(&mut state.center, &mut state.velocity, target, dt);
    } else {
        let movement = (target - state.center) * (1.0 - (-dt / 0.20).exp());
        state.center += movement.normalized() * movement.length().min(640.0 * dt);
    }
    let mut anchor = state.center + offset;
    let response = state
        .placement
        .interact(ctx, state_id(), &mut anchor, size, interactive);
    if state.stage == Stage::Ready {
        state.center = anchor;
    }
    if response.dragged_by(egui::PointerButton::Primary)
        || response.drag_stopped_by(egui::PointerButton::Primary)
    {
        if state.stage != Stage::Ready {
            state.enter(Stage::Ready);
        }
        state.center = anchor;
        state.velocity = Vec2::ZERO;
        state.resume_at = state.clock + 1.2;
        state.focus = None;
        state.candidate = None;
        state.candidate_since = state.clock;
        if page.is_some() && response.drag_stopped_by(egui::PointerButton::Primary) {
            state.parking.dropped(anchor, size);
        }
        yaw = None;
        roll = 0.0;
    }
    let focus = Target::read(ctx, layout.as_ref(), &response);
    let hovered = focus.map(|target| target.attention);
    if state.stage == Stage::Ready && !paused && state.clock >= state.resume_at {
        state.attend(focus, response.clicked(), &scene);
    } else {
        state.candidate_since = state.clock;
    }
    let attentive = state.stage == Stage::Ready && state.clock - state.last_activity < 12.0;
    let gaze = if attentive && !paused {
        pointer.map_or(Gaze::default(), |point| {
            Gaze::toward(point - state.center, 420.0)
        })
    } else {
        Gaze::default()
    };
    let reaction = state.feedback.update(app, state.clock);
    let expression =
        if state.stage == Stage::Greet || (attentive && hovered == Some(Attention::Avatar)) {
            Expression::Happy
        } else {
            reaction.unwrap_or(Expression::Calm)
        };
    let mut pose = Pose::animated(ctx, state_id(), expression, gaze);
    if let Some(yaw) = yaw {
        pose.gaze.yaw = yaw;
    }
    pose.roll = roll;
    let mouth = state.speech.advance(state.clock);
    state.mouth += (mouth - state.mouth) * (1.0 - (-dt / 0.045).exp());
    pose.speech = state.mouth;

    let layer = egui::LayerId::new(egui::Order::Foreground, state_id());
    let speed = if visual_dt > 0.0 {
        anchor.distance(previous_center) / visual_dt
    } else {
        0.0
    };
    let opacity = placement::opacity_at_speed(speed);
    let fade_time = if opacity < state.opacity { 0.08 } else { 0.3 };
    state.opacity += (opacity - state.opacity) * (1.0 - (-visual_dt / fade_time).exp());
    let mut painter = ctx.layer_painter(layer).with_clip_rect(screen);
    painter.multiply_opacity(state.opacity);
    Classic::model().paint(&painter, state_id(), anchor, size, pose);
    let talking = state.speech.paint_avoiding(
        &painter,
        if state.stage == Stage::Ready {
            screen.shrink(12.0)
        } else {
            bounds
        },
        anchor,
        size,
        state.clock,
        visual_dt,
        state.parking.occupied(),
        speed < 12.0 && anchor.distance(target) < 2.0 && !response.dragged(),
    );
    if interactive
        && (response.dragged() || (state.placement.was_dragged() && (size - small).abs() > 0.1))
    {
        ctx.request_repaint_after(Duration::from_millis(16));
    }
    if !paused {
        // Sleep once settled; pointer events and Pose's scheduled blink wake us.
        let moving = state.stage != Stage::Ready || state.center.distance(target) > 0.2;
        let dwelling = state.pending.is_some()
            || state.candidate != state.focus.map(|target| target.attention)
            || hovered.is_some_and(|topic| topic.bit() != 0 && state.mentioned & topic.bit() == 0);
        ctx.request_repaint_after(
            if moving || talking || (state.opacity - opacity).abs() > 0.005 {
                Duration::from_millis(16)
            } else if dwelling {
                Duration::from_millis(80)
            } else {
                Duration::from_secs(1)
            },
        );
    }
    app.vr_overlay_plugin
        .manager()
        .present_companion(Presentation {
            pose,
            speech: state.speech.clone(),
            clock: state.clock,
            visible: !hidden,
        });
    if vr_active {
        ctx.request_repaint_after(Duration::from_millis(33));
    }
    ctx.data_mut(|data| data.insert_temp(state_id(), state));
}

/// eframe's hidden-window logic hook runs without a paint pass. Keep the same
/// Guide/mailbox alive using a monotonic clock rather than frozen egui input time.
pub(crate) fn tick_background(ctx: &egui::Context, app: &mut crate::XRTranslateApp) {
    let Some(mut state) = ctx.data(|data| data.get_temp::<Guide>(state_id())) else {
        return;
    };
    if !app.vr_overlay_plugin.manager().companion_active() || app.modal_dialog.open {
        state.background_tick = None;
        app.vr_overlay_plugin
            .manager()
            .present_companion(Presentation::default());
        ctx.data_mut(|data| data.insert_temp(state_id(), state));
        return;
    }
    let dt = state.advance_background(std::time::Instant::now());
    let scene = dialogue::Context::read(app, None);
    state.follow_scene(&scene, state.center, false);
    if state.language != app.ui_language {
        state.language = app.ui_language;
    }
    state.read_mail(&mut app.companion_inbox);
    let expression = state
        .feedback
        .update(app, state.clock)
        .unwrap_or(Expression::Calm);
    let mut pose = Pose::idle_at(state.clock, expression);
    let mouth = state.speech.advance(state.clock);
    state.mouth += (mouth - state.mouth) * (1.0 - (-dt / 0.045).exp());
    pose.speech = state.mouth;
    app.vr_overlay_plugin
        .manager()
        .present_companion(Presentation {
            pose,
            speech: state.speech.clone(),
            clock: state.clock,
            visible: true,
        });
    ctx.data_mut(|data| data.insert_temp(state_id(), state));
    ctx.request_repaint_after(Duration::from_millis(33));
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mail_waits_for_current_speech_and_cooldown() {
        let scene = dialogue::Context {
            route: Route::Page(crate::ui::Page::Translation),
            cue: Cue::Translation,
            blocked: None,
        };
        let mut guide = Guide::new(0.0, UiLanguage::English, Pos2::ZERO, &scene, false);
        let mut inbox = Inbox::default();
        guide.clock = 2.9;
        assert!(!guide.ready_to_speak()); // Even an empty bubble respects the cooldown.
        guide.clock = 3.0;
        assert!(guide.ready_to_speak());
        guide.say("Select audio and start.");
        inbox.post("Translation session is not active. Please start translation first.");
        inbox.post("Translation session is not active. Please start translation first.");
        guide.clock = 6.0;
        assert!(!guide.ready_to_speak()); // The current sentence is still being spoken.
        guide.speech.advance(guide.clock);
        guide.clock = 20.0;
        assert!(guide.ready_to_speak());
        guide.read_mail(&mut inbox);
        assert_eq!(
            guide.line,
            "Translation session is not active. Please start translation first."
        );
        assert_eq!(inbox.read(), None);
        assert!(!guide.ready_to_speak());
    }

    #[test]
    fn background_clock_reads_mail_without_a_paint_pass() {
        let scene = dialogue::Context {
            route: Route::Page(crate::ui::Page::Translation),
            cue: Cue::Translation,
            blocked: None,
        };
        let mut guide = Guide::new(123.0, UiLanguage::English, Pos2::ZERO, &scene, false);
        guide.say("Select audio and start.");
        let mut inbox = Inbox::default();
        let message = "Translation session is not active. Please start translation first.";
        inbox.post(message);
        inbox.post(message);
        let now = std::time::Instant::now();
        for tick in 0..=100 {
            guide.advance_background(now + Duration::from_millis(tick * 100));
            guide.read_mail(&mut inbox);
            guide.speech.advance(guide.clock);
        }
        assert_eq!(guide.line, message);
        assert_eq!(inbox.read(), None);
        assert!((guide.clock - 10.0).abs() < 0.001);
        assert_eq!(guide.last_wall, 123.0); // No egui wall clock or paint needed.
        assert!(guide.paused); // Resuming painting skips its first elapsed interval.
        let before = guide.clock;
        guide.advance_background(now + Duration::from_secs(200));
        assert!((guide.clock - before - 0.5).abs() < 0.001);
    }

    #[test]
    fn mail_waits_for_entrance_and_pending_dialogue() {
        let scene = dialogue::Context {
            route: Route::Page(crate::ui::Page::Translation),
            cue: Cue::Translation,
            blocked: None,
        };
        let mut guide = Guide::new(0.0, UiLanguage::English, Pos2::ZERO, &scene, true);
        let mut inbox = Inbox::default();
        inbox.post("Translation session is not active. Please start translation first.");
        guide.clock = 20.0;
        guide.read_mail(&mut inbox);
        assert_eq!(guide.line, scene.cue.text());
        guide.enter(Stage::Ready);
        guide.pending = Some((Cue::Hello, guide.clock));
        guide.read_mail(&mut inbox);
        assert_eq!(guide.line, scene.cue.text());
        guide.pending = None;
        guide.read_mail(&mut inbox);
        assert_eq!(
            guide.line,
            "Translation session is not active. Please start translation first."
        );
        assert_eq!(inbox.read(), None);
    }
}
