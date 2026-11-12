//! The Iliada system service.
//!
//! Everything that changes the system (command-line tools, services,
//! updates, backups) goes through one root service, `iliada-systemd`, so the
//! `iliada` command and the Settings app never need `sudo`: the service
//! checks that whoever is asking is an administrator (a member of `wheel`)
//! from the socket's credentials, and does the work itself.
//!
//! The system's configuration is `/etc/iliada`, a git repository: every
//! change is a commit, and backups are that history, pushed to GitHub.

pub mod apply;
pub mod backup;
pub mod client;
pub mod github;
pub mod init;
pub mod protocol;
pub mod settings;
pub mod software;
pub mod update;
pub mod users;

use std::path::PathBuf;

pub const SOCKET: &str = "/run/iliada-system/socket";

/// `/etc/iliada` (`ILIADA_CONFIG_DIR` overrides it).
pub fn config_dir() -> PathBuf {
    std::env::var_os("ILIADA_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| "/etc/iliada".into())
}

/// One recorded change to this computer's configuration.
#[derive(Debug, Clone, PartialEq)]
pub struct Change {
    /// Seconds since the Unix epoch.
    pub time: u64,
    /// What changed ("Enable ssh", "Add tools: python3").
    pub what: String,
}

/// The newest `count` changes to /etc/iliada (every change made through
/// Settings, the Store, the `iliada` command or the assistant is recorded).
pub fn changes(count: usize) -> anyhow::Result<Vec<Change>> {
    use anyhow::Context;
    let dir = config_dir();
    let out = std::process::Command::new("git")
        .arg("-c")
        .arg(format!("safe.directory={}", dir.display()))
        .arg("-C")
        .arg(&dir)
        .args(["log", "-n", &count.to_string(), "--format=%at %s"])
        .output()
        .context("git is needed to read the history")?;
    anyhow::ensure!(out.status.success(), "{} has no history yet", dir.display());
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.split_once(' '))
        .map(|(t, what)| Change { time: t.parse().unwrap_or(0), what: what.to_owned() })
        .collect())
}
