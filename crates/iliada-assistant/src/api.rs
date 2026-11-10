//! The Claude Messages API over HTTPS (there is no Rust SDK; this is the
//! documented wire format). Each assistant turn is kept exactly as returned,
//! thinking blocks included, and earlier turns are never edited.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

const URL: &str = "https://api.anthropic.com/v1/messages";
const VERSION: &str = "2026-10-21";
const MAX_TOKENS: u32 = 16000;

/// Where the user's API key is kept: `~/.config/iliada/anthropic-key`
/// (readable only by them; never backed up). `ANTHROPIC_API_KEY` wins.
pub fn key_path() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let cfg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
    Some(cfg.join("iliada/anthropic-key"))
}

pub fn load_key() -> Option<String> {
    if let Ok(k) = std::env::var("ANTHROPIC_API_KEY")
        && !k.trim().is_empty()
    {
        return Some(k.trim().to_owned());
    }
    let k = std::fs::read_to_string(key_path()?).ok()?;
    let k = k.trim();
    (!k.is_empty()).then(|| k.to_owned())
}

pub fn save_key(key: &str) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let p = key_path().context("HOME is not set")?;
    std::fs::create_dir_all(p.parent().expect("has a parent"))?;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&p)?;
    f.write_all(key.trim().as_bytes())?;
    Ok(())
}

#[derive(Clone)]
pub struct Claude {
    http: reqwest::Client,
    key: String,
    url: String,
    pub model: String,
}

/// Never prints the key.
impl std::fmt::Debug for Claude {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Claude").field("model", &self.model).finish_non_exhaustive()
    }
}

impl Claude {
    pub fn new(key: String, model: String) -> Self {
        Self::with_url(key, model, URL.into())
    }

    /// Against another endpoint (tests).
    pub fn with_url(key: String, model: String, url: String) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("iliada-assistant/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(600))
            .build()
            .expect("http client");
        Claude { http, key, url, model }
    }

    /// One Messages API call. Returns the response object.
    pub async fn create(&self, system: &str, tools: &Value, messages: &[Value]) -> Result<Value> {
        let body = json!({
            "model": self.model,
            "max_tokens": MAX_TOKENS,
            "system": system,
            "tools": tools,
            "messages": messages,
        });
        let r = self
            .http
            .post(&self.url)
            .header("x-api-key", &self.key)
            .header("anthropic-version", VERSION)
            .json(&body)
            .send()
            .await
            .context("the assistant can't reach Claude; check the internet connection")?;
        let status = r.status();
        let v: Value = r.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            let msg = v["error"]["message"].as_str().unwrap_or("unknown error").to_owned();
            match status.as_u16() {
                401 => bail!("the API key was not accepted (Settings → Assistant)"),
                429 => bail!("Claude is busy or the account's limit was reached; try again shortly ({msg})"),
                _ => bail!("Claude returned an error ({status}): {msg}"),
            }
        }
        Ok(v)
    }
}
