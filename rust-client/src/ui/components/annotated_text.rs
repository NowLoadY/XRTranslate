use eframe::egui::{self, Color32, RichText, Ui, text::LayoutJob};
use std::{ops::Range, sync::Arc};
use xrtranslate_protocol::CorpusTermMatch;

#[derive(Default)]
pub struct AnnotatedText<'a> {
    job: LayoutJob,
    terms: Vec<(Range<usize>, &'a CorpusTermMatch)>,
}

pub struct TextLayout<'a> {
    galley: Arc<egui::Galley>,
    terms: Vec<(Range<usize>, &'a CorpusTermMatch)>,
}

impl<'a> AnnotatedText<'a> {
    /// Join separate sentences without changing the offsets of their annotations.
    pub fn append_segment(
        &mut self,
        ui: &Ui,
        text: &str,
        primary: &'a [CorpusTermMatch],
        secondary: &'a [CorpusTermMatch],
        color: Color32,
        size: f32,
    ) {
        if crate::streaming::needs_separator(&self.job.text, text) {
            self.append(ui, RichText::new(" ").color(color).size(size));
        }
        self.append_terms(ui, text, primary, secondary, color, size);
    }

    pub fn append(&mut self, ui: &Ui, text: RichText) {
        text.append_to(
            &mut self.job,
            ui.style(),
            egui::FontSelection::Default,
            egui::Align::Center,
        );
    }

    pub fn append_terms(
        &mut self,
        ui: &Ui,
        text: &str,
        primary: &'a [CorpusTermMatch],
        secondary: &'a [CorpusTermMatch],
        color: Color32,
        size: f32,
    ) {
        let mut matches = secondary
            .iter()
            .map(|term| (term, false))
            .chain(primary.iter().map(|term| (term, true)))
            .collect::<Vec<_>>();
        matches.sort_by(|(left, lp), (right, rp)| {
            left.start_byte
                .cmp(&right.start_byte)
                .then_with(|| rp.cmp(lp))
                .then_with(|| right.end_byte.cmp(&left.end_byte))
        });
        let mut cursor = 0;
        for (term, primary) in matches {
            let (Ok(start), Ok(end)) = (
                usize::try_from(term.start_byte),
                usize::try_from(term.end_byte),
            ) else {
                continue;
            };
            if start < cursor || end <= start || text.get(start..end) != Some(term.text.as_str()) {
                continue;
            }
            self.append(
                ui,
                RichText::new(&text[cursor..start]).color(color).size(size),
            );
            let offset = self.job.text.len();
            self.append(
                ui,
                RichText::new(&text[start..end])
                    .size(size)
                    .color(if primary {
                        crate::ui::theme::primary_dark()
                    } else {
                        Color32::from_rgb(96, 165, 250)
                    }),
            );
            self.terms.push((offset..self.job.text.len(), term));
            cursor = end;
        }
        self.append(ui, RichText::new(&text[cursor..]).color(color).size(size));
    }

    pub fn layout(mut self, ui: &Ui, width: f32) -> TextLayout<'a> {
        self.job.wrap.max_width = width.max(1.0);
        TextLayout {
            galley: ui.painter().layout_job(self.job),
            terms: self.terms,
        }
    }
}

impl TextLayout<'_> {
    pub fn height(&self) -> f32 {
        self.galley.size().y
    }

    pub fn show(&self, ui: &mut Ui) -> egui::Response {
        let response = ui.add(egui::Label::new(self.galley.clone()).wrap());
        if let Some(position) = response.hover_pos() {
            let cursor = self.galley.cursor_from_pos(position - response.rect.min);
            let byte = self
                .galley
                .job
                .text
                .char_indices()
                .nth(cursor.index.0)
                .map_or(self.galley.job.text.len(), |(byte, _)| byte);
            if let Some((_, term)) = self.terms.iter().find(|(range, _)| range.contains(&byte)) {
                let tooltip = term
                    .sources
                    .iter()
                    .map(|source| {
                        format!(
                            "{}\n{} / {}\n{}",
                            source.title, source.domain, source.subdomain, source.corpus_id
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                return response.on_hover_text(tooltip);
            }
        }
        response
    }
}
