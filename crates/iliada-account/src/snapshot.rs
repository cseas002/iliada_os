//! What a person's backup holds, packed as one compressed tar file (sealed
//! by the caller before it leaves the computer), and unpacked again into a
//! home folder.
//!
//! Only the parts in [`PARTS`] are packed, and unpacking writes nowhere
//! else: no links, no `..`, no other folder, whatever the file says.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// What is backed up, relative to the home folder, and what people call it.
pub const PARTS: [(&str, &str); 3] = [
    (".config/cosmic", "Desktop: appearance, dock and panel, wallpaper, shortcuts"),
    (".local/share/iliada/permissions.toml", "Privacy: what each app may use"),
    (".local/share/iliada/conversations", "Assistant conversations"),
];
/// Bigger files are left out (settings are small; this skips stray caches).
const MAX_FILE: u64 = 8 << 20;
/// Most a backup may unpack to.
const MAX_UNPACKED: u64 = 512 << 20;
const MANIFEST: &str = "iliada-backup.json";

/// Where a backup came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// The computer's name.
    pub computer: String,
    /// Seconds since 1970.
    pub created: u64,
    /// Iliada's version on that computer.
    pub iliada: String,
}

#[derive(Debug)]
pub struct Packed {
    /// zstd-compressed tar.
    pub bytes: Vec<u8>,
    /// Of the files only (not the manifest): unchanged files, same digest.
    pub digest: String,
    pub files: usize,
}

fn is_backed_up(rel: &Path) -> bool {
    PARTS.iter().any(|(p, _)| rel.starts_with(p))
}

/// The backed-up files under `home`, in a stable order.
fn collect(home: &Path) -> Result<Vec<PathBuf>> {
    fn walk(home: &Path, rel: PathBuf, out: &mut Vec<PathBuf>) -> Result<()> {
        let Ok(m) = std::fs::symlink_metadata(home.join(&rel)) else { return Ok(()) };
        if m.is_dir() {
            let mut names: Vec<_> = std::fs::read_dir(home.join(&rel))?.flatten().map(|e| e.file_name()).collect();
            names.sort();
            for n in names {
                walk(home, rel.join(n), out)?;
            }
        } else if m.is_file() && m.len() <= MAX_FILE {
            out.push(rel);
        }
        Ok(())
    }
    let mut out = Vec::new();
    for (p, _) in PARTS {
        walk(home, PathBuf::from(p), &mut out)?;
    }
    Ok(out)
}

pub fn pack(home: &Path, manifest: &Manifest) -> Result<Packed> {
    use std::os::unix::fs::PermissionsExt;
    let files = collect(home)?;
    let mut digest = Sha256::new();
    let mut tar = tar::Builder::new(Vec::new());
    let m = serde_json::to_vec_pretty(manifest)?;
    tar.append_data(&mut header(m.len() as u64, 0o600, 0), MANIFEST, m.as_slice())?;
    for rel in &files {
        let path = home.join(rel);
        let Ok(data) = std::fs::read(&path) else { continue }; // gone meanwhile
        let meta = std::fs::metadata(&path)?;
        let mtime =
            meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        let name = rel.to_str().context("a file name that isn't text")?;
        digest.update(name.as_bytes());
        digest.update([0]);
        digest.update((data.len() as u64).to_le_bytes());
        digest.update(&data);
        tar.append_data(
            &mut header(data.len() as u64, meta.permissions().mode() & 0o777 | 0o600, mtime),
            rel,
            data.as_slice(),
        )?;
    }
    let tar = tar.into_inner()?;
    Ok(Packed {
        bytes: zstd::encode_all(tar.as_slice(), 9)?,
        digest: hex::encode(digest.finalize()),
        files: files.len(),
    })
}

fn header(size: u64, mode: u32, mtime: u64) -> tar::Header {
    let mut h = tar::Header::new_gnu();
    h.set_entry_type(tar::EntryType::Regular);
    h.set_size(size);
    h.set_mode(mode);
    h.set_mtime(mtime);
    h
}

/// Reads only the manifest.
pub fn manifest(bytes: &[u8]) -> Result<Manifest> {
    let mut found = None;
    entries(bytes, |path, _, data| {
        if path == Path::new(MANIFEST) {
            found = Some(serde_json::from_slice(data)?);
        }
        Ok(())
    })?;
    found.context("this backup has no description")
}

/// Calls `f(path, mode, contents)` for each file, after checking the archive
/// as a whole (size, kinds of entries).
fn entries(bytes: &[u8], mut f: impl FnMut(&Path, u32, &[u8]) -> Result<()>) -> Result<()> {
    let mut tar = Vec::new();
    zstd::stream::read::Decoder::new(bytes)?.take(MAX_UNPACKED + 1).read_to_end(&mut tar)?;
    if tar.len() as u64 > MAX_UNPACKED {
        bail!("this backup is too big to restore");
    }
    let mut archive = tar::Archive::new(tar.as_slice());
    for e in archive.entries()? {
        let mut e = e?;
        if e.header().entry_type() != tar::EntryType::Regular {
            continue; // folders are made as needed; links are never restored
        }
        let path = e.path()?.into_owned();
        let mode = e.header().mode()?;
        let mut data = Vec::new();
        e.read_to_end(&mut data)?;
        f(&path, mode, &data)?;
    }
    Ok(())
}

