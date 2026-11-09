//! Confinement for the extraction worker.
//!
//! Path checks in [`crate::extract`] are the primary defence; this is the
//! second layer. On Linux the worker thread is restricted with Landlock to
//! *read* anywhere but *write* only inside the staging directory, so even a
//! bug in the extractor (or in a decompressor) cannot modify the host.
//! Landlock is per-thread, so only the worker is confined, not the daemon.
//!
//! On other hosts (macOS development machines) the closure runs unconfined
//! and a warning is logged.

use std::path::PathBuf;

use anyhow::{Result, anyhow};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confinement {
    /// Kernel-enforced write restriction is active.
    Enforced,
    /// The kernel does not support Landlock (or only partially).
    BestEffort,
    /// Not available on this OS.
    Unavailable,
}

/// Run `f` on a dedicated thread that can only write below `writable`.
pub async fn run_confined<T, F>(writable: PathBuf, f: F) -> Result<(T, Confinement)>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    let handle = std::thread::Builder::new().name("iliada-extract".into()).spawn(move || {
        let level = confine_current_thread(&writable)?;
        f().map(|v| (v, level))
    })?;
    tokio::task::spawn_blocking(move || handle.join().map_err(|_| anyhow!("extraction worker panicked"))?).await?
}

#[cfg(target_os = "linux")]
fn confine_current_thread(writable: &std::path::Path) -> Result<Confinement> {
    use landlock::{
        ABI, Access, AccessFs, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus, path_beneath_rules,
    };
    let abi = ABI::V3;
    let status = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))?
        .create()?
        .add_rules(path_beneath_rules(["/"], AccessFs::from_read(abi)))?
        .add_rules(path_beneath_rules([writable], AccessFs::from_all(abi)))?
        .restrict_self()?;
    Ok(match status.ruleset {
        RulesetStatus::FullyEnforced => Confinement::Enforced,
        RulesetStatus::PartiallyEnforced => Confinement::BestEffort,
        RulesetStatus::NotEnforced => {
            tracing::warn!("kernel lacks Landlock; extraction runs with path checks only");
            Confinement::BestEffort
        }
    })
}

#[cfg(not(target_os = "linux"))]
fn confine_current_thread(_writable: &std::path::Path) -> Result<Confinement> {
    static WARN: std::sync::Once = std::sync::Once::new();
    WARN.call_once(|| tracing::warn!("no Landlock on this OS; extraction runs with path checks only"));
    Ok(Confinement::Unavailable)
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn worker_cannot_write_outside_staging() {
        let staging = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("escape");
        let inside = staging.path().join("ok");
        let (res, level) = run_confined(staging.path().to_owned(), move || {
            std::fs::write(&inside, b"ok")?;
            Ok(std::fs::write(&target, b"pwned").is_err())
        })
        .await
        .unwrap();
        if level == Confinement::Enforced {
            assert!(res, "write outside the staging dir must be denied");
        }
        assert!(staging.path().join("ok").exists());
    }
}
