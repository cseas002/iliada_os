//! Which desktop apps Debian has: its AppStream data (DEP-11
//! `Components-<arch>.yml`), the same list GNOME Software and KDE Discover
//! show. The translator keeps a compact copy (`debian-apps.json` in its
//! cache folder, readable by everyone) so the Store, the `iliada` command
//! and the assistant can offer apps that aren't in the Iliada Store yet.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::deb::decompress_xz;
use crate::fetch::Fetcher;

/// The file clients read (in the translator's cache folder).
pub const APPS_FILE: &str = "debian-apps.json";

/// One desktop app Debian packages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DebianApp {
    /// The Debian package to translate.
    pub package: String,
    /// AppStream id ("org.kde.krita").
    pub id: String,
    pub name: String,
    pub summary: String,
    #[serde(default)]
    pub categories: Vec<String>,
}

/// Desktop apps in DEP-11 YAML (one document per component). Only the
/// fields Iliada needs are read; the English ("C") name and summary.
pub fn parse(yaml: &str) -> Vec<DebianApp> {
    let mut out = Vec::new();
    for doc in yaml.split("\n---\n") {
        let (mut kind, mut id, mut package, mut name, mut summary) = (None, None, None, None, None);
        let mut categories = Vec::new();
        // Which mapping/list the indented lines belong to.
        let mut section = "";
        for line in doc.lines() {
            // A list may sit at the key's own indentation ("Categories:\n- System").
            if !line.starts_with(' ') && !line.starts_with("- ") {
                let (key, value) = line.split_once(':').unwrap_or((line, ""));
                let value = value.trim();
                section = key;
                match key {
                    "Type" => kind = Some(value),
                    "ID" => id = Some(value),
                    "Package" => package = Some(value),
                    _ => {}
                }
                continue;
            }
            let item = line.trim_start();
            match section {
                "Name" => {
                    if let Some(v) = item.strip_prefix("C: ") {
                        name = Some(unquote(v));
                    }
                }
                "Summary" => {
                    if let Some(v) = item.strip_prefix("C: ") {
                        summary = Some(unquote(v));
                    }
                }
                "Categories" => {
                    if let Some(v) = item.strip_prefix("- ") {
                        categories.push(v.trim().to_owned());
                    }
                }
                _ => {}
            }
        }
        if kind != Some("desktop-application") {
            continue;
        }
        let (Some(package), Some(id), Some(name)) = (package, id, name) else { continue };
        out.push(DebianApp {
            package: package.to_owned(),
            id: id.to_owned(),
            name,
            summary: summary.unwrap_or_default(),
            categories,
        });
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    out.dedup_by(|a, b| a.package == b.package);
    out
}

/// YAML scalars here are plain or single-quoted (`'It''s'`).
fn unquote(v: &str) -> String {
    let v = v.trim();
    match v.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        Some(inner) => inner.replace("''", "'"),
        None => v.trim_matches('"').to_owned(),
    }
}

/// Downloads Debian's app list for `arch` (at most once a day) and writes
/// `debian-apps.json` into `cache_dir`. Returns how many apps it lists.
pub async fn refresh(fetcher: &Fetcher, cache_dir: &Path, mirror: &str, suite: &str, arch: &str) -> Result<usize> {
    let out = cache_dir.join(APPS_FILE);
    if fresh(&out, Duration::from_secs(24 * 3600)) {
        return Ok(0);
    }
    let url = format!("{}/dists/{suite}/main/dep11/Components-{arch}.yml.xz", mirror.trim_end_matches('/'));
    tracing::info!(%url, "downloading Debian's app list");
    let f = fetcher.fetch(&url).await.with_context(|| format!("fetching {url}"))?;
    let yaml = tokio::task::spawn_blocking(move || decompress_xz(&f.bytes, 1 << 30)).await??;
    let apps = parse(&String::from_utf8_lossy(&yaml));
    let tmp: PathBuf = cache_dir.join(format!(".{APPS_FILE}.tmp"));
    tokio::fs::write(&tmp, serde_json::to_vec(&apps)?).await?;
    tokio::fs::rename(&tmp, &out).await?;
    tracing::info!(apps = apps.len(), "Debian's app list ready");
    Ok(apps.len())
}

fn fresh(path: &Path, max_age: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < max_age)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "---\nFile: DEP-11\nVersion: '1.0'\n---\nType: desktop-application\nID: org.kde.krita\nPackage: krita\nName:\n  de: Krita\n  C: Krita\nSummary:\n  C: Digital painting, creative freedom\n  fr: Peinture\nCategories:\n- Graphics\n- 2DGraphics\nIcon:\n  cached:\n  - name: krita.png\n---\nType: addon\nID: org.kde.krita.plugin\nPackage: krita-plugin\nName:\n  C: Plugin\n---\nType: desktop-application\nID: org.gnome.gitg\nPackage: gitg\nName:\n  C: gitg\nSummary:\n  C: 'Git''s repository viewer'\n";

    #[test]
    fn desktop_apps_are_read_with_their_english_name() {
        let apps = parse(SAMPLE);
        assert_eq!(apps.len(), 2, "add-ons are not apps");
        assert_eq!(apps[0].package, "gitg");
        assert_eq!(apps[0].summary, "Git's repository viewer");
        assert_eq!(apps[1].name, "Krita");
        assert_eq!(apps[1].summary, "Digital painting, creative freedom");
        assert_eq!(apps[1].categories, ["Graphics", "2DGraphics"]);
    }
}

#[cfg(test)]
mod real {
    /// `ILIADA_DEP11=/path/Components-amd64.yml cargo test -p iliada-ingest real`
    #[test]
    fn debians_real_list_parses() {
        let Some(p) = std::env::var_os("ILIADA_DEP11") else { return };
        let apps = super::parse(&std::fs::read_to_string(p).unwrap());
        assert!(apps.len() > 1500, "{}", apps.len());
        let krita = apps.iter().find(|a| a.package == "krita").unwrap();
        assert_eq!(krita.name, "Krita");
        assert!(krita.categories.iter().any(|c| c == "Graphics"), "{:?}", krita.categories);
        let files = apps.iter().find(|a| a.package == "nautilus").unwrap();
        assert_eq!(files.categories, ["System"]);
        eprintln!("{} apps; krita: {}", apps.len(), krita.summary);
    }
}
