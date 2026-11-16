//! The computer side of the Iliada account (see `docs/account.md`).
//!
//! * **Signing in**: the computer shows a code; the person approves it on the
//!   account website (with a passkey or an email code) and the computer gets
//!   its own token. Linux has no passkeys outside web browsers, so this is
//!   how a computer signs in.
//! * **The account key** ([`key`]): made on the first computer and never sent
//!   to the server; a new computer gets it from the recovery key.
//! * **Backups** ([`snapshot`]): the desktop settings, Privacy answers and
//!   assistant conversations, sealed with the account key before they leave.
//!
//! Each person signs in for themselves: everything lives in their home
//! folder, readable only by them (`~/.config/iliada/account.json` and
//! `account-key`).

pub mod key;
pub mod server;
pub mod snapshot;

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

pub use key::AccountKey;
pub use server::{DeviceCode, Server};
pub use snapshot::{Manifest, PARTS};

/// What proves a computer holds the account key: this text, sealed.
pub const KEY_CHECK: &str = "key-check";
const KEY_CHECK_TEXT: &[u8] = b"Iliada account key";
/// The latest backup of a person's settings.
pub const SETTINGS: &str = "settings";

/// The server this computer uses, if Iliada was set up with one
/// (`ILIADA_ACCOUNT_SERVER`, set by `iliada.account.server`).
pub fn default_server() -> Option<String> {
    std::env::var("ILIADA_ACCOUNT_SERVER").ok().filter(|s| !s.trim().is_empty())
}

/// This computer's sign-in, as saved.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    pub server: String,
    pub email: String,
    pub token: String,
    /// The name the website shows for this computer.
    pub computer: String,
    pub last_backup: Option<u64>,
    pub last_error: Option<String>,
    /// Of the files last backed up: skip the upload when nothing changed.
    pub last_digest: Option<String>,
}

/// Where a person's sign-in and key are kept.
#[derive(Debug, Clone)]
pub struct Files {
    pub home: PathBuf,
    pub state: PathBuf,
    pub key: PathBuf,
}

impl Files {
    /// The current user's.
    pub fn user() -> Result<Files> {
        let home = PathBuf::from(std::env::var_os("HOME").context("no home folder")?);
        let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
        Ok(Files::at(&home, &config.join("iliada")))
    }

    pub fn at(home: &Path, dir: &Path) -> Files {
        Files { home: home.to_owned(), state: dir.join("account.json"), key: dir.join("account-key") }
    }

    pub fn load(&self) -> Option<State> {
        serde_json::from_slice(&std::fs::read(&self.state).ok()?).ok()
    }

    fn save(&self, s: &State) -> Result<()> {
        write_private(&self.state, &serde_json::to_vec_pretty(s)?)
    }

    pub fn account_key(&self) -> Option<AccountKey> {
        AccountKey::from_recovery_text(&std::fs::read_to_string(&self.key).ok()?).ok()
    }

    fn save_key(&self, k: &AccountKey) -> Result<()> {
        write_private(&self.key, format!("{}\n", k.recovery_text()).as_bytes())
    }

    /// Forgets the sign-in and the key.
    fn forget(&self) -> Result<()> {
        for p in [&self.state, &self.key] {
            match std::fs::remove_file(p) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e.into()),
                _ => {}
            }
        }
        Ok(())
    }

    fn signed_in(&self) -> Result<(State, Server)> {
        let s = self.load().context("this computer isn't signed in to an Iliada account")?;
        let server = Server::new(&s.server, Some(s.token.clone()));
        Ok((s, server))
    }

    fn ready(&self) -> Result<(State, Server, AccountKey)> {
        let (s, server) = self.signed_in()?;
        let key = self.account_key().context("this computer needs your recovery key first")?;
        Ok((s, server, key))
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Status {
    SignedOut,
    /// Signed in, but this computer doesn't have the account key yet.
    NeedsKey {
        email: String,
    },
    Ready {
        email: String,
        last_backup: Option<u64>,
        last_error: Option<String>,
    },
}

pub fn status(files: &Files) -> Status {
    match (files.load(), files.account_key()) {
        (None, _) => Status::SignedOut,
        (Some(s), None) => Status::NeedsKey { email: s.email },
        (Some(s), Some(_)) => Status::Ready { email: s.email, last_backup: s.last_backup, last_error: s.last_error },
    }
}

/// This computer's name, as the website lists it.
pub fn computer_name() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .ok()
        .map(|h| h.trim().to_owned())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "Iliada computer".into())
}

/// The first half of signing in: a code to show the person.
pub async fn start_sign_in(server: &str) -> Result<DeviceCode> {
    Server::new(server, None).device_code(&computer_name()).await
}

/// What the account key needs after signing in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyStep {
    /// The account's first computer: a new key, to be shown to the person
    /// once as their recovery key.
    Created(String),
    /// The account has a key this computer doesn't: ask for the recovery key
    /// ([`use_recovery_key`]).
    Needed,
    /// This computer already had it.
    Ready,
}

