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
pub mod update;
pub mod users;

use std::path::PathBuf;

pub const SOCKET: &str = "/run/iliada-system/socket";

/// `/etc/iliada` (`ILIADA_CONFIG_DIR` overrides it).
pub fn config_dir() -> PathBuf {
    std::env::var_os("ILIADA_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| "/etc/iliada".into())
}
