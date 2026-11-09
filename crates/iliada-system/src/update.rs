//! Iliada updates: a signed release manifest names the source of the newest
//! release; updating swaps `/etc/iliada/source` for it and applies. A failed
//! build changes nothing, and every update is a boot-menu entry, so a bad
//! one is undone by choosing the previous entry.
//!
//! `release.json` (signed like the app catalog; `release.json.sig` beside it):
//! ```json
//! { "kind": "iliada-release", "version": "0.2.0",
//!   "source": "https://…/iliada-0.2.0-source.tar.zst", "sha256": "…", "size": 1234 }
//! ```

use std::io::BufReader;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const KIND: &str = "iliada-release";
pub const DEFAULT_URL: &str = "https://github.com/cseas002/iliada_os/releases/latest/download/release.json";
const MAX_SOURCE: u64 = 256 << 20;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub kind: String,
    pub version: String,
    pub source: String,
    pub sha256: String,
    pub size: u64,
}

/// `a` is a newer version than `b` (dotted numbers; "0.10.0" > "0.9.1").
pub fn newer(a: &str, b: &str) -> bool {
    let parse =
        |v: &str| -> Vec<u64> { v.trim_start_matches('v').split(['.', '-']).map(|p| p.parse().unwrap_or(0)).collect() };
    parse(a) > parse(b)
}

fn http() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("iliada-system/", env!("CARGO_PKG_VERSION")))
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .expect("http client")
}

async fn get(url: &str, max: u64) -> Result<Vec<u8>> {
    let r = http().get(url).send().await.with_context(|| format!("contacting {url}"))?;
    if r.status() == reqwest::StatusCode::NOT_FOUND {
        bail!("no release found at {url}");
    }
    let r = r.error_for_status()?;
    if r.content_length().is_some_and(|n| n > max) {
        bail!("{url} is too large");
    }
    let b = r.bytes().await?;
    ensure!(b.len() as u64 <= max, "{url} is too large");
    Ok(b.to_vec())
}

/// Fetch and verify the release manifest.
pub async fn latest(url: &str, key: &str) -> Result<Release> {
    let bytes = get(url, 64 << 10).await?;
    let sig = String::from_utf8(get(&format!("{url}.sig"), 1 << 10).await?)?;
    let key = iliada_catalog::parse_key(key)?;
    iliada_catalog::verify_bytes(&key, &bytes, &sig).context("the release manifest is not signed by Iliada")?;
    let r: Release = serde_json::from_slice(&bytes).context("malformed release manifest")?;
    ensure!(r.kind == KIND, "not an Iliada release manifest");
    ensure!(r.size <= MAX_SOURCE, "the release is too large");
    Ok(r)
}

/// Download the release's source, check it against the manifest and unpack
/// it into `dest`. Returns the source directory (the archive's one folder).
pub async fn fetch_source(r: &Release, dest: &Path) -> Result<PathBuf> {
    let bytes = get(&r.source, r.size).await?;
    ensure!(bytes.len() as u64 == r.size, "the download does not match the release manifest (size)");
    ensure!(
        hex::encode(Sha256::digest(&bytes)) == r.sha256,
        "the download does not match the release manifest (checksum)"
    );
    if dest.exists() {
        std::fs::remove_dir_all(dest)?;
    }
    let d = dest.to_owned();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let decoder = ruzstd::decoding::StreamingDecoder::new(BufReader::new(std::io::Cursor::new(bytes)))
            .map_err(|e| anyhow::anyhow!("zstd: {e}"))?;
        iliada_ingest::extract::extract_tar_stream(decoder, &d, Default::default())?;
        Ok(())
    })
    .await??;
    let entries: Vec<PathBuf> = std::fs::read_dir(dest)?.flatten().map(|e| e.path()).collect();
    let [dir] = entries.as_slice() else { bail!("the release archive must contain one folder") };
    ensure!(dir.join("templates/default/flake.nix").exists(), "the release archive is not an Iliada source");
    Ok(dir.clone())
}

#[cfg(test)]
mod tests {
    #[test]
    fn versions() {
        assert!(super::newer("0.2.0", "0.1.0"));
        assert!(super::newer("0.10.0", "0.9.9"));
        assert!(super::newer("v1.0", "0.99"));
        assert!(!super::newer("0.1.0", "0.1.0"));
        assert!(!super::newer("0.1.0", "0.2.0"));
    }
}
