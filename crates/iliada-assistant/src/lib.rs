//! The Iliada assistant.
//!
//! Claude, with Iliada's own operations as tools (`tools.rs`): apps, command
//! line tools, services, updates, backups, permissions. Never a shell, never
//! arbitrary files. In **Ask** mode every action is shown to the user first;
//! in **Auto** mode actions run on their own, which needs a model trusted for
//! it (`AUTO_MODELS`). System changes go through the system service either
//! way, so the assistant can never do more than the user it works for.
//!
//! Configuration: `[assistant]` in settings.toml (Settings → Assistant, or
//! `iliada assistant`); the API key is the user's own
//! (`~/.config/iliada/anthropic-key`).

pub mod api;
pub mod tools;

use anyhow::{Context, Result};
use async_trait::async_trait;
use iliada_store::{Store, StoreConfig};
use iliada_system::protocol::{AssistantMode, AssistantSettings};
use serde_json::{Value, json};

/// Tool calls per request before the assistant stops and reports.
const MAX_STEPS: usize = 25;

const SYSTEM: &str = "You are the assistant built into Iliada, a Mac-like operating system. \
You help the person using this computer by doing things for them with the tools you have: \
installing, opening and removing apps from the Iliada Store, installing command-line software \
(Python, Node.js, …), turning background services (SSH server, printing, nginx, databases, …) on and off, updating apps and Iliada, backing up, and \
changing app permissions. Those tools are all you can do; you cannot run commands, read or \
change files, or browse the web.

How to work:
- Before acting, look things up (get_status, search_store, list_installed_apps, \
list_permissions) instead of guessing names, ids or states. Never invent an app, a package or \
a result: if a tool did not confirm something, do not say it happened.
- Do what was asked, and only that. If a request is ambiguous or would remove something the \
person may want, ask first in your reply instead of acting.
- Changing tools or services rebuilds the system and takes minutes; say so.
- Some actions may be declined by the person; respect that and do not retry them.
- When done, say plainly what you did and what (if anything) failed, in a sentence or two. \
Iliada's users are not technical: no jargon, no commands.";

/// How the assistant shows itself while it works.
#[async_trait]
pub trait Frontend: Send + Sync {
    /// Ask mode: may this action run? (`action` reads like "Install Firefox".)
    async fn approve(&self, action: &str) -> bool;
    /// An action is running / finished.
    fn progress(&self, text: &str);
}

/// A conversation with the assistant.
#[derive(Debug)]
pub struct Assistant {
    claude: api::Claude,
    mode: AssistantMode,
    store: Store,
    tools: Value,
    messages: Vec<Value>,
}

impl Assistant {
    /// From the system's settings and the user's key.
    pub fn new(settings: &AssistantSettings) -> Result<Self> {
        anyhow::ensure!(settings.enabled, "the assistant is off (Settings → Assistant)");
        let key = api::load_key().context("no API key yet: add one in Settings → Assistant")?;
        Ok(Assistant {
            claude: api::Claude::new(key, settings.model.clone()),
            mode: settings.effective_mode(),
            store: Store::new(StoreConfig::from_env()?)?,
            tools: tools::definitions(),
            messages: Vec::new(),
        })
    }

    /// From parts (tests, other frontends).
    pub fn with_parts(claude: api::Claude, mode: AssistantMode, store: Store) -> Self {
        Assistant { claude, mode, store, tools: tools::definitions(), messages: Vec::new() }
    }

    pub fn mode(&self) -> AssistantMode {
        self.mode
    }

