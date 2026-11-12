//! Saved conversations: each one is a file in
//! `~/.local/share/iliada/conversations/<id>.json`, readable only by its
//! owner. It keeps what was shown (the transcript) and what the model needs
//! to continue (the messages).

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineKind {
    /// What the person asked.
    You,
    /// The assistant's answer.
    Assistant,
    /// An action and its outcome ("✓ Install Firefox").
    Note,
    Error,
}

/// One line of the transcript, as shown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub kind: LineKind,
    pub text: String,
}

impl Line {
    pub fn new(kind: LineKind, text: impl Into<String>) -> Self {
        Line { kind, text: text.into() }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Conversation {
    pub id: String,
    pub title: String,
    /// Seconds since the Unix epoch.
    pub created: u64,
    pub updated: u64,
    /// The model it was last continued with.
    pub model: String,
    /// What it has cost (US dollars, when known).
    pub spent: f64,
    pub lines: Vec<Line>,
    /// The model's view of the conversation (provider format).
    pub messages: Vec<Value>,
    /// What happened outside the conversation since the last answer (an
    /// action undone from Activity): told to the model with the next question.
    #[serde(default)]
    pub pending: Vec<String>,
}

/// A conversation in the list (without its contents).
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub id: String,
    pub title: String,
    pub updated: u64,
}

pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn dir() -> Option<PathBuf> {
    crate::data_path("conversations")
}

/// Ids are file names: only these characters.
fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// A title from the first question: its first line, at most ~50 characters,
/// cut at a word.
pub fn title_from(question: &str) -> String {
    let first = question.lines().next().unwrap_or("").trim();
    if first.chars().count() <= 50 {
        return first.to_owned();
    }
    let cut: String = first.chars().take(50).collect();
    let cut = cut.rsplit_once(' ').map(|(a, _)| a).filter(|a| a.len() > 20).unwrap_or(&cut);
    format!("{}…", cut.trim_end_matches([',', '.', ';', ':']))
}

impl Conversation {
    pub fn new(model: &str) -> Self {
        let t = now();
        let nanos =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        Conversation {
            id: format!("{t}-{:08x}", nanos ^ std::process::id()),
            created: t,
            updated: t,
            model: model.to_owned(),
            ..Default::default()
        }
    }

    pub fn save(&mut self) -> Result<()> {
        self.save_in(&dir().context("HOME is not set")?)
    }

    fn save_in(&mut self, dir: &std::path::Path) -> Result<()> {
        use std::io::Write;
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        anyhow::ensure!(valid_id(&self.id), "bad conversation id");
        if self.title.is_empty()
            && let Some(q) = self.lines.iter().find(|l| l.kind == LineKind::You)
        {
            self.title = title_from(&q.text);
        }
        self.updated = now();
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
        let tmp = dir.join(format!(".{}.tmp", self.id));
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(&serde_json::to_vec(self)?)?;
        f.sync_all()?;
        std::fs::rename(&tmp, dir.join(format!("{}.json", self.id)))?;
        Ok(())
    }

    pub fn load(id: &str) -> Result<Self> {
        Self::load_from(&dir().context("HOME is not set")?, id)
    }

    fn load_from(dir: &std::path::Path, id: &str) -> Result<Self> {
        anyhow::ensure!(valid_id(id), "bad conversation id");
        let bytes = std::fs::read(dir.join(format!("{id}.json"))).context("that conversation is gone")?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    pub fn delete(id: &str) -> Result<()> {
        anyhow::ensure!(valid_id(id), "bad conversation id");
        std::fs::remove_file(dir().context("HOME is not set")?.join(format!("{id}.json")))?;
        Ok(())
    }

    /// Saved conversations, newest first.
    pub fn list() -> Vec<Summary> {
        dir().map(|d| list_in(&d)).unwrap_or_default()
    }
}

fn list_in(dir: &std::path::Path) -> Vec<Summary> {
    let mut out: Vec<Summary> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| std::fs::read(e.path()).ok())
        .filter_map(|b| serde_json::from_slice::<Conversation>(&b).ok())
        .filter(|c| !c.lines.is_empty())
        .map(|c| Summary {
            id: c.id,
            title: if c.title.is_empty() { "Conversation".into() } else { c.title },
            updated: c.updated,
        })
        .collect();
    out.sort_by_key(|s| std::cmp::Reverse(s.updated));
    out
}

/// The conversation as plain text turns, for a model from another provider
/// (thinking blocks and tool calls are provider-specific).
pub fn portable(messages: &[Value]) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    for m in messages {
        let role = m["role"].as_str().unwrap_or("");
        let text = match &m["content"] {
            Value::String(s) => s.clone(),
            Value::Array(blocks) => blocks
                .iter()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        };
        if text.trim().is_empty() || !matches!(role, "user" | "assistant") {
            continue;
        }
        match out.last_mut() {
            Some(last) if last["role"] == role => {
                let joined = format!("{}\n{}", last["content"].as_str().unwrap_or(""), text);
                last["content"] = Value::String(joined);
            }
            _ => out.push(json!({ "role": role, "content": text })),
        }
    }
    // A conversation sent to a model starts with the person.
    while out.first().is_some_and(|m| m["role"] != "user") {
        out.remove(0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_are_short() {
        assert_eq!(title_from("Install Firefox"), "Install Firefox");
        let t = title_from("Please install Firefox and VLC and then turn on the SSH server and dark mode");
        assert!(t.ends_with('…') && t.chars().count() <= 51, "{t}");
        assert_eq!(title_from("first line\nsecond"), "first line");
    }

    #[test]
    fn conversations_are_saved_listed_and_loaded() {
        let d = std::env::temp_dir().join(format!("iliada-conv-{}", std::process::id()));
        let mut c = Conversation::new("deepseek/deepseek-v4-flash");
        c.lines.push(Line::new(LineKind::You, "Install Firefox"));
        c.lines.push(Line::new(LineKind::Assistant, "Done."));
        c.messages.push(json!({ "role": "user", "content": "Install Firefox" }));
        c.save_in(&d).unwrap();
        let mut empty = Conversation::new("x");
        empty.id = "0-empty".into();
        empty.save_in(&d).unwrap();
        let list = list_in(&d);
        assert_eq!(list.len(), 1, "empty conversations are not listed");
        assert_eq!(list[0].title, "Install Firefox");
        let back = Conversation::load_from(&d, &c.id).unwrap();
        assert_eq!(back.lines, c.lines);
        assert!(Conversation::load_from(&d, "../etc/passwd").is_err());
        let mode = std::fs::metadata(d.join(format!("{}.json", c.id))).unwrap();
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&mode.permissions()) & 0o777, 0o600);
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn other_providers_get_plain_turns() {
        let messages = vec![
            json!({ "role": "user", "content": "Install VLC" }),
            json!({ "role": "assistant", "content": [
                { "type": "thinking", "thinking": "…", "signature": "x" },
                { "type": "text", "text": "Installing." },
                { "type": "tool_use", "id": "t1", "name": "install_app", "input": { "app": "vlc" } } ] }),
            json!({ "role": "user", "content": [{ "type": "tool_result", "tool_use_id": "t1", "content": "ok" }] }),
            json!({ "role": "assistant", "content": [{ "type": "text", "text": "VLC is installed." }] }),
        ];
        let p = portable(&messages);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0], json!({ "role": "user", "content": "Install VLC" }));
        assert_eq!(p[1]["content"], "Installing.\nVLC is installed.");
    }
}
