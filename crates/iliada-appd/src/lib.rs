//! The Iliada application registry.
//!
//! Desktops find apps through freedesktop `.desktop` entries in
//! `$XDG_DATA_DIRS/applications`. `iliada-appd` keeps one exported entry per
//! GUI bundle in `/Applications`, whose `Exec` line starts the app through
//! `iliada-run`, so a click in the launcher always lands in a sandbox.
//!
//! Exported entries are named `iliada-<app-id>.desktop`; anything else in
//! the export directory is left alone. Phase 2 adds drag-to-install
//! validation, signature checks and MIME registration here.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use iliada_bundle::Bundle;

const PREFIX: &str = "iliada-";

/// Quote one argument for a desktop-entry `Exec` key (Desktop Entry spec,
/// "The Exec key"): wrap in double quotes, backslash-escape `" ` $ \`, then
/// escape backslashes again for the string-value level.
pub fn exec_quote(arg: &str) -> String {
    let mut q = String::from("\"");
    for c in arg.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            q.push('\\');
        }
        q.push(c);
    }
    q.push('"');
    q.replace('\\', "\\\\")
}

/// Strip characters that would break the key=value line format.
fn value(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).collect()
}

/// Is this a GUI app that belongs in the launcher? Command-line tools
/// (no desktop entry, no categories) are not exported.
pub fn is_gui(b: &Bundle) -> bool {
    let has_entry = fs::read_dir(b.layout.resources())
        .map(|rd| rd.flatten().any(|e| e.path().extension().is_some_and(|x| x == "desktop")))
        .unwrap_or(false);
    has_entry || !b.info.categories.is_empty()
}

pub fn desktop_entry(b: &Bundle, iliada_run: &Path) -> String {
    let mut out = String::from("[Desktop Entry]\nType=Application\n");
    out += &format!("Name={}\n", value(&b.info.name));
    out += &format!(
        "Exec={} {}\n",
        exec_quote(&iliada_run.display().to_string()),
        exec_quote(&b.layout.root().display().to_string())
    );
    if let Some(icon) = &b.info.icon {
        let p = b.layout.contents().join(icon);
        if p.is_file() {
            out += &format!("Icon={}\n", value(&p.display().to_string()));
        }
    }
    if !b.info.categories.is_empty() {
        out += &format!("Categories={};\n", value(&b.info.categories.join(";")));
    }
    if !b.info.mime_types.is_empty() {
        out += &format!("MimeType={};\n", value(&b.info.mime_types.join(";")));
    }
    out +=
        &format!("X-Iliada-AppId={}\nX-Iliada-Bundle={}\n", b.info.id, value(&b.layout.root().display().to_string()));
    out
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub written: Vec<String>,
    pub removed: Vec<String>,
}

/// Make `<export>/applications` mirror the GUI bundles in `apps`.
pub fn sync(apps: &Path, export: &Path, iliada_run: &Path) -> Result<SyncReport> {
    let dir = export.join("applications");
    fs::create_dir_all(&dir)?;
    let mut report = SyncReport::default();
    let mut keep = BTreeSet::new();
    let mut bundles: Vec<PathBuf> = fs::read_dir(apps)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "app")).collect())
        .unwrap_or_default();
    bundles.sort();
    for path in bundles {
        let Ok(b) = Bundle::open(&path) else { continue };
        if !is_gui(&b) {
            continue;
        }
        let file = format!("{PREFIX}{}.desktop", b.info.id);
        let text = desktop_entry(&b, iliada_run);
        let target = dir.join(&file);
        if fs::read_to_string(&target).ok().as_deref() != Some(text.as_str()) {
            let tmp = dir.join(format!(".{file}.tmp"));
            fs::write(&tmp, &text)?;
            fs::rename(&tmp, &target)?;
            report.written.push(file.clone());
        }
        keep.insert(file);
    }
    for e in fs::read_dir(&dir)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with(PREFIX) && name.ends_with(".desktop") && !keep.contains(&name) {
            fs::remove_file(e.path())?;
            report.removed.push(name);
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use iliada_bundle::{BundleLayout, Info, IsolationTier, Permissions};
    use iliada_types::AppId;

    fn make(apps: &Path, name: &str, id: &str, gui: bool) -> PathBuf {
        let l = BundleLayout::in_dir(apps, name);
        l.create_dirs().unwrap();
        fs::write(l.executables().join("x"), "#!/bin/sh\n").unwrap();
        fs::write(l.resources().join("icon.svg"), "<svg/>").unwrap();
        if gui {
            fs::write(l.resources().join("x.desktop"), "[Desktop Entry]\n").unwrap();
        }
        let info = Info {
            id: AppId::new(id).unwrap(),
            name: name.into(),
            version: "1".into(),
            executable: "x".into(),
            entry: Some("usr/bin/x".into()),
            icon: Some("Resources/icon.svg".into()),
            runtime: "iliada-base-1".into(),
            isolation: IsolationTier::MicroVm,
            categories: if gui { vec!["Utility".into()] } else { vec![] },
            mime_types: vec![],
        };
        Bundle::write_manifests(&l, &info, &Permissions::default(), None).unwrap();
        l.root().to_owned()
    }

    #[test]
    fn exports_gui_apps_only_and_follows_removal() {
        let d = tempfile::tempdir().unwrap();
        let (apps, export) = (d.path().join("Applications"), d.path().join("export"));
        fs::create_dir_all(&apps).unwrap();
        let calc = make(&apps, "Calculator", "org.debian.gnome-calculator", true);
        make(&apps, "jq", "org.debian.jq", false);
        let run = Path::new("/run/current-system/sw/bin/iliada-run");

        let r = sync(&apps, &export, run).unwrap();
        assert_eq!(r.written, ["iliada-org.debian.gnome-calculator.desktop"]);
        let text = fs::read_to_string(export.join("applications/iliada-org.debian.gnome-calculator.desktop")).unwrap();
        assert!(text.contains(&format!("Exec=\"/run/current-system/sw/bin/iliada-run\" \"{}\"", calc.display())));
        assert!(text.contains("Categories=Utility;"));
        assert!(text.contains("Icon=") && text.contains("icon.svg"));

        // Idempotent.
        assert_eq!(sync(&apps, &export, run).unwrap(), SyncReport::default());

        // Uninstall removes the entry; unrelated files are untouched.
        fs::write(export.join("applications/other.desktop"), "x").unwrap();
        fs::remove_dir_all(&calc).unwrap();
        let r = sync(&apps, &export, run).unwrap();
        assert_eq!(r.removed, ["iliada-org.debian.gnome-calculator.desktop"]);
        assert!(export.join("applications/other.desktop").exists());
    }

    #[test]
    fn exec_quoting_follows_the_spec() {
        assert_eq!(exec_quote("/Applications/Hello World.app"), "\"/Applications/Hello World.app\"");
        assert_eq!(exec_quote("a\"b$c"), "\"a\\\\\"b\\\\$c\"");
    }
}
