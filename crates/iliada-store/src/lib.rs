//! The Iliada Store engine.
//!
//! 1. Fetch `catalog-2.json` (or, where there is none, `catalog.json`) and
//!    its `.sig`; verify the signature with the key built into Iliada before
//!    parsing anything. Only apps built for this computer's processor are
//!    offered.
//! 2. To install, download the app's archive, check its size and SHA-256
//!    against the signed catalog, unpack it with the path-checked extractor
//!    into a private staging folder inside `/Applications`, validate the
//!    bundle, then move it into place in one rename: exactly what dragging a
//!    `.app` into Applications does. `iliada-appd` takes it from there.

pub mod translate;

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use futures_util::StreamExt;
use iliada_bundle::Bundle;
use iliada_bundle::processor::processor_name;
use iliada_catalog::{AppEntry, Catalog, VerifyingKey};
use iliada_ingest::extract::{ExtractLimits, extract_tar_stream};
use sha2::{Digest, Sha256};

pub const DEFAULT_CATALOG: &str = "https://github.com/cseas002/iliada_apps/releases/download/catalog/catalog.json";
/// Written by the `iliada.store` NixOS module.
pub const SYSTEM_CONFIG: &str = "/etc/iliada/store.json";
/// The Iliada catalog signing key (public half).
pub const CATALOG_KEY: &str = include_str!("../catalog.pub");
const MAX_ARCHIVE: u64 = 8 << 30;

#[derive(Debug, Clone)]
pub struct StoreConfig {
    /// https URL, file:// URL or local path of catalog.json (the .sig sits next to it).
    pub catalog: String,
    pub key: VerifyingKey,
    pub apps_dir: PathBuf,
    /// The app list Store installs are recorded in (`/etc/iliada/apps.toml`).
    pub app_list: Option<PathBuf>,
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SystemConfig {
    catalog: Option<String>,
    app_list: Option<PathBuf>,
    applications: Option<PathBuf>,
}

impl StoreConfig {
    /// System settings (`/etc/iliada/store.json`), then defaults;
    /// `ILIADA_STORE_CATALOG` / `ILIADA_APP_LIST` override (development).
    pub fn from_env() -> Result<Self> {
        let sys: SystemConfig =
            fs::read(SYSTEM_CONFIG).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Ok(Self {
            catalog: std::env::var("ILIADA_STORE_CATALOG")
                .ok()
                .or(sys.catalog)
                .unwrap_or_else(|| DEFAULT_CATALOG.into()),
            key: iliada_catalog::parse_key(CATALOG_KEY)?,
            apps_dir: sys.applications.unwrap_or_else(|| PathBuf::from("/Applications")),
            app_list: std::env::var_os("ILIADA_APP_LIST").map(PathBuf::from).or(sys.app_list),
        })
    }
}

/// What `sync` did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SyncReport {
    pub installed: Vec<String>,
    pub updated: Vec<String>,
    pub failed: Vec<(String, String)>,
}

/// An app currently in Applications.
#[derive(Debug, Clone)]
pub struct Installed {
    pub id: String,
    pub name: String,
    pub version: String,
    pub path: PathBuf,
}

#[derive(Debug)]
pub struct Store {
    cfg: StoreConfig,
    client: reqwest::Client,
}

impl Store {
    pub fn new(cfg: StoreConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("iliada-store/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(std::time::Duration::from_secs(20))
            .build()?;
        Ok(Self { cfg, client })
    }

    pub fn apps_dir(&self) -> &Path {
        &self.cfg.apps_dir
    }

    async fn get(&self, location: &str, limit: u64) -> Result<Vec<u8>> {
        if location.starts_with("https://")
            || location.starts_with("http://127.0.0.1")
            || location.starts_with("http://10.0.2.2")
        {
            let resp = self
                .client
                .get(location)
                .send()
                .await?
                .error_for_status()
                .with_context(|| format!("GET {location}"))?;
            let mut body = Vec::new();
            let mut stream = resp.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                ensure!((body.len() + chunk.len()) as u64 <= limit, "{location} is too large");
                body.extend_from_slice(&chunk);
            }
            return Ok(body);
        }
        let path = location.strip_prefix("file://").unwrap_or(location);
        fs::read(path).with_context(|| format!("reading {path}"))
    }

    /// Download and verify the catalog: the apps this computer can run.
    pub async fn catalog(&self) -> Result<Catalog> {
        Ok(self.full_catalog().await?.for_arch(std::env::consts::ARCH))
    }