    /// Send what the person said; returns the assistant's answer once it has
    /// finished acting.
    pub async fn send(&mut self, text: &str, ui: &dyn Frontend) -> Result<String> {
        self.messages.push(json!({ "role": "user", "content": text }));
        for _ in 0..MAX_STEPS {
            let resp = self.claude.create(SYSTEM, &self.tools, &self.messages).await?;
            let content = resp["content"].clone();
            // Kept exactly as returned (thinking blocks included).
            self.messages.push(json!({ "role": "assistant", "content": content }));
            match resp["stop_reason"].as_str() {
                Some("tool_use") => {}
                Some("refusal") => return Ok("I can't help with that.".into()),
                _ => return Ok(text_of(&content)),
            }
            let mut results = Vec::new();
            for block in content.as_array().into_iter().flatten().filter(|b| b["type"] == "tool_use") {
                let (id, name, input) = (block["id"].clone(), block["name"].as_str().unwrap_or(""), &block["input"]);
                let result = if tools::acts(name) {
                    let action = tools::describe(name, input);
                    if self.mode == AssistantMode::Ask && !ui.approve(&action).await {
                        audit(&self.claude.model, self.mode, name, input, "declined");
                        Err(anyhow::anyhow!("the person declined: {action}"))
                    } else {
                        ui.progress(&format!("{action}…"));
                        let r = tools::run(&self.store, name, input).await;
                        audit(&self.claude.model, self.mode, name, input, if r.is_ok() { "done" } else { "failed" });
                        ui.progress(&match &r {
                            Ok(_) => format!("✓ {action}"),
                            Err(e) => format!("✗ {action}: {e:#}"),
                        });
                        r
                    }
                } else {
                    tools::run(&self.store, name, input).await
                };
                results.push(match result {
                    Ok(out) => json!({ "type": "tool_result", "tool_use_id": id, "content": out }),
                    Err(e) => json!({ "type": "tool_result", "tool_use_id": id, "content": format!("{e:#}"), "is_error": true }),
                });
            }
            self.messages.push(json!({ "role": "user", "content": results }));
        }
        Ok("I stopped because this was taking too many steps. Tell me what to do next.".into())
    }
}

/// Every action the assistant takes (or was refused) is appended to
/// `~/.local/share/iliada/assistant-log.jsonl`.
fn audit(model: &str, mode: AssistantMode, tool: &str, input: &Value, outcome: &str) {
    use std::io::Write;
    let Some(home) = std::env::var_os("HOME") else { return };
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(home).join(".local/share"));
    let path = base.join("iliada/assistant-log.jsonl");
    let _ = std::fs::create_dir_all(base.join("iliada"));
    let time = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let line = json!({ "time": time, "model": model, "mode": mode, "tool": tool, "input": input, "outcome": outcome });
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{line}");
    }
}

