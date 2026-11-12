//! The model, over HTTPS: Claude's Messages API (Anthropic), or OpenRouter's
//! chat completions for other models ("deepseek/deepseek-v4-flash"). There
//! is no Rust SDK for either; these are the documented wire formats.
//!
//! The conversation is kept in the Messages API's shape. For OpenRouter each
//! request is translated to chat completions and the answer back, so the
//! assistant loop is the same for both. Each assistant turn is kept exactly
//! as returned (thinking / reasoning included) and earlier turns are never
//! edited.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
pub use iliada_system::protocol::Provider;
use serde_json::{Value, json};

const ANTHROPIC_URL: &str = "https://api.anthropic.com/v1/messages";
const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1/chat/completions";
const VERSION: &str = "2026-10-21";
const MAX_TOKENS: u32 = 16000;

fn config_dir() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let cfg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
    Some(cfg.join("iliada"))
}

/// Where the user's key for `p` is kept: `~/.config/iliada/anthropic-key` or
/// `openrouter-key` (readable only by them; never backed up).
pub fn key_path(p: Provider) -> Option<PathBuf> {
    Some(config_dir()?.join(match p {
        Provider::Anthropic => "anthropic-key",
        Provider::OpenRouter => "openrouter-key",
    }))
}

fn env_var(p: Provider) -> &'static str {
    match p {
        Provider::Anthropic => "ANTHROPIC_API_KEY",
        Provider::OpenRouter => "OPENROUTER_API_KEY",
    }
}

/// The key for `p`; its environment variable wins over the file.
pub fn load_key(p: Provider) -> Option<String> {
    if let Ok(k) = std::env::var(env_var(p))
        && !k.trim().is_empty()
    {
        return Some(k.trim().to_owned());
    }
    let k = std::fs::read_to_string(key_path(p)?).ok()?;
    let k = k.trim();
    (!k.is_empty()).then(|| k.to_owned())
}

/// Which provider a key belongs to: OpenRouter keys start `sk-or-`,
/// Anthropic's `sk-ant-`.
pub fn provider_of_key(key: &str) -> Option<Provider> {
    let k = key.trim();
    if k.starts_with("sk-or-") {
        Some(Provider::OpenRouter)
    } else if k.starts_with("sk-ant-") {
        Some(Provider::Anthropic)
    } else {
        None
    }
}

/// Saves a key where its provider's keys go; returns the provider.
/// A key shown without giving it away: "sk-or-v1…ab12".
pub fn masked(key: &str) -> String {
    let k = key.trim();
    let n = k.chars().count();
    if n <= 14 {
        return "…".into();
    }
    let head: String = k.chars().take(8).collect();
    let tail: String = k.chars().skip(n - 4).collect();
    format!("{head}…{tail}")
}

/// Removes the saved key for `p`.
pub fn remove_key(p: Provider) -> Result<()> {
    let path = key_path(p).context("HOME is not set")?;
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}

/// Asks the provider whether `key` works, without asking the model anything
/// (free). For OpenRouter, also says how much credit is left.
pub async fn check_key(p: Provider, key: &str) -> Result<String> {
    let http = reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).build()?;
    let req = match p {
        Provider::OpenRouter => http.get("https://openrouter.ai/api/v1/key").bearer_auth(key.trim()),
        Provider::Anthropic => http
            .get("https://api.anthropic.com/v1/models")
            .header("x-api-key", key.trim())
            .header("anthropic-version", "2023-06-01"),
    };
    let r = req.send().await.with_context(|| format!("could not reach {}", p.name()))?;
    match r.status().as_u16() {
        200..=299 => {}
        401 | 403 => bail!("{} did not accept this key", p.name()),
        s => bail!("{} answered with an error ({s})", p.name()),
    }
    if p != Provider::OpenRouter {
        return Ok("The key works.".into());
    }
    let v: Value = r.json().await.unwrap_or_default();
    let d = &v["data"];
    Ok(match (d["limit"].as_f64(), d["limit_remaining"].as_f64(), d["usage"].as_f64()) {
        (Some(limit), Some(left), _) => format!("The key works: ${left:.2} of ${limit:.2} left."),
        (None, _, Some(used)) => format!("The key works (${used:.2} used so far)."),
        _ => "The key works.".into(),
    })
}