    /// Every app, for every processor. `catalog-2.json` (one archive per
    /// processor) sits next to `catalog.json` (x86-64 only, for Iliada 0.1)
    /// and is preferred when it exists.
    pub async fn full_catalog(&self) -> Result<Catalog> {
        let url = &self.cfg.catalog;
        if let Some(dir) = url.strip_suffix("catalog.json") {
            let v2 = format!("{dir}{}", iliada_catalog::CATALOG_2);
            match self.fetch_catalog(&v2).await {
                Ok(Some(c)) => return Ok(c),
                Ok(None) => {}
                Err(e) => return Err(e),
            }
        }
        self.fetch_catalog(url).await?.with_context(|| format!("could not download the catalog {url}"))
    }

    /// `None`: not there (or unreachable). A catalog that is there but does
    /// not verify is an error, never skipped.
    async fn fetch_catalog(&self, url: &str) -> Result<Option<Catalog>> {
        let bytes = match self.get(url, 16 << 20).await {
            Ok(b) => b,
            Err(e) => {
                tracing::debug!(%url, error = %format!("{e:#}"), "no catalog here");
                return Ok(None);
            }
        };
        let sig = self.get(&format!("{url}.sig"), 4096).await?;
        iliada_catalog::verify(&self.cfg.key, &bytes, &String::from_utf8_lossy(&sig)).map(Some)
    }

    /// Apps in Applications, by id.
    pub fn installed(&self) -> BTreeMap<String, Installed> {
        let mut out = BTreeMap::new();
        for e in fs::read_dir(&self.cfg.apps_dir).into_iter().flatten().flatten() {
            if e.path().extension().is_none_or(|x| x != "app") {
                continue;
            }
            if let Ok(b) = Bundle::open(e.path()) {
                out.insert(
                    b.info.id.to_string(),
                    Installed {
                        id: b.info.id.to_string(),
                        name: b.info.name.clone(),
                        version: b.info.version.clone(),
                        path: e.path(),
                    },
                );
            }
        }
        out
    }

    /// Install (or update) one app. `progress(done, total)` reports the download.
    pub async fn install(&self, app: &AppEntry, progress: &(dyn Fn(u64, u64) + Sync)) -> Result<PathBuf> {
        let archive_entry = app.local_archive().with_context(|| {
            format!(
                "{} is not available for this computer's processor ({})",
                app.name,
                processor_name(std::env::consts::ARCH)
            )
        })?;
        ensure!(archive_entry.size <= MAX_ARCHIVE, "{} is too large", app.name);
        let staging = tempfile::Builder::new().prefix(".iliada-store-").tempdir_in(&self.cfg.apps_dir)?;
        let archive = staging.path().join("archive.tar.zst");

        // Download, hashing as we go.
        let mut file = fs::File::create(&archive)?;
        let mut hasher = Sha256::new();
        let mut done = 0u64;
        if archive_entry.url.starts_with("https://") || archive_entry.url.starts_with("http://") {
            let resp = self.client.get(&archive_entry.url).send().await?.error_for_status()?;
            let mut stream = resp.bytes_stream();
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                done += chunk.len() as u64;
                ensure!(done <= archive_entry.size, "download is larger than the catalog says");
                hasher.update(&chunk);
                file.write_all(&chunk)?;
                progress(done, archive_entry.size);
            }
        } else {
            let bytes = self.get(&archive_entry.url, archive_entry.size).await?;
            done = bytes.len() as u64;
            hasher.update(&bytes);
            file.write_all(&bytes)?;
            progress(done, archive_entry.size);
        }
        file.flush()?;
        drop(file);
        ensure!(done == archive_entry.size, "download size {done} does not match the catalog ({})", archive_entry.size);
        let sha = hex::encode(hasher.finalize());
        ensure!(
            sha == archive_entry.sha256,
            "checksum mismatch for {}: the download is not the one the catalog signed",
            app.name
        );

        // Unpack (path-checked) and validate.
        let unpacked = staging.path().join("unpacked");
        let (a, u) = (archive.clone(), unpacked.clone());
        tokio::task::spawn_blocking(move || -> Result<()> {
            let decoder = ruzstd::decoding::StreamingDecoder::new(BufReader::new(fs::File::open(&a)?))
                .map_err(|e| anyhow::anyhow!("zstd: {e}"))?;
            extract_tar_stream(decoder, &u, ExtractLimits { max_total_bytes: 32 << 30, ..Default::default() })?;
            Ok(())
        })
        .await??;
        let entries: Vec<PathBuf> = fs::read_dir(&unpacked)?.flatten().map(|e| e.path()).collect();
        let [bundle_dir] = entries.as_slice() else { bail!("archive must contain exactly one .app") };
        ensure!(bundle_dir.extension().is_some_and(|x| x == "app"), "archive does not contain a .app");
        let bundle = Bundle::open(bundle_dir).context("downloaded bundle is invalid")?;
        ensure!(
            bundle.info.id.as_str() == app.id,
            "bundle id {} does not match the catalog ({})",
            bundle.info.id,
            app.id
        );
        make_group_writable(bundle_dir)?;