fn text_of(content: &Value) -> String {
    content
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A stand-in for the Messages API: answers with `replies` in order and
    /// records each request body.
    async fn fake_api(replies: Vec<Value>) -> (String, Arc<Mutex<Vec<Value>>>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/v1/messages", l.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        tokio::spawn(async move {
            for reply in replies {
                let (mut c, _) = l.accept().await.unwrap();
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                let body = loop {
                    let n = c.read(&mut chunk).await.unwrap();
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(i) = text.find("\r\n\r\n") {
                        let len: usize = text[..i]
                            .lines()
                            .find_map(|l| {
                                l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap())
                            })
                            .unwrap_or(0);
                        if buf.len() >= i + 4 + len {
                            break buf[i + 4..i + 4 + len].to_vec();
                        }
                    }
                };
                s.lock().unwrap().push(serde_json::from_slice::<Value>(&body).unwrap());
                let out = reply.to_string();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{out}",
                    out.len()
                );
                c.write_all(resp.as_bytes()).await.unwrap();
            }
        });
        (url, seen)
    }

    struct Decline(Mutex<Vec<String>>);
    #[async_trait]
    impl Frontend for Decline {
        async fn approve(&self, action: &str) -> bool {
            self.0.lock().unwrap().push(action.to_owned());
            false
        }
        fn progress(&self, _: &str) {}
    }

    #[tokio::test]
    async fn tool_loop_keeps_turns_verbatim_and_asks_before_acting() {
        let first = json!({
            "stop_reason": "tool_use",
            "content": [
                { "type": "thinking", "thinking": "The user wants Firefox.", "signature": "sig-abc" },
                { "type": "text", "text": "I'll install Firefox." },
                { "type": "tool_use", "id": "toolu_1", "name": "install_app", "input": { "app": "Firefox" } }
            ]
        });
        let second =
            json!({ "stop_reason": "end_turn", "content": [ { "type": "text", "text": "Okay, I left it." } ] });
        let (url, seen) = fake_api(vec![first.clone(), second]).await;
        let mut cfg = StoreConfig::from_env().unwrap();
        cfg.catalog = "/nonexistent/catalog.json".into();
        cfg.apps_dir = std::env::temp_dir().join("iliada-assistant-test-apps");
        cfg.app_list = None;
        let store = Store::new(cfg).unwrap();
        let claude = api::Claude::with_url("sk-test".into(), "claude-opus-5-5".into(), url);
        let mut a = Assistant::with_parts(claude, AssistantMode::Ask, store);
        let ui = Decline(Mutex::new(Vec::new()));
        let answer = a.send("install firefox", &ui).await.unwrap();

        assert_eq!(answer, "Okay, I left it.");
        assert_eq!(*ui.0.lock().unwrap(), ["Install Firefox"], "asked before acting");
        let reqs = seen.lock().unwrap();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0]["model"], "claude-opus-5-5");
        assert!(reqs[0].get("thinking").is_none(), "Opus 5.5 thinks without being told");
        let msgs = reqs[1]["messages"].as_array().unwrap();
        assert_eq!(msgs[1]["content"], first["content"], "assistant turn sent back verbatim");
        let result = &msgs[2]["content"][0];
        assert_eq!(result["type"], "tool_result");
        assert_eq!(result["tool_use_id"], "toolu_1");
        assert_eq!(result["is_error"], true);
        assert!(result["content"].as_str().unwrap().contains("declined"));
    }

    #[tokio::test]
    async fn auto_mode_acts_without_asking_and_reports_failures_as_failures() {
        let first = json!({ "stop_reason": "tool_use", "content": [
            { "type": "tool_use", "id": "toolu_9", "name": "install_app", "input": { "app": "Firefox" } } ] });
        let second = json!({ "stop_reason": "end_turn", "content": [ { "type": "text", "text": "It failed." } ] });
        let (url, seen) = fake_api(vec![first, second]).await;
        let mut cfg = StoreConfig::from_env().unwrap();
        cfg.catalog = "/nonexistent/catalog.json".into();
        cfg.apps_dir = std::env::temp_dir().join("iliada-assistant-test-apps");
        cfg.app_list = None;
        let claude = api::Claude::with_url("sk-test".into(), "claude-opus-5-5".into(), url);
        let mut a = Assistant::with_parts(claude, AssistantMode::Auto, Store::new(cfg).unwrap());
        let ui = Decline(Mutex::new(Vec::new()));
        assert_eq!(a.send("install firefox", &ui).await.unwrap(), "It failed.");
        assert!(ui.0.lock().unwrap().is_empty(), "auto mode does not ask");
        let reqs = seen.lock().unwrap();
        let result = &reqs[1]["messages"][2]["content"][0];
        assert_eq!(result["is_error"], true);
        assert!(!result["content"].as_str().unwrap().contains("declined"));
    }

    #[test]
    fn only_text_blocks_are_the_answer() {
        let c = json!([
            { "type": "thinking", "thinking": "…", "signature": "x" },
            { "type": "text", "text": "Firefox is installed." },
            { "type": "tool_use", "id": "t", "name": "get_status", "input": {} }
        ]);
        assert_eq!(text_of(&c), "Firefox is installed.");
    }

    #[test]
    fn auto_mode_needs_a_trusted_model() {
        let s = AssistantSettings { enabled: true, mode: AssistantMode::Auto, model: "claude-haiku-5-5".into() };
        assert_eq!(s.effective_mode(), AssistantMode::Ask);
        let s = AssistantSettings { model: "claude-opus-5-5".into(), ..s };
        assert_eq!(s.effective_mode(), AssistantMode::Auto);
    }
}
