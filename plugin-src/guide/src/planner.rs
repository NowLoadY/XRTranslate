use serde::Deserialize;
use xrtranslate_plugin::{
    Text,
    guide::{Chapter, Context, Session, State, Step},
};

#[derive(Deserialize)]
struct Lesson {
    id: String,
    title: Text,
    text: Text,
    targets: Vec<String>,
    reveal: Vec<String>,
    #[serde(default)]
    requires: Vec<String>,
}

#[derive(Deserialize)]
struct Page {
    page: String,
    steps: Vec<Lesson>,
}

pub struct Planner {
    lessons: Vec<Page>,
    context: Option<Context>,
    pub state: State,
}

impl Default for Planner {
    fn default() -> Self {
        Self {
            lessons: serde_json::from_str(include_str!("../assets/lessons.json"))
                .expect("bundled guide lessons"),
            context: None,
            state: State::default(),
        }
    }
}

impl Planner {
    pub fn catalog(&self) -> Result<Vec<Chapter>, String> {
        let context = self
            .context
            .as_ref()
            .ok_or("Waiting for the host's guide context")?;
        Ok(context
            .pages
            .iter()
            .filter_map(|page| {
                let source = self
                    .lessons
                    .iter()
                    .find(|source| source.page == page.route)?;
                let steps: Vec<_> = source
                    .steps
                    .iter()
                    .filter(|step| {
                        step.requires
                            .iter()
                            .all(|name| context.capabilities.contains(name))
                    })
                    .map(|step| Step {
                        id: step.id.clone(),
                        title: step.title.get(&context.language).into(),
                        text: step.text.get(&context.language).into(),
                        targets: step.targets.clone(),
                        reveal: step.reveal.clone(),
                        audio: crate::audio::metadata(
                            &page.route,
                            &step.id,
                            &context.language,
                            step.text.get(&context.language),
                        ),
                    })
                    .collect();
                (!steps.is_empty()).then(|| Chapter {
                    page: page.route.clone(),
                    label: page.label.clone(),
                    steps,
                })
            })
            .collect())
    }

    fn replace(&mut self, session: Option<Session>) {
        self.state.revision = self.state.revision.wrapping_add(1);
        self.state.session = session;
        self.state.ready = false;
    }

    pub fn update(&mut self, context: Context) {
        let page_changed = self
            .context
            .as_ref()
            .is_some_and(|old| old.page != context.page);
        let page = context.page.clone();
        self.context = Some(context);
        let Some(session) = &self.state.session else {
            return;
        };
        let Some((current, step)) = session.current() else {
            self.stop();
            return;
        };
        let route = if page_changed { &page } else { &current.page };
        let chapters = self.catalog().unwrap_or_default();
        let Some(chapter) = chapters.iter().position(|entry| entry.page == *route) else {
            self.stop();
            return;
        };
        let selected = if *route == current.page {
            chapters[chapter]
                .steps
                .iter()
                .position(|entry| entry.id == step.id)
                .unwrap_or(0)
        } else {
            0
        };
        let next = Session {
            chapters,
            chapter,
            step: selected,
        };
        if self.state.session.as_ref() != Some(&next) {
            self.replace(Some(next));
        }
    }

    pub fn start(&mut self, page: Option<&str>) -> Result<(), String> {
        let chapters = self.catalog()?;
        let preferred = page.or_else(|| self.context.as_ref().map(|context| context.page.as_str()));
        let chapter = chapters
            .iter()
            .position(|chapter| Some(chapter.page.as_str()) == preferred);
        if page.is_some() && chapter.is_none() {
            return Err("No guide is available for this page".into());
        }
        if chapters.is_empty() {
            return Err("No guide pages are available".into());
        }
        self.replace(Some(Session {
            chapters,
            chapter: chapter.unwrap_or(0),
            step: 0,
        }));
        Ok(())
    }

    pub fn select(&mut self, page: &str, topic: Option<&str>) -> Result<(), String> {
        let chapters = self.catalog()?;
        let chapter = chapters
            .iter()
            .position(|chapter| chapter.page == page)
            .ok_or("No guide is available for this page")?;
        let step = match topic {
            Some(id) => chapters[chapter]
                .steps
                .iter()
                .position(|step| step.id == id)
                .ok_or("This guide topic is unavailable")?,
            None => 0,
        };
        self.replace(Some(Session {
            chapters,
            chapter,
            step,
        }));
        Ok(())
    }

    pub fn advance(&mut self, forward: bool) -> Result<(), String> {
        let mut session = self
            .state
            .session
            .clone()
            .ok_or("The guide is not running")?;
        if forward {
            if session.step + 1 < session.chapters[session.chapter].steps.len() {
                session.step += 1;
            } else if session.chapter + 1 < session.chapters.len() {
                session.chapter += 1;
                session.step = 0;
            } else {
                self.stop();
                return Ok(());
            }
        } else if session.step > 0 {
            session.step -= 1;
        } else if session.chapter > 0 {
            session.chapter -= 1;
            session.step = session.chapters[session.chapter].steps.len() - 1;
        }
        self.replace(Some(session));
        Ok(())
    }

    pub fn presented(&mut self, revision: u64) {
        if revision == self.state.revision && self.state.session.is_some() {
            self.state.ready = true;
        }
    }

    pub fn stop(&mut self) {
        self.replace(None);
    }
}

#[cfg(test)]
mod tests;
