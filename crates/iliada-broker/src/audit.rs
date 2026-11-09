//! Append-only JSON-lines audit log of every decision.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use iliada_types::{AppId, Capability};
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio::sync::Mutex;

#[derive(Debug)]
pub struct AuditLog {
    path: PathBuf,
    lock: Mutex<()>,
}

#[derive(Debug, Serialize)]
pub struct AuditRecord<'a> {
    pub ts: u64,
    pub app: &'a AppId,
    pub session: u64,
    pub capability: &'a Capability,
    pub decision: &'a str,
    pub detail: String,
}

impl AuditLog {
    pub fn new(state_dir: &Path) -> Self {
        Self { path: state_dir.join("audit.jsonl"), lock: Mutex::new(()) }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn record(&self, app: &AppId, session: u64, capability: &Capability, decision: &str, detail: String) {
        let rec = AuditRecord {
            ts: SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            app,
            session,
            capability,
            decision,
            detail,
        };
        let Ok(mut line) = serde_json::to_vec(&rec) else { return };
        line.push(b'\n');
        let _g = self.lock.lock().await;
        let res = async {
            let mut f = tokio::fs::OpenOptions::new().create(true).append(true).open(&self.path).await?;
            f.write_all(&line).await?;
            f.flush().await
        }
        .await;
        if let Err(e) = res {
            tracing::error!(error = %e, path = %self.path.display(), "cannot write audit log");
        }
    }
}
