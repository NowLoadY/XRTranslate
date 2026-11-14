//! Host presentation for the external guide planner. Content and sequencing live in the plugin.
mod reveal;

use super::{Page, components, theme};
use crate::i18n::{UiLanguage, tr};
use eframe::egui::{self, Id, Rect};
use serde_json::json;
use xrtranslate_plugin::guide;

#[derive(Clone)]
struct Session {
    chapters: Vec<guide::Chapter>,
    chapter: usize,
    step: usize,
    started: f64,
    panel: Rect,
    bounds: Rect,
    reveal: reveal::Reveal,
}
impl Session {
    fn lesson(&self) -> &guide::Step {
        &self.steps()[self.step]
    }
    fn steps(&self) -> &[guide::Step] {
        &self.chapters[self.chapter].steps
    }
    fn route(&self) -> &str {
        &self.chapters[self.chapter].page
    }
}

#[derive(Clone)]
enum Action {
    Start,
    Previous,
    Next,
    Page(String),
    Topic(String),
    Exit,
}

#[derive(Clone, Default)]
struct Tour {
    session: Option<Session>,
    action: Option<Action>,
    revision: u64,
    observed: Option<u64>,
    presented: Option<u64>,
    dismissed: bool,
}
fn id() -> Id {
    Id::unique("application_tutorial")
}

fn queue(ctx: &egui::Context, action: Action) {
    ctx.data_mut(|data| {
        let mut tour = data.get_temp::<Tour>(id()).unwrap_or_default();
        tour.action = Some(action);
        data.insert_temp(id(), tour);
    });
    ctx.request_repaint();
}

pub(crate) fn start(ctx: &egui::Context) {
    queue(ctx, Action::Start);
}
pub(crate) fn active(ctx: &egui::Context) -> bool {
    ctx.data(|data| data.get_temp::<Tour>(id()))
        .is_some_and(|tour| tour.session.is_some())
}
fn clear(tour: &mut Tour, inbox: &mut super::companion::Inbox) {
    if let Some(session) = &mut tour.session {
        session.reveal.cancel();
    }
    tour.session = None;
    inbox.end_guide();
}
pub(crate) fn cancel(ctx: &egui::Context, app: &mut crate::XRTranslateApp) {
    ctx.data_mut(|data| {
        let mut tour = data.get_temp::<Tour>(id()).unwrap_or_default();
        if tour.session.is_some() || matches!(tour.action, Some(Action::Start)) {
            let _ = app.plugin_host.invoke(ui_director::ToolCall {
                name: "guide.stop".into(),
                arguments: json!({}),
            });
            tour.action = None;
            tour.dismissed = true;
        }
        clear(&mut tour, &mut app.companion_inbox);
        data.insert_temp(id(), tour);
    });
}

