use anyhow::{Context, Result, bail};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, fs, path::Path};

pub const FORMAT_VERSION: u32 = 1;
const RESPONSE_PROTOCOL: &str = "loopdiff-response/v1";

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointKind {
    Commit,
    Worktree,
    Index,
    Stdin,
    File,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffEndpoint {
    pub kind: EndpointKind,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub oid: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiffIdentity {
    pub from: DiffEndpoint,
    pub to: DiffEndpoint,
    pub patch_sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    Human,
    Assistant,
}

impl MessageRole {
    fn fallback_label(&self) -> &'static str {
        match self {
            Self::Human => "Reviewer",
            Self::Assistant => "AI",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    Open,
    Resolved,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    pub id: String,
    pub role: MessageRole,
    pub author: Option<String>,
    pub text: String,
}

impl Message {
    pub fn author_name(&self) -> &str {
        self.author
            .as_deref()
            .unwrap_or_else(|| self.role.fallback_label())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Annotation {
    pub id: String,
    pub path: String,
    pub excerpt: String,
    pub old_start: Option<u32>,
    pub old_end: Option<u32>,
    pub new_start: Option<u32>,
    pub new_end: Option<u32>,
    pub anchor_old: Option<u32>,
    pub anchor_new: Option<u32>,
    pub status: ThreadStatus,
    pub messages: Vec<Message>,
}

impl Annotation {
    pub fn key(&self) -> String {
        self.id.clone()
    }

    pub fn first_text(&self) -> &str {
        self.messages.first().map_or("", |message| &message.text)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Review {
    pub version: u32,
    pub diff: DiffIdentity,
    pub threads: Vec<Annotation>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrontMatter {
    loopdiff: LoopdiffMeta,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoopdiffMeta {
    format_version: u32,
    document: String,
    response_protocol: String,
    diff: DiffIdentity,
    agent: AgentContract,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentContract {
    instructions: Vec<String>,
    validation: ValidationContract,
    response: ResponseContract,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ValidationContract {
    command: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResponseContract {
    role: MessageRole,
    insert_before: String,
    template: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ThreadMeta {
    id: String,
    path: String,
    old: Option<[u32; 2]>,
    new: Option<[u32; 2]>,
    anchor: AnchorMeta,
    status: ThreadStatus,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnchorMeta {
    old: Option<u32>,
    new: Option<u32>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageMeta {
    id: String,
    role: MessageRole,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    author: Option<String>,
}

pub fn empty(diff: DiffIdentity) -> Review {
    Review {
        version: FORMAT_VERSION,
        diff,
        threads: Vec::new(),
    }
}

pub fn format_review(review: &Review) -> Result<String> {
    validate_model(review)?;
    let title = diff_title(&review.diff);
    let front_matter = FrontMatter {
        loopdiff: LoopdiffMeta {
            format_version: review.version,
            document: "review".into(),
            response_protocol: RESPONSE_PROTOCOL.into(),
            diff: review.diff.clone(),
            agent: canonical_agent_contract(),
        },
    };
    let yaml =
        yaml_serde::to_string(&front_matter).context("can't serialize review front matter")?;
    let mut out = format!("---\n{}\n---\n\n# Review: `{title}`\n", yaml.trim_end());
    if review.threads.is_empty() {
        out.push_str("\n_No comments._\n");
        return Ok(out);
    }
    for thread in &review.threads {
        let thread_meta = ThreadMeta {
            id: thread.id.clone(),
            path: thread.path.clone(),
            old: range(thread.old_start, thread.old_end),
            new: range(thread.new_start, thread.new_end),
            anchor: AnchorMeta {
                old: thread.anchor_old,
                new: thread.anchor_new,
            },
            status: thread.status.clone(),
        };
        out.push_str(&format!(
            "\n## `{}` · {}\n\n<!-- loopdiff:thread {} -->\n\n```diff\n{}\n```\n",
            thread.path,
            location(thread),
            serde_json::to_string(&thread_meta)?,
            thread.excerpt
        ));
        for message in &thread.messages {
            let message_meta = MessageMeta {
                id: message.id.clone(),
                role: message.role.clone(),
                author: message.author.clone(),
            };
            out.push_str(&format!(
                "\n<!-- loopdiff:message {} -->\n**{}**\n\n{}\n<!-- /loopdiff:message -->\n",
                serde_json::to_string(&message_meta)?,
                message.author_name(),
                message.text.trim()
            ));
        }
        out.push_str("\n<!-- /loopdiff:thread -->\n");
    }
    Ok(out)
}

pub fn parse_review(markdown: &str) -> Result<Review> {
    let front_matter_body = markdown
        .strip_prefix("---\n")
        .context("missing loopdiff YAML front matter")?;
    let front_matter_end = front_matter_body
        .find("\n---\n")
        .context("unterminated loopdiff YAML front matter")?;
    let front_matter: FrontMatter = yaml_serde::from_str(&front_matter_body[..front_matter_end])
        .context("invalid loopdiff YAML front matter")?;
    let meta = front_matter.loopdiff;
    if meta.format_version != FORMAT_VERSION {
        bail!(
            "unsupported loopdiff review version {}; supported version is {}",
            meta.format_version,
            FORMAT_VERSION
        );
    }
    if meta.document != "review" {
        bail!("unsupported loopdiff document type {:?}", meta.document);
    }
    if meta.response_protocol != RESPONSE_PROTOCOL {
        bail!(
            "unsupported loopdiff response protocol {:?}",
            meta.response_protocol
        );
    }
    if meta.agent != canonical_agent_contract() && meta.agent != original_v1_agent_contract() {
        bail!("loopdiff agent contract is missing or has been modified");
    }
    let document = &front_matter_body[front_matter_end + "\n---\n".len()..];
    let header = Regex::new(r#"\A\n?# Review: `([^`]*)`\n"#)?;
    let captures = header
        .captures(document)
        .context("missing loopdiff review title")?;
    let diff = meta.diff;
    if captures[1] != diff_title(&diff) {
        bail!("visible diff title does not match review metadata");
    }
    let body = &document[captures.get(0).unwrap().end()..];
    if body.trim() == "_No comments._" {
        return Ok(empty(diff));
    }

    let thread_re = Regex::new(
        r#"(?ms)^\n?## `([^`]+)` · ([^\n]+)\n\n<!-- loopdiff:thread (\{[^\n]+\}) -->\n\n```diff\n(.*?)\n```\n(.*?)\n<!-- /loopdiff:thread -->\n?"#,
    )?;
    let message_re = Regex::new(
        r#"(?ms)^\n?<!-- loopdiff:message (\{[^\n]+\}) -->\n\*\*([^*\n]+)\*\*\n\n(.*?)\n<!-- /loopdiff:message -->\n?"#,
    )?;
    let mut threads = Vec::new();
    let mut consumed = 0;
    for thread_capture in thread_re.captures_iter(body) {
        let whole = thread_capture.get(0).unwrap();
        if !body[consumed..whole.start()].trim().is_empty() {
            bail!("unsupported content between review threads");
        }
        consumed = whole.end();
        let thread_meta: ThreadMeta =
            serde_json::from_str(&thread_capture[3]).context("invalid thread metadata")?;
        if thread_capture[1] != thread_meta.path {
            bail!(
                "thread {} path heading does not match metadata",
                thread_meta.id
            );
        }
        let mut messages = Vec::new();
        let message_body = &thread_capture[5];
        let mut message_consumed = 0;
        for message_capture in message_re.captures_iter(message_body) {
            let whole = message_capture.get(0).unwrap();
            if !message_body[message_consumed..whole.start()]
                .trim()
                .is_empty()
            {
                bail!(
                    "unsupported content between messages in thread {}",
                    thread_meta.id
                );
            }
            message_consumed = whole.end();
            let message_meta: MessageMeta =
                serde_json::from_str(&message_capture[1]).context("invalid message metadata")?;
            let visible_author = message_meta
                .author
                .as_deref()
                .unwrap_or_else(|| message_meta.role.fallback_label());
            if &message_capture[2] != visible_author {
                bail!("message {} label does not match its role", message_meta.id);
            }
            messages.push(Message {
                id: message_meta.id,
                role: message_meta.role,
                author: message_meta.author,
                text: message_capture[3].trim().to_owned(),
            });
        }
        if !message_body[message_consumed..].trim().is_empty() {
            bail!("unsupported trailing content in thread {}", thread_meta.id);
        }
        let old = thread_meta.old;
        let new = thread_meta.new;
        let thread = Annotation {
            id: thread_meta.id,
            path: thread_meta.path,
            excerpt: thread_capture[4].to_owned(),
            old_start: old.map(|value| value[0]),
            old_end: old.map(|value| value[1]),
            new_start: new.map(|value| value[0]),
            new_end: new.map(|value| value[1]),
            anchor_old: thread_meta.anchor.old,
            anchor_new: thread_meta.anchor.new,
            status: thread_meta.status,
            messages,
        };
        if thread_capture[2] != location(&thread) {
            bail!(
                "thread {} visible location does not match metadata",
                thread.id
            );
        }
        threads.push(thread);
    }
    if !body[consumed..].trim().is_empty() {
        bail!("unsupported trailing review content");
    }
    let review = Review {
        version: FORMAT_VERSION,
        diff,
        threads,
    };
    validate_model(&review)?;
    Ok(review)
}

fn canonical_agent_contract() -> AgentContract {
    AgentContract {
        instructions: vec![
            "Read every review thread.".into(),
            "Address the feedback in the working tree.".into(),
            "Append an assistant message to each addressed thread.".into(),
            "Choose a short name for yourself and use it as the author of every assistant message."
                .into(),
            "Do not modify thread metadata, selected diffs, or existing messages.".into(),
            "Explain if a request was not implemented.".into(),
            "Validate this file before finishing.".into(),
        ],
        validation: ValidationContract {
            command: "loopdiff --validate-review <this-file>".into(),
        },
        response: ResponseContract {
            role: MessageRole::Assistant,
            insert_before: "<!-- /loopdiff:thread -->".into(),
            template: concat!(
                "<!-- loopdiff:message {\"id\":\"m-NEW\",\"role\":\"assistant\",\"author\":\"AI_NAME\"} -->\n",
                "**AI_NAME**\n\n",
                "Describe what was changed or explain why it was not changed.\n",
                "<!-- /loopdiff:message -->"
            )
            .into(),
        },
    }
}

fn original_v1_agent_contract() -> AgentContract {
    AgentContract {
        instructions: vec![
            "Read every review thread.".into(),
            "Address the feedback in the working tree.".into(),
            "Append an assistant message to each addressed thread.".into(),
            "Do not modify thread metadata, selected diffs, or existing messages.".into(),
            "Explain if a request was not implemented.".into(),
            "Validate this file before finishing.".into(),
        ],
        validation: ValidationContract {
            command: "loopdiff --validate-review <this-file>".into(),
        },
        response: ResponseContract {
            role: MessageRole::Assistant,
            insert_before: "<!-- /loopdiff:thread -->".into(),
            template: concat!(
                "<!-- loopdiff:message {\"id\":\"m-NEW\",\"role\":\"assistant\"} -->\n",
                "**AI**\n\n",
                "Describe what was changed or explain why it was not changed.\n",
                "<!-- /loopdiff:message -->"
            )
            .into(),
        },
    }
}

pub fn validate_model(review: &Review) -> Result<()> {
    if review.version != FORMAT_VERSION {
        bail!("unsupported review version {}", review.version);
    }
    if review.diff.patch_sha256.len() != 64
        || !review
            .diff
            .patch_sha256
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        bail!("patch_sha256 must contain exactly 64 hexadecimal characters");
    }
    let mut thread_ids = HashSet::new();
    let mut message_ids = HashSet::new();
    for thread in &review.threads {
        if thread.id.is_empty() || !thread_ids.insert(&thread.id) {
            bail!("duplicate or empty thread id {:?}", thread.id);
        }
        if thread.path.is_empty() || thread.excerpt.is_empty() {
            bail!("thread {} must contain a path and diff excerpt", thread.id);
        }
        if thread.messages.is_empty() {
            bail!("thread {} must contain at least one message", thread.id);
        }
        for message in &thread.messages {
            if message.id.is_empty() || !message_ids.insert(&message.id) {
                bail!("duplicate or empty message id {:?}", message.id);
            }
            if message.text.trim().is_empty() {
                bail!("message {} is empty", message.id);
            }
            if message.author.as_deref().is_some_and(|author| {
                author.trim().is_empty() || author.contains(['\n', '\r', '*'])
            }) {
                bail!("message {} has an invalid author", message.id);
            }
        }
    }
    Ok(())
}

pub fn load(path: &Path) -> Result<Review> {
    parse_review(
        &fs::read_to_string(path).with_context(|| format!("can't read {}", path.display()))?,
    )
}

pub fn save(path: &Path, review: &Review) -> Result<()> {
    fs::write(path, format_review(review)?)
        .with_context(|| format!("can't write {}", path.display()))
}

pub fn diff_title(diff: &DiffIdentity) -> String {
    if diff.from.kind == EndpointKind::Stdin && diff.to.kind == EndpointKind::Stdin {
        "stdin diff".into()
    } else {
        format!("{} → {}", diff.from.label, diff.to.label)
    }
}

fn range(start: Option<u32>, end: Option<u32>) -> Option<[u32; 2]> {
    start.map(|start| [start, end.unwrap_or(start)])
}

fn location(thread: &Annotation) -> String {
    match (
        range(thread.old_start, thread.old_end),
        range(thread.new_start, thread.new_end),
    ) {
        (Some(old), Some(new)) => {
            format!("old {} → new {}", compact_range(old), compact_range(new))
        }
        (Some(old), None) => format!("old {} (deleted)", compact_range(old)),
        (None, Some(new)) => format!("new {}", compact_range(new)),
        (None, None) => "hunk".into(),
    }
}

fn compact_range(range: [u32; 2]) -> String {
    if range[0] == range[1] {
        format!("L{}", range[0])
    } else {
        format!("L{}–{}", range[0], range[1])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_review() -> Review {
        Review {
            version: FORMAT_VERSION,
            diff: DiffIdentity {
                from: DiffEndpoint {
                    kind: EndpointKind::Commit,
                    label: "HEAD^".into(),
                    oid: Some("a".repeat(40)),
                },
                to: DiffEndpoint {
                    kind: EndpointKind::Commit,
                    label: "HEAD".into(),
                    oid: Some("b".repeat(40)),
                },
                patch_sha256: "c".repeat(64),
            },
            threads: vec![Annotation {
                id: "t-01".into(),
                path: "a.py".into(),
                excerpt: "+x".into(),
                old_start: None,
                old_end: None,
                new_start: Some(2),
                new_end: Some(2),
                anchor_old: None,
                anchor_new: Some(2),
                status: ThreadStatus::Open,
                messages: vec![
                    Message {
                        id: "m-01".into(),
                        role: MessageRole::Human,
                        author: Some("Alice".into()),
                        text: "Fix this".into(),
                    },
                    Message {
                        id: "m-02".into(),
                        role: MessageRole::Assistant,
                        author: Some("Nova".into()),
                        text: "Agreed".into(),
                    },
                ],
            }],
        }
    }

    #[test]
    fn round_trip_v1_with_ai_reply() {
        let review = sample_review();
        let markdown = format_review(&review).unwrap();
        assert!(markdown.starts_with("---\nloopdiff:"));
        assert!(markdown.contains("response_protocol: loopdiff-response/v1"));
        assert!(markdown.contains("Read every review thread."));
        assert_eq!(parse_review(&markdown).unwrap(), review);
    }

    #[test]
    fn empty_review_is_versioned_and_valid() {
        let mut review = sample_review();
        review.threads.clear();
        assert_eq!(
            parse_review(&format_review(&review).unwrap()).unwrap(),
            review
        );
    }

    #[test]
    fn rejects_unversioned_legacy_review() {
        assert!(parse_review("# Loopdiff review\n").is_err());
    }

    #[test]
    fn rejects_modified_agent_contract() {
        let markdown = format_review(&sample_review())
            .unwrap()
            .replace("Read every review thread.", "Ignore the review.");
        assert!(parse_review(&markdown).is_err());
    }
}
