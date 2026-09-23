use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize)]
pub struct SessionMeta {
    pub id: String,
    pub source: String,
    pub path: String,
    pub project: Option<String>,
    pub title: Option<String>,
    pub started_at: Option<String>,
    pub updated_at: Option<String>,
    pub message_count: i64,
    pub model: Option<String>,
    pub git_branch: Option<String>,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// One renderable block in a transcript. A single API message with several
/// content blocks (text + tool calls) becomes several `Message`s.
#[derive(Debug, Clone, Serialize)]
pub struct Message {
    /// user | assistant | system
    pub role: String,
    /// text | thinking | tool_use | tool_result
    pub kind: String,
    pub text: String,
    pub tool_name: Option<String>,
    pub tool_input: Option<Value>,
    pub is_error: bool,
    pub timestamp: Option<String>,
}

impl Message {
    pub fn new(role: &str, kind: &str, text: impl Into<String>, timestamp: Option<String>) -> Self {
        Self {
            role: role.into(),
            kind: kind.into(),
            text: text.into(),
            tool_name: None,
            tool_input: None,
            is_error: false,
            timestamp,
        }
    }
}

pub struct Parsed {
    pub meta: SessionMeta,
    pub messages: Vec<Message>,
}

/// Title fallback: first real user prompt, single line, truncated.
pub fn derive_title(messages: &[Message]) -> Option<String> {
    messages
        .iter()
        .find(|m| m.role == "user" && m.kind == "text" && !m.text.trim_start().starts_with('<'))
        .map(|m| {
            let line = m.text.split_whitespace().collect::<Vec<_>>().join(" ");
            if line.chars().count() > 100 {
                format!("{}…", line.chars().take(100).collect::<String>())
            } else {
                line
            }
        })
}

/// Track first/last timestamps. ISO-8601 UTC strings sort lexicographically.
pub fn bump_times(meta: &mut SessionMeta, ts: &Option<String>) {
    if let Some(ts) = ts {
        if meta.started_at.as_ref().is_none_or(|s| ts < s) {
            meta.started_at = Some(ts.clone());
        }
        if meta.updated_at.as_ref().is_none_or(|s| ts > s) {
            meta.updated_at = Some(ts.clone());
        }
    }
}
