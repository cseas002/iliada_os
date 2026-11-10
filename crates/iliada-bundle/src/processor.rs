//! Which processor a bundle's program is built for.
//!
//! Apps are prebuilt programs, so an x86-64 (Intel/AMD) app cannot run on an
//! ARM computer (Apple Silicon in UTM, a Raspberry Pi) or the other way round.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};

use crate::Bundle;

/// The processor an ELF file is for, in Rust's naming
/// (`std::env::consts::ARCH`); `None` for anything that is not ELF.
pub fn elf_machine(path: &Path) -> Option<&'static str> {
    let mut head = [0u8; 20];
    std::fs::File::open(path).ok()?.read_exact(&mut head).ok()?;
    if &head[..4] != b"\x7fELF" {
        return None;
    }
    // e_machine, in the file's byte order (EI_DATA: 1 little, 2 big endian).
    let m =
        if head[5] == 2 { u16::from_be_bytes([head[18], head[19]]) } else { u16::from_le_bytes([head[18], head[19]]) };
    Some(match m {
        0x3e => "x86_64",
        0xb7 => "aarch64",
        0x03 => "x86",
        0x28 => "arm",
        0xf3 => "riscv64",
        _ => "unknown",
    })
}

/// A name people know for a processor family.
pub fn processor_name(arch: &str) -> &'static str {
    match arch {
        "x86_64" => "Intel or AMD (x86-64)",
        "aarch64" => "ARM (64-bit, such as Apple Silicon)",
        "x86" => "32-bit Intel or AMD",
        "arm" => "32-bit ARM",
        "riscv64" => "RISC-V",
        _ => "another kind of",
    }
}

/// Resolve `rel` inside `root`, following symlinks as if `root` were `/`
/// (the bundle's tree is mounted at `/` when the app runs).
fn resolve_in(root: &Path, rel: &Path) -> Option<PathBuf> {
    let mut todo: VecDeque<OsString> = VecDeque::new();
    let push_front = |todo: &mut VecDeque<OsString>, p: &Path| -> bool {
        let mut absolute = false;
        for comp in p.components().rev() {
            match comp {
                Component::RootDir => absolute = true,
                Component::ParentDir => todo.push_front("..".into()),
                Component::Normal(c) => todo.push_front(c.to_owned()),
                _ => {}
            }
        }
        absolute
    };
    push_front(&mut todo, rel);
    let mut cur = PathBuf::new();
    let mut hops = 0;
    while let Some(c) = todo.pop_front() {
        if c == ".." {
            cur.pop();
            continue;
        }
        cur.push(&c);
        if let Ok(target) = std::fs::read_link(root.join(&cur)) {
            hops += 1;
            if hops > 40 {
                return None;
            }
            cur.pop();
            if push_front(&mut todo, &target) {
                cur = PathBuf::new();
            }
        }
    }
    let full = root.join(cur);
    full.is_file().then_some(full)
}

impl Bundle {
    /// The processor the app is built for, read from its program (or, for
    /// a script, from its libraries). `None` if no program could be read.
    pub fn processor(&self) -> Option<&'static str> {
        let root = self.layout.fhs_root();
        if let Some(entry) = &self.info.entry
            && let Some(p) = resolve_in(&root, Path::new(entry))
            && let Some(m) = elf_machine(&p)
        {
            return Some(m);
        }
        let dirs = [self.layout.libraries(), self.layout.executables()];
        dirs.iter()
            .flat_map(|d| std::fs::read_dir(d).into_iter().flatten().flatten())
            .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
            .find_map(|e| elf_machine(&e.path()))
    }

    /// `Some(explanation)` when this computer cannot run the app.
    pub fn wrong_processor(&self) -> Option<String> {
        let built = self.processor()?;
        let here = std::env::consts::ARCH;
        (built != here).then(|| {
            format!(
                "{} is made for {} computers, and this computer has a {} processor, so it cannot open. \
                 Get {} again from the Store to get the version for this computer.",
                self.info.name,
                processor_name(built),
                processor_name(here),
                self.info.name
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elf(machine: u16) -> Vec<u8> {
        let mut h = b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0\x02\0".to_vec();
        h.extend_from_slice(&machine.to_le_bytes());
        h.extend_from_slice(&[0; 44]);
        h
    }

    #[test]
    fn reads_the_machine_through_symlinks() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::create_dir_all(root.join("usr/lib/app")).unwrap();
        std::fs::create_dir_all(root.join("usr/bin")).unwrap();
        std::fs::write(root.join("usr/lib/app/app"), elf(0xb7)).unwrap();
        std::fs::write(root.join("x86"), elf(0x3e)).unwrap();
        std::fs::write(root.join("script"), "#!/bin/sh\n").unwrap();
        // Absolute links point inside the bundle's tree, not the host's.
        std::os::unix::fs::symlink("/usr/lib/app/app", root.join("usr/bin/app")).unwrap();
        std::os::unix::fs::symlink("../lib/app/app", root.join("usr/bin/rel")).unwrap();
        let p = resolve_in(root, Path::new("usr/bin/app")).unwrap();
        assert_eq!(elf_machine(&p), Some("aarch64"));
        let p = resolve_in(root, Path::new("usr/bin/rel")).unwrap();
        assert_eq!(elf_machine(&p), Some("aarch64"));
        assert_eq!(elf_machine(&root.join("x86")), Some("x86_64"));
        assert_eq!(elf_machine(&root.join("script")), None);
        assert!(resolve_in(root, Path::new("usr/bin/missing")).is_none());
    }
}
