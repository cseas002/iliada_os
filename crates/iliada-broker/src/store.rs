//! Persistent ("Always") grants, stored as JSON in the broker state directory.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use iliada_types::{AppId, Capability};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
struct OnDisk {
    version: u32,
    grants: BTreeMap<AppId, BTreeSet<Capability>>,
}

/// Grants are keyed by app id. Phase 2 extends the key with the bundle's
/// signing-key fingerprint so a re-signed bundle (new publisher) starts clean.
#[derive(Debug)]
pub struct GrantStore {
    path: PathBuf,
    data: OnDisk,
}

impl GrantStore {
    pub fn open(state_dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(state_dir)?;
        let path = state_dir.join("grants.json");
        let data = match std::fs::read(&path) {
            Ok(b) => serde_json::from_slice(&b).with_context(|| format!("corrupt grant store {}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => OnDisk { version: 1, ..Default::default() },
            Err(e) => return Err(e.into()),
        };
        Ok(Self { path, data })
    }

    pub fn has(&self, app: &AppId, cap: &Capability) -> bool {
        self.data.grants.get(app).is_some_and(|s| s.iter().any(|g| g.covers(cap)))
    }

    pub fn list(&self, app: &AppId) -> Vec<Capability> {
        self.data.grants.get(app).map(|s| s.iter().copied().collect()).unwrap_or_default()
    }

    pub fn insert(&mut self, app: &AppId, cap: Capability) -> Result<()> {
        self.data.grants.entry(app.clone()).or_default().insert(cap);
        self.save()
    }

    pub fn remove(&mut self, app: &AppId, cap: &Capability) -> Result<bool> {
        let removed = self.data.grants.get_mut(app).is_some_and(|s| s.remove(cap));
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    /// Write-then-rename so a crash never leaves a half-written store.
    fn save(&self) -> Result<()> {
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&self.data)?)?;
        std::fs::rename(&tmp, &self.path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let app = AppId::new("org.example.a").unwrap();
        {
            let mut s = GrantStore::open(dir.path()).unwrap();
            s.insert(&app, Capability::Microphone).unwrap();
        }
        let mut s = GrantStore::open(dir.path()).unwrap();
        assert!(s.has(&app, &Capability::Microphone));
        assert!(!s.has(&app, &Capability::Camera));
        assert!(s.remove(&app, &Capability::Microphone).unwrap());
        assert!(!GrantStore::open(dir.path()).unwrap().has(&app, &Capability::Microphone));
    }
}
