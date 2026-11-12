//! Growing the catalog by itself: the most used Debian desktop apps that
//! have no recipe yet, each with a recipe written from Debian's own
//! description of it. The build server builds them, launch-tests them, and
//! publishes only the ones that start (`iliada_apps/scripts/grow.sh`).

use std::collections::{HashMap, HashSet};

use iliada_ingest::appstream::DebianApp;

/// Regular users per package, from Debian's popularity contest
/// (`popcon.debian.org/by_inst`: rank, name, inst, vote, …).
pub fn popcon(text: &str) -> HashMap<String, u64> {
    text.lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let (_rank, name, _inst, vote) = (f.next()?, f.next()?, f.next()?, f.next()?);
            Some((name.to_owned(), vote.parse().ok()?))
        })
        .collect()
}

/// The Store category for AppStream categories.
pub fn category(cats: &[String]) -> &'static str {
    let has = |c: &str| cats.iter().any(|x| x == c);
    if has("Game") {
        "Games"
    } else if has("Office") {
        "Office"
    } else if has("Graphics") {
        "Graphics"
    } else if has("AudioVideo") || has("Audio") || has("Video") {
        "Media"
    } else if has("Network") {
        "Internet"
    } else if has("Development") {
        "Development"
    } else {
        "Utilities"
    }
}

/// Apps a sandboxed .app can't be: they manage the system, or are part of
/// another desktop.
fn unsuitable(app: &DebianApp) -> bool {
    const SYSTEM: [&str; 6] = ["System", "Settings", "PackageManager", "TerminalEmulator", "Core", "DesktopSettings"];
    // Popular, but they manage disks or backups, capture the screen, live
    // in a terminal, or belong to another desktop: not sandboxed apps.
    const DENY: [&str; 9] = [
        "gnome-disk-utility",
        "deja-dup",
        "spectacle",
        "kde-spectacle",
        "mc",
        "xwaylandvideobridge",
        "plasma-welcome",
        "gnome-system-monitor",
        "ktnef",
    ];
    app.categories.iter().any(|c| SYSTEM.contains(&c.as_str()))
        || DENY.contains(&app.package.as_str())
        || app.package.starts_with("imagemagick")
        || app.package.starts_with("cosmic-")
        || app.package.ends_with("-common")
        || app.package.contains("-settings")
}

/// A recipe file stem from the app's name ("Krita" → "krita").
pub fn stem(name: &str) -> String {
    let s: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    s.chars().take(40).collect()
}

/// The recipe for an app (see iliada_apps/README.md).
pub fn recipe(app: &DebianApp) -> String {
    let cat = category(&app.categories);
    let has = |c: &str| app.categories.iter().any(|x| x == c);
    let network = has("Network") || has("WebBrowser") || has("Email") || has("Chat") || has("InstantMessaging");
    let folders: Vec<&str> = match cat {
        "Graphics" => vec!["pictures"],
        "Office" => vec!["documents"],
        "Media" if has("Audio") && !has("Video") => vec!["music"],
        "Media" => vec!["videos", "music"],
        _ if network => vec!["downloads"],
        _ => vec![],
    };
    let summary = app.summary.trim_end_matches('.').replace('"', "'");
    let mut t = format!(
        "# Added by the catalog builder from Debian's description (iliada-catalog suggest).\n\
         name = \"{}\"\nsummary = \"{}\"\ncategory = \"{cat}\"\nsource = {{ debian = \"{}\" }}\n",
        app.name.replace('"', "'"),
        summary,
        app.package
    );
    if network {
        t.push_str("network = true\n");
    }
    if !folders.is_empty() {
        t.push_str(&format!(
            "folders = [{}]\n",
            folders.iter().map(|f| format!("\"{f}\"")).collect::<Vec<_>>().join(", ")
        ));
    }
    t
}

/// The `count` most used apps not yet in the catalog (`skip`: packages
/// that have a recipe or were tried and rejected), as (stem, recipe).
pub fn suggest(
    apps: &[DebianApp],
    users: &HashMap<String, u64>,
    skip: &HashSet<String>,
    count: usize,
) -> Vec<(String, String)> {
    let mut candidates: Vec<(&DebianApp, u64)> = apps
        .iter()
        .filter(|a| !skip.contains(&a.package) && !unsuitable(a))
        .filter_map(|a| users.get(&a.package).map(|u| (a, *u)))
        .collect();
    candidates.sort_by_key(|(a, u)| (std::cmp::Reverse(*u), a.package.clone()));
    let (mut stems, mut packages) = (HashSet::new(), HashSet::new());
    candidates
        .into_iter()
        .filter(|(a, _)| packages.insert(a.package.clone()) && stems.insert(stem(&a.name)))
        .take(count)
        .map(|(a, _)| (stem(&a.name), recipe(a)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(package: &str, name: &str, cats: &[&str]) -> DebianApp {
        DebianApp {
            package: package.into(),
            id: format!("org.example.{package}"),
            name: name.into(),
            summary: "Does \"things\".".into(),
            categories: cats.iter().map(|c| c.to_string()).collect(),
        }
    }

    #[test]
    fn popcon_lines_are_read() {
        let p = popcon("#rank name inst vote\n1     dpkg   297579 270863  9157\n2 krita 5000 1200 3 4 5 (KDE)\n");
        assert_eq!(p["krita"], 1200);
        assert_eq!(p.len(), 2);
    }

    #[test]
    fn the_most_used_suitable_apps_come_first() {
        let apps = [
            app("krita", "Krita", &["Graphics"]),
            app("gparted", "GParted", &["System"]),
            app("gimp", "GIMP", &["Graphics"]),
            app("pidgin", "Pidgin", &["Network", "InstantMessaging"]),
            app("nopop", "No Popcon", &["Utility"]),
        ];
        let users = HashMap::from([
            ("krita".to_owned(), 100),
            ("gparted".to_owned(), 999),
            ("gimp".to_owned(), 500),
            ("pidgin".to_owned(), 300),
        ]);
        let skip = HashSet::from(["gimp".to_owned()]);
        let s = suggest(&apps, &users, &skip, 5);
        let stems: Vec<_> = s.iter().map(|(st, _)| st.as_str()).collect();
        assert_eq!(stems, ["pidgin", "krita"], "gparted manages the system; gimp has a recipe");
        assert!(s[0].1.contains("network = true") && s[0].1.contains("category = \"Internet\""));
        assert!(s[1].1.contains("folders = [\"pictures\"]"));
        assert!(s[1].1.contains("summary = \"Does 'things'\""));
        let parsed: toml::Value = toml::from_str(&s[0].1).unwrap();
        assert_eq!(parsed["source"]["debian"].as_str(), Some("pidgin"));
    }

    #[test]
    fn stems_are_file_names() {
        assert_eq!(stem("GNU Image Manipulation Program"), "gnu-image-manipulation-program");
        assert_eq!(stem("Kdenlive (video)"), "kdenlive-video");
    }
}