/// The second half: waits for the person to approve the code, then sets up
/// the account key.
pub async fn finish_sign_in(files: &Files, server: &str, code: &DeviceCode) -> Result<(String, KeyStep)> {
    let token = Server::new(server, None).wait_for_token(code).await?;
    let api = Server::new(server, Some(token.clone()));
    let email = api.account().await?.email;
    // A different account than before: its key isn't this one.
    if files.load().is_some_and(|old| old.email != email || old.server != server.trim_end_matches('/')) {
        files.forget()?;
    }
    files.save(&State {
        server: server.trim_end_matches('/').to_owned(),
        email: email.clone(),
        token,
        computer: computer_name(),
        ..Default::default()
    })?;
    let step = match api.get(KEY_CHECK).await? {
        None => {
            let key = AccountKey::generate()?;
            api.put(KEY_CHECK, key.seal(KEY_CHECK, KEY_CHECK_TEXT)?).await?;
            files.save_key(&key)?;
            KeyStep::Created(key.recovery_text())
        }
        Some(sealed) => match files.account_key() {
            Some(k) if k.open(KEY_CHECK, &sealed).is_ok() => KeyStep::Ready,
            _ => {
                let _ = std::fs::remove_file(&files.key);
                KeyStep::Needed
            }
        },
    };
    Ok((email, step))
}

/// Takes the account key from the recovery key the person typed.
pub async fn use_recovery_key(files: &Files, text: &str) -> Result<()> {
    let key = AccountKey::from_recovery_text(text)?;
    let (_, api) = files.signed_in()?;
    let sealed = api.get(KEY_CHECK).await?.context("this account has no key yet; sign in again")?;
    if key.open(KEY_CHECK, &sealed).is_err() {
        bail!("That isn't this account's recovery key. Check it and try again.");
    }
    files.save_key(&key)
}

/// The recovery key, to show the person again.
pub fn recovery_key(files: &Files) -> Option<String> {
    files.account_key().map(|k| k.recovery_text())
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// What a backup did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Backup {
    Uploaded { files: usize, bytes: usize },
    Unchanged,
}

/// Backs up this person's settings unless nothing changed (or `force`).
pub async fn backup(files: &Files, force: bool) -> Result<Backup> {
    let (mut s, api, key) = files.ready()?;
    let result = async {
        let manifest =
            Manifest { computer: s.computer.clone(), created: now(), iliada: env!("CARGO_PKG_VERSION").into() };
        let home = files.home.clone();
        let packed = tokio::task::spawn_blocking(move || snapshot::pack(&home, &manifest)).await??;
        if !force && s.last_digest.as_deref() == Some(packed.digest.as_str()) {
            return Ok((Backup::Unchanged, packed.digest));
        }
        let sealed = key.seal(SETTINGS, &packed.bytes)?;
        let bytes = sealed.len();
        api.put(SETTINGS, sealed).await?;
        Ok::<_, anyhow::Error>((Backup::Uploaded { files: packed.files, bytes }, packed.digest))
    }
    .await;
    match result {
        Ok((b, digest)) => {
            if b != Backup::Unchanged {
                s.last_backup = Some(now());
            }
            s.last_digest = Some(digest);
            s.last_error = None;
            files.save(&s)?;
            Ok(b)
        }
        Err(e) => {
            s.last_error = Some(format!("{e:#}"));
            files.save(&s)?;
            Err(e)
        }
    }
}

async fn latest_bytes(api: &Server, key: &AccountKey) -> Result<Option<Vec<u8>>> {
    match api.get(SETTINGS).await? {
        Some(sealed) => Ok(Some(key.open(SETTINGS, &sealed)?)),
        None => Ok(None),
    }
}

/// Where the account's backup came from, if it has one.
pub async fn latest(files: &Files) -> Result<Option<Manifest>> {
    let (_, api, key) = files.ready()?;
    latest_bytes(&api, &key).await?.map(|b| snapshot::manifest(&b)).transpose()
}

/// Puts the account's backup in place. Returns where it came from and how
/// many files were written.
pub async fn restore(files: &Files) -> Result<(Manifest, usize)> {
    let (mut s, api, key) = files.ready()?;
    let bytes = latest_bytes(&api, &key).await?.context("your account has no backup yet")?;
    let home = files.home.clone();
    let (manifest, n, digest) = tokio::task::spawn_blocking(move || -> Result<_> {
        let (m, n) = snapshot::unpack(&home, &bytes)?;
        // What is here now: the next backup only uploads real changes.
        let digest = snapshot::pack(&home, &m)?.digest;
        Ok((m, n, digest))
    })
    .await??;
    s.last_digest = Some(digest);
    files.save(&s)?;
    Ok((manifest, n))
}

/// Signs this computer out: the server forgets its token, and the computer
/// forgets the sign-in and the account key.
pub async fn sign_out(files: &Files) -> Result<()> {
    if let Ok((_, api)) = files.signed_in() {
        // Signed out here even when the server can't be reached; the website
        // can still remove the computer.
        let _ = api.sign_out().await;
    }
    files.forget()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_follows_the_files() {
        let t = tempfile::tempdir().unwrap();
        let f = Files::at(t.path(), &t.path().join(".config/iliada"));
        assert_eq!(status(&f), Status::SignedOut);
        f.save(&State { email: "ana@example.com".into(), ..Default::default() }).unwrap();
        assert_eq!(status(&f), Status::NeedsKey { email: "ana@example.com".into() });
        f.save_key(&AccountKey::generate().unwrap()).unwrap();
        assert!(matches!(status(&f), Status::Ready { .. }));
        use std::os::unix::fs::PermissionsExt;
        for p in [&f.state, &f.key] {
            assert_eq!(std::fs::metadata(p).unwrap().permissions().mode() & 0o777, 0o600);
        }
        f.forget().unwrap();
        assert_eq!(status(&f), Status::SignedOut);
    }
}
