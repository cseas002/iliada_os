//! How the broker asks the user.

use std::collections::VecDeque;
use std::io::Write as _;
use std::path::PathBuf;

use async_trait::async_trait;
use iliada_ipc::GrantScope;
use iliada_types::{AppId, Capability};
use tokio::io::{AsyncBufReadExt, BufReader, Lines, Stdin};
use tokio::sync::Mutex;

#[derive(Debug, Clone)]
pub struct PromptRequest {
    pub app_id: AppId,
    pub app_name: String,
    pub capability: Capability,
    /// From the bundle's Permissions.toml (reviewed at install time).
    pub declared_purpose: String,
    /// Supplied by the running app: untrusted, already sanitised, shown labelled.
    pub app_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptAnswer {
    Allow(GrantScope),
    Deny,
}

#[async_trait]
pub trait PromptFrontend: Send + Sync + std::fmt::Debug {
    async fn ask(&self, req: &PromptRequest) -> PromptAnswer;
    /// Host-drawn file chooser. `None` = cancelled.
    async fn pick_file(&self, app_name: &str) -> Option<PathBuf>;
}

/// Strip control characters (terminal escape injection) and cap length.
pub fn sanitize_untrusted(s: &str, max: usize) -> String {
    let cleaned: String = s.chars().filter(|c| !c.is_control()).collect();
    match cleaned.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &cleaned[..i]),
        None => cleaned,
    }
}

/// Denies everything without asking (headless / kiosk).
#[derive(Debug, Default)]
pub struct DenyAll;

#[async_trait]
impl PromptFrontend for DenyAll {
    async fn ask(&self, _req: &PromptRequest) -> PromptAnswer {
        PromptAnswer::Deny
    }
    async fn pick_file(&self, _app: &str) -> Option<PathBuf> {
        None
    }
}

/// Development frontend: prompts on the broker's own terminal. Prompts are
/// serialised so concurrent requests never interleave.
#[derive(Debug)]
pub struct TerminalPrompt {
    lines: Mutex<Lines<BufReader<Stdin>>>,
}

impl Default for TerminalPrompt {
    fn default() -> Self {
        Self { lines: Mutex::new(BufReader::new(tokio::io::stdin()).lines()) }
    }
}

impl TerminalPrompt {
    async fn read_answer(&self, question: &str) -> Option<String> {
        let mut lines = self.lines.lock().await;
        eprint!("{question}");
        let _ = std::io::stderr().flush();
        lines.next_line().await.ok().flatten().map(|l| l.trim().to_owned())
    }
}

#[async_trait]
impl PromptFrontend for TerminalPrompt {
    async fn ask(&self, req: &PromptRequest) -> PromptAnswer {
        let mut q = format!(
            "\n┌─ Permission request ─────────────────────────────\n│ \"{}\" ({}) wants to {}.\n│ Declared purpose: {}\n",
            req.app_name, req.app_id, req.capability, req.declared_purpose
        );
        if let Some(r) = &req.app_reason {
            q.push_str(&format!("│ The app says: \"{r}\"\n"));
        }
        q.push_str("└─ Allow [o]nce, while [r]unning, [a]lways, or [d]eny? ");
        match self.read_answer(&q).await.as_deref() {
            Some("o" | "once") => PromptAnswer::Allow(GrantScope::Once),
            Some("r" | "running") => PromptAnswer::Allow(GrantScope::WhileRunning),
            Some("a" | "always") => PromptAnswer::Allow(GrantScope::Always),
            _ => PromptAnswer::Deny,
        }
    }

    async fn pick_file(&self, app_name: &str) -> Option<PathBuf> {
        let q = format!("  File to share with \"{app_name}\" (empty to cancel): ");
        self.read_answer(&q).await.filter(|s| !s.is_empty()).map(PathBuf::from)
    }
}

/// Test frontend with pre-recorded answers. Records every prompt it sees.
#[derive(Debug, Default)]
pub struct Scripted {
    answers: Mutex<VecDeque<PromptAnswer>>,
    files: Mutex<VecDeque<PathBuf>>,
    pub seen: Mutex<Vec<PromptRequest>>,
}

impl Scripted {
    pub fn new(answers: impl IntoIterator<Item = PromptAnswer>, files: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            answers: Mutex::new(answers.into_iter().collect()),
            files: Mutex::new(files.into_iter().collect()),
            seen: Mutex::default(),
        }
    }
}

#[async_trait]
impl PromptFrontend for Scripted {
    async fn ask(&self, req: &PromptRequest) -> PromptAnswer {
        self.seen.lock().await.push(req.clone());
        self.answers.lock().await.pop_front().unwrap_or(PromptAnswer::Deny)
    }
    async fn pick_file(&self, _app: &str) -> Option<PathBuf> {
        self.files.lock().await.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_escapes_and_truncates() {
        assert_eq!(sanitize_untrusted("hi\x1b[2Jthere", 100), "hi[2Jthere");
        assert_eq!(sanitize_untrusted("abcdef", 3), "abc…");
    }
}
