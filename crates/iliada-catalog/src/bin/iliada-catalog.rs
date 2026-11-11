//! The catalog builder (runs on the Iliada build server).
//!
//!   iliada-catalog keygen --out ~/.config/iliada
//!   iliada-catalog build --recipes iliada_apps/recipes --out dist [--only firefox,gimp] [--arch arm64]
//!   iliada-catalog assemble --out dist [--include firefox,gimp]
//!   iliada-catalog sign --key ~/.config/iliada/catalog.key dist/catalog.json dist/catalog-2.json
//!   iliada-catalog release --source iliada_os --version 0.2.0 --key … --out dist-os

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufWriter, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use iliada_catalog::{AppEntry, Archive, Archives, Catalog, FORMAT};
use iliada_ingest::resolvers::{LibraryResolver, NamingRules};
use iliada_ingest::{IngestConfig, Ingestor, Source};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const DEFAULT_BASE: &str = "https://github.com/cseas002/iliada_apps/releases/download";

#[derive(Parser)]
#[command(name = "iliada-catalog", about = "Build the Iliada app catalog")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a catalog signing key pair (catalog.key, catalog.pub).
    Keygen {
        #[arg(long)]
        out: PathBuf,
    },
    /// Translate recipes into .app archives and catalog entries.
    Build {
        #[arg(long)]
        recipes: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// Only these recipes (file stems, comma-separated).
        #[arg(long, value_delimiter = ',')]
        only: Vec<String>,
        #[arg(long, default_value = DEFAULT_BASE)]
        base_url: String,
        #[arg(long)]
        cache: Option<PathBuf>,
        /// Debian architecture to build for: amd64 (Intel/AMD) or arm64.
        #[arg(long, default_value = "amd64")]
        arch: String,
    },
    /// Collect entries into dist/catalog-2.json (every processor) and
    /// dist/catalog.json (x86-64 only, for Iliada 0.1).
    Assemble {
        #[arg(long)]
        out: PathBuf,
        /// Only these recipes (those that passed the launch test).
        #[arg(long, value_delimiter = ',')]
        include: Vec<String>,
    },
    /// Sign catalogs, writing <file>.sig next to each.
    Sign {
        #[arg(long)]
        key: PathBuf,
        #[arg(required = true)]
        catalogs: Vec<PathBuf>,
    },
    /// Package an Iliada release: the committed source tree as
    /// iliada-<version>-source.tar.zst, and a signed release.json naming it.
    /// Upload both (and release.json.sig) to the GitHub release v<version>.
    /// The committed VERSION file must say <version>.
    Release {
        /// A git checkout of iliada_os at the release commit.
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        version: String,
        #[arg(long)]
        key: PathBuf,
        #[arg(long)]
        out: PathBuf,
        /// Where the source archive will be downloadable from.
        #[arg(long, default_value = "https://github.com/cseas002/iliada_os/releases/download")]
        base_url: String,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recipe {
    name: String,
    /// User folders the app works with ("pictures", "documents", …): asked
    /// for when it first runs.
    #[serde(default)]
    folders: Vec<iliada_types::KnownDir>,
    /// Any other permissions, as in Permissions.toml.
    #[serde(default)]
    permissions: Vec<iliada_bundle::PermissionRequest>,
    /// Files to put in place in the app (destination = source), what a
    /// Debian maintainer script would have done.
    #[serde(default)]
    copy: BTreeMap<String, String>,
    summary: String,
    category: String,
    source: RecipeSource,
    exec: Option<String>,
    #[serde(default)]
    network: bool,
    #[serde(default)]
    full_closure: bool,
    /// Extra packages unpacked into the app (plugins, programs outside the library path).
    #[serde(default)]
    include: Vec<String>,
    /// Arguments always passed to the program.
    #[serde(default)]
    args: Vec<String>,
    /// Environment always set for the app.
    #[serde(default)]
    env: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecipeSource {
    debian: String,
}

/// Git tag / file-name safe form of a Debian version (`1:48.1-2+b1`).
fn safe(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || "._-".contains(c) { c } else { '_' }).collect()
}

fn pack(app_dir: &Path, dest: &Path) -> Result<(String, u64)> {
    let file = fs::File::create(dest)?;
    let mut enc = zstd::Encoder::new(BufWriter::new(file), 15)?;
    {
        let mut tar = tar::Builder::new(&mut enc);
        tar.follow_symlinks(false);
        let name = app_dir.file_name().context("bundle has no name")?;
        tar.append_dir_all(name, app_dir)?;
        tar.finish()?;
    }
    enc.finish()?.flush()?;
    let mut hasher = Sha256::new();
    let mut f = fs::File::open(dest)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok((hex::encode(hasher.finalize()), fs::metadata(dest)?.len()))
}

/// Rust's name for a Debian architecture (the catalog's `archives` keys).
fn rust_arch(deb: &str) -> Result<&'static str> {
    Ok(match deb {
        "amd64" => "x86_64",
        "arm64" => "aarch64",
        _ => bail!("unsupported architecture {deb:?} (amd64 or arm64)"),
    })
}

