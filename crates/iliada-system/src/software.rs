//! What the Store offers besides apps: **software** (command-line programs,
//! installed system-wide from nixpkgs: Python, Node.js, ffmpeg…) and
//! **services** (programs that run in the background: a web server, a
//! database). Neither is an `.app`: they are part of the system, installed
//! through the system service, recorded in settings.toml and backed up.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// Command-line software (`[tools] packages`).
    Software,
    /// A background service (`[services]`).
    Service,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Item {
    /// nixpkgs attribute (software) or service name (services).
    pub id: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    pub kind: Kind,
    pub category: &'static str,
}

const fn sw(id: &'static str, name: &'static str, summary: &'static str, category: &'static str) -> Item {
    Item { id, name, summary, kind: Kind::Software, category }
}

const fn svc(id: &'static str, name: &'static str, summary: &'static str, category: &'static str) -> Item {
    Item { id, name, summary, kind: Kind::Service, category }
}

pub const CATALOG: &[Item] = &[
    sw("python3", "Python", "The Python programming language", "Programming"),
    sw("nodejs", "Node.js", "JavaScript outside the browser, with npm", "Programming"),
    sw("go", "Go", "The Go programming language", "Programming"),
    sw("rustup", "Rust (rustup)", "The Rust toolchain installer", "Programming"),
    sw("gcc", "GCC", "C and C++ compilers", "Programming"),
    sw("gnumake", "Make", "Build automation", "Programming"),
    sw("cmake", "CMake", "Cross-platform build system", "Programming"),
    sw("git", "Git", "Version control", "Developer tools"),
    sw("gh", "GitHub CLI", "GitHub from the terminal", "Developer tools"),
    sw("jq", "jq", "Process JSON", "Developer tools"),
    sw("ripgrep", "ripgrep", "Search inside files, fast", "Developer tools"),
    sw("vim", "Vim", "Text editor for the terminal", "Editors"),
    sw("neovim", "Neovim", "Modern Vim", "Editors"),
    sw("helix", "Helix", "Modal text editor", "Editors"),
    sw("htop", "htop", "See what is running", "System"),
    sw("ffmpeg", "FFmpeg", "Convert audio and video", "Media"),
    sw("imagemagick", "ImageMagick", "Convert and edit images", "Media"),
    sw("yt-dlp", "yt-dlp", "Download videos", "Media"),
    sw("wget", "Wget", "Download files", "Network"),
    sw("curl", "curl", "Transfer data with URLs", "Network"),
    svc("ssh", "SSH server", "Log in to this computer from another one", "Remote access"),
    svc("printing", "Printing", "Print to printers on your network", "Devices"),
    svc("nginx", "nginx", "Web server, serving /srv/www on this computer", "Web"),
    svc("postgresql", "PostgreSQL", "SQL database", "Databases"),
    svc("mariadb", "MariaDB", "MySQL-compatible database", "Databases"),
    svc("redis", "Redis", "In-memory data store", "Databases"),
    svc("docker", "Docker", "Run containers (for administrators)", "Developer tools"),
];

pub fn find(id: &str) -> Option<&'static Item> {
    CATALOG.iter().find(|i| i.id == id)
}

/// Everyday names for catalog items ("python" is `python3`).
const ALIASES: &[(&str, &str)] = &[
    ("python", "python3"),
    ("pip", "python3"),
    ("node", "nodejs"),
    ("npm", "nodejs"),
    ("golang", "go"),
    ("rust", "rustup"),
    ("cargo", "rustup"),
    ("make", "gnumake"),
    ("nvim", "neovim"),
    ("hx", "helix"),
    ("rg", "ripgrep"),
    ("youtube-dl", "yt-dlp"),
    ("sshd", "ssh"),
    ("openssh", "ssh"),
    ("cups", "printing"),
    ("postgres", "postgresql"),
    ("mysql", "mariadb"),
];

/// The catalog item a person means by `name`: its id, its shown name or an
/// everyday name, in any case.
pub fn lookup(name: &str) -> Option<&'static Item> {
    let n = name.trim().to_lowercase();
    let id = ALIASES.iter().find(|(a, _)| *a == n).map(|(_, id)| *id).unwrap_or(&n);
    find(id).or_else(|| CATALOG.iter().find(|i| i.name.to_lowercase() == n))
}

/// Catalog items whose id, name, summary or category mention `word`.
pub fn search(word: &str) -> Vec<&'static Item> {
    let w = word.trim().to_lowercase();
    let exact = lookup(&w);
    CATALOG
        .iter()
        .filter(|i| {
            Some(*i) == exact || [i.id, i.name, i.summary, i.category].iter().any(|f| f.to_lowercase().contains(&w))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_services_match_settings() {
        let mut ids: Vec<_> = CATALOG.iter().map(|i| i.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), CATALOG.len());
        for i in CATALOG.iter().filter(|i| i.kind == Kind::Software) {
            assert!(crate::settings::valid_tool_name(i.id), "{}", i.id);
        }
        let services: Vec<_> = crate::settings::SERVICES.iter().map(|(n, _)| *n).collect();
        for i in CATALOG.iter().filter(|i| i.kind == Kind::Service) {
            assert!(services.contains(&i.id), "{} is not a known service", i.id);
        }
        for (alias, id) in ALIASES {
            assert!(find(id).is_some(), "{alias} points at {id}, which is not in the catalog");
        }
    }

    #[test]
    fn everyday_names_find_the_software() {
        assert_eq!(lookup("python").unwrap().id, "python3");
        assert_eq!(lookup("Python").unwrap().id, "python3");
        assert_eq!(lookup("python3").unwrap().id, "python3");
        assert_eq!(lookup("Node.js").unwrap().id, "nodejs");
        assert_eq!(lookup("postgres").unwrap().id, "postgresql");
        assert!(lookup("firefox").is_none());
        assert!(search("database").iter().any(|i| i.id == "postgresql"));
        assert!(search("python").iter().any(|i| i.id == "python3"));
    }
}
