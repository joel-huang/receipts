//! Claude Code: ~/.claude/projects/<encoded-cwd>/<session-id>.jsonl

use std::collections::HashSet;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{flatten_text, home, jsonl_files, Source};
use crate::model::{bump_times, derive_title, Message, Parsed, SessionMeta};

pub struct Claude;

impl Claude {
    fn root() -> PathBuf {
        std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".claude"))
            .join("projects")
    }
}

impl Source for Claude {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn discover(&self) -> Vec<PathBuf> {
        // Depth 2 = projects/<project>/<session>.jsonl; subagent transcripts live deeper.
        jsonl_files(&Self::root(), 2)
    }

    fn parse(&self, path: &Path) -> anyhow::Result<Parsed> {
        let reader = BufReader::new(std::fs::File::open(path)?);
        let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let mut meta = SessionMeta {
            id: format!("claude:{stem}"),
            source: "claude".into(),
            path: path.to_string_lossy().into(),
            ..Default::default()
        };
        let mut messages = Vec::new();
        let (mut ai_title, mut custom_title, mut summary) = (None, None, None);
        let mut seen_usage = HashSet::new();

        for line in reader.lines() {
            let Ok(line) = line else { continue };
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);

            match v.get("type").and_then(Value::as_str) {
                Some("ai-title") => ai_title = s("aiTitle"),
                Some("custom-title") => custom_title = s("customTitle"),
                Some("summary") => summary = s("summary"),
                Some(role @ ("user" | "assistant")) => {
                    if v.get("isMeta").and_then(Value::as_bool) == Some(true) {
                        continue;
                    }
                    let ts = s("timestamp");
                    bump_times(&mut meta, &ts);
                    if meta.project.is_none() {
                        meta.project = s("cwd");
                    }
                    if meta.git_branch.is_none() {
                        meta.git_branch = s("gitBranch").filter(|b| !b.is_empty());
                    }
                    let msg = &v["message"];
                    if let Some(m) = msg.get("model").and_then(Value::as_str) {
                        if m != "<synthetic>" {
                            meta.model = Some(m.to_string());
                        }
                    }
                    // Streaming writes one line per content block with the same message id + usage.
                    if let (Some(id), Some(u)) = (msg.get("id").and_then(Value::as_str), msg.get("usage")) {
                        if seen_usage.insert(id.to_string()) {
                            let n = |k: &str| u.get(k).and_then(Value::as_i64).unwrap_or(0);
                            meta.input_tokens += n("input_tokens")
                                + n("cache_creation_input_tokens")
                                + n("cache_read_input_tokens");
                            meta.output_tokens += n("output_tokens");
                        }
                    }
                    push_content(&mut messages, role, &msg["content"], ts);
                }
                _ => {}
            }
        }

        meta.message_count = messages.iter().filter(|m| m.kind == "text").count() as i64;
        meta.title = custom_title.or(ai_title).or(summary).or_else(|| derive_title(&messages));
        Ok(Parsed { meta, messages })
    }
}

fn push_content(out: &mut Vec<Message>, role: &str, content: &Value, ts: Option<String>) {
    let blocks = match content {
        Value::String(text) => {
            out.push(Message::new(role, "text", text.clone(), ts));
            return;
        }
        Value::Array(b) => b,
        _ => return,
    };
    for b in blocks {
        match b.get("type").and_then(Value::as_str) {
            Some("text") => {
                let text = b["text"].as_str().unwrap_or_default();
                if !text.trim().is_empty() {
                    out.push(Message::new(role, "text", text, ts.clone()));
                }
            }
            Some("thinking") | Some("redacted_thinking") => {
                // Claude Code often stores thinking as an empty string with only a signature. The
                // empty block still marks when thinking happened, which the timeline needs.
                let text = b["thinking"].as_str().unwrap_or_default();
                out.push(Message::new(role, "thinking", text, ts.clone()));
            }
            Some("tool_use") | Some("server_tool_use") => {
                let mut m = Message::new(role, "tool_use", "", ts.clone());
                m.tool_name = b["name"].as_str().map(str::to_string);
                m.tool_input = Some(b["input"].clone());
                out.push(m);
            }
            Some("tool_result") => {
                let mut m = Message::new("user", "tool_result", flatten_text(&b["content"]), ts.clone());
                m.is_error = b["is_error"].as_bool().unwrap_or(false);
                out.push(m);
            }
            Some("image") => out.push(Message::new(role, "text", "[image]", ts.clone())),
            _ => {}
        }
    }
}