/// Where a build's outputs go. amd64 keeps the original names.
fn arch_suffix(deb: &str) -> String {
    if deb == "amd64" { String::new() } else { format!("-{deb}") }
}

async fn build(
    recipes: &Path,
    out: &Path,
    only: &[String],
    base: &str,
    cache: Option<PathBuf>,
    deb_arch: &str,
) -> Result<()> {
    let arch = rust_arch(deb_arch)?;
    let sfx = arch_suffix(deb_arch);
    let mut stems: Vec<String> = fs::read_dir(recipes)?
        .flatten()
        .filter_map(|e| e.path().file_stem().map(|s| s.to_string_lossy().into_owned()))
        .collect();
    stems.sort();
    stems.retain(|s| only.is_empty() || only.contains(s));
    let mut failed = Vec::new();
    for stem in stems {
        let recipe: Recipe = toml::from_str(&fs::read_to_string(recipes.join(format!("{stem}.toml")))?)
            .with_context(|| format!("recipe {stem}"))?;
        eprintln!("==> {stem}: translating debian:{}", recipe.source.debian);
        let work = out.join(format!("work{sfx}")).join(&stem);
        let cfg = IngestConfig {
            out_dir: work.clone(),
            cache_dir: cache.clone().unwrap_or_else(iliada_ingest::pipeline::default_cache_dir),
            arch: Some(deb_arch.to_owned()),
            exec_override: recipe.exec.clone(),
            full_closure: recipe.full_closure,
            network: recipe.network,
            display_name: Some(recipe.name.clone()),
            include: recipe.include.clone(),
            args: recipe.args.clone(),
            env: recipe.env.clone(),
            copies: recipe.copy.clone(),
            permissions: recipe
                .folders
                .iter()
                .map(|d| iliada_bundle::PermissionRequest {
                    capability: iliada_types::Capability::Files(iliada_types::FileScope::KnownDir(*d)),
                    purpose: format!("To open and save files in your {} folder.", d.name()),
                })
                .chain(recipe.permissions.iter().cloned())
                .collect(),
            force: true,
            ..Default::default()
        };
        let resolvers: Vec<Box<dyn LibraryResolver>> = vec![Box::new(NamingRules)];
        let report = match Ingestor::new(cfg, resolvers)?.ingest(&Source::Debian(recipe.source.debian.clone())).await {
            Ok(r) => r,
            Err(e) => {
                eprintln!("    FAILED: {e:#}");
                failed.push(stem);
                continue;
            }
        };
        if !report.unresolved.is_empty() {
            eprintln!(
                "    unresolved libraries: {} (built anyway; check the launch test)",
                report.unresolved.join(", ")
            );
        }
        let tag = safe(&format!("{stem}-{}", report.version));
        let dist = out.join("dist").join(&tag);
        fs::create_dir_all(&dist)?;
        let archive_name = format!("{stem}{sfx}.app.tar.zst");
        let (sha256, size) = pack(&report.bundle, &dist.join(&archive_name))?;
        let bundle = iliada_bundle::Bundle::open(&report.bundle)?;
        let icon = bundle.info.icon.as_ref().and_then(|rel| {
            let src = bundle.layout.contents().join(rel);
            let ext = src.extension()?.to_string_lossy().into_owned();
            let name = format!("{stem}.{ext}");
            fs::copy(&src, dist.join(&name)).ok()?;
            Some(format!("{base}/{tag}/{name}"))
        });
        let entry = AppEntry {
            id: report.app_id.to_string(),
            name: recipe.name,
            summary: recipe.summary,
            category: recipe.category,
            version: report.version.clone(),
            network: recipe.network,
            archive: None,
            archives: {
                let mut a = Archives::default();
                *a.slot(arch).expect("known architecture") =
                    Some(Archive { url: format!("{base}/{tag}/{archive_name}"), sha256, size, version: None });
                a
            },
            icon,
        };
        fs::create_dir_all(out.join("entries"))?;
        fs::write(
            out.join("entries")
                .join(format!("{stem}{}.json", if sfx.is_empty() { String::new() } else { format!(".{deb_arch}") })),
            serde_json::to_vec_pretty(&entry)?,
        )?;
        // Keep the built bundle for the launch test.
        let tested = out.join(format!("bundles{sfx}"));
        fs::create_dir_all(&tested)?;
        let dest = tested.join(report.bundle.file_name().unwrap());
        let _ = fs::remove_dir_all(&dest);
        fs::rename(&report.bundle, &dest)?;
        eprintln!("    ok: {} {} ({:.1} MB) -> {}", entry.name, entry.version, size as f64 / 1e6, dest.display());
    }
    if !failed.is_empty() {
        eprintln!("failed: {}", failed.join(", "));
    }
    Ok(())
}