/// The first API key in `text`: a pasted key, or a file such as a `.env`
/// with `OPENROUTER_API_KEY="sk-or-…"` in it.
pub fn find_key(text: &str) -> Option<&str> {
    text.split(|c: char| c.is_whitespace() || matches!(c, '=' | '"' | '\'' | ':' | ','))
        .find(|t| t.starts_with("sk-ant-") || t.starts_with("sk-or-"))
}

pub fn save_key(key: &str) -> Result<Provider> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let p = provider_of_key(key)
        .context("that doesn't look like an API key (Anthropic keys start sk-ant-, OpenRouter keys sk-or-)")?;
    let path = key_path(p).context("HOME is not set")?;
    std::fs::create_dir_all(path.parent().expect("has a parent"))?;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path)?;
    f.write_all(key.trim().as_bytes())?;
    Ok(p)
}

#[derive(Clone)]
pub struct Api {
    http: reqwest::Client,
    key: String,
    url: String,
    provider: Provider,
    pub model: String,
}

/// Never prints the key.
impl std::fmt::Debug for Api {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Api").field("provider", &self.provider).field("model", &self.model).finish_non_exhaustive()
    }
}

impl Api {
    /// For `model`, with the provider its name implies.
    pub fn new(key: String, model: String) -> Self {
        let provider = Provider::of(&model);
        let url = match provider {
            Provider::Anthropic => ANTHROPIC_URL,
            Provider::OpenRouter => OPENROUTER_URL,
        };
        Self::with_url(key, model, url.into())
    }

    /// Against another endpoint (tests).
    pub fn with_url(key: String, model: String, url: String) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(concat!("iliada-assistant/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(600))
            .build()
            .expect("http client");
        Api { http, key, url, provider: Provider::of(&model), model }
    }

    pub fn provider(&self) -> Provider {
        self.provider
    }

    /// One call. Returns a Messages API response (`content`, `stop_reason`,
    /// `usage`), translated if the model is on OpenRouter; `usage.cost` is
    /// what the call cost in US dollars when the provider says.
    pub async fn create(&self, system: &str, tools: &Value, messages: &[Value]) -> Result<Value> {
        self.create_streaming(system, tools, messages, &|_| {}).await
    }

    /// Like [`Api::create`], with the answer's text handed to `on_text` piece
    /// by piece as the model writes it. The result is the same.
    pub async fn create_streaming(
        &self,
        system: &str,
        tools: &Value,
        messages: &[Value],
        on_text: &(dyn Fn(&str) + Send + Sync),
    ) -> Result<Value> {
        let (body, req) = match self.provider {
            Provider::Anthropic => {
                let body = json!({
                    "model": self.model, "max_tokens": MAX_TOKENS, "stream": true,
                    "system": system, "tools": tools, "messages": messages,
                });
                (body, self.http.post(&self.url).header("x-api-key", &self.key).header("anthropic-version", VERSION))
            }
            Provider::OpenRouter => {
                let mut body = to_chat(system, tools, messages);
                body["model"] = json!(self.model);
                body["max_tokens"] = json!(MAX_TOKENS);
                body["usage"] = json!({ "include": true });
                body["stream"] = json!(true);
                let req = self
                    .http
                    .post(&self.url)
                    .bearer_auth(&self.key)
                    .header("X-Title", "Iliada")
                    .header("HTTP-Referer", "https://github.com/cseas002/iliada_os");
                (body, req)
            }
        };
        let who = self.provider.name();
        let r = req
            .json(&body)
            .send()
            .await
            .with_context(|| format!("the assistant can't reach {who}; check the internet connection"))?;
        let status = r.status();
        let streamed = r
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|t| t.to_str().ok())
            .is_some_and(|t| t.starts_with("text/event-stream"));
        if status.is_success() && streamed {
            return self.read_stream(r, on_text).await;
        }
        let v: Value = r.json().await.unwrap_or(Value::Null);
        if !status.is_success() {
            let msg = v["error"]["message"].as_str().unwrap_or("unknown error").to_owned();
            match status.as_u16() {
                401 => bail!("the {who} API key was not accepted (Settings → Assistant)"),
                402 => bail!("the {who} account is out of credit ({msg})"),
                429 => bail!("{who} is busy or the account's limit was reached; try again shortly ({msg})"),
                _ => bail!("{who} returned an error ({status}): {msg}"),
            }
        }
        match self.provider {
            Provider::Anthropic => Ok(v),
            Provider::OpenRouter => {
                // OpenRouter can report a provider's failure inside a 200.
                if let Some(msg) = v["error"]["message"].as_str() {
                    bail!("{who} returned an error: {msg}");
                }
                from_chat(&v)
            }
        }
    }