pub(crate) fn before_page(ctx: &egui::Context, app: &mut crate::XRTranslateApp) {
    let mut tour = ctx
        .data(|data| data.get_temp::<Tour>(id()))
        .unwrap_or_default();
    if tour.session.is_some()
        && (app.first_run
            || ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape)))
    {
        tour.action = Some(Action::Exit);
    }
    if !app.plugin_host.available("guide") {
        if matches!(tour.action, Some(Action::Start)) {
            app.notifications.error(
                tr(app.ui_language, "Enable User Guide in Settings → Plugins."),
                "",
            );
        }
        clear(&mut tour, &mut app.companion_inbox);
        tour.action = None;
        tour.observed = None;
        ctx.data_mut(|data| data.insert_temp(id(), tour));
        return;
    }

    let pages: Vec<_> = super::navigation_pages(
        &app.plugin_pages,
        app.service_config.tts_is_configured(),
        app.ui_language,
    )
    .into_iter()
    .filter(|page| !page.route.starts_with("onboarding:"))
    .map(|page| guide::Page {
        route: page.route,
        label: page.label,
    })
    .collect();
    if let Ok(state) =
        serde_json::from_value::<guide::State>(app.plugin_host.state("guide").clone())
        && tour.observed != Some(state.revision)
    {
        tour.observed = Some(state.revision);
        let previous_panel = tour
            .session
            .as_ref()
            .map_or(Rect::NOTHING, |session| session.panel);
        clear(&mut tour, &mut app.companion_inbox);
        if state.session.is_none() {
            tour.dismissed = false;
        }
        if !tour.dismissed
            && !app.first_run
            && let Some(plan) = state.session.filter(|plan| {
                plan.current().is_some()
                    && plan.chapters.iter().all(|chapter| {
                        !chapter.steps.is_empty()
                            && pages.iter().any(|page| page.route == chapter.page)
                    })
            })
            && let Ok(page) =
                serde_json::from_value::<Page>(json!(plan.chapters[plan.chapter].page))
        {
            app.navigation.open(page);
            tour.revision = tour.revision.wrapping_add(1);
            tour.presented = None;
            tour.session = Some(Session {
                chapters: plan.chapters,
                chapter: plan.chapter,
                step: plan.step,
                started: ctx.input(|input| input.time),
                panel: previous_panel,
                bounds: ctx.content_rect(),
                reveal: reveal::Reveal::default(),
            });
        }
    }

    let mut capabilities = Vec::new();
    if super::pages::prompt_studio::editor_available(
        ctx,
        app.update_channel == crate::client_settings::UpdateChannel::Beta,
        &app.project_root(),
    ) {
        capabilities.push("prompt_editor".into());
    }
    app.plugin_host.set_context(
        "guide.context",
        json!(guide::Context {
            page: serde_json::to_value(app.navigation.page)
                .unwrap()
                .as_str()
                .unwrap()
                .into(),
            language: crate::plugin_host::locale(app.ui_language).into(),
            pages,
            capabilities,
        }),
    );

    if let Some(action) = tour.action.take() {
        let (name, arguments) = match action {
            Action::Start => {
                tour.dismissed = false;
                ("guide.start", json!({}))
            }
            Action::Previous => ("guide.previous", json!({})),
            Action::Next => ("guide.next", json!({})),
            Action::Page(page) => ("guide.select", json!({"page": page})),
            Action::Topic(topic) => (
                "guide.select",
                json!({
                    "page": tour.session.as_ref().map(|session| session.route()), "topic": topic
                }),
            ),
            Action::Exit => {
                clear(&mut tour, &mut app.companion_inbox);
                tour.dismissed = true;
                ("guide.stop", json!({}))
            }
        };
        if let Err(error) = app.plugin_host.invoke(ui_director::ToolCall {
            name: name.into(),
            arguments,
        }) {
            clear(&mut tour, &mut app.companion_inbox);
            app.notifications.error(
                tr(app.ui_language, "Enable User Guide in Settings → Plugins."),
                &error,
            );
        }
    }

    if let Some(session) = &mut tour.session {
        let lesson = session.lesson().clone();
        let route = session.route().to_owned();
        if serde_json::to_value(app.navigation.page).unwrap() == route {
            session.reveal.update(ctx, &lesson, &route, app.ui_language);
            app.companion_inbox.guide(tour.revision, lesson.text);
            if session.reveal.finished() && tour.presented != tour.observed {
                app.plugin_host.send_event(
                    "guide",
                    "guide.presented",
                    json!({"revision": tour.observed}),
                );
                tour.presented = tour.observed;
            }
        }
    }
    ctx.data_mut(|data| data.insert_temp(id(), tour));
}

