use std::fs;
use std::path::{Path, PathBuf};

use crate::{BundleError, Info, Permissions, Provenance};

/// Path arithmetic for a bundle directory. Pure; touches no disk.
#[derive(Debug, Clone)]
pub struct BundleLayout {
    root: PathBuf,
}

impl BundleLayout {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `<dir>/<name>.app`, with the name reduced to a safe file name.
    pub fn in_dir(dir: &Path, name: &str) -> Self {
        let safe: String = name.chars().map(|c| if c == '/' || c == '\\' || c == '\0' { '_' } else { c }).collect();
        let safe = if safe.is_empty() || safe == "." || safe == ".." { "App".to_owned() } else { safe };
        Self::new(dir.join(format!("{safe}.app")))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn contents(&self) -> PathBuf {
        self.root.join("Contents")
    }
    pub fn executables(&self) -> PathBuf {
        self.contents().join("Executables")
    }
    pub fn libraries(&self) -> PathBuf {
        self.contents().join("Libraries")
    }
    pub fn fhs_root(&self) -> PathBuf {
        self.contents().join("Root")
    }
    pub fn resources(&self) -> PathBuf {
        self.contents().join("Resources")
    }
    pub fn info_path(&self) -> PathBuf {
        self.contents().join("Info.toml")
    }
    pub fn permissions_path(&self) -> PathBuf {
        self.contents().join("Permissions.toml")
    }
    pub fn provenance_path(&self) -> PathBuf {
        self.contents().join("Provenance.toml")
    }

    /// Create the empty directory skeleton.
    pub fn create_dirs(&self) -> Result<(), BundleError> {
        for dir in [self.executables(), self.libraries(), self.fhs_root(), self.resources()] {
            fs::create_dir_all(&dir).map_err(BundleError::io(&dir))?;
        }
        Ok(())
    }
}

/// A bundle whose manifests have been read and validated.
#[derive(Debug, Clone)]
pub struct Bundle {
    pub layout: BundleLayout,
    pub info: Info,
    pub permissions: Permissions,
    pub provenance: Option<Provenance>,
}

impl Bundle {
    /// Open and validate a bundle. `Provenance.toml` is optional (hand-built
    /// first-party bundles may not have one); the other two are required.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, BundleError> {
        let layout = BundleLayout::new(root);
        let info: Info = read_toml(&layout.info_path())?;
        info.validate()?;
        let permissions: Permissions = if layout.permissions_path().exists() {
            read_toml(&layout.permissions_path())?
        } else {
            Permissions::default()
        };
        permissions.validate()?;
        let provenance =
            if layout.provenance_path().exists() { Some(read_toml(&layout.provenance_path())?) } else { None };

        let exe = layout.executables().join(&info.executable);
        if !exe.is_file() {
            return Err(BundleError::Invalid(format!("entry point {} does not exist", exe.display())));
        }
        Ok(Self { layout, info, permissions, provenance })
    }

    /// Write manifests into an existing layout (payload files are written by
    /// the producer, e.g. the ingestion pipeline).
    pub fn write_manifests(
        layout: &BundleLayout,
        info: &Info,
        permissions: &Permissions,
        provenance: Option<&Provenance>,
    ) -> Result<(), BundleError> {
        info.validate()?;
        permissions.validate()?;
        write_toml(&layout.info_path(), info)?;
        write_toml(&layout.permissions_path(), permissions)?;
        if let Some(p) = provenance {
            write_toml(&layout.provenance_path(), p)?;
        }
        Ok(())
    }
}

fn read_toml<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, BundleError> {
    let text = fs::read_to_string(path).map_err(BundleError::io(path))?;
    toml::from_str(&text).map_err(|source| BundleError::Parse { path: path.to_owned(), source })
}

fn write_toml<T: serde::Serialize>(path: &Path, value: &T) -> Result<(), BundleError> {
    let text = toml::to_string_pretty(value)?;
    fs::write(path, text).map_err(BundleError::io(path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{IsolationTier, LibraryRecord, PackageRecord, PackageRole, ResolutionStatus, SourceRecord};
    use iliada_types::AppId;

    #[test]
    fn write_then_open() {
        let dir = tempfile::tempdir().unwrap();
        let layout = BundleLayout::in_dir(dir.path(), "jq");
        layout.create_dirs().unwrap();
        fs::write(layout.executables().join("jq"), "#!/bin/sh\n").unwrap();

        let info = Info {
            id: AppId::new("org.debian.jq").unwrap(),
            name: "jq".into(),
            version: "1.7.1".into(),
            executable: "jq".into(),
            entry: Some("usr/bin/jq".into()),
            icon: None,
            runtime: "iliada-base-1".into(),
            isolation: IsolationTier::MicroVm,
            categories: vec![],
            mime_types: vec![],
        };
        let prov = Provenance {
            generator: "test".into(),
            created_at: 0,
            source: SourceRecord { url: "https://x/jq.deb".into(), sha256: "00".into(), format: "deb".into() },
            packages: vec![PackageRecord {
                name: "jq".into(),
                version: "1.7.1".into(),
                url: "https://x/jq.deb".into(),
                sha256: "00".into(),
                role: PackageRole::Primary,
            }],
            libraries: vec![LibraryRecord {
                soname: "libc.so.6".into(),
                status: ResolutionStatus::Runtime,
                package: None,
                suggested_by: None,
            }],
        };
        Bundle::write_manifests(&layout, &info, &Permissions::default(), Some(&prov)).unwrap();

        let opened = Bundle::open(layout.root()).unwrap();
        assert_eq!(opened.info, info);
        assert_eq!(opened.provenance.unwrap(), prov);
    }

    #[test]
    fn missing_entry_point_is_invalid() {
        let dir = tempfile::tempdir().unwrap();
        let layout = BundleLayout::in_dir(dir.path(), "x");
        layout.create_dirs().unwrap();
        fs::write(
            layout.info_path(),
            "id = \"org.example.x\"\nname = \"x\"\nversion = \"1\"\nexecutable = \"x\"\nruntime = \"iliada-base-1\"\n",
        )
        .unwrap();
        assert!(matches!(Bundle::open(layout.root()), Err(BundleError::Invalid(_))));
    }
}