    /// A streamed answer (server-sent events), put back together.
    async fn read_stream(&self, mut r: reqwest::Response, on_text: &(dyn Fn(&str) + Send + Sync)) -> Result<Value> {
        let who = self.provider.name();
        let mut acc = match self.provider {
            Provider::Anthropic => Stream::Messages(MessagesStream::default()),
            Provider::OpenRouter => Stream::Chat(ChatStream::default()),
        };
        let mut buf = Vec::new();
        loop {
            let chunk = r.chunk().await.with_context(|| format!("the connection to {who} was cut off"))?;
            let done = chunk.is_none();
            buf.extend_from_slice(chunk.as_deref().unwrap_or(b"\n"));
            while let Some(i) = buf.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buf.drain(..=i).collect();
                let line = String::from_utf8_lossy(&line);
                let Some(data) = line.trim_end().strip_prefix("data:") else { continue };
                let data = data.trim_start();
                if data == "[DONE]" {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
                if let Some(msg) = v["error"]["message"].as_str() {
                    bail!("{who} returned an error: {msg}");
                }
                match &mut acc {
                    Stream::Messages(m) => m.event(&v, on_text),
                    Stream::Chat(c) => c.chunk(&v, on_text),
                }
            }
            if done {
                break;
            }
        }
        match acc {
            Stream::Messages(m) => m.finish(),
            Stream::Chat(c) => from_chat(&c.finish()),
        }
    }
}

enum Stream {
    Messages(MessagesStream),
    Chat(ChatStream),
}

/// A Messages API answer as it streams in.
#[derive(Default)]
struct MessagesStream {
    content: Vec<Value>,
    /// A tool call's input arrives as pieces of JSON.
    input: Vec<String>,
    stop_reason: Option<String>,
    usage: serde_json::Map<String, Value>,
}

impl MessagesStream {
    fn event(&mut self, v: &Value, on_text: &(dyn Fn(&str) + Send + Sync)) {
        let i = v["index"].as_u64().unwrap_or(0) as usize;
        match v["type"].as_str().unwrap_or("") {
            "message_start" => self.add_usage(&v["message"]["usage"]),
            "content_block_start" => {
                if self.content.len() <= i {
                    self.content.resize(i + 1, Value::Null);
                    self.input.resize(i + 1, String::new());
                }
                self.content[i] = v["content_block"].clone();
            }
            "content_block_delta" => {
                let (Some(block), d) = (self.content.get_mut(i), &v["delta"]) else { return };
                let append = |block: &mut Value, field: &str, piece: &str| {
                    let mut s = block[field].as_str().unwrap_or("").to_owned();
                    s.push_str(piece);
                    block[field] = json!(s);
                };
                match (d["type"].as_str().unwrap_or(""), d["text"].as_str()) {
                    ("text_delta", Some(t)) => {
                        append(block, "text", t);
                        on_text(t);
                    }
                    ("input_json_delta", _) => self.input[i].push_str(d["partial_json"].as_str().unwrap_or("")),
                    ("thinking_delta", _) => append(block, "thinking", d["thinking"].as_str().unwrap_or("")),
                    ("signature_delta", _) => block["signature"] = d["signature"].clone(),
                    _ => {}
                }
            }
            "content_block_stop" => {
                if let Some(block) = self.content.get_mut(i)
                    && block["type"] == "tool_use"
                {
                    let raw = self.input[i].trim();
                    if !raw.is_empty() {
                        block["input"] = serde_json::from_str::<Value>(raw)
                            .ok()
                            .filter(Value::is_object)
                            .unwrap_or_else(|| json!({}));
                    }
                }
            }
            "message_delta" => {
                if let Some(s) = v["delta"]["stop_reason"].as_str() {
                    self.stop_reason = Some(s.to_owned());
                }
                self.add_usage(&v["usage"]);
            }
            _ => {}
        }
    }

