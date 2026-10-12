//! Rebuild search data once per snapshot; filter and sort only when the query changes.
use super::*;

#[derive(Default)]
pub(super) struct TermIndex {
    search: Vec<String>,
    pub counts: HashMap<String, (usize, usize)>, // subtree terms, terms with independent rules
    paths: HashMap<String, HashSet<String>>,
    query: Option<(Option<String>, String)>,
    rows: Arc<Vec<usize>>,
}

impl TermIndex {
    pub fn rebuild(&mut self, snapshot: &GraphSnapshot) {
        self.query = None;
        self.paths = snapshot
            .domains
            .iter()
            .map(|domain| {
                (
                    domain.id.clone(),
                    domain_path(snapshot, &domain.id)
                        .iter()
                        .map(|d| d.id.clone())
                        .collect(),
                )
            })
            .collect();
        self.counts.clear();
        self.search = snapshot
            .nodes
            .iter()
            .map(|node| {
                for id in self
                    .paths
                    .get(&node.domain_id)
                    .into_iter()
                    .flatten()
                    .filter(|_| node.promptable)
                {
                    let count = self.counts.entry(id.clone()).or_default();
                    count.0 += 1;
                    count.1 += usize::from(!node.inherit_domain);
                }
                format!(
                    "{}\n{}\n{}\n{}",
                    node.id,
                    node.title,
                    node.values.join("\n"),
                    node.recognition_aliases
                        .values()
                        .flatten()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("\n")
                )
                .to_lowercase()
            })
            .collect();
    }

    pub fn rows(
        &mut self,
        snapshot: &GraphSnapshot,
        domain: Option<&str>,
        search: &str,
    ) -> Arc<Vec<usize>> {
        let query = (domain.map(str::to_owned), search.trim().to_lowercase());
        if self.query.as_ref() != Some(&query) {
            let mut rows: Vec<_> = snapshot
                .nodes
                .iter()
                .enumerate()
                .filter(|(i, node)| {
                    node.promptable
                        && domain.is_none_or(|id| {
                            self.paths
                                .get(&node.domain_id)
                                .is_some_and(|path| path.contains(id))
                        })
                        && self.search[*i].contains(&query.1)
                })
                .map(|(i, _)| i)
                .collect();
            rows.sort_by(|&a, &b| {
                let (a, b) = (&snapshot.nodes[a], &snapshot.nodes[b]);
                (&a.domain_id, &a.title, &a.id).cmp(&(&b.domain_id, &b.title, &b.id))
            });
            self.rows = Arc::new(rows);
            self.query = Some(query);
        }
        self.rows.clone()
    }
}
