//! Application adapter: bind the floating result to the shared task lifecycle.
use super::{Action, OWNER};
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
        self.quick_translate.close();
        let text = match text {
            Ok(text) if !text.trim().is_empty() => text,
            Ok(_) => {
                self.quick_translate
                    .fail("Copy some text, then try again.".into());
                return;
            }
            Err(error) => {
                self.quick_translate.fail(error);
                return;
            }
        };
        if text.chars().count() > 64_000 {
            self.quick_translate
                .fail("Select a shorter passage to translate.".into());
            return;
        }
        let operation = self.quick_translate.begin(text.clone(), ctx);
        let languages = self
            .service_config
            .language_capabilities()
            .and_then(|capabilities| {
                crate::text_translation::select_languages_with_options(
                    &text,
                    &self.source_lang,
                    &self.target_lang,
                    capabilities,
                    self.additional_target_lang.as_deref(),
                    false,
                )
            });
        match languages {
            Ok(languages) => {
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
        while let Some(action) = self.quick_translate.take_action() {
            match action {
                Action::Close => {
                    self.stop_quick_translation();
                    self.quick_translate.close();
                }
                Action::Retry => {
                    self.begin_quick_translation(Ok(self.quick_translate.source()), ctx)
                }
            }
        }
        self.quick_translate.poll(ctx);
        self.quick_translate
            .show(ctx, self.ui_language, self.ui_theme);
    }
}
