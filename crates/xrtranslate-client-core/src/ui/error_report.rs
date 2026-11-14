//! One report per displayed failure, shared by all error surfaces.
use eframe::egui;
use parking_lot::Mutex;
use std::{collections::VecDeque, sync::Arc};

#[derive(Clone)]
pub struct ErrorReport(pub Arc<Mutex<ReportContent>>);

pub struct ReportContent {
    pub text: String,
    pub collecting: bool,
    pub issue_url: String,
}

impl ErrorReport {
    pub fn new(error: &str, text: String, collecting: bool) -> Self {
        let mut url =
            reqwest::Url::parse("https://github.com/NowLoadY/XRTranslate/issues/new").unwrap();
        let summary: String = error.chars().take(120).collect();
        url.query_pairs_mut()
            .append_pair(
                "title",
                &format!(
                    "XRTranslate {}: {}",
                    env!("CARGO_PKG_VERSION"),
                    error
                        .lines()
                        .next()
                        .unwrap_or("Error")
                        .chars()
                        .take(60)
                        .collect::<String>()
                ),
            )
            .append_pair(
                "body",
                &format!(
                    "XRTranslate {} · {} {}\n\n{}\n\n",
                    env!("CARGO_PKG_VERSION"),
                    std::env::consts::OS,
                    std::env::consts::ARCH,
                    summary
                ),
            );
        Self(Arc::new(Mutex::new(ReportContent {
            text,
            collecting,
            issue_url: url.into(),
        })))
    }
}

pub type ReportProvider =
    Arc<dyn Fn(&str, Option<&str>, &egui::Context) -> ErrorReport + Send + Sync>;

pub fn install(ctx: &egui::Context, provider: ReportProvider) {
    ctx.data_mut(|data| data.insert_temp(egui::Id::unique("error_report_provider"), provider));
}

pub fn for_error(ctx: &egui::Context, error: &str, log: Option<&str>) -> ErrorReport {
    type Cache = VecDeque<(String, ErrorReport, u64)>;
    let id = egui::Id::unique("error_report_cache");
    let frame = ctx.cumulative_frame_nr();
    if let Some(report) = ctx.data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<Cache>(id);
        if let Some((_, report, seen)) = cache
            .iter_mut()
            .find(|(message, _, seen)| message == error && frame <= *seen + 1)
        {
            *seen = frame;
            return Some(report.clone());
        }
        cache.retain(|(message, _, _)| message != error);
        None
    }) {
        return report;
    }
    let provider =
        ctx.data(|data| data.get_temp::<ReportProvider>(egui::Id::unique("error_report_provider")));
    let report = provider.map_or_else(
        || {
            ErrorReport::new(
                error,
                format!(
                    "XRTranslate {} · {} {}\n\n{error}\n\n{}",
                    env!("CARGO_PKG_VERSION"),
                    std::env::consts::OS,
                    std::env::consts::ARCH,
                    log.unwrap_or_default()
                ),
                false,
            )
        },
        |provider| provider(error, log, ctx),
    );
    ctx.data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<Cache>(id);
        while cache.len() >= 8 {
            cache.pop_front();
        }
        cache.push_back((error.to_owned(), report.clone(), frame));
    });
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn diagnostics_reuses_the_error_snapshot_and_never_opens_an_issue_automatically() {
        let ctx = egui::Context::default();
        let captures = Arc::new(AtomicUsize::new(0));
        let count = captures.clone();
        install(
            &ctx,
            Arc::new(move |error, _, _| {
                let number = count.fetch_add(1, Ordering::Relaxed);
                ErrorReport::new(error, format!("snapshot {number}"), false)
            }),
        );
        for _ in 0..3 {
            let output = ctx.run_ui(Default::default(), |ui| {
                let report = for_error(ui.ctx(), "Request failed", None);
                assert_eq!(report.0.lock().text, "snapshot 0");
                crate::ui::components::error_actions(
                    ui,
                    crate::i18n::UiLanguage::English,
                    "Request failed",
                );
            });
            assert!(output.platform_output.commands.is_empty());
        }
        assert_eq!(captures.load(Ordering::Relaxed), 1);
        for _ in 0..2 {
            let _ = ctx.run_ui(Default::default(), |_| {});
        }
        let _ = ctx.run_ui(Default::default(), |ui| {
            for_error(ui.ctx(), "Request failed", None);
        });
        assert_eq!(captures.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn diagnostics_issue_link_contains_only_the_error_summary() {
        let report = ErrorReport::new(
            "错误 & request failed",
            "private conversation and full logs".into(),
            false,
        );
        let report = report.0.lock();
        let url = reqwest::Url::parse(&report.issue_url).unwrap();
        assert_eq!(url.host_str(), Some("github.com"));
        assert!(
            url.query_pairs()
                .any(|(key, value)| key == "body" && value.contains("错误 & request failed"))
        );
        assert!(!report.issue_url.contains("private"));
    }

    #[test]
    fn diagnostics_modal_keeps_the_error_separate_from_the_log() {
        let modal = crate::ui::modal::ModalDialog::error(
            crate::i18n::UiLanguage::English,
            "  Request failed  ",
            Some("Detailed backend log"),
        );
        assert_eq!(
            modal.pages[0].error_details.as_deref(),
            Some("Request failed")
        );
        assert_eq!(
            modal.pages[0].error_log.as_deref(),
            Some("Detailed backend log")
        );
    }
}
