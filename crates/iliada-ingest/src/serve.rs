//! Watch mode: periodically re-check upstream sources and re-ingest on change.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, mpsc};

use crate::elf::host_deb_arch;
use crate::pipeline::{Ingestor, Source};

/// ```toml
/// interval_secs = 3600
/// concurrency = 2
///
/// [[watch]]
/// debian = "jq"
///
/// [[watch]]
/// url = "https://example.org/releases/tool_latest_arm64.deb"
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

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Watch {
    pub url: Option<String>,
    pub debian: Option<String>,
}

fn default_interval() -> u64 {
    3600
}
fn default_concurrency() -> usize {
    2
}

impl Watchlist {
    pub fn load(path: &Path) -> Result<Self> {
        let list: Self =
            toml::from_str(&std::fs::read_to_string(path)?).with_context(|| format!("parsing {}", path.display()))?;
        for w in &list.watches {
            if w.url.is_some() == w.debian.is_some() {
                bail!("each [[watch]] needs exactly one of `url` or `debian`");
            }
        }
        Ok(list)
    }
}

impl Watch {
    fn source(&self) -> Source {
        match (&self.url, &self.debian) {
            (Some(u), _) => Source::Url(u.clone()),
            (_, Some(d)) => Source::Debian(d.clone()),
            _ => unreachable!("validated in Watchlist::load"),
        }
    }
    fn key(&self) -> String {
        self.url.clone().unwrap_or_else(|| format!("debian:{}", self.debian.as_deref().unwrap_or_default()))
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct State {
    /// watch key → last successfully ingested fingerprint
    fingerprints: BTreeMap<String, String>,
}

pub async fn serve(ingestor: Arc<Ingestor>, list: Watchlist) -> Result<()> {
    let state_path: PathBuf = ingestor.config().out_dir.join(".iliada-watch-state.json");
    let state: State =
        std::fs::read(&state_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let state = Arc::new(Mutex::new(state));
    let (tx, rx) = mpsc::channel::<(Watch, String)>(64);
    let rx = Arc::new(Mutex::new(rx));

    for worker in 0..list.concurrency.max(1) {
        let (ing, rx, state, state_path) = (ingestor.clone(), rx.clone(), state.clone(), state_path.clone());
        tokio::spawn(async move {
            loop {
                let Some((watch, fp)) = rx.lock().await.recv().await else { break };
                match ing.ingest(&watch.source()).await {
                    Ok(r) => {
                        tracing::info!(worker, bundle = %r.bundle.display(), version = %r.version, "re-ingested");
                        let mut s = state.lock().await;
                        s.fingerprints.insert(watch.key(), fp);
                        if let Err(e) = std::fs::write(&state_path, serde_json::to_vec_pretty(&*s).unwrap_or_default())
                        {
                            tracing::warn!(error = %e, "could not persist watch state");
                        }
                    }
                    Err(e) => {
                        tracing::error!(worker, source = %watch.key(), error = %format!("{e:#}"), "ingestion failed")
                    }
                }
            }
        });
    }

    tracing::info!(watches = list.watches.len(), interval = list.interval_secs, "watching upstream sources");
    loop {
        ingestor.invalidate_indexes().await;
        for w in &list.watches {
            let fp = match fingerprint(&ingestor, w).await {
                Ok(fp) => fp,
                Err(e) => {
                    tracing::warn!(source = %w.key(), error = %format!("{e:#}"), "cannot check source");
                    continue;
                }
            };
            let known = state.lock().await.fingerprints.get(&w.key()).cloned();
            if known.as_deref() != Some(fp.as_str()) {
                tracing::info!(source = %w.key(), "change detected, queueing");
                tx.send((w.clone(), fp)).await?;
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(list.interval_secs)) => {}
            _ = tokio::signal::ctrl_c() => { tracing::info!("shutting down"); return Ok(()); }
        }
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
        _ => unreachable!(),
    }
}
