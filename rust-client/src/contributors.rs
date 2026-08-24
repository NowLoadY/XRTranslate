use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

const EMBEDDED_CONTRIBUTORS_MD: &str = include_str!("../../docs/contributors.md");

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SocialLink {
    pub label: String,
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Contributor {
    pub name: String,
    pub links: Vec<SocialLink>,
    pub contributions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineMarkdownSpan {
    pub text: String,
    pub strong: bool,
}

/// Parses paired Markdown strong markers while preserving unmatched markers.
pub fn parse_inline_markdown(text: &str) -> Vec<InlineMarkdownSpan> {
    let mut spans = Vec::new();
    let mut cursor = 0;
    while cursor < text.len() {
        let remaining = &text[cursor..];
        let next = [(remaining.find("**"), "**"), (remaining.find("__"), "__")]
            .into_iter()
            .filter_map(|(position, marker)| position.map(|position| (position, marker)))
            .min_by_key(|(position, _)| *position);
        let Some((open, marker)) = next else {
            push_inline_span(&mut spans, remaining, false);
            break;
        };
        push_inline_span(&mut spans, &remaining[..open], false);
        let content_start = open + marker.len();
        let after_open = &remaining[content_start..];
        let Some(close) = after_open.find(marker) else {
            push_inline_span(&mut spans, &remaining[open..], false);
            break;
        };
        if close == 0 {
            push_inline_span(
                &mut spans,
                &remaining[open..content_start + marker.len()],
                false,
            );
            cursor += content_start + marker.len();
            continue;
        }
        push_inline_span(&mut spans, &after_open[..close], true);
        cursor += content_start + close + marker.len();
    }
    spans
}

fn push_inline_span(spans: &mut Vec<InlineMarkdownSpan>, text: &str, strong: bool) {
    if text.is_empty() {
        return;
    }
    if let Some(previous) = spans.last_mut()
        && previous.strong == strong
    {
        previous.text.push_str(text);
    } else {
        spans.push(InlineMarkdownSpan {
            text: text.to_owned(),
            strong,
        });
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContributorRole {
    CodeContributors,
    BetaTesters,
    Other(String),
}

impl ContributorRole {
    pub fn priority(&self) -> u8 {
        match self {
            Self::CodeContributors => 0,
            Self::BetaTesters => 1,
            Self::Other(_) => 2,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContributorGroup {
    pub role: ContributorRole,
    pub title: String,
    pub contributors: Vec<Contributor>,
}

/// Parses contributor groups from standard Markdown formats (lists, nested bullets, or GFM tables).
///
/// Supported Markdown formats:
///
/// 1. Standard List with nested bullets:
/// ```markdown
/// ## Code Contributors
/// - **NowLoadY** — [GitHub](https://github.com/NowLoadY)
///   - Project Creator & Core Architecture
///   - Prompt Studio Workflow
/// ```
///
/// 2. List with sub-bullet links and contributions:
/// ```markdown
/// - **NowLoadY**
///   - [GitHub](https://github.com/NowLoadY)
///   - Project Creator & Core Architecture
/// ```
///
/// 3. Standard GFM Table:
/// ```markdown
/// | 贡献者 | 社交链接 | 贡献详情 |
/// | :--- | :--- | :--- |
/// | **NowLoadY** | [GitHub](https://github.com/NowLoadY) | 项目发起人与核心架构<br>Prompt Studio 节点流 |
/// ```
pub fn parse_contributors_markdown(content: &str) -> Vec<ContributorGroup> {
    let mut groups: Vec<ContributorGroup> = Vec::new();
    let mut current_role: Option<(ContributorRole, String)> = None;
    let mut current_members: Vec<Contributor> = Vec::new();

    let lines: Vec<&str> = content.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let raw_line = lines[i];
        let trimmed_end = raw_line.trim_end();
        let trimmed = trimmed_end.trim();
        i += 1;

        if trimmed.is_empty() || trimmed.starts_with('>') || trimmed.starts_with("<!--") {
            continue;
        }

        if trimmed.starts_with("##") {
            // Save previous section if it had contributors
            if let Some((role, title)) = current_role.take() {
                if !current_members.is_empty() {
                    groups.push(ContributorGroup {
                        role,
                        title,
                        contributors: std::mem::take(&mut current_members),
                    });
                }
            }

            let heading = trimmed.trim_start_matches('#').trim();
            let heading_lower = heading.to_lowercase();

            let role = if heading_lower.contains("code")
                || heading_lower.contains("contributor")
                || heading_lower.contains("代码")
                || heading_lower.contains("开发")
                || heading_lower.contains("贡献")
            {
                ContributorRole::CodeContributors
            } else if heading_lower.contains("beta")
                || heading_lower.contains("test")
                || heading_lower.contains("测试")
            {
                ContributorRole::BetaTesters
            } else {
                ContributorRole::Other(heading.to_string())
            };

            current_role = Some((role, heading.to_string()));
            continue;
        }

        // Only parse member lines if we are inside a section
        if current_role.is_some() {
            // Check if this is a GFM table row
            if trimmed.starts_with('|') && trimmed.ends_with('|') {
                // If this is a table header followed by a separator line, skip both
                if i < lines.len() && is_table_separator_line(lines[i].trim()) {
                    i += 1; // skip separator line as well
                    continue;
                }

                // If this is a separator line itself, skip
                if is_table_separator_line(trimmed) {
                    continue;
                }

                if let Some(contributor) = parse_table_row(trimmed) {
                    current_members.push(contributor);
                }
                continue;
            }

            let is_sub_bullet = raw_line.starts_with("  ") || raw_line.starts_with('\t');

            if is_sub_bullet && !current_members.is_empty() {
                handle_sub_bullet(trimmed, current_members.last_mut().unwrap());
            } else if let Some(contributor) = parse_contributor_line(trimmed) {
                current_members.push(contributor);
            }
        }
    }

    // Push the last group
    if let Some((role, title)) = current_role {
        if !current_members.is_empty() {
            groups.push(ContributorGroup {
                role,
                title,
                contributors: current_members,
            });
        }
    }

    // Sort groups strictly by priority: Code Contributors -> Beta Testers -> Other
    groups.sort_by_key(|g| g.role.priority());
    groups
}

fn is_table_separator_line(line: &str) -> bool {
    let trimmed = line.trim();
    if !trimmed.starts_with('|') || !trimmed.ends_with('|') {
        return false;
    }
    let inner = trimmed.trim_matches('|');
    !inner.is_empty()
        && inner
            .chars()
            .all(|ch| ch == '-' || ch == ':' || ch == '|' || ch == ' ' || ch == '\t')
}

fn handle_sub_bullet(line: &str, contributor: &mut Contributor) {
    let mut trimmed = line;
    if let Some(rest) = trimmed
        .strip_prefix('-')
        .or_else(|| trimmed.strip_prefix('*'))
        .or_else(|| trimmed.strip_prefix('+'))
    {
        trimmed = rest.trim_start();
    }

    let (links, spans) = extract_markdown_links_with_spans(trimmed);
    let is_pure_link =
        !links.is_empty() && spans.len() == 1 && spans[0].0 == 0 && spans[0].1 == trimmed.len();

    if is_pure_link
        || (trimmed.to_lowercase().starts_with("links:") || trimmed.starts_with("社交链接:"))
    {
        for link in links {
            if contributor.links.len() < 2 {
                contributor.links.push(link);
            }
        }
    } else {
        let cleaned = clean_description_item(trimmed);
        if !cleaned.is_empty() {
            contributor.contributions.push(cleaned);
        }
    }
}

fn parse_table_row(row: &str) -> Option<Contributor> {
    let cells: Vec<&str> = row.trim_matches('|').split('|').map(|c| c.trim()).collect();

    if cells.len() < 2 {
        return None;
    }

    // Skip markdown table header separators like | :--- | :--- |
    if cells
        .iter()
        .all(|c| c.chars().all(|ch| ch == '-' || ch == ':' || ch == ' '))
    {
        return None;
    }

    let name_cell = cells[0];
    let name = clean_name(name_cell);
    if name.is_empty() {
        return None;
    }

    let mut links = Vec::new();
    let mut contributions = Vec::new();

    for cell in &cells[1..] {
        let (extracted_links, _) = extract_markdown_links_with_spans(cell);
        if !extracted_links.is_empty() && links.is_empty() {
            links = extracted_links;
        } else {
            // Split by <br>, <br/>, <br />, or semicolons
            let cell_content = cell
                .replace("<br/>", "\n")
                .replace("<br />", "\n")
                .replace("<br>", "\n");
            for part in cell_content.lines() {
                for item in split_contributions(part) {
                    if !item.is_empty() {
                        contributions.push(item);
                    }
                }
            }
        }
    }

    if links.len() > 2 {
        links.truncate(2);
    }

    Some(Contributor {
        name,
        links,
        contributions,
    })
}

fn parse_contributor_line(line: &str) -> Option<Contributor> {
    let mut trimmed = line;
    // Strip list item markers: '-', '*', '+', or numbered like '1.'
    if let Some(rest) = trimmed
        .strip_prefix('-')
        .or_else(|| trimmed.strip_prefix('*'))
        .or_else(|| trimmed.strip_prefix('+'))
    {
        trimmed = rest.trim_start();
    } else if let Some(idx) = trimmed.find('.') {
        if trimmed[..idx].chars().all(|c| c.is_ascii_digit()) {
            trimmed = trimmed[idx + 1..].trim_start();
        }
    }

    if trimmed.is_empty() {
        return None;
    }

    let (links, link_spans) = extract_markdown_links_with_spans(trimmed);

    let (name_raw, desc_raw) = if let Some(first_span) = link_spans.first() {
        let prefix = &trimmed[..first_span.0];
        let last_span = link_spans.last().unwrap();
        let suffix = &trimmed[last_span.1..];
        (prefix, suffix)
    } else {
        (trimmed, "")
    };

    let mut name = String::new();
    let mut inline_contributions = Vec::new();

    // Check if name_raw has parentheses: Name (Desc)
    if let Some((clean_n, paren_desc)) = extract_parenthesized_name_desc(name_raw) {
        name = clean_n;
        if !paren_desc.is_empty() {
            inline_contributions.extend(split_contributions(&paren_desc));
        }
    } else if desc_raw.trim().is_empty()
        && (name_raw.contains(" - ") || name_raw.contains(" — ") || name_raw.contains(" – "))
    {
        // e.g. Name — Desc (with no links)
        let sep = if name_raw.contains(" — ") {
            " — "
        } else if name_raw.contains(" – ") {
            " – "
        } else {
            " - "
        };
        if let Some((n, d)) = name_raw.split_once(sep) {
            name = clean_name(n);
            inline_contributions.extend(split_contributions(d));
        }
    } else if desc_raw.trim().is_empty() && name_raw.contains(" | ") {
        if let Some((n, d)) = name_raw.split_once(" | ") {
            name = clean_name(n);
            inline_contributions.extend(split_contributions(d));
        }
    } else {
        name = clean_name(name_raw);
    }

    // If suffix contains description after links (e.g. — Contribution A; Contribution B)
    if !desc_raw.trim().is_empty() {
        inline_contributions.extend(split_contributions(desc_raw));
    }

    if name.is_empty() {
        return None;
    }

    // Limit to max 2 social links budget per participant
    let mut limited_links = links;
    if limited_links.len() > 2 {
        limited_links.truncate(2);
    }

    Some(Contributor {
        name,
        links: limited_links,
        contributions: inline_contributions,
    })
}

fn extract_parenthesized_name_desc(s: &str) -> Option<(String, String)> {
    let s = s.trim();
    // Check ASCII ( ) and Fullwidth （ ）
    let open_ascii = s.find('(');
    let close_ascii = s.rfind(')');
    let open_full = s.find('（');
    let close_full = s.rfind('）');

    let (open_idx, close_idx, is_full) = match (open_ascii, close_ascii, open_full, close_full) {
        (Some(o), Some(c), _, _) if c > o => (o, c, false),
        (_, _, Some(o), Some(c)) if c > o => (o, c, true),
        _ => return None,
    };

    let name_part = &s[..open_idx];
    let desc_part = if is_full {
        &s[open_idx + '（'.len_utf8()..close_idx]
    } else {
        &s[open_idx + 1..close_idx]
    };

    let cleaned_name = clean_name(name_part);
    if cleaned_name.is_empty() {
        return None;
    }

    Some((cleaned_name, desc_part.trim().to_string()))
}

fn split_contributions(text: &str) -> Vec<String> {
    let mut trimmed = text.trim();
    trimmed = trimmed
        .trim_start_matches('-')
        .trim_start_matches('—')
        .trim_start_matches('–')
        .trim_start_matches(':')
        .trim_start_matches('|')
        .trim_start_matches('~')
        .trim();

    if trimmed.is_empty() {
        return Vec::new();
    }

    // Split on common delimiters: ';', '；', '|'
    let mut items = Vec::new();
    for part in trimmed.split([';', '；', '|']) {
        let part_trimmed = part.trim();
        if !part_trimmed.is_empty() {
            let cleaned = clean_description_item(part_trimmed);
            if !cleaned.is_empty() {
                items.push(cleaned);
            }
        }
    }

    if items.is_empty() {
        let cleaned = clean_description_item(trimmed);
        if !cleaned.is_empty() {
            items.push(cleaned);
        }
    }

    items
}

fn clean_description_item(s: &str) -> String {
    let mut cleaned = s.trim();
    cleaned = cleaned
        .trim_start_matches('-')
        .trim_start_matches('—')
        .trim_start_matches('–')
        .trim_start_matches(':')
        .trim_start_matches('|')
        .trim_start_matches('~')
        .trim();

    cleaned.to_string()
}

fn clean_name(s: &str) -> String {
    let mut cleaned = s.trim();

    // Strip trailing punctuation first so **Name**: becomes **Name**
    cleaned = cleaned
        .trim_end_matches(':')
        .trim_end_matches('—')
        .trim_end_matches('–')
        .trim_end_matches('-')
        .trim_end_matches('|')
        .trim();

    // Strip markdown formatting like **Name** or __Name__
    while (cleaned.starts_with("**") && cleaned.ends_with("**") && cleaned.len() >= 4)
        || (cleaned.starts_with("__") && cleaned.ends_with("__") && cleaned.len() >= 4)
    {
        cleaned = &cleaned[2..cleaned.len() - 2];
        cleaned = cleaned.trim();
    }
    while (cleaned.starts_with('*') && cleaned.ends_with('*') && cleaned.len() >= 2)
        || (cleaned.starts_with('_') && cleaned.ends_with('_') && cleaned.len() >= 2)
    {
        cleaned = &cleaned[1..cleaned.len() - 1];
        cleaned = cleaned.trim();
    }

    // Strip again in case colons or dashes were inside bold/italic
    cleaned = cleaned
        .trim_end_matches(':')
        .trim_end_matches('—')
        .trim_end_matches('–')
        .trim_end_matches('-')
        .trim_end_matches('|')
        .trim();

    cleaned.to_string()
}

fn extract_markdown_links_with_spans(text: &str) -> (Vec<SocialLink>, Vec<(usize, usize)>) {
    let mut links = Vec::new();
    let mut spans = Vec::new();
    let mut cursor_idx = 0;

    while let Some(relative_start) = text[cursor_idx..].find('[') {
        let start_bracket = cursor_idx + relative_start;
        let after_bracket = &text[start_bracket + 1..];
        if let Some(relative_end_bracket) = after_bracket.find(']') {
            let label = &after_bracket[..relative_end_bracket];
            let after_end_bracket = &after_bracket[relative_end_bracket + 1..];

            if after_end_bracket.starts_with('(') {
                let after_paren = &after_end_bracket[1..];
                if let Some(relative_end_paren) = after_paren.find(')') {
                    let mut url = after_paren[..relative_end_paren].trim().to_string();
                    if !url.is_empty()
                        && !url.starts_with("http://")
                        && !url.starts_with("https://")
                        && !url.starts_with("mailto:")
                    {
                        if url.contains('.') {
                            url = format!("https://{url}");
                        }
                    }

                    let end_pos =
                        start_bracket + 1 + relative_end_bracket + 1 + 1 + relative_end_paren + 1;
                    spans.push((start_bracket, end_pos));

                    if !label.trim().is_empty() && !url.is_empty() {
                        links.push(SocialLink {
                            label: label.trim().to_string(),
                            url,
                        });
                    }
                    cursor_idx = end_pos;
                    continue;
                }
            }
        }
        cursor_idx = start_bracket + 1;
    }

    (links, spans)
}

/// Loads contributors from disk if available, otherwise falls back to compile-time embedded markdown.
pub fn load_contributors(project_root: &Path) -> Vec<ContributorGroup> {
    let candidate_paths: [PathBuf; 3] = [
        project_root.join("docs").join("contributors.md"),
        project_root.join("docs").join("credits.md"),
        project_root.join("contributors.md"),
    ];

    for path in candidate_paths {
        if let Ok(content) = std::fs::read_to_string(&path) {
            let groups = parse_contributors_markdown(&content);
            if !groups.is_empty() {
                return groups;
            }
        }
    }

    parse_contributors_markdown(EMBEDDED_CONTRIBUTORS_MD)
}

static CACHE: Mutex<Option<(Instant, Vec<ContributorGroup>)>> = Mutex::new(None);

/// Loads contributors with a short-lived cache (3 seconds) to prevent redundant disk I/O on every frame.
pub fn load_contributors_cached(project_root: &Path) -> Vec<ContributorGroup> {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((last_read, groups)) = &*cache {
        if last_read.elapsed() < std::time::Duration::from_secs(3) {
            return groups.clone();
        }
    }
    let loaded = load_contributors(project_root);
    *cache = Some((Instant::now(), loaded.clone()));
    loaded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_standard_gfm_list() {
        let md = r#"
# Contributors & Credits

## Code Contributors

- **NowLoadY** — [GitHub](https://github.com/NowLoadY)
  - Project Creator & Core Architecture
  - Prompt Studio Workflow

- **ContributorA** — [GitHub](https://github.com/contributor-a), [X](https://x.com/contributor-a)
  - Audio Pipeline & VAD
  - Translation Engine

## Beta Testers

- **Tester1** — [Bilibili](https://space.bilibili.com/123456)
  - VRChat OSC Test
  - Multi-speaker Test
"#;
        let groups = parse_contributors_markdown(md);
        assert_eq!(groups.len(), 2);

        // Group 1: Code Contributors
        assert_eq!(groups[0].role, ContributorRole::CodeContributors);
        assert_eq!(groups[0].contributors.len(), 2);

        assert_eq!(groups[0].contributors[0].name, "NowLoadY");
        assert_eq!(groups[0].contributors[0].links.len(), 1);
        assert_eq!(groups[0].contributors[0].contributions.len(), 2);
        assert_eq!(
            groups[0].contributors[0].contributions[0],
            "Project Creator & Core Architecture"
        );
        assert_eq!(
            groups[0].contributors[0].contributions[1],
            "Prompt Studio Workflow"
        );

        assert_eq!(groups[0].contributors[1].name, "ContributorA");
        assert_eq!(groups[0].contributors[1].links.len(), 2);
        assert_eq!(groups[0].contributors[1].contributions.len(), 2);

        // Group 2: Beta Testers
        assert_eq!(groups[1].role, ContributorRole::BetaTesters);
        assert_eq!(groups[1].contributors.len(), 1);
        assert_eq!(groups[1].contributors[0].name, "Tester1");
        assert_eq!(groups[1].contributors[0].links[0].label, "Bilibili");
        assert_eq!(groups[1].contributors[0].contributions.len(), 2);
    }

    #[test]
    fn test_parse_gfm_table() {
        let md = r#"
## Code Contributors

| 贡献者 | 社交链接 | 贡献详情 |
| :--- | :--- | :--- |
| **NowLoadY** | [GitHub](https://github.com/NowLoadY) | 项目发起人与核心架构<br>Prompt Studio 节点流 |
| **DevA** | [GitHub](https://github.com/deva) · [X](https://x.com/deva) | 算法优化 |

## Beta Testers

| 测试者 | 社交主页 | 详情 |
| :--- | :--- | :--- |
| **Tester1** | [Bilibili](https://space.bilibili.com/123) | 实机联机测试 |
"#;
        let groups = parse_contributors_markdown(md);
        assert_eq!(groups.len(), 2);

        assert_eq!(groups[0].role, ContributorRole::CodeContributors);
        assert_eq!(groups[0].contributors.len(), 2);
        assert_eq!(groups[0].contributors[0].name, "NowLoadY");
        assert_eq!(groups[0].contributors[0].links.len(), 1);
        assert_eq!(groups[0].contributors[0].contributions.len(), 2);
        assert_eq!(
            groups[0].contributors[0].contributions[0],
            "项目发起人与核心架构"
        );
        assert_eq!(
            groups[0].contributors[0].contributions[1],
            "Prompt Studio 节点流"
        );

        assert_eq!(groups[0].contributors[1].name, "DevA");
        assert_eq!(groups[0].contributors[1].links.len(), 2);

        assert_eq!(groups[1].role, ContributorRole::BetaTesters);
        assert_eq!(groups[1].contributors[0].name, "Tester1");
        assert_eq!(groups[1].contributors[0].contributions[0], "实机联机测试");
    }

    #[test]
    fn test_empty_role_omitted() {
        let md = r#"
## Code Contributors
- **Developer1** — [GitHub](https://github.com/dev1)

## Beta Testers
"#;
        let groups = parse_contributors_markdown(md);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].role, ContributorRole::CodeContributors);
    }

    #[test]
    fn test_ordering_code_then_beta() {
        let md = r#"
## Beta Testers
- **Tester1** — [Bilibili](https://space.bilibili.com/123456)

## Code Contributors
- **Dev1** — [GitHub](https://github.com/dev1)
"#;
        let groups = parse_contributors_markdown(md);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].role, ContributorRole::CodeContributors);
        assert_eq!(groups[1].role, ContributorRole::BetaTesters);
    }

    #[test]
    fn parses_inline_strong_markdown_without_dropping_unmatched_markers() {
        assert_eq!(
            parse_inline_markdown("With **Tony** and **Fox**, fixed it."),
            vec![
                InlineMarkdownSpan {
                    text: "With ".into(),
                    strong: false,
                },
                InlineMarkdownSpan {
                    text: "Tony".into(),
                    strong: true,
                },
                InlineMarkdownSpan {
                    text: " and ".into(),
                    strong: false,
                },
                InlineMarkdownSpan {
                    text: "Fox".into(),
                    strong: true,
                },
                InlineMarkdownSpan {
                    text: ", fixed it.".into(),
                    strong: false,
                },
            ]
        );
        assert_eq!(
            parse_inline_markdown("Keep **unfinished"),
            vec![InlineMarkdownSpan {
                text: "Keep **unfinished".into(),
                strong: false,
            }]
        );
    }

    #[test]
    fn contributor_descriptions_preserve_inline_markdown_for_rendering() {
        let groups = parse_contributors_markdown(
            "## Beta Testers\n- **Tester**\n  - With **Tony** and **Fox**, fixed it.\n",
        );
        assert_eq!(
            groups[0].contributors[0].contributions,
            ["With **Tony** and **Fox**, fixed it."]
        );
    }
}
