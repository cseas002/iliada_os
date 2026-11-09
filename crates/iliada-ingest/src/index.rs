//! The Debian `Packages` index for one suite/architecture.
//!
//! Integrity: package `.deb`s are verified against the SHA-256 recorded here.
//! The index itself is fetched over HTTPS. Verifying the OpenPGP-signed
//! `InRelease` chain (and the `Packages.xz` hash it lists) is a Phase 2 item;
//! until then the trust root is the TLS connection to the mirror.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};

use crate::control::{for_each_field, paragraphs, parse_relations};
use crate::deb::decompress_xz;
use crate::fetch::Fetcher;

#[derive(Debug, Clone)]
pub struct IndexEntry {
    pub name: String,
    pub version: String,
    pub filename: String,
    pub sha256: String,
    pub depends: Vec<Vec<String>>,
}

#[derive(Debug, Clone)]
pub struct IndexLocation {
    pub mirror: String,
    pub suite: String,
    pub components: Vec<String>,
    pub arch: String,
}

#[derive(Debug, Default)]
pub struct PackageIndex {
    mirror: String,
    entries: HashMap<String, IndexEntry>,
    provides: HashMap<String, Vec<String>>,
}

impl PackageIndex {
    pub fn parse(text: &str, mirror: &str) -> Self {
        let mut idx = Self { mirror: mirror.trim_end_matches('/').to_owned(), ..Default::default() };
        idx.add(text);
        idx
    }

    fn add(&mut self, text: &str) {
        for para in paragraphs(text) {
            let (mut name, mut version, mut filename, mut sha256) = (None, None, None, None);
            let (mut depends, mut pre, mut provides) = (String::new(), String::new(), String::new());
            for_each_field(para, |k, v| match k {
                "Package" => name = Some(v),
                "Version" => version = Some(v),
                "Filename" => filename = Some(v),
                "SHA256" => sha256 = Some(v),
                "Depends" => depends = v,
                "Pre-Depends" => pre = v,
                "Provides" => provides = v,
                _ => {}
            });
            let (Some(name), Some(version), Some(filename), Some(sha256)) = (name, version, filename, sha256) else {
                continue;
            };
            for group in parse_relations(&provides) {
                for virt in group {
                    self.provides.entry(virt).or_default().push(name.clone());
                }
            }
            let mut deps = parse_relations(&pre);
            deps.extend(parse_relations(&depends));
            self.entries.entry(name.clone()).or_insert(IndexEntry { name, version, filename, sha256, depends: deps });
        }
    }

    /// Look up a real package, falling back to the first provider of a virtual one.
    pub fn get(&self, name: &str) -> Option<&IndexEntry> {
        self.entries
            .get(name)
            .or_else(|| self.provides.get(name).and_then(|p| p.iter().find_map(|n| self.entries.get(n))))
    }

    pub fn url_for(&self, e: &IndexEntry) -> String {
        format!("{}/{}", self.mirror, e.filename)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Download (or reuse a <24h old cached copy of) `Packages.xz` for every
    /// component and merge them.
    pub async fn load(fetcher: &Fetcher, cache_dir: &Path, loc: &IndexLocation) -> Result<Self> {
        let mut idx = Self { mirror: loc.mirror.trim_end_matches('/').to_owned(), ..Default::default() };
        let dir = cache_dir.join("indexes");
        tokio::fs::create_dir_all(&dir).await?;
        for component in &loc.components {
            let url = format!("{}/dists/{}/{}/binary-{}/Packages.xz", idx.mirror, loc.suite, component, loc.arch);
            let cache: PathBuf = dir.join(format!("{}_{}_{}_Packages.xz", loc.suite, component, loc.arch));
            let xz = match fresh(&cache, Duration::from_secs(24 * 3600)).await {
                Some(bytes) => bytes,
                None => {
                    tracing::info!(%url, "downloading package index");
                    let f = fetcher.fetch(&url).await.with_context(|| format!("fetching index {url}"))?;
                    tokio::fs::write(&cache, &f.bytes).await?;
                    f.bytes
                }
            };
            let text = tokio::task::spawn_blocking(move || decompress_xz(&xz, 2 << 30)).await??;
            idx.add(&String::from_utf8_lossy(&text));
        }
        tracing::info!(packages = idx.len(), suite = %loc.suite, arch = %loc.arch, "package index ready");
        Ok(idx)
    }
}

async fn fresh(path: &Path, max_age: Duration) -> Option<Vec<u8>> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    let age = SystemTime::now().duration_since(meta.modified().ok()?).ok()?;
    if age > max_age {
        return None;
    }
    tokio::fs::read(path).await.ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "Package: jq\nVersion: 1.7.1-6\nDepends: libjq1 (= 1.7.1-6), libc6 (>= 2.34)\nFilename: pool/main/j/jq/jq_1.7.1-6_arm64.deb\nSHA256: aa\nDescription: x\n more\n\nPackage: libonig5\nVersion: 6.9.9-1\nProvides: libonig\nFilename: pool/main/libo/libonig/libonig5_6.9.9-1_arm64.deb\nSHA256: bb\n";

    #[test]
    fn parses_and_resolves_virtuals() {
        let idx = PackageIndex::parse(SAMPLE, "https://deb.debian.org/debian/");
        assert_eq!(idx.len(), 2);
        let jq = idx.get("jq").unwrap();
        assert_eq!(jq.depends, vec![vec!["libjq1".to_string()], vec!["libc6".to_string()]]);
        assert_eq!(idx.url_for(jq), "https://deb.debian.org/debian/pool/main/j/jq/jq_1.7.1-6_arm64.deb");
        assert_eq!(idx.get("libonig").unwrap().name, "libonig5");
    }
}
