//! Path-checked extraction of untrusted tar streams.
//!
//! Threat model: the tar comes from an arbitrary upstream. It may contain
//! absolute paths, `..` components, symlinks that point outside the
//! destination, entries that *write through* a previously planted symlink,
//! hard links to host files, device nodes, setuid bits, or simply be huge.
//!
//! Defences (all enforced here, independent of the OS sandbox):
//! * every entry path is made relative and must not contain `..`;
//! * no path component below the destination root may be a symlink when we
//!   create or write something under it — so a planted symlink can never
//!   redirect a later write;
//! * symlink targets are checked *lexically*: absolute targets are re-rooted
//!   at the bundle root (that is how they resolve inside the micro-VM), and
//!   neither form may climb above the root;
//! * hard links are materialised as copies of files already extracted;
//! * device nodes / FIFOs are skipped; permission bits are masked to 0o755;
//! * total bytes and entry count are capped.

use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone, Copy)]
pub struct ExtractLimits {
    pub max_total_bytes: u64,
    pub max_entries: u64,
}

impl Default for ExtractLimits {
    fn default() -> Self {
        Self { max_total_bytes: 4 << 30, max_entries: 250_000 }
    }
}

#[derive(Debug, Default, Clone)]
pub struct ExtractReport {
    pub files: u64,
    pub dirs: u64,
    pub symlinks: u64,
    pub bytes: u64,
    /// Entries deliberately not extracted (device nodes, FIFOs, …).
    pub skipped: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("entry {0:?} has an unsafe path (absolute or contains '..')")]
    UnsafePath(PathBuf),
    #[error("entry {0:?} would be written through a symlink")]
    TraversesSymlink(PathBuf),
    #[error("symlink {link:?} -> {target:?} escapes the bundle root")]
    EscapingSymlink { link: PathBuf, target: PathBuf },
    #[error("hard link {link:?} -> {target:?} does not point to an extracted regular file")]
    BadHardlink { link: PathBuf, target: PathBuf },
    #[error("archive exceeds limit: {0}")]
    Limit(&'static str),
    #[error("i/o error on {path:?}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("malformed tar: {0}")]
    Tar(#[source] io::Error),
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> ExtractError + '_ {
    move |source| ExtractError::Io { path: path.to_owned(), source }
}

/// Normalise an archive path to a safe relative path. `Ok(None)` means the
/// entry is the archive root itself (`./`).
pub fn sanitize(raw: &Path) -> Result<Option<PathBuf>, ExtractError> {
    let mut out = PathBuf::new();
    for c in raw.components() {
        match c {
            Component::CurDir => {}
            Component::Normal(p) => out.push(p),
            Component::RootDir | Component::Prefix(_) | Component::ParentDir => {
                return Err(ExtractError::UnsafePath(raw.to_owned()));
            }
        }
    }
    Ok((!out.as_os_str().is_empty()).then_some(out))
}

/// Does a symlink at `link_rel` with `target` stay inside the root?
pub fn symlink_stays_inside(link_rel: &Path, target: &Path) -> bool {
    let mut depth: Vec<&std::ffi::OsStr> = Vec::new();
    if !target.is_absolute() {
        if let Some(parent) = link_rel.parent() {
            for c in parent.components() {
                if let Component::Normal(p) = c {
                    depth.push(p);
                }
            }
        }
    }
    for c in target.components() {
        match c {
            Component::RootDir | Component::Prefix(_) | Component::CurDir => {}
            Component::ParentDir => {
                if depth.pop().is_none() {
                    return false;
                }
            }
            Component::Normal(p) => depth.push(p),
        }
    }
    true
}

/// Create the parent directories of `rel` under `root`, refusing to pass
/// through any existing symlink or non-directory.
fn prepare_parents(root: &Path, rel: &Path) -> Result<(), ExtractError> {
    let Some(parent) = rel.parent() else { return Ok(()) };
    let mut cur = root.to_path_buf();
    for c in parent.components() {
        cur.push(c);
        match fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => return Err(ExtractError::TraversesSymlink(rel.to_owned())),
            Ok(m) if m.is_dir() => {}
            Ok(_) => return Err(ExtractError::TraversesSymlink(rel.to_owned())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => fs::create_dir(&cur).map_err(io_err(&cur))?,
            Err(e) => return Err(ExtractError::Io { path: cur, source: e }),
        }
    }
    Ok(())
}

/// Remove whatever non-directory currently sits at `dest` (tar archives may
/// legitimately contain the same path twice). Symlinks are removed, never followed.
fn clear_dest(dest: &Path) -> Result<(), ExtractError> {
    match fs::symlink_metadata(dest) {
        Ok(m) if m.is_dir() => Err(ExtractError::TraversesSymlink(dest.to_owned())),
        Ok(_) => fs::remove_file(dest).map_err(io_err(dest)),
        Err(_) => Ok(()),
    }
}

pub fn extract_tar(tar_bytes: &[u8], root: &Path, limits: ExtractLimits) -> Result<ExtractReport, ExtractError> {
    extract_tar_stream(tar_bytes, root, limits)
}

/// Like [`extract_tar`], reading the archive from a stream (large archives,
/// e.g. app downloads, never need to fit in memory).
pub fn extract_tar_stream<R: Read>(
    reader: R,
    root: &Path,
    limits: ExtractLimits,
) -> Result<ExtractReport, ExtractError> {
    fs::create_dir_all(root).map_err(io_err(root))?;
    let mut report = ExtractReport::default();
    let mut archive = tar::Archive::new(reader);
    let mut count = 0u64;

    for entry in archive.entries().map_err(ExtractError::Tar)? {
        let mut entry = entry.map_err(ExtractError::Tar)?;
        count += 1;
        if count > limits.max_entries {
            return Err(ExtractError::Limit("too many entries"));
        }
        let raw = entry.path().map_err(ExtractError::Tar)?.into_owned();
        let Some(rel) = sanitize(&raw)? else { continue };
        let dest = root.join(&rel);
        let kind = entry.header().entry_type();

        match kind {
            tar::EntryType::Directory => {
                prepare_parents(root, &rel)?;
                match fs::symlink_metadata(&dest) {
                    Ok(m) if m.is_dir() => {}
                    Ok(_) => return Err(ExtractError::TraversesSymlink(rel)),
                    Err(_) => fs::create_dir(&dest).map_err(io_err(&dest))?,
                }
                fs::set_permissions(&dest, fs::Permissions::from_mode(0o755)).map_err(io_err(&dest))?;
                report.dirs += 1;
            }
            tar::EntryType::Regular | tar::EntryType::Continuous => {
                prepare_parents(root, &rel)?;
                clear_dest(&dest)?;
                let size = entry.header().size().map_err(ExtractError::Tar)?;
                if report.bytes + size > limits.max_total_bytes {
                    return Err(ExtractError::Limit("total extracted size"));
                }
                let mode = entry.header().mode().map_err(ExtractError::Tar)? & 0o755;
                let mut f = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(mode | 0o600)
                    .custom_flags(libc_nofollow())
                    .open(&dest)
                    .map_err(io_err(&dest))?;
                let written = io::copy(&mut (&mut entry).take(size), &mut f).map_err(io_err(&dest))?;
                fs::set_permissions(&dest, fs::Permissions::from_mode(mode | 0o400)).map_err(io_err(&dest))?;
                report.bytes += written;
                report.files += 1;
            }
            tar::EntryType::Symlink => {
                let target = entry.link_name().map_err(ExtractError::Tar)?.map(|c| c.into_owned()).unwrap_or_default();
                if target.as_os_str().is_empty() || !symlink_stays_inside(&rel, &target) {
                    return Err(ExtractError::EscapingSymlink { link: rel, target });
                }
                prepare_parents(root, &rel)?;
                clear_dest(&dest)?;
                symlink(&target, &dest).map_err(io_err(&dest))?;
                report.symlinks += 1;
            }
            tar::EntryType::Link => {
                let raw_target =
                    entry.link_name().map_err(ExtractError::Tar)?.map(|c| c.into_owned()).unwrap_or_default();
                let target_rel = sanitize(&raw_target)?
                    .ok_or_else(|| ExtractError::BadHardlink { link: rel.clone(), target: raw_target.clone() })?;
                // The source must be a regular file we already extracted, reached without symlinks.
                prepare_parents(root, &target_rel)
                    .map_err(|_| ExtractError::BadHardlink { link: rel.clone(), target: raw_target.clone() })?;
                let src = root.join(&target_rel);
                let is_file = fs::symlink_metadata(&src).map(|m| m.is_file()).unwrap_or(false);
                if !is_file {
                    return Err(ExtractError::BadHardlink { link: rel, target: raw_target });
                }
                prepare_parents(root, &rel)?;
                clear_dest(&dest)?;
                let n = fs::copy(&src, &dest).map_err(io_err(&dest))?;
                report.bytes += n;
                report.files += 1;
            }
            tar::EntryType::XGlobalHeader
            | tar::EntryType::XHeader
            | tar::EntryType::GNULongName
            | tar::EntryType::GNULongLink => {}
            other => report.skipped.push(format!("{} ({other:?})", rel.display())),
        }
    }
    Ok(report)
}

/// `O_NOFOLLOW`: belt and braces on top of `clear_dest` — never open through
/// a symlink that might have raced into place.
fn libc_nofollow() -> i32 {
    libc::O_NOFOLLOW
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    enum E<'a> {
        File(&'a [u8], &'a [u8]),
        Dir(&'a [u8]),
        Symlink(&'a [u8], &'a [u8]),
        Hardlink(&'a [u8], &'a [u8]),
    }

    /// Write raw header names so we can produce archives the `tar` builder
    /// would (rightly) refuse to create.
    fn raw_tar(entries: &[E]) -> Vec<u8> {
        let mut out = Vec::new();
        for e in entries {
            let mut h = tar::Header::new_old();
            let (name, link, data, kind): (&[u8], &[u8], &[u8], tar::EntryType) = match e {
                E::File(n, d) => (n, b"", d, tar::EntryType::Regular),
                E::Dir(n) => (n, b"", b"", tar::EntryType::Directory),
                E::Symlink(n, t) => (n, t, b"", tar::EntryType::Symlink),
                E::Hardlink(n, t) => (n, t, b"", tar::EntryType::Link),
            };
            h.as_old_mut().name[..name.len()].copy_from_slice(name);
            h.as_old_mut().linkname[..link.len()].copy_from_slice(link);
            h.set_size(data.len() as u64);
            h.set_mode(0o4755);
            h.set_entry_type(kind);
            h.set_cksum();
            out.write_all(h.as_bytes()).unwrap();
            out.write_all(data).unwrap();
            let pad = (512 - data.len() % 512) % 512;
            out.write_all(&vec![0; pad]).unwrap();
        }
        out.write_all(&[0; 1024]).unwrap();
        out
    }

    fn run(entries: &[E]) -> (tempfile::TempDir, Result<ExtractReport, ExtractError>) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("root");
        let r = extract_tar(&raw_tar(entries), &root, ExtractLimits::default());
        (dir, r)
    }

    #[test]
    fn extracts_normal_tree_and_strips_setuid() {
        let (dir, r) = run(&[
            E::Dir(b"./usr/"),
            E::File(b"./usr/bin/tool", b"bin"),
            E::Symlink(b"./usr/lib/libx.so.1", b"libx.so.1.2"),
            E::Symlink(b"./etc/alternatives/x", b"/usr/bin/tool"),
            E::Hardlink(b"./usr/bin/tool2", b"./usr/bin/tool"),
        ]);
        let r = r.unwrap();
        assert_eq!((r.files, r.symlinks), (2, 2));
        let mode = fs::metadata(dir.path().join("root/usr/bin/tool")).unwrap().permissions().mode();
        assert_eq!(mode & 0o7777, 0o755, "setuid bit must be stripped");
    }

    #[test]
    fn rejects_parent_dir_paths() {
        let (dir, r) = run(&[E::File(b"../evil", b"x")]);
        assert!(matches!(r, Err(ExtractError::UnsafePath(_))));
        assert!(!dir.path().join("evil").exists());
    }

    #[test]
    fn rejects_absolute_paths() {
        let (_d, r) = run(&[E::File(b"/tmp/evil", b"x")]);
        assert!(matches!(r, Err(ExtractError::UnsafePath(_))));
    }

    #[test]
    fn rejects_escaping_symlink() {
        let (_d, r) = run(&[E::Symlink(b"./usr/lib/x", b"../../../outside")]);
        assert!(matches!(r, Err(ExtractError::EscapingSymlink { .. })));
    }

    #[test]
    fn rejects_write_through_planted_symlink() {
        // `/tmp` re-roots inside the bundle, so the link itself is allowed…
        // …but nothing may be written *through* it on the host.
        let (dir, r) = run(&[E::Symlink(b"./sneaky", b"/tmp"), E::File(b"./sneaky/payload", b"x")]);
        assert!(matches!(r, Err(ExtractError::TraversesSymlink(_))));
        assert!(fs::symlink_metadata(dir.path().join("root/sneaky")).unwrap().file_type().is_symlink());
    }

    #[test]
    fn rejects_overwriting_via_symlink_to_host_file() {
        let (_d, r) = run(&[E::Symlink(b"./a", b"../../etc/passwd"), E::File(b"./a", b"x")]);
        assert!(matches!(r, Err(ExtractError::EscapingSymlink { .. })));
    }

    #[test]
    fn rejects_hardlink_to_outside() {
        let (_d, r) = run(&[E::Hardlink(b"./passwd", b"/etc/passwd")]);
        assert!(matches!(r, Err(ExtractError::UnsafePath(_))));
        let (_d, r) = run(&[E::Hardlink(b"./passwd", b"etc/passwd")]);
        assert!(matches!(r, Err(ExtractError::BadHardlink { .. })));
    }

    #[test]
    fn lexical_symlink_check() {
        assert!(symlink_stays_inside(Path::new("usr/lib/a"), Path::new("../share/x")));
        assert!(symlink_stays_inside(Path::new("usr/lib/a"), Path::new("/etc/x")));
        assert!(!symlink_stays_inside(Path::new("usr/a"), Path::new("../../x")));
        assert!(!symlink_stays_inside(Path::new("a"), Path::new("/../x")));
    }
}
