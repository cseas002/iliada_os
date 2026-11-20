//! The account server's API, as a computer uses it: sign in with a code
//! (OAuth 2.0 device authorization), the account's email, and its sealed
//! files.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;

const AGENT: &str = concat!("iliada-account/", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    /// What the person types on the account website (`ABCD-EFGH`).
    pub user_code: String,
    pub verification_uri: String,
    /// The same page with the code filled in.
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    pub interval: u64,
}

impl DeviceCode {
    /// The page to open for the person.
    pub fn page(&self) -> &str {
        self.verification_uri_complete.as_deref().unwrap_or(&self.verification_uri)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AccountInfo {
    pub email: String,
    #[serde(default)]
    pub used: u64,
    #[serde(default)]
    pub quota: u64,
}

/// Another computer of the account asking for the account key.
#[derive(Debug, Clone, Deserialize)]
pub struct KeyRequestInfo {
    pub id: String,
    /// Its name, as it signed in.
    pub computer: String,
    /// Its one-time public key, base64.
    pub public_key: String,
}

#[derive(Debug, Clone)]
pub struct Server {
    base: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl Server {
    pub fn new(base: &str, token: Option<String>) -> Self {
        let http =
            reqwest::Client::builder().user_agent(AGENT).timeout(Duration::from_secs(60)).build().expect("http client");
        Server { base: base.trim_end_matches('/').to_owned(), token, http }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn authed(&self, r: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(t) => r.bearer_auth(t),
            None => r,
        }
    }

    async fn send(&self, r: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        r.send().await.with_context(|| format!("can't reach the Iliada account server ({})", self.base))
    }

    /// The server's own words for a failure, or the status.
    async fn failure(r: reqwest::Response) -> anyhow::Error {
        let status = r.status();
        if status == StatusCode::UNAUTHORIZED {
            return anyhow::anyhow!("this computer was signed out of the Iliada account; sign in again");
        }
        let said = r.json::<serde_json::Value>().await.ok().and_then(|v| v["error"].as_str().map(str::to_owned));
        match said {
            Some(m) => anyhow::anyhow!("{m}"),
            None => anyhow::anyhow!("the Iliada account server answered {status}"),
        }
    }

    /// Asks for a code for this computer (`name` is shown on the website).
    pub async fn device_code(&self, name: &str) -> Result<DeviceCode> {
        let r = self.send(self.http.post(self.url("/v1/device/code")).json(&json!({ "name": name }))).await?;
        if !r.status().is_success() {
            return Err(Self::failure(r).await);
        }
        Ok(r.json().await?)
    }

    /// Waits until the person approves the code; returns this computer's token.
    pub async fn wait_for_token(&self, code: &DeviceCode) -> Result<String> {
        #[derive(Deserialize)]
        struct Token {
            token: String,
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(code.expires_in);
        let interval = Duration::from_secs(code.interval.clamp(1, 30));
        while std::time::Instant::now() < deadline {
            tokio::time::sleep(interval).await;
            let r = self
                .send(self.http.post(self.url("/v1/device/token")).json(&json!({ "device_code": code.device_code })))
                .await?;
            match r.status() {
                s if s.is_success() => return Ok(r.json::<Token>().await?.token),
                StatusCode::PRECONDITION_REQUIRED => {}
                StatusCode::GONE => break,
                _ => return Err(Self::failure(r).await),
            }
        }
        bail!("the code expired before it was approved; try again")
    }

    pub async fn account(&self) -> Result<AccountInfo> {
        let r = self.send(self.authed(self.http.get(self.url("/v1/account")))).await?;
        if !r.status().is_success() {
            return Err(Self::failure(r).await);
        }
        Ok(r.json().await?)
    }

    /// A sealed file the account keeps, if there is one by that name.
    pub async fn get(&self, name: &str) -> Result<Option<Vec<u8>>> {
        let r = self.send(self.authed(self.http.get(self.url(&format!("/v1/blobs/{name}"))))).await?;
        match r.status() {
            s if s.is_success() => Ok(Some(r.bytes().await?.to_vec())),
            StatusCode::NOT_FOUND => Ok(None),
            _ => Err(Self::failure(r).await),
        }
    }

    pub async fn put(&self, name: &str, bytes: Vec<u8>) -> Result<()> {
        let r = self.send(self.authed(self.http.put(self.url(&format!("/v1/blobs/{name}"))).body(bytes))).await?;
        if !r.status().is_success() {
            return Err(Self::failure(r).await);
        }
        Ok(())
    }

    /// Sends a JSON request with this computer's token; the answer as JSON
    /// (`None` when the server knows no such thing).
    async fn json(&self, r: reqwest::RequestBuilder) -> Result<Option<serde_json::Value>> {
        let r = self.send(self.authed(r)).await?;
        match r.status() {
            StatusCode::NO_CONTENT => Ok(Some(serde_json::Value::Null)),
            s if s.is_success() => Ok(Some(r.json().await?)),
            StatusCode::NOT_FOUND => Ok(None),
            _ => Err(Self::failure(r).await),
        }
    }

    /// Asks the account's other computers for the account key, leaving a
    /// one-time public key (base64). Returns the request's id.
    pub async fn ask_for_key(&self, public_key: &str) -> Result<String> {
        let v = self
            .json(self.http.post(self.url("/v1/keys")).json(&json!({ "public_key": public_key })))
            .await?
            .context("this Iliada account server can't hand keys between computers yet")?;
        v["id"].as_str().map(str::to_owned).context("the account server sent no request id")
    }

    /// Where a request stands: `waiting`, `refused`, or `answered` with the
    /// sealed key (base64). `None`: expired or unknown.
    pub async fn key_request(&self, id: &str) -> Result<Option<(String, Option<String>)>> {
        Ok(self
            .json(self.http.get(self.url(&format!("/v1/keys/{id}"))))
            .await?
            .map(|v| (v["state"].as_str().unwrap_or_default().to_owned(), v["sealed"].as_str().map(str::to_owned))))
    }

    /// The account's other computers waiting for the key.
    pub async fn key_requests(&self) -> Result<Vec<KeyRequestInfo>> {
        #[derive(Deserialize)]
        struct List {
            requests: Vec<KeyRequestInfo>,
        }
        match self.json(self.http.get(self.url("/v1/keys"))).await? {
            Some(v) => Ok(serde_json::from_value::<List>(v)?.requests),
            None => Ok(Vec::new()),
        }
    }

    /// Answers a request with the sealed key (base64).
    pub async fn approve_key_request(&self, id: &str, sealed: &str) -> Result<()> {
        self.json(self.http.post(self.url(&format!("/v1/keys/{id}/approve"))).json(&json!({ "sealed": sealed })))
            .await?
            .context("that computer stopped waiting (the request expired); ask it to try again")?;
        Ok(())
    }

    pub async fn refuse_key_request(&self, id: &str) -> Result<()> {
        self.json(self.http.post(self.url(&format!("/v1/keys/{id}/refuse"))).json(&json!({}))).await?;
        Ok(())
    }

    /// Ends this computer's sign-in on the server.
    pub async fn sign_out(&self) -> Result<()> {
        let r = self.send(self.authed(self.http.post(self.url("/v1/signout")))).await?;
        if !r.status().is_success() && r.status() != StatusCode::UNAUTHORIZED {
            return Err(Self::failure(r).await);
        }
        Ok(())
    }
}
