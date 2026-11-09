//! ELF inspection: which shared objects does an extracted tree need and provide?

use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::Result;
use goblin::elf::{Elf, header::ET_DYN, header::ET_EXEC};

#[derive(Debug, Clone)]
pub struct ElfObject {
    /// Path relative to the scanned root.
    pub rel_path: PathBuf,
    pub soname: Option<String>,
    pub needed: Vec<String>,
    pub interpreter: Option<String>,
    pub is_executable: bool,
    pub is_shared_lib: bool,
}

const MAX_ELF_BYTES: u64 = 1 << 30;

/// Recursively scan `root` (without following symlinks) for ELF objects.
pub fn scan_tree(root: &Path) -> Result<Vec<ElfObject>> {
    let mut out = Vec::new();
    walk(root, Path::new(""), &mut out)?;
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}

fn walk(root: &Path, rel: &Path, out: &mut Vec<ElfObject>) -> Result<()> {
    let dir = root.join(rel);
    let Ok(rd) = fs::read_dir(&dir) else { return Ok(()) };
    for entry in rd {
        let entry = entry?;
        let ft = entry.file_type()?;
        let child_rel = rel.join(entry.file_name());
        if ft.is_dir() {
            walk(root, &child_rel, out)?;
        } else if ft.is_file()
            && let Some(obj) = inspect(&root.join(&child_rel), &child_rel)?
        {
            out.push(obj);
        }
    }
    Ok(())
}

pub fn inspect(path: &Path, rel: &Path) -> Result<Option<ElfObject>> {
    let mut f = fs::File::open(path)?;
    let mut magic = [0u8; 4];
    if f.read_exact(&mut magic).is_err() || &magic != b"\x7fELF" {
        return Ok(None);
    }
    if f.metadata()?.len() > MAX_ELF_BYTES {
        tracing::warn!(path = %rel.display(), "ELF larger than 1 GiB, not inspected");
        return Ok(None);
    }
    let bytes = fs::read(path)?;
    let elf = match Elf::parse(&bytes) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(path = %rel.display(), error = %e, "unparseable ELF, ignored");
            return Ok(None);
        }
    };
    let e_type = elf.header.e_type;
    Ok(Some(ElfObject {
        rel_path: rel.to_owned(),
        soname: elf.soname.map(str::to_owned),
        needed: elf.libraries.iter().map(|s| (*s).to_owned()).collect(),
        interpreter: elf.interpreter.map(str::to_owned),
        is_executable: e_type == ET_EXEC || (e_type == ET_DYN && elf.interpreter.is_some()),
        is_shared_lib: e_type == ET_DYN && elf.soname.is_some(),
    }))
}

/// Sonames a tree satisfies by itself: DT_SONAME of its shared libraries plus
/// the file names of `lib*.so*` entries (Debian ships `libfoo.so.1 -> libfoo.so.1.2.3`).
pub fn provided_sonames(root: &Path, objects: &[ElfObject]) -> BTreeSet<String> {
    let mut set: BTreeSet<String> = objects.iter().filter_map(|o| o.soname.clone()).collect();
    // Libraries the program finds through its own RUNPATH ($ORIGIN) anywhere
    // in the tree, often without a DT_SONAME.
    for o in objects.iter().filter(|o| !o.is_executable) {
        if let Some(name) = o.rel_path.file_name().map(|n| n.to_string_lossy().into_owned())
            && name.contains(".so")
        {
            set.insert(name);
        }
    }
    for dir in library_dirs(root) {
        if let Ok(rd) = fs::read_dir(&dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.starts_with("lib") && name.contains(".so") {
                    set.insert(name);
                }
            }
        }
    }
    set
}

/// Directories under an FHS root that the dynamic loader would search.
pub fn library_dirs(root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    for base in ["lib", "usr/lib", "lib64", "usr/lib64"] {
        let p = root.join(base);
        if p.is_dir() {
            if let Ok(rd) = fs::read_dir(&p) {
                for e in rd.flatten() {
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name.ends_with("-linux-gnu") || name.ends_with("-linux-gnueabihf") {
                        dirs.push(e.path());
                    }
                }
            }
            dirs.push(p);
        }
    }
    dirs
}

/// Debian architecture → GNU multiarch triplet.
pub fn multiarch_triplet(deb_arch: &str) -> Option<&'static str> {
    Some(match deb_arch {
        "amd64" => "x86_64-linux-gnu",
        "arm64" => "aarch64-linux-gnu",
        "armhf" => "arm-linux-gnueabihf",
        "i386" => "i386-linux-gnu",
        "riscv64" => "riscv64-linux-gnu",
        "ppc64el" => "powerpc64le-linux-gnu",
        "s390x" => "s390x-linux-gnu",
        _ => return None,
    })
}

/// Debian architecture name of the machine we are running on.
pub fn host_deb_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "arm" => "armhf",
        "x86" => "i386",
        "riscv64" => "riscv64",
        "powerpc64" => "ppc64el",
        "s390x" => "s390x",
        other => other,
    }
}
