//! Application adapter: bind the floating result to the shared task lifecycle.
use super::OWNER;
use crate::history::archive::vocabulary;
use crate::overlay_ipc::{OverlayEvent, QuickTranslationAction as Action};
use crate::{
    XRTranslateApp,
    session_coordinator::{
        PluginSessionBinding, PluginSessionOwner, SessionOutputPolicy, TranslationTask,
    },
};
use eframe::egui;
impl XRTranslateApp {
    fn stop_quick_translation(&mut self) {
        let owners: Vec<_> = self
            .text_translation
            .scopes()
            .filter(|scope| scope.owner.is_plugin(OWNER))
            .map(|scope| scope.owner.clone())
            .collect();
        for owner in owners {
            self.cancel_text_tasks(Some(&owner));
        }
    }
    pub(crate) fn begin_quick_translation(
        &mut self,
        text: Result<String, String>,
        ctx: &egui::Context,
    ) {
        self.stop_quick_translation();
        self.quick_translate.inline = None;
        self.quick_translate.route = None;
        let text = match text {
            Ok(text) if !text.trim().is_empty() => text,
            Ok(_) => {
                self.quick_translate.begin(String::new());
                self.quick_translate
                    .fail("Copy some text, then try again.".into());
                return;
            }
            Err(error) => {
                self.quick_translate.begin(String::new());
                self.quick_translate.fail(error);
                return;
            }
        };
        self.quick_translate.route = Some((self.source_lang.clone(), self.target_lang.clone()));
        self.submit_quick_translation(text, None, ctx);
    }

    pub(crate) fn begin_history_lookup(
        &mut self,
        text: String,
        context: String,
        context_offset: usize,
        anchor: egui::Rect,
        ctx: &egui::Context,
    ) {
        self.stop_quick_translation();
        self.quick_translate.close();
        self.quick_translate.inline =
            Some(super::inline::Lookup::new(context, context_offset, anchor));
        self.submit_quick_translation(text.trim().to_owned(), None, ctx);
    }