        // Replace any installed version, then move into place.
        let dest = self.cfg.apps_dir.join(bundle_dir.file_name().context("no name")?);
        if let Some(old) = self.installed().get(&app.id) {
            fs::rename(&old.path, staging.path().join("previous"))?;
        }
        if dest.exists() {
            fs::rename(&dest, staging.path().join("replaced"))?;
        }
        // Record it before it appears, so iliada-appd sees a Store entry
        // rather than a hand-installed app.
        if let Some(list) = &self.cfg.app_list
            && let Err(e) = iliada_applist::record_store_install(list, &app.id, &app.name, &app.id)
        {
            tracing::warn!(error = %format!("{e:#}"), "could not record the install in the app list");
        }
        fs::rename(bundle_dir, &dest)?;
        Ok(dest)
    }

    /// Bring Applications in line with the app list's Store entries: install
    /// the missing ones (a restore on a new computer), update the outdated
    /// ones. With no app list, only updates installed apps.
    pub async fn sync(&self) -> Result<SyncReport> {
        let catalog = self.catalog().await?;
        let installed = self.installed();
        let wanted: Vec<String> = match &self.cfg.app_list {
            Some(list) => iliada_applist::store_entries(list)?.into_iter().map(|(store, _)| store).collect(),
            None => installed.keys().cloned().collect(),
        };
        let mut report = SyncReport::default();
        for app in catalog.apps.iter().filter(|a| wanted.contains(&a.id) || installed.contains_key(&a.id)) {
            let action = match installed.get(&app.id) {
                None => "install",
                Some(i) if i.version != app.version => "update",
                Some(_) => continue,
            };
            match self.install(app, &|_, _| {}).await {
                Ok(_) if action == "install" => report.installed.push(app.name.clone()),
                Ok(_) => report.updated.push(app.name.clone()),
                Err(e) => report.failed.push((app.name.clone(), format!("{e:#}"))),
            }
        }
        Ok(report)
    }

    /// Remove an app (what dragging it to the Trash and emptying it does).
    pub fn remove(&self, id: &str) -> Result<()> {
        let app = self.installed().remove(id).with_context(|| format!("{id} is not installed"))?;
        fs::remove_dir_all(&app.path)?;
        Ok(())
    }
}

