//! Watch mode: install the apps on the app list and update them when
//! upstream changes. Users only ever see `.app` bundles; this daemon is the
//! plumbing that produces them.
//!
//! The app list (normally `/etc/iliada/apps.toml`) is *mutable state*: it is
//! re-read every tick, so an entry added by a restore or by `iliada-appd`
//! takes effect within seconds, without rebuilding the system.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result};
use iliada_bundle::Bundle;
use iliada_types::AppId;
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc};

use crate::elf::host_deb_arch;
use crate::pipeline::{Ingestor, Source};

/// ```toml
/// interval_secs = 21600          # how often to look for upstream updates
///
/// [[watch]]
/// debian = "gnome-calculator"    # a Debian package
///
/// [[watch]]
/// url = "https://example.org/tool_latest_amd64.deb"
///
/// [[watch]]                      # installed by hand (drag-and-drop): recorded,
/// id = "org.example.tool"        # not managed
/// name = "Tool"
/// manual = true
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Watchlist {
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    #[serde(rename = "watch", default)]
    pub watches: Vec<Watch>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Watch {
    pub url: Option<String>,
    pub debian: Option<String>,
    /// App id of the installed bundle (written by iliada-appd).
    pub id: Option<String>,
    /// Display name (informational).
    pub name: Option<String>,
    /// Installed by hand; no source to install or update from.
    #[serde(default)]
    pub manual: bool,
    /// Installed from the Iliada Store; `iliada-store sync` manages it.
    pub store: Option<String>,
}

fn default_interval() -> u64 {
    21600
}
fn default_concurrency() -> usize {
    2
}

impl Watchlist {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let list: Self = toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        for w in &list.watches {
            if w.url.is_some() && w.debian.is_some() {
                anyhow::bail!("a [[watch]] entry may have `url` or `debian`, not both");
            }
        }
        Ok(list)
    }
}

impl Watch {
    /// Entries we can install and update from.
    fn source(&self) -> Option<Source> {
        match (&self.url, &self.debian) {
            _ if self.manual || self.store.is_some() => None,
            (Some(u), _) => Some(Source::Url(u.clone())),
            (_, Some(d)) => Some(Source::Debian(d.clone())),
            _ => None,
        }
    }
    fn key(&self) -> String {
        match (&self.url, &self.debian) {
            (Some(u), _) => u.clone(),
            (_, Some(d)) => format!("debian:{d}"),
            _ => format!("id:{}", self.id.as_deref().unwrap_or_default()),
        }
    }
    /// The app id this entry installs, when it can be known in advance.
    pub fn app_id(&self, vendor_prefix: &str) -> Option<AppId> {
        if let Some(id) = &self.id {
            return AppId::new(id.clone()).ok();
        }
        self.debian.as_deref().and_then(|d| AppId::from_upstream(vendor_prefix, d).ok())
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    /// watch key → last successfully ingested fingerprint
    fingerprints: BTreeMap<String, String>,
}

/// Where each app on the list stands, for the Store and the `iliada`
/// command (in the cache folder, readable by everyone).
pub const STATUS_FILE: &str = "status.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildState {
    Queued,
    Building,
    Installed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BuildStatus {
    pub state: BuildState,
    /// Why it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Seconds since the Unix epoch.
    pub time: u64,
}

/// Status by app-list key ("debian:krita").
pub type Statuses = BTreeMap<String, BuildStatus>;

/// The statuses the translator last wrote into `cache_dir`.
pub fn read_status(cache_dir: &Path) -> Statuses {
    std::fs::read(cache_dir.join(STATUS_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

async fn set_status(statuses: &Mutex<Statuses>, path: &Path, key: &str, state: BuildState, error: Option<String>) {
    let time = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let mut s = statuses.lock().await;
    s.insert(key.to_owned(), BuildStatus { state, error, time });
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, serde_json::to_vec_pretty(&*s).unwrap_or_default())
        .and_then(|_| std::fs::rename(&tmp, path))
        .is_err()
    {
        tracing::warn!("could not write the build status");
    }
}

/// Where the daemon looks for work.
#[derive(Debug, Clone)]
pub struct ServeConfig {
    pub list: PathBuf,
    pub tick: Duration,
}

enum Job {
    Watch(Watch, String),
}

/// Ids of the bundles currently installed in `dir`.
pub fn installed_ids(dir: &Path) -> BTreeSet<String> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "app"))
                .filter_map(|e| Bundle::open(e.path()).ok())
                .map(|b| b.info.id.to_string())
                .collect()
        })
        .unwrap_or_default()
}