    fn add_usage(&mut self, u: &Value) {
        for (k, v) in u.as_object().into_iter().flatten() {
            if !v.is_null() {
                self.usage.insert(k.clone(), v.clone());
            }
        }
    }

    fn finish(self) -> Result<Value> {
        let content: Vec<Value> = self.content.into_iter().filter(|b| !b.is_null()).collect();
        let stop = self.stop_reason.context("the answer was cut off before it finished")?;
        Ok(json!({ "content": content, "stop_reason": stop, "usage": self.usage }))
    }
}

/// An OpenRouter chat completion as it streams in.
#[derive(Default)]
struct ChatStream {
    text: String,
    /// (id, name, arguments) by position.
    calls: Vec<(Value, String, String)>,
    reasoning: Vec<Value>,
    finish_reason: Value,
    usage: Value,
}

impl ChatStream {
    fn chunk(&mut self, v: &Value, on_text: &(dyn Fn(&str) + Send + Sync)) {
        if v["usage"].is_object() {
            self.usage = v["usage"].clone();
        }
        let Some(choice) = v["choices"].get(0) else { return };
        if !choice["finish_reason"].is_null() {
            self.finish_reason = choice["finish_reason"].clone();
        }
        let d = &choice["delta"];
        if let Some(t) = d["content"].as_str().filter(|t| !t.is_empty()) {
            self.text.push_str(t);
            on_text(t);
        }
        for c in d["tool_calls"].as_array().into_iter().flatten() {
            let i = c["index"].as_u64().map(|i| i as usize).unwrap_or(self.calls.len());
            if self.calls.len() <= i {
                self.calls.resize(i + 1, (Value::Null, String::new(), String::new()));
            }
            let call = &mut self.calls[i];
            if !c["id"].is_null() {
                call.0 = c["id"].clone();
            }
            call.1.push_str(c["function"]["name"].as_str().unwrap_or(""));
            call.2.push_str(c["function"]["arguments"].as_str().unwrap_or(""));
        }
        for r in d["reasoning_details"].as_array().into_iter().flatten() {
            self.add_reasoning(r);
        }
    }

    /// Reasoning arrives in pieces with the same `index`: their text joins up.
    fn add_reasoning(&mut self, piece: &Value) {
        let existing = self
            .reasoning
            .iter_mut()
            .find(|r| !piece["index"].is_null() && r["index"] == piece["index"] && r["type"] == piece["type"]);
        let Some(r) = existing else {
            self.reasoning.push(piece.clone());
            return;
        };
        for (k, v) in piece.as_object().into_iter().flatten() {
            match (k.as_str(), v.as_str(), r[k.as_str()].as_str()) {
                ("text" | "summary" | "data", Some(more), Some(so_far)) => {
                    r[k.as_str()] = json!(format!("{so_far}{more}"))
                }
                _ if !v.is_null() => r[k.as_str()] = v.clone(),
                _ => {}
            }
        }
    }

    /// The whole completion, as a non-streamed answer would have been.
    fn finish(self) -> Value {
        let calls: Vec<Value> = self
            .calls
            .into_iter()
            .map(|(id, name, args)| json!({ "id": id, "type": "function", "function": { "name": name, "arguments": args } }))
            .collect();
        let mut message = json!({ "content": self.text });
        if !calls.is_empty() {
            message["tool_calls"] = json!(calls);
        }
        if !self.reasoning.is_empty() {
            message["reasoning_details"] = json!(self.reasoning);
        }
        json!({ "choices": [{ "finish_reason": self.finish_reason, "message": message }], "usage": self.usage })
    }
}

