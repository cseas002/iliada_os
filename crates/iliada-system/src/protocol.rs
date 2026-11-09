//! The wire protocol between the system service and its clients (the
//! `iliada` command, Settings, the Setup Assistant).
//!
//! One connection carries one request: the client writes a [`Request`] as a
//! JSON line; the service answers with [`Event`] lines and ends with exactly
//! one [`Event::Done`] or [`Event::Failed`]. Who is asking comes from the
//! socket's peer credentials, never from the request.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    /// Everything a settings screen shows. Anyone may ask.
    Status,
    /// Install command-line tools (nixpkgs attribute names) and apply.
    AddTools { packages: Vec<String> },
    /// Uninstall command-line tools and apply.
    RemoveTools { packages: Vec<String> },
    /// Turn a system service on or off and apply.
    SetService { name: String, on: bool },
    /// Apply /etc/iliada as it is.
    Apply,
    /// Install missing Store apps from the app list and update the rest now.
    UpdateApps,
    /// Look for a new Iliada release (the answer is also in [`Status`]).
    CheckSystemUpdate,
    /// Download and apply the newest Iliada release.
    UpdateSystem,
    /// Sign in to GitHub (device flow): emits [`Event::SignInCode`], then
    /// finishes when the user has approved the code in a browser.
    BackupSignIn,
    /// Back up to the private repository `iliada_<name>` (created if needed).
    BackupEnable { name: String },
    /// Back up now.
    BackupNow,
    /// The signed-in user's Iliada backups (repositories named `iliada_*`).
    BackupList,
    /// Restore apps and settings from a backup (`owner/iliada_name`), then
    /// keep backing up to it.
    Restore { repo: String },
    /// Stop backing up and forget the GitHub sign-in.
    BackupSignOut,
}

impl Request {
    /// Requests that only read; everything else needs an administrator.
    pub fn read_only(&self) -> bool {
        matches!(self, Request::Status)
    }

    /// What the service is doing while it handles this request (shown to
    /// other clients as "busy").
    pub fn activity(&self) -> &'static str {
        match self {
            Request::Status | Request::BackupList | Request::CheckSystemUpdate => "",
            Request::AddTools { .. } => "Installing tools",
            Request::RemoveTools { .. } => "Removing tools",
            Request::SetService { .. } | Request::Apply => "Applying settings",
            Request::UpdateApps => "Updating apps",
            Request::UpdateSystem => "Updating Iliada",
            Request::BackupSignIn => "Signing in to GitHub",
            Request::BackupEnable { .. } | Request::BackupNow => "Backing up",
            Request::Restore { .. } => "Restoring a backup",
            Request::BackupSignOut => "Signing out",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// A line of progress (build output, git output…).
    Log {
        line: String,
    },
    /// Show this code and open the URL; the user approves it there.
    SignInCode {
        code: String,
        url: String,
    },
    Done {
        result: serde_json::Value,
    },
    Failed {
        error: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub version: String,
    pub hostname: String,
    /// The asking user may change settings.
    pub admin: bool,
    /// What the service is doing right now, if anything.
    pub busy: Option<String>,
    pub tools: Vec<String>,
    pub services: Vec<ServiceInfo>,
    pub backup: BackupInfo,
    pub system_update: UpdateInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceInfo {
    pub name: String,
    pub description: String,
    pub on: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackupInfo {
    /// This system can back up to GitHub (a GitHub app is configured).
    pub available: bool,
    /// The signed-in GitHub account.
    pub account: Option<String>,
    /// `owner/iliada_name`.
    pub repo: Option<String>,
    /// Unix time of the last successful backup.
    pub last_backup: Option<u64>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateInfo {
    /// The running Iliada release.
    pub current: String,
    /// Unix time of the last check, and the newest release seen then.
    pub last_check: Option<u64>,
    pub latest: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Backup {
    /// `owner/iliada_name`.
    pub repo: String,
    /// The part after `iliada_`.
    pub name: String,
    /// When it was last pushed (as GitHub reports it, RFC 3339).
    pub updated: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_format_is_tagged_json() {
        let r = Request::SetService { name: "ssh".into(), on: true };
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(s, r#"{"op":"set_service","name":"ssh","on":true}"#);
        assert_eq!(serde_json::from_str::<Request>(&s).unwrap(), r);
        assert!(Request::Status.read_only());
        assert!(!Request::Apply.read_only());
        let e = Event::Log { line: "x".into() };
        assert_eq!(serde_json::to_string(&e).unwrap(), r#"{"event":"log","line":"x"}"#);
    }
}
