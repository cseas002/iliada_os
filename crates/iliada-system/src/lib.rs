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

/// How long ago `t` (seconds since 1970) was, in words: "just now",
/// "1 hour ago", "3 days ago". One wording for Settings, Homer and the
/// `iliada` command.
pub fn ago(t: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(t);
    ago_from(now.saturating_sub(t))
}

fn ago_from(secs: u64) -> String {
    let count = |n: u64, unit: &str| if n == 1 { format!("1 {unit} ago") } else { format!("{n} {unit}s ago") };
    match secs {
        s if s < 90 => "just now".into(),
        s if s < 90 * 60 => count(s / 60, "minute"),
        s if s < 36 * 3600 => count(s / 3600, "hour"),
        s => count(s / 86400, "day"),
    }
}

#[cfg(test)]
mod ago_tests {
    use super::ago_from;

    #[test]
    fn reads_like_a_person_would_say_it() {
        assert_eq!(ago_from(10), "just now");
        assert_eq!(ago_from(5 * 60), "5 minutes ago");
        assert_eq!(ago_from(90 * 60), "1 hour ago");
        assert_eq!(ago_from(3 * 3600), "3 hours ago");
        assert_eq!(ago_from(36 * 3600), "1 day ago");
        assert_eq!(ago_from(3 * 86400), "3 days ago");
    }
}