/// `entries/firefox.json` (amd64) or `entries/firefox.arm64.json`:
/// (recipe, Debian architecture).
fn entry_name(file_stem: &str) -> (&str, &str) {
    match file_stem.rsplit_once('.') {
        Some((recipe, arch)) if rust_arch(arch).is_ok() => (recipe, arch),
        _ => (file_stem, "amd64"),
    }
}

/// `include` names recipes (every processor) or `recipe.arch` (one).
fn assemble(out: &Path, include: &[String]) -> Result<()> {
    let mut builds: Vec<(String, String, AppEntry)> = Vec::new();
    for e in fs::read_dir(out.join("entries"))?.flatten() {
        let file_stem = e.path().file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let (recipe, deb) = entry_name(&file_stem);
        if !include.is_empty() && !include.iter().any(|i| *i == recipe || *i == format!("{recipe}.{deb}")) {
            continue;
        }
        let mut entry: AppEntry = serde_json::from_slice(&fs::read(e.path())?)?;
        // Entries written before per-processor archives: x86-64.
        if let Some(a) = entry.archive.take() {
            entry.archives.x86_64.get_or_insert(a);
        }
        builds.push((recipe.to_owned(), deb.to_owned(), entry));
    }
    // amd64 first, so its entry carries the shared details and version.
    builds.sort_by_key(|(recipe, deb, _)| (recipe.clone(), deb != "amd64"));
    let mut apps: BTreeMap<String, AppEntry> = BTreeMap::new();
    for (recipe, deb, entry) in builds {
        let arch = rust_arch(&deb)?;
        let Some(mut archive) = entry.archives.get(arch).cloned() else {
            bail!("entry {recipe}.{deb} has no {arch} archive");
        };
        match apps.get_mut(&recipe) {
            None => {
                apps.insert(recipe, entry);
            }
            Some(app) => {
                if app.id != entry.id {
                    bail!("{recipe}: the {deb} build is {} but another build is {}", entry.id, app.id);
                }
                if entry.version != app.version {
                    archive.version = Some(entry.version.clone());
                }
                *app.archives.slot(arch).expect("known architecture") = Some(archive);
            }
        }
    }
    let mut apps: Vec<AppEntry> = apps.into_values().collect();
    apps.sort_by_key(|a| a.name.to_lowercase());
    let catalog = Catalog { format: FORMAT, generated: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(), apps };
    fs::write(out.join(iliada_catalog::CATALOG_2), serde_json::to_vec_pretty(&catalog)?)?;
    let legacy = catalog.legacy();
    fs::write(out.join("catalog.json"), serde_json::to_vec_pretty(&legacy)?)?;
    for (arch, name) in [("x86_64", "Intel/AMD"), ("aarch64", "ARM")] {
        let n = catalog.apps.iter().filter(|a| a.archives.get(arch).is_some()).count();
        eprintln!("{}: {n} apps for {name}", iliada_catalog::CATALOG_2);
    }
    eprintln!("catalog.json (Iliada 0.1): {} apps", legacy.apps.len());
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()))
        .with_writer(std::io::stderr)
        .init();
    match Cli::parse().cmd {
        Cmd::Keygen { out } => {
            fs::create_dir_all(&out)?;
            let (key, public) = (out.join("catalog.key"), out.join("catalog.pub"));
            if key.exists() {
                bail!("{} exists; refusing to overwrite a signing key", key.display());
            }
            let mut seed = [0u8; 32];
            getrandom::fill(&mut seed).map_err(|e| anyhow::anyhow!("no randomness: {e}"))?;
            let sk = ed25519_dalek::SigningKey::from_bytes(&seed);
            fs::write(&key, iliada_catalog::encode_key(&seed))?;
            fs::set_permissions(&key, fs::Permissions::from_mode(0o600))?;
            fs::write(&public, iliada_catalog::encode_key(sk.verifying_key().as_bytes()) + "\n")?;
            eprintln!("wrote {} (keep it secret, back it up) and {}", key.display(), public.display());
        }
        Cmd::Build { recipes, out, only, base_url, cache, arch } => {
            build(&recipes, &out, &only, &base_url, cache, &arch).await?
        }
        Cmd::Assemble { out, include } => assemble(&out, &include)?,
        Cmd::Sign { key, catalogs } => {
            let sk = iliada_catalog::parse_signing_key(&fs::read_to_string(&key)?)?;
            for catalog in catalogs {
                let bytes = fs::read(&catalog)?;
                let sig = iliada_catalog::sign(&sk, &bytes);
                let sig_path = PathBuf::from(format!("{}.sig", catalog.display()));
                fs::write(&sig_path, sig + "\n")?;
                // Self-check before anything is published.
                iliada_catalog::verify(&sk.verifying_key(), &bytes, &fs::read_to_string(&sig_path)?)?;
                eprintln!("signed: {}", sig_path.display());
            }
        }
        Cmd::Release { source, version, key, out, base_url } => release(&source, &version, &key, &out, &base_url)?,
    }
    Ok(())
}

