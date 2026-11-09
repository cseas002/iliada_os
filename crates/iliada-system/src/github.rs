//! The little of GitHub's API that backups need: device-flow sign-in (the
//! user approves a code in a browser; no password ever reaches Iliada), and
//! creating and listing the private `iliada_*` repositories.

use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::json;

use crate::protocol::Backup;

const API: &str = "https://api.github.com";
const AGENT: &str = concat!("iliada-system/", env!("CARGO_PKG_VERSION"));
/// Backups are private repositories: creating and pushing them needs `repo`.
const SCOPE: &str = "repo";
pub const PREFIX: &str = "iliada_";

fn http() -> reqwest::Client {
    reqwest::Client::builder().user_agent(AGENT).timeout(Duration::from_secs(30)).build().expect("http client")
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    pub interval: u64,
}

pub async fn device_code(client_id: &str) -> Result<DeviceCode> {
    let r = http()
        .post("https://github.com/login/device/code")
        .header("Accept", "application/json")
        .form(&[("client_id", client_id), ("scope", SCOPE)])
        .send()
        .await
        .context("contacting GitHub")?;
    if !r.status().is_success() {
        bail!("GitHub refused the sign-in ({})", r.status());
    }
    r.json().await.context("reading GitHub's answer")
}

/// Wait until the user approves (or rejects) the code; returns the token.
pub async fn wait_for_token(client_id: &str, code: &DeviceCode) -> Result<String> {
    #[derive(Deserialize)]
    struct Answer {
        access_token: Option<String>,
        error: Option<String>,
        interval: Option<u64>,
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(code.expires_in);
    let mut interval = code.interval.max(5);
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        let a: Answer = http()
            .post("https://github.com/login/oauth/access_token")
            .header("Accept", "application/json")
            .form(&[
                ("client_id", client_id),
                ("device_code", code.device_code.as_str()),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await
            .context("contacting GitHub")?
            .json()
            .await?;
        if let Some(t) = a.access_token {
            return Ok(t);
        }
        match a.error.as_deref() {
            Some("authorization_pending") => {}
            Some("slow_down") => interval = a.interval.unwrap_or(interval + 5),
            Some("access_denied") => bail!("the sign-in was cancelled on GitHub"),
            Some("expired_token") => break,
            Some(other) => bail!("GitHub sign-in failed: {other}"),
            None => bail!("GitHub sign-in failed"),
        }
    }
    bail!("the sign-in code expired; try again")
}

#[derive(Debug)]
pub struct GitHub {
    token: String,
}

impl GitHub {
    pub fn new(token: String) -> Self {
        GitHub { token }
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        http().get(format!("{API}{path}")).bearer_auth(&self.token).header("Accept", "application/vnd.github+json")
    }

    /// The signed-in account's login.
    pub async fn login(&self) -> Result<String> {
        #[derive(Deserialize)]
        struct User {
            login: String,
        }
        let r = self.get("/user").send().await.context("contacting GitHub")?;
        if r.status() == reqwest::StatusCode::UNAUTHORIZED {
            bail!("the GitHub sign-in is no longer valid; sign in again");
        }
        Ok(r.error_for_status()?.json::<User>().await?.login)
    }

    /// The account's Iliada backups (private or not), newest first.
    pub async fn backups(&self) -> Result<Vec<Backup>> {
        #[derive(Deserialize)]
        struct Repo {
            name: String,
            full_name: String,
            pushed_at: Option<String>,
        }
        let mut out = Vec::new();
        for page in 1..=10 {
            let repos: Vec<Repo> = self
                .get(&format!("/user/repos?affiliation=owner&sort=pushed&per_page=100&page={page}"))
                .send()
                .await
                .context("contacting GitHub")?
                .error_for_status()?
                .json()
                .await?;
            let last = repos.len() < 100;
            out.extend(repos.into_iter().filter_map(|r| {
                let name = r.name.strip_prefix(PREFIX)?.to_owned();
                Some(Backup { repo: r.full_name, name, updated: r.pushed_at.unwrap_or_default() })
            }));
            if last {
                break;
            }
        }
        Ok(out)
    }

    /// `owner/iliada_<name>`, created as a private repository if needed.
    pub async fn ensure_repo(&self, owner: &str, name: &str) -> Result<String> {
        let repo = format!("{PREFIX}{name}");
        let r = self.get(&format!("/repos/{owner}/{repo}")).send().await.context("contacting GitHub")?;
        if r.status().is_success() {
            return Ok(format!("{owner}/{repo}"));
        }
        if r.status() != reqwest::StatusCode::NOT_FOUND {
            bail!("GitHub: {}", r.status());
        }
        let r = http()
            .post(format!("{API}/user/repos"))
            .bearer_auth(&self.token)
            .header("Accept", "application/vnd.github+json")
            .json(&json!({
                "name": repo,
                "private": true,
                "description": "Iliada backup: apps and settings (restore it from Iliada's Setup Assistant)",
                "has_issues": false,
                "has_wiki": false,
                "has_projects": false,
            }))
            .send()
            .await
            .context("contacting GitHub")?;
        if !r.status().is_success() {
            let body = r.text().await.unwrap_or_default();
            bail!("GitHub could not create {repo}: {body}");
        }
        Ok(format!("{owner}/{repo}"))
    }
}

/// A backup name: what follows `iliada_` (GitHub repository name rules).
pub fn valid_name(s: &str) -> bool {
    (1..=80).contains(&s.len())
        && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
        && !s.starts_with('.')
}

/// `owner/repo` as GitHub allows them.
pub fn valid_repo(s: &str) -> bool {
    let Some((owner, repo)) = s.split_once('/') else { return false };
    (1..=39).contains(&owner.len())
        && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && repo.strip_prefix(PREFIX).is_some_and(valid_name)
}

#[cfg(test)]
mod tests {
    #[test]
    fn names() {
        assert!(super::valid_name("laptop") && super::valid_name("my-pc_2"));
        assert!(!super::valid_name("") && !super::valid_name("a b") && !super::valid_name("../x"));
        assert!(super::valid_repo("cseas002/iliada_laptop"));
        assert!(!super::valid_repo("cseas002/laptop") && !super::valid_repo("a/b/iliada_x") && !super::valid_repo("x"));
    }
}
