//! OpenAI Codex CLI: ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl
//!
//! Current format wraps each line as {"timestamp","type","payload"} where type is
//! session_meta | turn_context | response_item | event_msg. Older rollouts wrote
//! response items directly, one per line; both are handled.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{flatten_text, home, jsonl_files, Source};
use crate::model::{bump_times, derive_title, Message, Parsed, SessionMeta};

pub struct Codex;

impl Codex {
    fn root() -> PathBuf {
        std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".codex"))
    }
}

impl Source for Codex {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn discover(&self) -> Vec<PathBuf> {
        let root = Self::root();
        let mut files = jsonl_files(&root.join("sessions"), 5);
        files.extend(jsonl_files(&root.join("archived_sessions"), 5));
        files
    }

    fn parse(&self, path: &Path) -> anyhow::Result<Parsed> {
        let reader = BufReader::new(std::fs::File::open(path)?);
        let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
        let mut meta = SessionMeta {
            id: format!("codex:{stem}"),
            source: "codex".into(),
            path: path.to_string_lossy().into(),
            ..Default::default()
        };
        let mut messages = Vec::new();

        for line in reader.lines() {
            let Ok(line) = line else { continue };
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
            let ts = v.get("timestamp").and_then(Value::as_str).map(str::to_string);

            let (kind, payload) = match v.get("payload") {
                Some(p) => (v.get("type").and_then(Value::as_str).unwrap_or(""), p),
                // Legacy: bare response items; the first line is session metadata.
                None if v.get("id").is_some() && v.get("type").is_none() => ("session_meta", &v),
                None => ("response_item", &v),
            };
            let str_of = |val: &Value, k: &str| val.get(k).and_then(Value::as_str).map(str::to_string);

            match kind {
                "session_meta" => {
                    meta.project = meta.project.take().or_else(|| str_of(payload, "cwd"));
                    meta.git_branch = payload.get("git").and_then(|g| str_of(g, "branch"));
                    bump_times(&mut meta, &str_of(payload, "timestamp"));
                }
                "turn_context" => {
                    if let Some(m) = str_of(payload, "model") {
                        meta.model = Some(m);
                    }
                    meta.project = meta.project.take().or_else(|| str_of(payload, "cwd"));
                }
                "event_msg" if payload["type"] == "token_count" => {
                    // Cumulative totals; keep the latest.
                    let u = &payload["info"]["total_token_usage"];
                    if let Some(i) = u["input_tokens"].as_i64() {
                        meta.input_tokens = i;
                        meta.output_tokens = u["output_tokens"].as_i64().unwrap_or(0);
                    }
                }
                "response_item" => {
                    bump_times(&mut meta, &ts);
                    push_item(&mut messages, payload, ts);
                }
                _ => {}
            }
        }

        meta.message_count = messages.iter().filter(|m| m.kind == "text" && m.role != "system").count() as i64;
        meta.title = derive_title(&messages);
        Ok(Parsed { meta, messages })
    }
}

fn push_item(out: &mut Vec<Message>, item: &Value, ts: Option<String>) {
    match item.get("type").and_then(Value::as_str) {
        Some("message") => {
            let text = flatten_text(&item["content"]);
            if text.trim().is_empty() {
                return;
            }
            let role = match item["role"].as_str() {
                Some("assistant") => "assistant",
                Some("user") if !is_injected_context(&text) => "user",
                _ => "system",
            };
            out.push(Message::new(role, "text", text, ts));
        }
        Some("reasoning") => {
            // Reasoning without a summary is encrypted. The empty block still marks when
            // thinking happened, which the timeline needs.
            out.push(Message::new("assistant", "thinking", flatten_text(&item["summary"]), ts));
        }
        Some("function_call") | Some("custom_tool_call") => {
            let mut m = Message::new("assistant", "tool_use", "", ts);
            m.tool_name = item["name"].as_str().map(str::to_string);
            let raw = item.get("arguments").or_else(|| item.get("input")).cloned().unwrap_or(Value::Null);
            // Arguments arrive as a JSON-encoded string.
            m.tool_input = Some(match &raw {
                Value::String(s) => serde_json::from_str(s).unwrap_or(raw.clone()),
                _ => raw,
            });
            out.push(m);
        }
        Some("local_shell_call") => {
            let mut m = Message::new("assistant", "tool_use", "", ts);
            m.tool_name = Some("shell".into());
            m.tool_input = Some(item["action"].clone());
            out.push(m);
        }
        Some("function_call_output") | Some("custom_tool_call_output") => {
            let output = &item["output"];
            // Output is often a JSON string like {"output": "...", "metadata": {"exit_code": 1}}.
            let (text, is_error) = match output.as_str().and_then(|s| serde_json::from_str::<Value>(s).ok()) {
                Some(obj) if obj.get("output").is_some() => (
                    flatten_text(&obj["output"]),
                    obj["metadata"]["exit_code"].as_i64().is_some_and(|c| c != 0),
                ),
                _ => (flatten_text(output), false),
            };
            let mut m = Message::new("user", "tool_result", text, ts);
            m.is_error = is_error;
            out.push(m);
        }
        _ => {}
    }
}

fn is_injected_context(text: &str) -> bool {
    let t = text.trim_start();
    t.starts_with("<environment_context>") || t.starts_with("<user_instructions>") || t.starts_with("# AGENTS.md")
}