/// Writes the backup's files into `home`; files it doesn't have are left as
/// they are. Nothing is written unless every file in it is one it may hold.
/// Returns its manifest and how many files were written.
pub fn unpack(home: &Path, bytes: &[u8]) -> Result<(Manifest, usize)> {
    use std::os::unix::fs::PermissionsExt;
    let manifest = manifest(bytes)?;
    let mut files = Vec::new();
    entries(bytes, |rel, mode, data| {
        if rel == Path::new(MANIFEST) {
            return Ok(());
        }
        if !rel.components().all(|c| matches!(c, Component::Normal(_))) || !is_backed_up(rel) {
            bail!("this backup has a file it shouldn't: {}", rel.display());
        }
        files.push((rel.to_owned(), mode, data.to_vec()));
        Ok(())
    })?;
    for (rel, mode, data) in &files {
        let dest = home.join(rel);
        let dir = dest.parent().expect("inside home");
        std::fs::create_dir_all(dir)?;
        // Written beside it and renamed over it: programs watching the file
        // (COSMIC does) see the whole new file at once, and a link there is
        // replaced rather than followed.
        let tmp = dir.join(format!(".{}.restoring", dest.file_name().expect("a file").to_string_lossy()));
        std::fs::write(&tmp, data)?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode & 0o777 | 0o600))?;
        std::fs::rename(&tmp, &dest)?;
    }
    Ok((manifest, files.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(home: &Path, rel: &str, text: &str) {
        let p = home.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn manifest() -> Manifest {
        Manifest { computer: "laptop".into(), created: 1, iliada: "0.2.2".into() }
    }

    #[test]
    fn packs_only_settings_and_restores_them() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        put(a.path(), ".config/cosmic/com.system76.CosmicTheme.Mode/v1/is_dark", "true");
        put(a.path(), ".local/share/iliada/permissions.toml", "[x]\ncamera = \"deny\"\n");
        put(a.path(), ".local/share/iliada/conversations/1.json", "{}");
        put(a.path(), ".config/iliada/openrouter-key", "secret");
        put(a.path(), ".local/share/iliada/assistant-spend.json", "{}");
        std::os::unix::fs::symlink("/etc/passwd", a.path().join(".config/cosmic/link")).unwrap();
        let p = pack(a.path(), &manifest()).unwrap();
        assert_eq!(p.files, 3);

        put(b.path(), ".config/cosmic/com.system76.CosmicTheme.Mode/v1/is_dark", "false");
        put(b.path(), ".config/cosmic/only-here", "kept");
        let (m, n) = unpack(b.path(), &p.bytes).unwrap();
        assert_eq!((m, n), (manifest(), 3));
        let read = |rel: &str| std::fs::read_to_string(b.path().join(rel)).ok();
        assert_eq!(read(".config/cosmic/com.system76.CosmicTheme.Mode/v1/is_dark").as_deref(), Some("true"));
        assert_eq!(read(".config/cosmic/only-here").as_deref(), Some("kept"));
        assert_eq!(read(".local/share/iliada/conversations/1.json").as_deref(), Some("{}"));
        assert_eq!(read(".config/iliada/openrouter-key"), None, "keys are never backed up");
        assert!(!b.path().join(".config/cosmic/link").exists());

        // Unchanged files, same digest; a change, a new one.
        assert_eq!(pack(a.path(), &Manifest { created: 2, ..manifest() }).unwrap().digest, p.digest);
        put(a.path(), ".local/share/iliada/permissions.toml", "");
        assert_ne!(pack(a.path(), &manifest()).unwrap().digest, p.digest);
    }

    #[test]
    fn refuses_files_outside_the_backed_up_parts() {
        for bad in ["../evil", ".bashrc", ".config/cosmic/../../.bashrc"] {
            let mut tar = tar::Builder::new(Vec::new());
            let m = serde_json::to_vec(&manifest()).unwrap();
            tar.append_data(&mut header(m.len() as u64, 0o600, 0), MANIFEST, m.as_slice()).unwrap();
            // Written by hand: the tar crate itself refuses `..` in names.
            let mut h = header(1, 0o600, 0);
            h.as_gnu_mut().unwrap().name[..bad.len()].copy_from_slice(bad.as_bytes());
            h.set_cksum();
            tar.append(&h, b"x".as_slice()).unwrap();
            let bytes = zstd::encode_all(tar.into_inner().unwrap().as_slice(), 1).unwrap();
            let home = tempfile::tempdir().unwrap();
            assert!(unpack(&home.path().join("h"), &bytes).is_err(), "{bad}");
            assert!(!home.path().join("evil").exists() && !home.path().join("h/.bashrc").exists());
        }
    }
}
