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
                        Err(anyhow::anyhow!("the person declined: {action}"))
                    } else {
                        ui.progress(&format!("{action}…"));
                        let r = tools::run(&self.store, name, input).await;
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