/// A Messages API request (system, tools, messages) as chat completions.
fn to_chat(system: &str, tools: &Value, messages: &[Value]) -> Value {
    let tools: Vec<Value> = tools
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| {
            json!({ "type": "function", "function": {
                "name": t["name"], "description": t["description"], "parameters": t["input_schema"] } })
        })
        .collect();
    let mut out = vec![json!({ "role": "system", "content": system })];
    for m in messages {
        let content = &m["content"];
        match (m["role"].as_str(), content.as_str()) {
            (Some(role), Some(text)) => out.push(json!({ "role": role, "content": text })),
            (Some("user"), None) => {
                let mut texts = Vec::new();
                for b in content.as_array().into_iter().flatten() {
                    match b["type"].as_str() {
                        Some("tool_result") => {
                            let text =
                                b["content"].as_str().map(str::to_owned).unwrap_or_else(|| b["content"].to_string());
                            let text = if b["is_error"] == true { format!("Error: {text}") } else { text };
                            out.push(json!({ "role": "tool", "tool_call_id": b["tool_use_id"], "content": text }));
                        }
                        Some("text") => texts.push(b["text"].as_str().unwrap_or("").to_owned()),
                        _ => {}
                    }
                }
                if !texts.is_empty() {
                    out.push(json!({ "role": "user", "content": texts.join("\n") }));
                }
            }
            (Some("assistant"), None) => {
                let mut msg = json!({ "role": "assistant" });
                let mut text = String::new();
                let mut calls = Vec::new();
                for b in content.as_array().into_iter().flatten() {
                    match b["type"].as_str() {
                        Some("text") => text.push_str(b["text"].as_str().unwrap_or("")),
                        Some("tool_use") => calls.push(json!({ "id": b["id"], "type": "function", "function": {
                            "name": b["name"], "arguments": b["input"].to_string() } })),
                        // The model's reasoning, handed back as it came.
                        Some("reasoning") => msg["reasoning_details"] = b["details"].clone(),
                        _ => {}
                    }
                }
                msg["content"] = if text.is_empty() { Value::Null } else { json!(text) };
                if !calls.is_empty() {
                    msg["tool_calls"] = json!(calls);
                }
                out.push(msg);
            }
            _ => {}
        }
    }
    json!({ "messages": out, "tools": tools })
}

