use std::cell::RefCell;
pub use vocabulary::PAGE_SIZE;
pub use xrtranslate_client_core::bookmarks as vocabulary;
use xrtranslate_plugin::{Client, Request};

pub struct Archive {
    pub revision: u64,
    client: Client,
    pending: RefCell<
        Vec<(
            Request,
            crossbeam_channel::Sender<Result<vocabulary::Output, String>>,
        )>,
    >,
}
impl Archive {
    pub fn new(client: Client) -> Self {
        Self {
            revision: 0,
            client,
            pending: Default::default(),
        }
    }
    pub fn vocabulary_revision(&self) -> u64 {
        self.revision
    }
    pub fn vocabulary(
        &self,
        action: vocabulary::Action,
        _: &eframe::egui::Context,
    ) -> vocabulary::Reply {
        let (send, reply) = crossbeam_channel::bounded(1);
        self.pending.borrow_mut().push((
            self.client
                .request("bookmarks.query", serde_json::to_value(action).unwrap()),
            send,
        ));
        reply
    }
    pub fn poll(&self) {
        self.pending.borrow_mut().retain(|(request, send)| {
            if let Some(result) = request.poll() {
                let _ = send
                    .send(result.and_then(|value| {
                        serde_json::from_value(value).map_err(|e| e.to_string())
                    }));
                false
            } else {
                true
            }
        });
    }
}
