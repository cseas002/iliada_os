//! Keep the app list (`/etc/iliada/apps.toml`) equal to what is installed.
//!
//! * A bundle that appears in /Applications (drag-and-drop, dropped package,
//!   translation) gets an entry, with its source when one is known, so that a
//!   restore on another computer can reinstall it.
//! * A bundle that *this machine had* and that is now gone (dragged to the
//!   Trash) loses its entry. Entries for apps never installed here (e.g. just
//!   restored from a backup and still being installed) are left alone.
//!
//! The file is edited with `toml_edit`, so the user's comments and layout
//! survive. It is rewritten in place (it lives in a root-owned directory).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use iliada_applist::{Table, entry_id, value};
use iliada_bundle::{Bundle, PackageRole};

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ListReport {
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

/// What we know about an installed bundle, for its list entry.
#[derive(Debug, Clone)]
struct Installed {
    id: String,
    name: String,
    /// Debian package name, when the bundle was translated from Debian.
    debian: Option<String>,
    /// https URL of the upstream package, for non-Debian sources.
    url: Option<String>,
}

fn installed(apps: &Path) -> Vec<Installed> {
    let mut v: Vec<Installed> = fs::read_dir(apps)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "app")).collect())
        .unwrap_or_else(|_| Vec::<PathBuf>::new())
        .into_iter()
        .filter_map(|p| Bundle::open(p).ok())
        .map(|b| {
            let (mut debian, mut url) = (None, None);
            if let Some(prov) = &b.provenance {
                let primary = prov.packages.iter().find(|p| p.role == PackageRole::Primary);
                let from_debian =
                    prov.source.url.contains("/debian/pool/") || prov.source.url.contains("deb.debian.org");
                match primary {
                    Some(p) if from_debian => debian = Some(p.name.clone()),
                    _ if prov.source.url.starts_with("https://") => url = Some(prov.source.url.clone()),
                    _ => {}
                }
            }
            Installed { id: b.info.id.to_string(), name: b.info.name.clone(), debian, url }
        })
        .collect();
    v.sort_by(|a, b| a.id.cmp(&b.id));
    v
}

pub fn sync_list(apps: &Path, list: &Path, seen_file: &Path) -> Result<ListReport> {
    let seen: BTreeSet<String> =
        fs::read(seen_file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let now = installed(apps);
    let now_ids: BTreeSet<String> = now.iter().map(|i| i.id.clone()).collect();
    let mut report = ListReport::default();

    iliada_applist::edit(list, |aot| {
        // Uninstalled here: was seen on this machine, is gone now.
        let gone: BTreeSet<&String> = seen.difference(&now_ids).collect();
        let mut i = 0;
        while i < aot.len() {
            match aot.get(i).and_then(entry_id) {
                Some(id) if gone.contains(&id) => {
                    aot.remove(i);
                    report.removed.push(id);
                }
                _ => i += 1,
            }
        }
        // Installed here but not listed: record it.
        let listed: BTreeSet<String> = aot.iter().filter_map(entry_id).collect();
        for app in now.iter().filter(|a| !listed.contains(&a.id)) {
            let mut t = Table::new();
            match (&app.debian, &app.url) {
                (Some(d), _) => t["debian"] = value(d.as_str()),
                (_, Some(u)) => t["url"] = value(u.as_str()),
                _ => t["manual"] = value(true),
            }
            t["id"] = value(app.id.as_str());
            t["name"] = value(app.name.as_str());
            aot.push(t);
            report.added.push(app.id.clone());
        }
        Ok(report != ListReport::default())
    })?;

    if now_ids != seen {
        if let Some(dir) = seen_file.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(seen_file, serde_json::to_vec(&now_ids)?)?;
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use iliada_bundle::{BundleLayout, Info, IsolationTier, PackageRecord, Permissions, Provenance, SourceRecord};
    use iliada_types::AppId;

    const DEBIAN_PREFIX: &str = "org.debian";

    fn bundle(apps: &Path, name: &str, pkg: &str, url: &str) -> PathBuf {
        let l = BundleLayout::in_dir(apps, name);
        l.create_dirs().unwrap();
        fs::write(l.executables().join("x"), "").unwrap();
        let id = AppId::from_upstream(DEBIAN_PREFIX, pkg).unwrap();
        let info = Info {
            id,
            name: name.into(),
            version: "1".into(),
            executable: "x".into(),
            entry: None,
            args: vec![],
            env: Default::default(),
            icon: None,
            runtime: "iliada-base-1".into(),
            isolation: IsolationTier::MicroVm,
            network: false,
            categories: vec![],
            mime_types: vec![],
        };
        let prov = Provenance {
            generator: "t".into(),
            created_at: 0,
            source: SourceRecord { url: url.into(), sha256: "0".into(), format: "deb".into() },
            packages: vec![PackageRecord {
                name: pkg.into(),
                version: "1".into(),
                url: url.into(),
                sha256: "0".into(),
                role: iliada_bundle::PackageRole::Primary,
            }],
            libraries: vec![],
        };
        Bundle::write_manifests(&l, &info, &Permissions::default(), Some(&prov)).unwrap();
        l.root().to_owned()
    }

    #[test]
    fn records_installs_and_trash_and_keeps_comments() {
        let d = tempfile::tempdir().unwrap();
        let (apps, list, seen) =
            (d.path().join("Applications"), d.path().join("apps.toml"), d.path().join("seen.json"));
        fs::create_dir_all(&apps).unwrap();
        fs::write(
            &list,
            "# My apps (comments survive)\ninterval_secs = 3600\n\n[[watch]]\ndebian = \"gnome-calculator\"\n\n[[watch]]\ndebian = \"restored-not-yet-installed\"\n",
        )
        .unwrap();

        // Calculator is installed (translated from Debian); a hand-dropped package too.
        bundle(&apps, "Calculator", "gnome-calculator", "https://deb.debian.org/debian/pool/main/g/gc/gc.deb");
        let dropped = bundle(&apps, "Hello", "hello", "/home/alex/Downloads/hello.deb");
        let r = sync_list(&apps, &list, &seen).unwrap();
        assert_eq!(r.added, ["org.debian.hello"], "calculator already listed; hello recorded");
        let text = fs::read_to_string(&list).unwrap();
        assert!(text.starts_with("# My apps (comments survive)"));
        assert!(text.contains("manual = true") && text.contains("name = \"Hello\""));
        assert!(text.contains("restored-not-yet-installed"), "not-yet-installed entries are kept");

        // Dragging Hello to the Trash removes its entry; nothing else changes.
        fs::remove_dir_all(&dropped).unwrap();
        let r = sync_list(&apps, &list, &seen).unwrap();
        assert_eq!(r.removed, ["org.debian.hello"]);
        let text = fs::read_to_string(&list).unwrap();
        assert!(
            !text.contains("Hello") && text.contains("gnome-calculator") && text.contains("restored-not-yet-installed")
        );

        // Idempotent.
        assert_eq!(sync_list(&apps, &list, &seen).unwrap(), ListReport::default());
    }
}
