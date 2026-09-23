use std::path::{Path, PathBuf};

use crate::model::Parsed;

mod claude;
mod codex;

/// An agent whose on-disk transcripts Receipts can read.
/// To add a new agent, implement this trait and register it in `all()`.
pub trait Source: Send + Sync {
    fn name(&self) -> &'static str;
    fn discover(&self) -> Vec<PathBuf>;
    fn parse(&self, path: &Path) -> anyhow::Result<Parsed>;
}

pub fn all() -> Vec<Box<dyn Source>> {
    vec![Box::new(claude::Claude), Box::new(codex::Codex)]
}

pub(crate) fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn jsonl_files(root: &Path, max_depth: usize) -> Vec<PathBuf> {
    walkdir::WalkDir::new(root)
        .max_depth(max_depth)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|x| x == "jsonl"))
        .map(|e| e.into_path())
        .collect()
}

/// Flatten a tool result / content value (string or array of blocks) into text.
pub(crate) fn flatten_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(items) => items
            .iter()
            .filter_map(|b| match b.get("type").and_then(|t| t.as_str()) {
                Some("image") | Some("input_image") => Some("[image]".to_string()),
                _ => b.get("text").and_then(|t| t.as_str()).map(str::to_string),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}
