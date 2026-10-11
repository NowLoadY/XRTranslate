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
        let operation = self.quick_translate.begin(text.clone());
        self.quick_translate.route = Some((self.source_lang.clone(), self.target_lang.clone()));
        if text.chars().count() > 64_000 {
            self.quick_translate
                .fail("Select a shorter passage to translate.".into());
            return;
        }
        let languages = self
            .service_config
            .language_capabilities()
            .and_then(|capabilities| {
                crate::text_translation::select_languages_with_options(
                    &text,
                    &self.source_lang,
                    &self.target_lang,
                    capabilities,
                    None,
                    false,
                )
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
                    view.source_lang = source;
                    view.target_lang = target;
                }
                let owner = PluginSessionOwner::new(
                    OWNER,
                    operation,
                    "Quick translation",
                    "Quick translation",
                    "Translating…",
                );
                let task = TranslationTask::text(
                    text,
                    languages,
                    Some(PluginSessionBinding::text(
                        owner,
                        SessionOutputPolicy::PluginOnly,
                    )),
                );
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
        #[cfg(windows)]
        {
            self.quick_translate.register(ctx);
            if let Some(text) = self.quick_translate.take_capture() {
                self.begin_quick_translation(text, ctx);
            }
        }
        for event in self.quick_translate.overlay.poll_events() {
            match event {
                OverlayEvent::QuickTranslation { id, action }
                    if self
                        .quick_translate
                        .view
                        .as_ref()
                        .is_some_and(|view| view.id == id) =>
                {
                    match action {
                        Action::Close => {
                            self.stop_quick_translation();
                            self.quick_translate.close();
                        }
                        Action::Target(target) => {
                            self.change_quick_translation_target(&target, ctx)
                        }
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
        if let Some(view) = &self.quick_translate.view {
            self.quick_translate.overlay.set_language(self.ui_language);
            self.quick_translate.overlay.send_quick_translation(view);
            self.quick_translate.overlay.start();
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
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
            Some(folder) if view.folders.iter().any(|f| f.id == folder) => {
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
        let capabilities = self.language_capabilities();
        let pair = format!("{},{}", view.source_lang, target);
        let (source, target) = if self.source_lang == "auto"
            && self.target_lang.contains(',')
            && capabilities.select("auto", &pair).is_ok()
        {
            ("auto".to_owned(), pair)
        } else if capabilities.select(&view.source_lang, target).is_ok() {
            (view.source_lang.clone(), target.to_owned())
        } else {
            ("auto".to_owned(), target.to_owned())
        };
        if capabilities.select(&source, &target).is_err() {
            return;
        }
        if capabilities
            .select_with_options(
                &source,
                &target,
                self.additional_target_lang.as_deref(),
                false,
            )
            .is_err()
        {
            self.additional_target_lang = None;
        }
        self.source_lang = source;
        self.target_lang = target;
        self.asr_only = false;
        self.apply_language_route();
        self.begin_quick_translation(Ok(self.quick_translate.source()), ctx);
    }
}
