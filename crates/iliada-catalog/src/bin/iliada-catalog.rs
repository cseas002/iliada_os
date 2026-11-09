//! The catalog builder (runs on the Iliada build server).
//!
//!   iliada-catalog keygen --out ~/.config/iliada
//!   iliada-catalog build --recipes iliada_apps/recipes --out dist [--only firefox,gimp]
//!   iliada-catalog assemble --out dist [--include firefox,gimp]
//!   iliada-catalog sign --key ~/.config/iliada/catalog.key dist/catalog.json

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufWriter, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use iliada_catalog::{AppEntry, Archive, Catalog, FORMAT};
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
    },
    /// Collect entries into dist/catalog.json.
    Assemble {
        #[arg(long)]
        out: PathBuf,
        /// Only these recipes (those that passed the launch test).
        #[arg(long, value_delimiter = ',')]
        include: Vec<String>,
    },
    /// Sign catalog.json, writing catalog.json.sig.
    Sign {
        #[arg(long)]
        key: PathBuf,
        catalog: PathBuf,
    },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Recipe {
    name: String,
    summary: String,
    category: String,
    source: RecipeSource,
    exec: Option<String>,
    #[serde(default)]
    network: bool,
    #[serde(default)]
    full_closure: bool,
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

async fn build(recipes: &Path, out: &Path, only: &[String], base: &str, cache: Option<PathBuf>) -> Result<()> {
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
        let work = out.join("work").join(&stem);
        let cfg = IngestConfig {
            out_dir: work.clone(),
            cache_dir: cache.clone().unwrap_or_else(iliada_ingest::pipeline::default_cache_dir),
            exec_override: recipe.exec.clone(),
            full_closure: recipe.full_closure,
            network: recipe.network,
            display_name: Some(recipe.name.clone()),
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
        let archive_name = format!("{stem}.app.tar.zst");
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
            archive: Archive { url: format!("{base}/{tag}/{archive_name}"), sha256, size },
            icon,
        };
        fs::create_dir_all(out.join("entries"))?;
        fs::write(out.join("entries").join(format!("{stem}.json")), serde_json::to_vec_pretty(&entry)?)?;
        // Keep the built bundle for the launch test.
        let tested = out.join("bundles");
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

fn assemble(out: &Path, include: &[String]) -> Result<()> {
    let mut apps: BTreeMap<String, AppEntry> = BTreeMap::new();
    for e in fs::read_dir(out.join("entries"))?.flatten() {
        let stem = e.path().file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if !include.is_empty() && !include.contains(&stem) {
            continue;
        }
        let entry: AppEntry = serde_json::from_slice(&fs::read(e.path())?)?;
        apps.insert(entry.name.to_lowercase(), entry);
    }
    let catalog = Catalog {
        format: FORMAT,
        generated: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        apps: apps.into_values().collect(),
    };
    fs::write(out.join("catalog.json"), serde_json::to_vec_pretty(&catalog)?)?;
    eprintln!("catalog.json: {} apps", catalog.apps.len());
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
        Cmd::Build { recipes, out, only, base_url, cache } => build(&recipes, &out, &only, &base_url, cache).await?,
        Cmd::Assemble { out, include } => assemble(&out, &include)?,
        Cmd::Sign { key, catalog } => {
            let sk = iliada_catalog::parse_signing_key(&fs::read_to_string(&key)?)?;
            let bytes = fs::read(&catalog)?;
            let sig = iliada_catalog::sign(&sk, &bytes);
            let sig_path = PathBuf::from(format!("{}.sig", catalog.display()));
            fs::write(&sig_path, sig + "\n")?;
            // Self-check before anything is published.
            iliada_catalog::verify(&sk.verifying_key(), &bytes, &fs::read_to_string(&sig_path)?)?;
            eprintln!("signed: {}", sig_path.display());
        }
    }
    Ok(())
}