pub(crate) fn controls(ctx: &egui::Context, bounds: Rect, language: UiLanguage) {
    let Some(mut tour) = ctx.data(|data| data.get_temp::<Tour>(id())) else {
        return;
    };
    let Some(session) = &mut tour.session else {
        return;
    };
    let width = (bounds.width() - 48.0).clamp(220.0, 300.0);
    let mut action = None;
    let panel = egui::Area::new(id().with("controls"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::RIGHT_BOTTOM)
        .default_pos(bounds.right_bottom() - egui::vec2(12.0, 12.0))
        .constrain_to(bounds.shrink(8.0))
        .movable(true)
        .show(ctx, |ui| {
            egui::Frame::new().fill(theme::surface_panel())
                .stroke(egui::Stroke::new(1.0, theme::border())).corner_radius(14).inner_margin(10)
                .shadow(egui::epaint::Shadow { offset: [0, 3], blur: 14, spread: 0, color: egui::Color32::from_black_alpha(16) })
                .show(ui, |ui| {
                    ui.set_width(width);
                    ui.horizontal(|ui| {
                        components::combobox_ui_with_width(ui, "guide_page", &session.chapters[session.chapter].label, Some(width - 36.0), |ui| {
                            for (index, chapter) in session.chapters.iter().enumerate() {
                                if components::selectable_label(ui, index == session.chapter, &chapter.label).clicked() { action = Some(Action::Page(chapter.page.clone())); ui.close(); }
                            }
                        });
                        let response = ui.add(egui::Button::new("×").frame(false).min_size(egui::Vec2::splat(24.0)));
                        if ui_director::egui::Control::button(tr(language, "Exit guide")).response(ui, response)
                            .on_hover_text(format!("{} · Esc", tr(language, "Exit guide"))).clicked() { action = Some(Action::Exit); }
                    });
                    ui.horizontal(|ui| {
                        let previous = ui.add_enabled_ui(session.chapter > 0 || session.step > 0, |ui| {
                            components::icon_button(ui, tr(language, "Previous feature"), egui::include_image!("../../../crates/xrtranslate-client-core/resources/icons/chevron-left.svg"))
                        }).inner;
                        if previous.clicked() { action = Some(Action::Previous); }
                        let label = format!("{} · {}/{}", &session.lesson().title, session.step + 1, session.steps().len());
                        components::combobox_ui_with_width(ui, "guide_topic", label, Some(width - 80.0), |ui| {
                            for (index, step) in session.steps().iter().enumerate() {
                                if components::selectable_label(ui, index == session.step, &step.title).clicked() { action = Some(Action::Topic(step.id.clone())); ui.close(); }
                            }
                        });
                        let last = session.chapter + 1 == session.chapters.len() && session.step + 1 == session.steps().len();
                        if components::icon_button(ui, tr(language, if last { "Finish guide" } else { "Next feature" }),
                            egui::include_image!("../../../crates/xrtranslate-client-core/resources/icons/chevron-right.svg")).clicked() { action = Some(Action::Next); }
                    });
                    ui_director::egui::record_output(ui, "Guide narration", ui_director::egui::ElementValue::Text(session.lesson().text.clone()), ui.min_rect());
                });
        }).response.rect;
    session.panel = panel;
    session.bounds = bounds;
    if action.is_some() {
        tour.action = action;
        ctx.request_repaint();
    }
    ctx.data_mut(|data| data.insert_temp(id(), tour));
}

pub(super) fn panel(ctx: &egui::Context) -> Option<Rect> {
    ctx.data(|data| data.get_temp::<Tour>(id()))?
        .session
        .map(|session| session.panel)
        .filter(|rect| rect.is_finite() && rect.is_positive())
}

/// The guide and its controls occupy opposite sides; neither reserves page space.
pub(super) fn avatar_position(bounds: Rect, radius: f32) -> egui::Pos2 {
    egui::pos2(
        bounds.left() + radius * 2.2,
        if bounds.width() < 620.0 {
            bounds.top() + radius * 3.0
        } else {
            bounds.bottom() - radius * 2.4
        },
    )
}

pub(super) fn paint(ctx: &egui::Context, painter: &egui::Painter, language: UiLanguage) {
    let Some(session) = ctx
        .data(|data| data.get_temp::<Tour>(id()))
        .and_then(|tour| tour.session)
    else {
        return;
    };
    let snapshot = ui_director::egui::driver().rendered_snapshot();
    let controls: Vec<_> = snapshot
        .elements
        .into_iter()
        .filter(|control| reveal::matches(&control.label, &session.lesson().targets, language))
        .map(|mut control| {
            let rect = control
                .visible_rect()
                .unwrap_or(Rect::NOTHING)
                .intersect(session.bounds);
            control.clip_rect = Some([rect.min.x, rect.min.y, rect.width(), rect.height()]);
            control
        })
        .take(2)
        .collect();
    let age = (ctx.input(|input| input.time) - session.started).max(0.0) as f32;
    ui_director::egui::feedback::paint_guidance(
        painter,
        &controls,
        None,
        theme::primary(),
        age,
        age < 0.3,
    );
}
