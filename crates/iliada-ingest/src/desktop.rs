//! Minimal freedesktop `.desktop` entry parsing and icon lookup.

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct DesktopEntry {
    pub file: PathBuf,
    pub name: Option<String>,
    /// Basename of the program in `Exec=`, field codes stripped.
    pub exec: Option<String>,
    pub icon: Option<String>,
    pub categories: Vec<String>,
    pub mime_types: Vec<String>,
    pub no_display: bool,
}

/// Find the most relevant desktop entry under `<root>/usr/share/applications`.
pub fn find(root: &Path, package: &str) -> Option<DesktopEntry> {
    let dir = root.join("usr/share/applications");
    let mut entries: Vec<DesktopEntry> = fs::read_dir(&dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "desktop"))
        .filter_map(|e| parse(&e.path()))
        .filter(|d| !d.no_display)
        .collect();
    entries.sort_by_key(|d| {
        let stem = d.file.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
        (!stem.contains(package), d.file.clone())
    });
    entries.into_iter().next()
}

pub fn parse(path: &Path) -> Option<DesktopEntry> {
    let text = fs::read_to_string(path).ok()?;
    let mut d = DesktopEntry { file: path.to_owned(), ..Default::default() };
    let mut in_main = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "Name" => d.name = Some(v.to_owned()),
            "Exec" => {
                d.exec = v
                    .split_whitespace()
                    .find(|t| !t.starts_with('%') && *t != "env" && !t.contains('='))
                    .map(|t| t.trim_matches('"').rsplit('/').next().unwrap_or(t).to_owned())
            }
            "Icon" => d.icon = Some(v.to_owned()),
            "Categories" => d.categories = split_list(v),
            "MimeType" => d.mime_types = split_list(v),
            "NoDisplay" => d.no_display = v.eq_ignore_ascii_case("true"),
            _ => {}
        }
    }
    Some(d)
}

fn split_list(v: &str) -> Vec<String> {
    v.split(';').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

/// Resolve an `Icon=` value to a file inside the extracted tree, preferring
/// large PNGs, then SVG, then pixmaps. Absolute values are re-rooted.
pub fn find_icon(root: &Path, icon: &str) -> Option<PathBuf> {
    if icon.starts_with('/') {
        let p = root.join(icon.trim_start_matches('/'));
        return p.is_file().then_some(p);
    }
    let sizes = ["512x512", "256x256", "128x128", "96x96", "64x64", "48x48", "scalable"];
    for size in sizes {
        for ext in ["png", "svg"] {
            let p = root.join(format!("usr/share/icons/hicolor/{size}/apps/{icon}.{ext}"));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    ["png", "svg", "xpm"]
        .iter()
        .map(|ext| root.join(format!("usr/share/pixmaps/{icon}.{ext}")))
        .find(|p| p.is_file())
        .or_else(|| opt_icon(root, icon))
}

/// Vendor packages (Google Chrome, Brave, Edge…) keep their icon under /opt
/// and install it from a script, which a translation never runs.
fn opt_icon(root: &Path, icon: &str) -> Option<PathBuf> {
    let wanted = [
        format!("{icon}.png"),
        format!("{icon}.svg"),
        "product_logo_256.png".into(),
        "product_logo_128.png".into(),
        "product_logo_64.png".into(),
        "product_logo_48.png".into(),
    ];
    let mut found = Vec::new();
    collect_files(&root.join("opt"), 4, &mut found);
    wanted.iter().find_map(|w| found.iter().find(|p| p.file_name().is_some_and(|n| n == w.as_str())).cloned())
}

fn collect_files(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() && depth > 0 => collect_files(&p, depth - 1, out),
            Ok(t) if t.is_file() => out.push(p),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exec_and_lists() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.desktop");
        fs::write(
            &p,
            "[Desktop Entry]\nName=Gimp\nExec=env FOO=1 /usr/bin/gimp-2.10 %U\nIcon=gimp\nCategories=Graphics;2DGraphics;\nMimeType=image/png;\n[Desktop Action new]\nName=Other\n",
        )
        .unwrap();
        let d = parse(&p).unwrap();
        assert_eq!(d.name.as_deref(), Some("Gimp"));
        assert_eq!(d.exec.as_deref(), Some("gimp-2.10"));
        assert_eq!(d.categories, ["Graphics", "2DGraphics"]);
        assert_eq!(d.mime_types, ["image/png"]);
    }

    #[test]
    fn finds_a_vendor_icon_under_opt() {
        // Google Chrome: Icon=google-chrome, the logo only in /opt.
        let root = tempfile::tempdir().unwrap();
        let chrome = root.path().join("opt/google/chrome");
        fs::create_dir_all(&chrome).unwrap();
        for size in [48, 256] {
            fs::write(chrome.join(format!("product_logo_{size}.png")), "png").unwrap();
        }
        assert_eq!(find_icon(root.path(), "google-chrome"), Some(chrome.join("product_logo_256.png")));
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(find_icon(empty.path(), "google-chrome"), None);
    }
}