    fn submit_quick_translation(
        &mut self,
        text: String,
        target: Option<String>,
        ctx: &egui::Context,
    ) {
        let known_source = self
            .quick_translate
            .view
            .as_ref()
            .map(|view| view.source_lang.clone())
            .unwrap_or_default();
        let operation = self.quick_translate.begin(text.clone());
        if self.quick_translate.inline.is_some() && !vocabulary::can_save(&text) {
            self.quick_translate
                .fail("Vocabulary is available for words and short phrases.".into());
            return;
        }
        if text.chars().count() > 64_000 {
            self.quick_translate
                .fail("Select a shorter passage to translate.".into());
            return;
        }
        let languages = self
            .service_config
            .language_capabilities()
            .and_then(|capabilities| {
                if let Some(target) = &target {
                    let capabilities = capabilities.for_text();
                    capabilities
                        .select(&known_source, target)
                        .or_else(|_| capabilities.select("auto", target))
                } else if let Some(lookup) = &self.quick_translate.inline {
                    super::inline::languages(
                        &text,
                        &lookup.context,
                        self.ui_language,
                        [&self.source_lang, &self.target_lang],
                        capabilities,
                    )
                } else {
                    crate::text_translation::select_languages_with_options(
                        &text,
                        &self.source_lang,
                        &self.target_lang,
                        capabilities,
                        None,
                        false,
                    )
                }
            });
        match languages {
            Ok(mut languages) => {
                let capabilities = self.language_capabilities().for_text();
                let (source, target) = languages.wire();
                if target.contains(',') {
                    // Ambiguous short text still needs one concrete output language.
                    if let Ok(selection) =
                        capabilities.select(&source, target.split(',').next().unwrap_or("en"))
                    {
                        languages = selection;
                    }
                }
                let (source, target) = languages.wire();
                if let Some(view) = &mut self.quick_translate.view {
                    view.targets = capabilities
                        .targets()
                        .iter()
                        .filter(|language| {
                            !crate::languages_conflict(language.code(), &source)
                                && capabilities.select(&source, language.code()).is_ok()
                        })
                        .map(|language| language.code().to_owned())
                        .collect();
                    view.source_lang = source.clone();
                    view.target_lang = target;
                }
                let owner = PluginSessionOwner::new(
                    OWNER,
                    operation,
                    "Quick translation",
                    "Quick translation",
                    "Translating…",
                );
                let context = self.quick_translate.inline.as_ref().map_or_else(
                    xrtranslate_prompt::TaskContext::default,
                    |lookup| {
                        super::inline::task_context(
                            &text,
                            &lookup.context,
                            lookup.context_offset,
                            &source,
                        )
                    },
                );
                let task = TranslationTask::text(
                    text,
                    languages,
                    Some(PluginSessionBinding::text(
                        owner,
                        SessionOutputPolicy::PluginOnly,
                    )),
                )
                .with_context(context);
                // Submission errors belong inside this small result window.
                self.enable_translation_service();
                if let Err(error) = self.text_translation.submit(task) {
                    self.quick_translate.fail(error);
                }
            }
            Err(error) => self.quick_translate.fail(error),
        }
        ctx.request_repaint();
    }
    pub(crate) fn poll_quick_translation(&mut self, ctx: &egui::Context) {
        if !self.fullscreen_history
            || self.navigation.page != crate::ui::Page::Translation
            || self.history_archive.open
        {
            self.close_history_lookup();
        }
        #[cfg(windows)]
        {
            self.quick_translate.register(ctx);
            if let Some(text) = self.quick_translate.take_capture() {
                self.begin_quick_translation(text, ctx);
            }
        }
        for event in self.quick_translate.overlay.poll_events() {
            if self.quick_translate.inline.is_some() {
                continue;
            }
            match event {
                OverlayEvent::QuickTranslation { id, action }
                    if self
                        .quick_translate
                        .view
                        .as_ref()
                        .is_some_and(|view| view.id == id) =>
                {
                    self.handle_quick_translation_action(action, ctx);
                }
                OverlayEvent::CloseRequested | OverlayEvent::Failed(_) => {
                    self.stop_quick_translation();
                    self.quick_translate.close();
                    if let OverlayEvent::Failed(error) = event {
                        self.notify_error("Could not open quick translation.", error);
                    }
                }
                _ => {}
            }
        }
        let route = (self.source_lang.clone(), self.target_lang.clone());
        if self
            .quick_translate
            .route
            .as_ref()
            .is_some_and(|previous| previous != &route)
            && !self.quick_translate.source().is_empty()
        {
            self.begin_quick_translation(Ok(self.quick_translate.source()), ctx);
        }
        if self.quick_translate.poll(ctx) {
            self.save_quick_vocabulary(None, ctx);
        }
        if self.quick_translate.inline.is_none()
            && let Some(view) = &self.quick_translate.view
        {
            self.quick_translate.overlay.set_language(self.ui_language);
            self.quick_translate.overlay.send_quick_translation(view);
            self.quick_translate.overlay.start();
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }

    pub(crate) fn show_history_lookup(&mut self, ctx: &egui::Context) {
        if self.quick_translate.inline.is_none() {
            return;
        }
        if !self.fullscreen_history {
            self.stop_quick_translation();
            self.quick_translate.close();
            return;
        }
        let presentation_dark = (self.history_display.mode
            == crate::ui::pages::translation::history::DisplayMode::Bubbles)
            .then_some(self.history_display.presentation_dark);
        let action = match (
            self.quick_translate.view.as_ref(),
            self.quick_translate.inline.as_mut(),
        ) {
            (Some(view), Some(lookup)) => {
                super::inline::show(view, lookup, self.ui_language, presentation_dark, ctx)
            }
            _ => None,
        };
        match action {
            Some(super::inline::Interaction::Action(action)) => {
                self.handle_quick_translation_action(action, ctx);
            }
            Some(super::inline::Interaction::Retry) => {
                let target = self.quick_translate.view.as_ref().and_then(|view| {
                    (!view.target_lang.is_empty()).then(|| view.target_lang.clone())
                });
                self.stop_quick_translation();
                self.submit_quick_translation(self.quick_translate.source(), target, ctx);
            }
            None => {}
        }
    }

    pub(crate) fn is_history_lookup_open(&self) -> bool {
        self.quick_translate.inline.is_some()
    }

    pub(crate) fn close_history_lookup(&mut self) {
        if self.is_history_lookup_open() {
            self.stop_quick_translation();
            self.quick_translate.close();
        }
    }

    fn handle_quick_translation_action(&mut self, action: Action, ctx: &egui::Context) {
        match action {
            Action::Close => {
                self.stop_quick_translation();
                self.quick_translate.close();
            }
            Action::Target(target) => self.change_quick_translation_target(&target, ctx),
            Action::Folders => self.save_quick_vocabulary(None, ctx),
            Action::Save(folder) => self.save_quick_vocabulary(Some(folder), ctx),
            Action::Copy => {
                if let Some(view) = &mut self.quick_translate.view
                    && !view.busy
                    && view.error.is_none()
                    && !view.translated.is_empty()
                {
                    match self.quick_translate.clipboard.write(&view.translated) {
                        Ok(()) => {
                            view.copy_count += 1;
                            view.copy_error = None;
                        }
                        Err(error) => view.copy_error = Some(error),
                    }
                }
            }
        }
        ctx.request_repaint();
    }

    fn save_quick_vocabulary(&mut self, folder: Option<i64>, ctx: &egui::Context) {
        let Some(entry) = self.quick_translate.entry() else {
            return;
        };
        let view = self.quick_translate.view.as_mut().unwrap();
        if view.saving {
            return;
        }
        view.saving = true;
        view.save_error = None;
        let action = match folder {
            Some(folder)
                if folder == vocabulary::DEFAULT_FOLDER
                    || view.folders.iter().any(|f| f.id == folder) =>
            {
                vocabulary::Action::Add { folder, entry }
            }
            Some(_) => {
                view.saving = false;
                return;
            }
            None => vocabulary::Action::Folders(entry),
        };
        self.quick_translate.bookmark = Some((
            view.id.clone(),
            self.history_archive.vocabulary(action, ctx),
        ));
    }

    fn change_quick_translation_target(&mut self, target: &str, ctx: &egui::Context) {
        let Some(view) = &self.quick_translate.view else {
            return;
        };
        if view.target_lang == target || !view.targets.iter().any(|code| code == target) {
            return;
        }
        let text = view.source.clone();
        self.stop_quick_translation();
        self.submit_quick_translation(text, Some(target.to_owned()), ctx);
    }
}