pub async fn serve(ingestor: Arc<Ingestor>, cfg: ServeConfig) -> Result<()> {
    let out_dir = ingestor.config().out_dir.clone();
    let state_path = ingestor.config().cache_dir.join("watch-state.json");
    let state: State =
        std::fs::read(&state_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let state = Arc::new(Mutex::new(state));
    let in_flight: Arc<Mutex<BTreeSet<String>>> = Arc::default();
    let status_path = ingestor.config().cache_dir.join(STATUS_FILE);
    let statuses = Arc::new(Mutex::new(read_status(&ingestor.config().cache_dir)));
    let (tx, rx) = mpsc::channel::<Job>(256);
    let rx = Arc::new(Mutex::new(rx));

    let mut list = Watchlist::load(&cfg.list).unwrap_or_else(|e| {
        tracing::warn!(error = %format!("{e:#}"), "app list unreadable; starting empty");
        Watchlist { interval_secs: default_interval(), concurrency: default_concurrency(), watches: vec![] }
    });
    for worker in 0..list.concurrency.max(1) {
        let (ing, rx, state, state_path, in_flight, statuses, status_path) = (
            ingestor.clone(),
            rx.clone(),
            state.clone(),
            state_path.clone(),
            in_flight.clone(),
            statuses.clone(),
            status_path.clone(),
        );
        tokio::spawn(async move {
            loop {
                let Some(job) = rx.lock().await.recv().await else { break };
                match job {
                    Job::Watch(watch, fp) => {
                        let Some(source) = watch.source() else { continue };
                        set_status(&statuses, &status_path, &watch.key(), BuildState::Building, None).await;
                        match ing.ingest(&source).await {
                            Ok(r) => {
                                tracing::info!(worker, bundle = %r.bundle.display(), version = %r.version, "installed");
                                let mut s = state.lock().await;
                                s.fingerprints.insert(watch.key(), fp);
                                save_state(&state_path, &s);
                                drop(s);
                                set_status(&statuses, &status_path, &watch.key(), BuildState::Installed, None).await;
                            }
                            Err(e) => {
                                tracing::error!(worker, source = %watch.key(), error = %format!("{e:#}"), "install failed");
                                let why = Some(format!("{e:#}"));
                                set_status(&statuses, &status_path, &watch.key(), BuildState::Failed, why).await;
                            }
                        }
                        in_flight.lock().await.remove(&watch.key());
                    }
                }
            }
        });
    }

    let mut list_mtime = mtime(&cfg.list);
    let mut next_update_check = Instant::now();
    tracing::info!(list = %cfg.list.display(), "watching");
    loop {
        // 1. The app list changed (an app was added by a restore, by hand, or by appd).
        let m = mtime(&cfg.list);
        let list_changed = m != list_mtime;
        if list_changed {
            list_mtime = m;
            match Watchlist::load(&cfg.list) {
                Ok(l) => list = l,
                Err(e) => tracing::warn!(error = %format!("{e:#}"), "app list unreadable; keeping the previous one"),
            }
        }

        // 2. New entries now; upstream updates every interval_secs.
        let full_check = Instant::now() >= next_update_check;
        if full_check {
            ingestor.invalidate_indexes().await;
            // Which apps Debian has, for the Store and the iliada command.
            let ing = ingestor.clone();
            tokio::spawn(async move {
                let c = ing.config();
                let arch = c.arch.clone().unwrap_or_else(|| host_deb_arch().to_owned());
                if let Err(e) = crate::appstream::refresh(ing.fetcher(), &c.cache_dir, &c.mirror, &c.suite, &arch).await
                {
                    tracing::warn!(error = %format!("{e:#}"), "could not get Debian's app list");
                }
            });
            next_update_check = Instant::now() + Duration::from_secs(list.interval_secs.max(60));
        }
        if full_check || list_changed {
            let installed = installed_ids(&out_dir);
            for w in list.watches.iter().filter(|w| w.source().is_some()) {
                let known = state.lock().await.fingerprints.get(&w.key()).cloned();
                if known.is_some() && !full_check {
                    continue;
                }
                let fp = match fingerprint(&ingestor, w).await {
                    Ok(fp) => fp,
                    Err(e) => {
                        tracing::warn!(source = %w.key(), error = %format!("{e:#}"), "cannot check source");
                        if known.is_none() {
                            let why = Some(format!("{e:#}"));
                            set_status(&statuses, &status_path, &w.key(), BuildState::Failed, why).await;
                        }
                        continue;
                    }
                };
                if known.as_deref() == Some(fp.as_str()) {
                    continue;
                }
                // Already installed (e.g. recorded by iliada-appd): adopt it
                // instead of reinstalling; update only when upstream changes.
                let present =
                    w.app_id(&ingestor.config().vendor_prefix).is_some_and(|id| installed.contains(id.as_str()));
                if known.is_none() && present {
                    let mut s = state.lock().await;
                    s.fingerprints.insert(w.key(), fp);
                    save_state(&state_path, &s);
                    continue;
                }
                if in_flight.lock().await.insert(w.key()) {
                    tracing::info!(source = %w.key(), "queueing install/update");
                    set_status(&statuses, &status_path, &w.key(), BuildState::Queued, None).await;
                    tx.send(Job::Watch(w.clone(), fp)).await?;
                }
            }
        }

        tokio::select! {
            _ = tokio::time::sleep(cfg.tick) => {}
            _ = tokio::signal::ctrl_c() => { tracing::info!("shutting down"); return Ok(()); }
        }
    }
}

fn mtime(p: &Path) -> Option<SystemTime> {
    std::fs::metadata(p).and_then(|m| m.modified()).ok()
}

fn save_state(path: &Path, s: &State) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Err(e) = std::fs::write(path, serde_json::to_vec_pretty(s).unwrap_or_default()) {
        tracing::warn!(error = %e, "could not persist watch state");
    }
}

