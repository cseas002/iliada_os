//! Apps that aren't in the Iliada Store yet: Iliada builds them from
//! Debian's package on this computer (the Package Translator,
//! `iliada-ingest serve`), as an ordinary sandboxed `.app`.
//!
//! Asking for one adds `debian = "<package>"` to the app list; the
//! translator notices within seconds, builds it and reports how it went in
//! its status file. Most apps work this way; some don't (they need things a
//! sandboxed app doesn't get), and the status says so.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, bail};
use iliada_applist::value;
pub use iliada_ingest::appstream::DebianApp;
pub use iliada_ingest::serve::{BuildState, BuildStatus};

/// The translator's cache folder (its app list and statuses are readable by
/// everyone). `ILIADA_INGEST_CACHE` overrides it (development).
pub fn cache_dir() -> PathBuf {
    std::env::var_os("ILIADA_INGEST_CACHE").map(PathBuf::from).unwrap_or_else(|| "/var/cache/iliada-ingest".into())
}

/// Desktop apps Debian has (empty until the translator has fetched the list).
pub fn debian_apps() -> Vec<DebianApp> {
    std::fs::read(cache_dir().join(iliada_ingest::appstream::APPS_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// The name people type for an app: "GNU Image Manipulation Program" is
/// `gnu-image-manipulation-program`.
fn short(name: &str) -> String {
    name.to_lowercase().replace(' ', "-")
}

/// The app someone means: its package, its name, or its name with dashes.
pub fn find<'a>(apps: &'a [DebianApp], typed: &str) -> Option<&'a DebianApp> {
    let t = typed.trim().to_lowercase();
    apps.iter()
        .find(|a| a.package == t)
        .or_else(|| apps.iter().find(|a| a.name.to_lowercase() == t || short(&a.name) == t))
}

/// Apps whose name, package or summary mention `word` (names first).
pub fn search<'a>(apps: &'a [DebianApp], word: &str) -> Vec<&'a DebianApp> {
    let w = word.trim().to_lowercase();
    if w.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(u8, &DebianApp)> = apps
        .iter()
        .filter_map(|a| {
            let rank = if a.name.to_lowercase().contains(&w) || a.package.contains(&w) {
                0
            } else if a.summary.to_lowercase().contains(&w) {
                1
            } else {
                return None;
            };
            Some((rank, a))
        })
        .collect();
    hits.sort_by_key(|(rank, a)| (*rank, a.name.len()));
    hits.into_iter().map(|(_, a)| a).collect()
}

fn key(package: &str) -> String {
    format!("debian:{package}")
}

/// Where building `package` stands (`None`: never asked for).
pub fn status(package: &str) -> Option<BuildStatus> {
    iliada_ingest::serve::read_status(&cache_dir()).remove(&key(package))
}

/// Asks the translator to build `package`: adds it to the app list.
/// `false`: it was asked for already.
pub fn request(app_list: &Path, package: &str) -> Result<bool> {
    let valid = !package.is_empty()
        && package.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+-.".contains(c));
    if !valid {
        bail!("{package:?} is not a Debian package name");
    }
    iliada_applist::edit(app_list, |aot| {
        if aot.iter().any(|t| t.get("debian").and_then(|v| v.as_str()) == Some(package)) {
            return Ok(false);
        }
        let mut t = iliada_applist::Table::new();
        t["debian"] = value(package);
        aot.push(t);
        Ok(true)
    })
}

/// Waits until `package` is built (or fails), reporting each new state.
/// Gives up after `limit`.
pub async fn wait(package: &str, limit: Duration, on_state: &(dyn Fn(BuildState) + Sync)) -> Result<()> {
    let start = std::time::Instant::now();
    let mut last = None;
    // A status older than the request belongs to an earlier try.
    let asked = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
        .saturating_sub(5);
    loop {
        if let Some(s) = status(package).filter(|s| s.time >= asked) {
            if last != Some(s.state) {
                on_state(s.state);
                last = Some(s.state);
            }
            match s.state {
                BuildState::Installed => return Ok(()),
                BuildState::Failed => {
                    bail!("{}", explain(s.error.as_deref().unwrap_or("it could not be built")))
                }
                _ => {}
            }
        }
        if start.elapsed() > limit {
            bail!("it is taking long; it keeps building in the background and appears in Applications when ready");
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

/// The translator's error, in words a person can use.
pub fn explain(error: &str) -> String {
    if error.contains("not in index") {
        return "Debian has no package by that name".into();
    }
    if error.contains("fetching") || error.contains("dns") || error.contains("connect") {
        return "Debian's servers could not be reached; check the internet connection".into();
    }
    format!("it could not be made into an app ({error})")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(package: &str, name: &str, summary: &str) -> DebianApp {
        DebianApp {
            package: package.into(),
            id: format!("org.example.{package}"),
            name: name.into(),
            summary: summary.into(),
            categories: vec![],
        }
    }

    #[test]
    fn apps_are_found_by_package_name_or_dashed_name() {
        let apps = vec![
            app("krita", "Krita", "Digital painting"),
            app("gimp", "GNU Image Manipulation Program", "Edit images"),
        ];
        assert_eq!(find(&apps, "krita").unwrap().package, "krita");
        assert_eq!(find(&apps, "Krita").unwrap().package, "krita");
        assert_eq!(find(&apps, "gnu-image-manipulation-program").unwrap().package, "gimp");
        assert!(find(&apps, "photoshop").is_none());
        let hits = search(&apps, "paint");
        assert_eq!(hits.len(), 1);
        assert!(search(&apps, "").is_empty());
    }

    #[test]
    fn requests_add_one_entry_and_keep_the_file() {
        let p = std::env::temp_dir().join(format!("iliada-translate-{}.toml", std::process::id()));
        std::fs::write(&p, "# my apps\n[[watch]]\nstore = \"org.debian.vlc\"\n").unwrap();
        assert!(request(&p, "krita").unwrap());
        assert!(!request(&p, "krita").unwrap(), "asked once");
        assert!(request(&p, "Krita; rm -rf /").is_err());
        let text = std::fs::read_to_string(&p).unwrap();
        std::fs::remove_file(&p).unwrap();
        assert!(text.starts_with("# my apps"));
        assert_eq!(text.matches("debian = \"krita\"").count(), 1);
    }

    #[test]
    fn errors_read_plainly() {
        assert_eq!(explain("krita not in index"), "Debian has no package by that name");
        assert!(explain("odd").contains("could not be made into an app"));
    }
}
