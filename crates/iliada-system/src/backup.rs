//! Backing up `/etc/iliada` (settings, the app list, the configuration) to
//! the user's private GitHub repository, and restoring from it.
//!
//! The repository is `/etc/iliada`'s own history, pushed. A new computer
//! restoring a backup takes its `settings.toml` and `apps.toml` and keeps
//! its own hardware configuration; its history then continues the backup's,
//! so later pushes are ordinary fast-forwards.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// What is restored from a backup: the parts that are about the person, not
/// the machine.
pub const RESTORED: [&str; 2] = ["settings.toml", "apps.toml"];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    pub account: Option<String>,
    pub repo: Option<String>,
    pub last_backup: Option<u64>,
    pub last_error: Option<String>,
    /// The commit last pushed (skip pushing when nothing changed).
    pub last_pushed: Option<String>,
}

/// Where backup state and the GitHub token live (root only).
#[derive(Debug, Clone)]
pub struct Store {
    pub dir: PathBuf,
}

impl Store {
    pub fn load(&self) -> State {
        std::fs::read(self.dir.join("backup.json"))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, s: &State) -> Result<()> {
        write_private(&self.dir.join("backup.json"), &serde_json::to_vec_pretty(s)?)
    }

    pub fn token(&self) -> Option<String> {
        std::fs::read_to_string(self.dir.join("github-token"))
            .ok()
            .map(|t| t.trim().to_owned())
            .filter(|t| !t.is_empty())
    }

    pub fn set_token(&self, token: Option<&str>) -> Result<()> {
        let p = self.dir.join("github-token");
        match token {
            Some(t) => write_private(&p, t.as_bytes()),
            None => match std::fs::remove_file(&p) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
                _ => Ok(()),
            },
        }
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::create_dir_all(path.parent().expect("has a parent"))?;
    let tmp = path.with_extension("tmp");
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(bytes)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// git, authenticated to GitHub with `token`. The token goes through the
/// environment (readable only by root), never the command line.
fn git(dir: &Path, token: &str, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Iliada", "-c", "user.email=iliada@localhost", "-c", "commit.gpgsign=false"])
        .args(["-c", "credential.helper="])
        .args([
            "-c",
            "credential.helper=!f() { echo username=x-access-token; echo \"password=$ILIADA_GITHUB_TOKEN\"; }; f",
        ])
        .args(args)
        .env("ILIADA_GITHUB_TOKEN", token)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .context("running git")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).replace(token, "***");
        bail!("{}", err.trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
}

/// The HTTPS remote of a GitHub repository (`owner/name`).
pub fn github_url(repo: &str) -> String {
    format!("https://github.com/{repo}.git")
}

/// Commit any changes in `dir` (app installs land in apps.toml without a
/// commit) and push them. Returns the pushed commit.
pub fn push(dir: &Path, remote: &str, token: &str, hostname: &str) -> Result<String> {
    crate::init::commit_all(dir, &format!("Changes on {hostname}"))?;
    let u = remote;
    if let Err(e) = git(dir, token, &["push", "-q", u, "HEAD:refs/heads/main"]) {
        let msg = e.to_string();
        if !(msg.contains("rejected") || msg.contains("fetch first") || msg.contains("non-fast-forward")) {
            return Err(e);
        }
        // Another computer backed up to the same repository: keep its
        // history, but this computer's files win.
        git(dir, token, &["fetch", "-q", u, "main"])?;
        git(
            dir,
            token,
            &[
                "merge",
                "-q",
                "-s",
                "ours",
                "--allow-unrelated-histories",
                "--no-edit",
                "-m",
                "Merge another computer's backup",
                "FETCH_HEAD",
            ],
        )?;
        git(dir, token, &["push", "-q", u, "HEAD:refs/heads/main"])?;
    }
    git(dir, token, &["rev-parse", "HEAD"])
}

/// Take `settings.toml` and `apps.toml` from the backup at `remote`.
/// Returns the previous settings.toml (to put back if applying fails).
pub fn restore(dir: &Path, remote: &str, token: &str, hostname: &str) -> Result<String> {
    let previous = std::fs::read_to_string(dir.join("settings.toml")).unwrap_or_default();
    git(dir, token, &["fetch", "-q", remote, "main"])
        .map_err(|e| anyhow::anyhow!("could not read the backup (is it empty?): {e}"))?;
    for f in RESTORED {
        if git(dir, token, &["cat-file", "-e", &format!("FETCH_HEAD:{f}")]).is_ok() {
            git(dir, token, &["checkout", "FETCH_HEAD", "--", f])?;
        }
    }
    // Continue the backup's history with this computer's files.
    git(dir, token, &["reset", "-q", "--soft", "FETCH_HEAD"])?;
    crate::init::commit_all(dir, &format!("Restored onto {hostname}"))?;
    Ok(previous)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(dir: &Path, args: &[&str]) {
        assert!(Command::new("git").arg("-C").arg(dir).args(args).status().unwrap().success(), "git {args:?}");
    }

    /// Push and restore against a local bare repository standing in for GitHub.
    #[test]
    fn restore_takes_settings_and_apps_and_continues_history() {
        let t = tempfile::tempdir().unwrap();
        let (a, b, remote) = (t.path().join("a"), t.path().join("b"), t.path().join("remote.git"));
        sh(t.path(), &["init", "-q", "--bare", "-b", "main", remote.to_str().unwrap()]);
        for (d, host) in [(&a, "a"), (&b, "b")] {
            std::fs::create_dir(d).unwrap();
            sh(d, &["init", "-q", "-b", "main"]);
            std::fs::write(d.join("settings.toml"), format!("# {host}\n")).unwrap();
            std::fs::write(d.join("apps.toml"), format!("# apps {host}\n")).unwrap();
            std::fs::write(d.join("hardware-configuration.nix"), format!("# hw {host}\n")).unwrap();
            crate::init::commit_all(d, "init").unwrap();
        }
        let u = format!("file://{}", remote.display());
        push(&a, &u, "gho_test_token", "a").unwrap();
        restore(&b, &u, "gho_test_token", "b").unwrap();
        assert_eq!(std::fs::read_to_string(b.join("settings.toml")).unwrap(), "# a\n");
        assert_eq!(std::fs::read_to_string(b.join("apps.toml")).unwrap(), "# apps a\n");
        assert_eq!(std::fs::read_to_string(b.join("hardware-configuration.nix")).unwrap(), "# hw b\n");
        // b's history continues a's: a plain push works…
        push(&b, &u, "gho_test_token", "b").unwrap();
        // …and a, now behind, merges instead of failing, keeping its own files.
        std::fs::write(a.join("settings.toml"), "# a2\n").unwrap();
        push(&a, &u, "gho_test_token", "a").unwrap();
        assert_eq!(std::fs::read_to_string(a.join("settings.toml")).unwrap(), "# a2\n");
    }

    #[test]
    fn state_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let s = Store { dir: t.path().join("state") };
        s.set_token(Some("secret")).unwrap();
        assert_eq!(s.token().as_deref(), Some("secret"));
        let mode = std::fs::metadata(t.path().join("state/github-token")).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        s.set_token(None).unwrap();
        assert!(s.token().is_none());
        s.set_token(None).unwrap();
    }
}
