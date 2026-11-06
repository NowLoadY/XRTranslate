//! Local execution facts for renderers. Never added to tool arguments or model context.

/// An opaque component/document identity; adapters map it to current geometry.
pub type Target = u64;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Effect {
    Read,
    Invoke,
    Pending,
    Failed(String),
    Change { before: String, after: String },
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Feedback {
    pub target: Target,
    pub label: String,
    pub effect: Effect,
}

/// A document adapter supplies only the affected items, never the whole application state.
pub struct Item {
    pub target: Target,
    pub label: String,
    pub text: String,
}

/// Compare affected items after a successful mutation. Deleted items use their container's anchor.
pub fn changes(before: Vec<Item>, after: Vec<Item>, container: Target) -> Vec<Feedback> {
    let mut before = before
        .into_iter()
        .map(|item| (item.target, item))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut events = Vec::new();
    for item in after {
        let old = before.remove(&item.target);
        if let Some(change) = Feedback::changed(
            item.target,
            item.label,
            old.as_ref().map_or("", |item| &item.text),
            &item.text,
        ) {
            events.push(change);
        }
    }
    events.extend(
        before
            .into_values()
            .filter_map(|item| Feedback::changed(container, item.label, &item.text, "")),
    );
    events
}

impl Feedback {
    pub fn new(target: Target, label: impl Into<String>, effect: Effect) -> Self {
        Self {
            target,
            label: label.into(),
            effect,
        }
    }

    /// Emit only a real change; absence/empty content represents insertion or deletion.
    pub fn changed(
        target: Target,
        label: impl Into<String>,
        before: &str,
        after: &str,
    ) -> Option<Self> {
        (before != after).then(|| {
            Self::new(
                target,
                label,
                Effect::Change {
                    before: before.to_owned(),
                    after: after.to_owned(),
                },
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn document_changes_keep_identity_and_deleted_content_without_reporting_noops() {
        let item = |target, text: &str| Item {
            target,
            label: target.to_string(),
            text: text.into(),
        };
        let events = changes(
            vec![item(1, "unchanged"), item(2, "old"), item(3, "deleted")],
            vec![item(1, "unchanged"), item(2, "new"), item(4, "added")],
            9,
        );
        assert_eq!(events.len(), 3);
        assert_eq!(events[0], Feedback::changed(2, "2", "old", "new").unwrap());
        assert_eq!(events[1], Feedback::changed(4, "4", "", "added").unwrap());
        assert_eq!(events[2], Feedback::changed(9, "3", "deleted", "").unwrap());
    }
}