fn make_group_writable(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    let mode = meta.permissions().mode() & 0o777;
    fs::set_permissions(path, fs::Permissions::from_mode(mode | if meta.is_dir() { 0o070 } else { 0o060 }))?;
    if meta.is_dir() {
        for e in fs::read_dir(path)?.flatten() {
            make_group_writable(&e.path())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use iliada_bundle::{BundleLayout, Info, IsolationTier, Permissions};
    use iliada_catalog::{Archive, Archives, FORMAT, SigningKey};
    use iliada_types::AppId;

    /// Build a tiny bundle, pack it like the catalog builder does, and sign a catalog for it.
    fn fixture(dir: &Path) -> (StoreConfig, AppEntry, Vec<u8>) {
        let src = dir.join("src");
        let l = BundleLayout::in_dir(&src, "Hello");
        l.create_dirs().unwrap();
        fs::write(l.executables().join("hello"), "#!/bin/sh\n").unwrap();
        let info = Info {
            id: AppId::new("org.example.hello").unwrap(),
            name: "Hello".into(),
            version: "1.0".into(),
            executable: "hello".into(),
            entry: None,
            args: vec![],
            env: Default::default(),
            icon: None,
            runtime: "iliada-base-1".into(),
            isolation: IsolationTier::Namespace,
            network: false,
            categories: vec![],
            mime_types: vec![],
        };
        Bundle::write_manifests(&l, &info, &Permissions::default(), None).unwrap();
        let mut enc = zstd::Encoder::new(Vec::new(), 3).unwrap();
        {
            let mut t = tar::Builder::new(&mut enc);
            t.append_dir_all("Hello.app", l.root()).unwrap();
            t.finish().unwrap();
        }
        let archive_bytes = enc.finish().unwrap();
        let archive = dir.join("hello.app.tar.zst");
        fs::write(&archive, &archive_bytes).unwrap();
        let mut entry = AppEntry {
            id: "org.example.hello".into(),
            name: "Hello".into(),
            summary: "Say hello".into(),
            category: "Utilities".into(),
            version: "1.0".into(),
            network: false,
            archive: None,
            archives: Archives::default(),
            icon: None,
        };
        let a = Archive {
            url: archive.display().to_string(),
            sha256: hex::encode(Sha256::digest(&archive_bytes)),
            size: archive_bytes.len() as u64,
            version: None,
        };
        *entry.archives.slot(std::env::consts::ARCH).unwrap() = Some(a);
        let sk = SigningKey::from_bytes(&[9u8; 32]);
        let cat = Catalog { format: FORMAT, generated: 0, apps: vec![entry.clone()] };
        write_signed(&sk, &dir.join("catalog-2.json"), &cat);
        write_signed(&sk, &dir.join("catalog.json"), &cat.legacy());
        let apps = dir.join("Applications");
        fs::create_dir_all(&apps).unwrap();
        let cfg = StoreConfig {
            catalog: dir.join("catalog.json").display().to_string(),
            key: sk.verifying_key(),
            apps_dir: apps,
            app_list: Some(dir.join("apps.toml")),
        };
        (cfg, entry, archive_bytes)
    }

    #[tokio::test]
    async fn installs_verified_apps_and_rejects_tampered_ones() {
        let d = tempfile::tempdir().unwrap();
        let (cfg, entry, _) = fixture(d.path());
        let store = Store::new(cfg.clone()).unwrap();

        let catalog = store.catalog().await.unwrap();
        assert_eq!(catalog.apps, vec![entry.clone()]);
        let path = store.install(&catalog.apps[0], &|_, _| {}).await.unwrap();
        assert_eq!(path, cfg.apps_dir.join("Hello.app"));
        assert_eq!(store.installed()["org.example.hello"].version, "1.0");
        // No staging leftovers.
        assert_eq!(fs::read_dir(&cfg.apps_dir).unwrap().count(), 1);

        // A swapped archive (same size, different bytes) is refused.
        let mut evil = entry.clone();
        evil.archives.slot(std::env::consts::ARCH).unwrap().as_mut().unwrap().sha256 = "0".repeat(64);
        assert!(store.install(&evil, &|_, _| {}).await.is_err());
        assert!(store.installed().contains_key("org.example.hello"), "a failed update keeps the installed app");

        // A catalog signed by someone else is refused.
        let mut wrong = cfg.clone();
        wrong.key = SigningKey::from_bytes(&[1u8; 32]).verifying_key();
        assert!(Store::new(wrong).unwrap().catalog().await.is_err());

        // The install was recorded as a Store entry.
        let list = std::fs::read_to_string(d.path().join("apps.toml")).unwrap();
        assert!(list.contains("store = \"org.example.hello\""));

        store.remove("org.example.hello").unwrap();
        assert!(store.installed().is_empty());

        // Sync reinstalls what the list says should be there (a restore).
        let report = store.sync().await.unwrap();
        assert_eq!(report.installed, vec!["Hello".to_string()]);
        assert!(store.installed().contains_key("org.example.hello"));
    }

    fn write_signed(sk: &SigningKey, path: &Path, cat: &Catalog) {
        let bytes = serde_json::to_vec(cat).unwrap();
        fs::write(path, &bytes).unwrap();
        fs::write(format!("{}.sig", path.display()), iliada_catalog::sign(sk, &bytes)).unwrap();
    }

    #[tokio::test]
    async fn offers_only_apps_for_this_processor() {
        let d = tempfile::tempdir().unwrap();
        let (cfg, entry, _) = fixture(d.path());
        let sk = SigningKey::from_bytes(&[9u8; 32]);
        let other = if std::env::consts::ARCH == "x86_64" { "aarch64" } else { "x86_64" };
        let mut foreign = AppEntry { id: "org.example.foreign".into(), name: "Foreign".into(), ..entry.clone() };
        foreign.archives = Archives::default();
        *foreign.archives.slot(other).unwrap() = entry.local_archive().cloned();
        let cat = Catalog { format: FORMAT, generated: 1, apps: vec![entry.clone(), foreign.clone()] };
        write_signed(&sk, &d.path().join("catalog-2.json"), &cat);
        let store = Store::new(cfg).unwrap();

        assert_eq!(store.full_catalog().await.unwrap().apps.len(), 2);
        let mine = store.catalog().await.unwrap();
        assert_eq!(mine.apps, vec![entry.clone()], "the other processor's app is hidden");
        let err = store.install(&foreign, &|_, _| {}).await.unwrap_err();
        assert!(format!("{err:#}").contains("not available for this computer"), "{err:#}");

        // Without catalog-2.json, catalog.json (format 1) is used.
        fs::remove_file(d.path().join("catalog-2.json")).unwrap();
        let legacy = store.catalog().await.unwrap();
        assert_eq!(legacy.format, iliada_catalog::LEGACY_FORMAT);
        assert_eq!(legacy.apps.len(), usize::from(std::env::consts::ARCH == "x86_64"));

        // A catalog-2.json that does not verify is an error, not skipped.
        fs::write(d.path().join("catalog-2.json"), b"{}").unwrap();
        fs::write(d.path().join("catalog-2.json.sig"), "AAAA").unwrap();
        assert!(store.catalog().await.is_err());
    }
}
