//! What the user decided about each app's permissions.
//!
//! `~/.local/share/iliada/permissions.toml`, outside every sandbox (apps see
//! only their own data directory), written by `iliada-run` after a
//! host-drawn prompt and by Settings → Privacy:
//!
//! ```toml
//! ["org.gimp.GIMP"]
//! "files.pictures" = "allow"
//! camera = "deny"
//! ```

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use iliada_types::{Capability, FileScope};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Allow,
    Deny,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Decisions {
    pub apps: BTreeMap<String, BTreeMap<String, Decision>>,
}

/// A stable name for a capability in the decisions file.
pub fn key(cap: &Capability) -> String {
    use iliada_types::{ClipboardAccess as C, LocationPrecision as L};
    match cap {
        Capability::Camera => "camera".into(),
        Capability::Microphone => "microphone".into(),
        Capability::Sound => "sound".into(),
        Capability::Location(L::Coarse) => "location.coarse".into(),
        Capability::Location(L::Precise) => "location.precise".into(),
        Capability::Clipboard(C::Read) => "clipboard.read".into(),
        Capability::Clipboard(C::Write) => "clipboard.write".into(),
        Capability::Files(FileScope::UserPicked) => "files.picked".into(),
        Capability::Files(FileScope::KnownDir(d)) => format!("files.{}", d.name().to_lowercase()),
        Capability::Notifications => "notifications".into(),
        Capability::ScreenCapture => "screen_capture".into(),
    }
}

impl Decisions {
    /// The default location: `$XDG_DATA_HOME/iliada/permissions.toml`.
    pub fn default_path() -> Option<PathBuf> {
        let base = match std::env::var_os("XDG_DATA_HOME") {
            Some(x) => PathBuf::from(x),
            None => PathBuf::from(std::env::var_os("HOME")?).join(".local/share"),
        };
        Some(base.join("iliada/permissions.toml"))
    }

    /// Missing or unreadable means "nothing decided yet".
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }

    pub fn get(&self, app: &str, cap: &Capability) -> Option<Decision> {
        self.apps.get(app)?.get(&key(cap)).copied()
    }

    pub fn set(&mut self, app: &str, cap: &Capability, d: Decision) {
        self.apps.entry(app.to_owned()).or_default().insert(key(cap), d);
    }

    /// Forget a decision (the app asks again next time).
    pub fn reset(&mut self, app: &str, cap: &Capability) {
        if let Some(m) = self.apps.get_mut(app) {
            m.remove(&key(cap));
            if m.is_empty() {
                self.apps.remove(app);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iliada_types::KnownDir;

    #[test]
    fn round_trip_and_reset() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("x/permissions.toml");
        let pics = Capability::Files(FileScope::KnownDir(KnownDir::Pictures));
        let mut d = Decisions::load(&p);
        assert_eq!(d.get("org.gimp.GIMP", &pics), None);
        d.set("org.gimp.GIMP", &pics, Decision::Allow);
        d.set("org.gimp.GIMP", &Capability::Camera, Decision::Deny);
        d.save(&p).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert!(text.contains("\"files.pictures\" = \"allow\""), "{text}");
        let mut d = Decisions::load(&p);
        assert_eq!(d.get("org.gimp.GIMP", &pics), Some(Decision::Allow));
        d.reset("org.gimp.GIMP", &pics);
        d.reset("org.gimp.GIMP", &Capability::Camera);
        assert!(d.apps.is_empty());
    }
}