async fn fingerprint(ing: &Ingestor, w: &Watch) -> Result<String> {
    match (&w.url, &w.debian) {
        (Some(url), _) => Ok(ing.fetcher().validator(url).await?.unwrap_or_else(|| "unversioned".into())),
        (_, Some(pkg)) => {
            let arch = ing.config().arch.clone().unwrap_or_else(|| host_deb_arch().to_owned());
            let idx = ing.index(&arch).await?;
            let e = idx.get(pkg).with_context(|| format!("{pkg} not in index"))?;
            Ok(format!("{}@{}", e.version, e.sha256))
        }
        _ => anyhow::bail!("entry has no source"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_managed_and_manual_entries() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("apps.toml");
        std::fs::write(
            &p,
            "# my apps\n[[watch]]\ndebian = \"jq\"\n\n[[watch]]\nid = \"org.example.tool\"\nname = \"Tool\"\nmanual = true\n",
        )
        .unwrap();
        let l = Watchlist::load(&p).unwrap();
        assert_eq!(l.interval_secs, 21600);
        assert!(l.watches[0].source().is_some());
        assert_eq!(l.watches[0].app_id("org.debian").unwrap().as_str(), "org.debian.jq");
        assert!(l.watches[1].source().is_none(), "manual entries are recorded, not managed");
        std::fs::write(&p, "[[watch]]\ndebian = \"a\"\nurl = \"https://x\"\n").unwrap();
        assert!(Watchlist::load(&p).is_err());
    }
}