/// A chat completion as a Messages API response.
fn from_chat(v: &Value) -> Result<Value> {
    let choice = v["choices"].get(0).context("the model sent no answer")?;
    let m = &choice["message"];
    let mut content = Vec::new();
    if let Some(d) = m.get("reasoning_details").filter(|d| d.as_array().is_some_and(|a| !a.is_empty())) {
        content.push(json!({ "type": "reasoning", "details": d }));
    }
    if let Some(t) = m["content"].as_str().filter(|t| !t.trim().is_empty()) {
        content.push(json!({ "type": "text", "text": t }));
    }
    let calls = m["tool_calls"].as_array().cloned().unwrap_or_default();
    for c in &calls {
        let args = c["function"]["arguments"].as_str().unwrap_or("{}");
        // Arguments that aren't a JSON object reach the tool as nothing, so it
        // fails and says what is missing.
        let input = serde_json::from_str::<Value>(if args.trim().is_empty() { "{}" } else { args })
            .ok()
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        content.push(json!({ "type": "tool_use", "id": c["id"], "name": c["function"]["name"], "input": input }));
    }
    let stop = if !calls.is_empty() {
        "tool_use"
    } else {
        match choice["finish_reason"].as_str() {
            Some("length") => "max_tokens",
            Some("content_filter") => "refusal",
            _ => "end_turn",
        }
    };
    let u = &v["usage"];
    Ok(json!({
        "content": content,
        "stop_reason": stop,
        "usage": { "input_tokens": u["prompt_tokens"], "output_tokens": u["completion_tokens"], "cost": u["cost"] },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_are_shown_masked() {
        assert_eq!(masked("sk-or-v1-0123456789abcdef"), "sk-or-v1…cdef");
        assert_eq!(masked("short"), "…");
    }

    #[test]
    fn keys_are_found_in_pasted_text_and_env_files() {
        assert_eq!(find_key("sk-or-v1-abc\n"), Some("sk-or-v1-abc"));
        assert_eq!(find_key("# mine\nOPENROUTER_API_KEY=\"sk-or-v1-abc\"\n"), Some("sk-or-v1-abc"));
        assert_eq!(find_key("ANTHROPIC_API_KEY='sk-ant-xyz'"), Some("sk-ant-xyz"));
        assert_eq!(find_key("hello"), None);
    }

    #[test]
    fn providers_follow_model_and_key_names() {
        assert_eq!(Provider::of("claude-opus-5-5"), Provider::Anthropic);
        assert_eq!(Provider::of("deepseek/deepseek-v4-flash"), Provider::OpenRouter);
        assert_eq!(provider_of_key("sk-or-v1-abc"), Some(Provider::OpenRouter));
        assert_eq!(provider_of_key(" sk-ant-api03-x\n"), Some(Provider::Anthropic));
        assert_eq!(provider_of_key("hello"), None);
    }

    #[test]
    fn a_conversation_becomes_chat_messages() {
        let tools = json!([{ "name": "get_status", "description": "d", "input_schema": { "type": "object" } }]);
        let msgs = [
            json!({ "role": "user", "content": "how are things?" }),
            json!({ "role": "assistant", "content": [
                { "type": "reasoning", "details": [{ "type": "reasoning.text", "text": "check" }] },
                { "type": "text", "text": "Let me look." },
                { "type": "tool_use", "id": "c1", "name": "get_status", "input": {} } ] }),
            json!({ "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "c1", "content": "{\"version\":\"0.1\"}" },
                { "type": "tool_result", "tool_use_id": "c2", "content": "nope", "is_error": true } ] }),
        ];
        let c = to_chat("be nice", &tools, &msgs);
        let m = c["messages"].as_array().unwrap();
        assert_eq!(m[0], json!({ "role": "system", "content": "be nice" }));
        assert_eq!(m[1], json!({ "role": "user", "content": "how are things?" }));
        assert_eq!(m[2]["content"], "Let me look.");
        assert_eq!(m[2]["tool_calls"][0]["function"], json!({ "name": "get_status", "arguments": "{}" }));
        assert_eq!(m[2]["reasoning_details"][0]["text"], "check", "reasoning goes back as it came");
        assert_eq!(m[3], json!({ "role": "tool", "tool_call_id": "c1", "content": "{\"version\":\"0.1\"}" }));
        assert_eq!(m[4]["content"], "Error: nope");
        assert_eq!(c["tools"][0]["function"]["parameters"], json!({ "type": "object" }));
    }

    #[test]
    fn a_chat_answer_becomes_content_blocks() {
        let v = json!({ "choices": [{ "finish_reason": "tool_calls", "message": {
            "content": "", "reasoning_details": [{ "type": "reasoning.text", "text": "hm" }],
            "tool_calls": [
                { "id": "c1", "type": "function", "function": { "name": "install_app", "arguments": "{\"app\":\"Firefox\"}" } },
                { "id": "c2", "type": "function", "function": { "name": "get_status", "arguments": "" } },
                { "id": "c3", "type": "function", "function": { "name": "open_app", "arguments": "not json" } } ] } }],
            "usage": { "prompt_tokens": 10, "completion_tokens": 5, "cost": 0.00002 } });
        let r = from_chat(&v).unwrap();
        assert_eq!(r["stop_reason"], "tool_use");
        let c = r["content"].as_array().unwrap();
        assert_eq!(c[0]["type"], "reasoning");
        assert_eq!(
            c[1],
            json!({ "type": "tool_use", "id": "c1", "name": "install_app", "input": { "app": "Firefox" } })
        );
        assert_eq!(c[2]["input"], json!({}));
        assert_eq!(c[3]["input"], json!({}), "bad arguments reach the tool as nothing");
        assert_eq!(r["usage"]["cost"], 0.00002);

        let done = json!({ "choices": [{ "finish_reason": "stop", "message": { "content": "Done." } }], "usage": {} });
        let r = from_chat(&done).unwrap();
        assert_eq!((r["stop_reason"].as_str(), r["content"][0]["text"].as_str()), (Some("end_turn"), Some("Done.")));
    }
}
