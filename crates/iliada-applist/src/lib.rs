//! The app list: `/etc/iliada/apps.toml`.
//!
//! One `[[watch]]` table per app. The keys say where an app comes from:
//!
//! | keys | meaning | who installs/updates it |
//! |---|---|---|
//! | `store = "<catalog id>"` | from the Iliada Store | `iliada-store sync` (download) |
//! | `debian = "<pkg>"` / `url = "<deb>"` | custom translation | `iliada-ingest serve` (local) |
//! | `manual = true` | dragged in by hand | nobody (recorded only) |
//!
//! plus `id` (the installed bundle's app id) and `name`. Several programs edit
//! this file (the Store, `iliada-appd`, the `iliada` command), so every edit
//! takes an exclusive lock, and the file is edited with `toml_edit` so the
//! user's comments and layout survive.

use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use anyhow::{Context, Result};
use iliada_types::AppId;
pub use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

const DEBIAN_PREFIX: &str = "org.debian";

/// The app id an entry refers to.
pub fn entry_id(t: &Table) -> Option<String> {
    if let Some(id) = t.get("id").and_then(Item::as_str) {
        return Some(id.to_owned());
    }
    let d = t.get("debian").and_then(Item::as_str)?;
    AppId::from_upstream(DEBIAN_PREFIX, d).ok().map(|i| i.to_string())
}

/// `(store id, app id)` for every Store-managed entry.
pub fn store_entries(path: &Path) -> Result<Vec<(String, Option<String>)>> {
    let doc: DocumentMut = fs::read_to_string(path).unwrap_or_default().parse()?;
    Ok(doc
        .get("watch")
        .and_then(Item::as_array_of_tables)
        .into_iter()
        .flat_map(|a| a.iter())
        .filter_map(|t| Some((t.get("store")?.as_str()?.to_owned(), entry_id(t))))
        .collect())
}

/// Lock the list, let `f` edit it, and write it back if `f` reports a change.
/// The file is rewritten in place (it may live in a directory only root can
/// write to).
pub fn edit(path: &Path, f: impl FnOnce(&mut ArrayOfTables) -> Result<bool>) -> Result<bool> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    file.lock().with_context(|| format!("locking {}", path.display()))?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let mut doc: DocumentMut = text.parse().with_context(|| format!("parsing {}", path.display()))?;
    if !doc.contains_key("watch") {
        doc["watch"] = Item::ArrayOfTables(ArrayOfTables::new());
    }
    let aot = doc["watch"].as_array_of_tables_mut().context("`watch` must be an array of tables")?;
    let changed = f(aot)?;
    if changed {
        file.set_len(0)?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(doc.to_string().as_bytes())?;
        file.flush()?;
    }
    Ok(changed)
}

/// Record that the Store installed `app_id` (catalog id `store_id`). An
/// existing entry for the same app is converted to a Store entry.
pub fn record_store_install(path: &Path, app_id: &str, name: &str, store_id: &str) -> Result<bool> {
    edit(path, |aot| {
        if let Some(t) = aot.iter_mut().find(|t| entry_id(t).as_deref() == Some(app_id)) {
            if t.get("store").and_then(Item::as_str) == Some(store_id) {
                return Ok(false);
            }
            for k in ["debian", "url", "manual"] {
                t.remove(k);
            }
            t["store"] = value(store_id);
            t["id"] = value(app_id);
            return Ok(true);
        }
        let mut t = Table::new();
        t["store"] = value(store_id);
        t["id"] = value(app_id);
        t["name"] = value(name);
        aot.push(t);
        Ok(true)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_installs_are_recorded_and_convert_entries() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("apps.toml");
        fs::write(&p, "# mine\n[[watch]]\ndebian = \"gnome-calculator\"\n").unwrap();
        assert!(record_store_install(&p, "org.debian.firefox-esr", "Firefox", "org.debian.firefox-esr").unwrap());
        assert!(
            record_store_install(&p, "org.debian.gnome-calculator", "Calculator", "org.debian.gnome-calculator")
                .unwrap()
        );
        assert!(!record_store_install(&p, "org.debian.firefox-esr", "Firefox", "org.debian.firefox-esr").unwrap());
        let text = fs::read_to_string(&p).unwrap();
        assert!(text.starts_with("# mine"));
        assert!(!text.contains("debian = "), "converted to a Store entry");
        let s = store_entries(&p).unwrap();
        assert_eq!(s.len(), 2);
        assert!(s.contains(&("org.debian.firefox-esr".into(), Some("org.debian.firefox-esr".into()))));
    }
}