fn release(source: &Path, version: &str, key: &Path, out: &Path, base_url: &str) -> Result<()> {
    let sk = iliada_catalog::parse_signing_key(&fs::read_to_string(key)?)?;
    let committed = |file: &str| -> Result<String> {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(source)
            .args(["show", &format!("HEAD:{file}")])
            .output()
            .context("running git show")?;
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    // Systems learn their version from the source's VERSION: a release whose
    // source says otherwise would be offered again and again after installing.
    let declared = committed("VERSION")?.trim().to_owned();
    if declared != version {
        bail!("the source's committed VERSION is {declared:?}, not {version:?}: change VERSION and commit first");
    }
    // The programs report their Cargo version (`iliada --version`).
    for manifest in ["Cargo.toml", "apps/Cargo.toml"] {
        let text = committed(manifest)?;
        let cargo = toml::from_str::<toml::Value>(&text)
            .ok()
            .and_then(|v| v.get("workspace")?.get("package")?.get("version")?.as_str().map(str::to_owned));
        if cargo.as_deref() != Some(version) {
            bail!("the committed {manifest} declares version {cargo:?}, not {version:?}: change it and commit first");
        }
    }
    fs::create_dir_all(out)?;
    // Committed files only, under one folder.
    let tar = std::process::Command::new("git")
        .arg("-C")
        .arg(source)
        .args(["archive", "--format=tar", &format!("--prefix=iliada-{version}/"), "HEAD"])
        .output()
        .context("running git archive")?;
    if !tar.status.success() {
        bail!("git archive: {}", String::from_utf8_lossy(&tar.stderr));
    }
    let name = format!("iliada-{version}-source.tar.zst");
    let archive = zstd::encode_all(tar.stdout.as_slice(), 19)?;
    fs::write(out.join(&name), &archive)?;
    let manifest = iliada_catalog::Release {
        kind: iliada_catalog::RELEASE_KIND.into(),
        version: version.into(),
        source: format!("{base_url}/v{version}/{name}"),
        sha256: hex::encode(Sha256::digest(&archive)),
        size: archive.len() as u64,
    };
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    fs::write(out.join("release.json"), &bytes)?;
    let sig = iliada_catalog::sign(&sk, &bytes);
    iliada_catalog::verify_bytes(&sk.verifying_key(), &bytes, &sig)?;
    fs::write(out.join("release.json.sig"), sig + "\n")?;
    eprintln!("{}: {name} ({} KB), release.json, release.json.sig", out.display(), archive.len() / 1024);
    Ok(())
}
