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
    /// The chat view got a shortened copy. The full message comes from its own endpoint.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
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
            truncated: false,
        }
    }
}

pub struct Parsed {
    pub meta: SessionMeta,
    pub messages: Vec<Message>,
}

/// Tags that agents write into user messages themselves, such as slash commands and background
/// task notifications. Other tags, such as <pasted_content>, come from the user.
const AGENT_TAGS: &[&str] = &[
    "bash-input",
    "bash-stderr",
    "bash-stdout",
    "command-args",
    "command-message",
    "command-name",
    "local-command-caveat",
    "local-command-stderr",
    "local-command-stdout",
    "persisted-output",
    "system-reminder",
    "task-notification",
    "tool_use_error",
    "user-memory-input",
];

/// True if the text is a prompt that the user typed, not a message that the agent wrote.
pub fn is_typed_prompt(text: &str) -> bool {
    let Some(rest) = text.trim_start().strip_prefix('<') else { return true };
    let tag: String = rest.chars().take_while(|c| c.is_ascii_lowercase() || *c == '-' || *c == '_').collect();
    !AGENT_TAGS.contains(&tag.as_str())
}

/// Title fallback: first typed user prompt, single line, truncated.
pub fn derive_title(messages: &[Message]) -> Option<String> {
    messages
        .iter()
        .find(|m| m.role == "user" && m.kind == "text" && is_typed_prompt(&m.text))
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

#[cfg(test)]
mod tests {
    use super::is_typed_prompt;

    #[test]
    fn tells_typed_prompts_from_agent_messages() {
        assert!(is_typed_prompt("fix the bug"));
        assert!(is_typed_prompt("  <pasted_content id=\"1\">\nsome text</pasted_content>"));
        assert!(!is_typed_prompt("<command-name>/clear</command-name>"));
        assert!(!is_typed_prompt("<task-notification>\n<task-id>1</task-id>"));
    }
}
