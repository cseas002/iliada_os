//! `iliada`: one command for apps, tools, services and the system.
//!
//!   iliada list | install <app> | remove <app> | update
//!   iliada tool list | tool add <pkg>… | tool remove <pkg>…
//!   iliada service list | service enable <name> | service disable <name>
//!   iliada apply
//!
//! Apps are `.app` bundles from the Store (no rebuild, no root). Tools and
//! services are system settings in /etc/iliada/settings.toml; changing them
//! applies the configuration (asks for your password once).

mod settings;

use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use iliada_store::{Store, StoreConfig};
use toml_edit::DocumentMut;

#[derive(Parser)]
#[command(name = "iliada", version, about = "Apps, tools, services and system configuration")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Apps in the Store.
    List,
    /// Install an app from the Store.
    Install { app: String },
    /// Remove an app.
    Remove { app: String },
    /// Update apps (and install any on your app list that are missing).
    Update,
    /// Command-line tools (vim, git, …), installed system-wide.
    Tool {
        #[command(subcommand)]
        cmd: ToolCmd,
    },
    /// System services (SSH server, printing).
    Service {
        #[command(subcommand)]
        cmd: ServiceCmd,
    },
    /// Apply /etc/iliada (after editing settings.toml or configuration.nix).
    Apply,
}

#[derive(Subcommand)]
enum ToolCmd {
    List,
    Add { packages: Vec<String> },
    Remove { packages: Vec<String> },
}

#[derive(Subcommand)]
enum ServiceCmd {
    List,
    Enable { name: String },
    Disable { name: String },
}

fn config_dir() -> PathBuf {
    std::env::var_os("ILIADA_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| "/etc/iliada".into())
}

fn settings_path() -> PathBuf {
    config_dir().join("settings.toml")
}

fn read_settings() -> Result<DocumentMut> {
    let text = std::fs::read_to_string(settings_path()).unwrap_or_default();
    Ok(text.parse()?)
}

fn is_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("Uid:")).map(|l| l.split_whitespace().nth(2) == Some("0")))
        .unwrap_or(false)
}

/// System changes need root: re-run this exact command through sudo.
fn ensure_root() -> Result<()> {
    if is_root() {
        return Ok(());
    }
    let exe = std::env::current_exe()?;
    let status = Command::new("sudo").arg(exe).args(std::env::args_os().skip(1)).status().context("running sudo")?;
    std::process::exit(status.code().unwrap_or(1));
}

/// Is `name` a package in the Iliada system's nixpkgs?
fn package_exists(name: &str) -> bool {
    let attr = format!("{}#nixosConfigurations.iliada.pkgs.{name}.name", config_dir().display());
    Command::new("nix")
        .args(["--extra-experimental-features", "nix-command flakes", "eval", "--raw", &attr])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Rebuild the system from /etc/iliada and record the change in its history.
fn apply(message: &str) -> Result<()> {
    let dir = config_dir();
    eprintln!("Applying the configuration (this can take a few minutes)…");
    let flake = format!("{}#iliada", dir.display());
    let ok = Command::new("nixos-rebuild").args(["switch", "--flake", &flake]).status()?.success();
    if !ok {
        bail!("applying the configuration failed; nothing was changed (the previous system is still active)");
    }
    if dir.join(".git").exists() {
        let _ = Command::new("git")
            .args(["-C", &dir.display().to_string(), "-c", "user.name=Iliada", "-c", "user.email=iliada@localhost"])
            .args(["commit", "-q", "-a", "-m", message])
            .status();
    }
    eprintln!("Done.");
    Ok(())
}

/// Write new settings, apply them, and put the old file back if applying fails.
fn change_settings(doc: &DocumentMut, message: &str) -> Result<()> {
    let path = settings_path();
    let previous = std::fs::read_to_string(&path).unwrap_or_default();
    std::fs::write(&path, doc.to_string()).with_context(|| format!("writing {}", path.display()))?;
    if let Err(e) = apply(message) {
        std::fs::write(&path, previous)?;
        return Err(e);
    }
    Ok(())
}

async fn store() -> Result<Store> {
    Store::new(StoreConfig::from_env()?)
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()))
        .with_writer(std::io::stderr)
        .init();
    match Cli::parse().cmd {
        Cmd::List => {
            let s = store().await?;
            let (catalog, installed) = (s.catalog().await?, s.installed());
            for a in &catalog.apps {
                let mark = match installed.get(&a.id) {
                    Some(i) if i.version == a.version => "installed",
                    Some(_) => "update available",
                    None => "",
                };
                println!("{:<20} {:<10} {:<34} {mark}", a.name, a.category, a.summary);
            }
        }
        Cmd::Install { app } => {
            let s = store().await?;
            let catalog = s.catalog().await?;
            let needle = app.to_lowercase();
            let entry = catalog
                .apps
                .iter()
                .find(|a| a.id == app || a.name.to_lowercase() == needle)
                .with_context(|| format!("there is no app called {app:?} in the Store (see `iliada list`)"))?;
            if entry.network {
                eprintln!("{} uses the internet.", entry.name);
            }
            s.install(entry, &|d, t| eprint!("\rDownloading {}… {:>3}%", entry.name, d * 100 / t.max(1))).await?;
            eprintln!("\n{} is in Applications.", entry.name);
        }
        Cmd::Remove { app } => {
            let s = store().await?;
            let needle = app.to_lowercase();
            let id = s
                .installed()
                .into_values()
                .find(|i| i.id == app || i.name.to_lowercase() == needle)
                .map(|i| i.id)
                .with_context(|| format!("{app:?} is not installed"))?;
            s.remove(&id)?;
            eprintln!("Removed {app}.");
        }
        Cmd::Update => {
            let r = store().await?.sync().await?;
            for a in r.installed.iter().chain(&r.updated) {
                eprintln!("✓ {a}");
            }
            for (a, e) in &r.failed {
                eprintln!("✗ {a}: {e}");
            }
            if r.installed.is_empty() && r.updated.is_empty() && r.failed.is_empty() {
                eprintln!("All apps are up to date.");
            }
        }
        Cmd::Tool { cmd: ToolCmd::List } => {
            for t in settings::tools(&read_settings()?) {
                println!("{t}");
            }
        }
        Cmd::Tool { cmd: ToolCmd::Add { packages } } => {
            ensure_root()?;
            for p in &packages {
                if !settings::valid_tool_name(p) || !package_exists(p) {
                    bail!("there is no package called {p:?}");
                }
            }
            let mut doc = read_settings()?;
            let added = settings::add_tools(&mut doc, &packages)?;
            if added.is_empty() {
                eprintln!("Already installed.");
                return Ok(());
            }
            change_settings(&doc, &format!("Add tools: {}", added.join(", ")))?;
        }
        Cmd::Tool { cmd: ToolCmd::Remove { packages } } => {
            ensure_root()?;
            let mut doc = read_settings()?;
            let removed = settings::remove_tools(&mut doc, &packages);
            if removed.is_empty() {
                eprintln!("None of those are installed.");
                return Ok(());
            }
            change_settings(&doc, &format!("Remove tools: {}", removed.join(", ")))?;
        }
        Cmd::Service { cmd: ServiceCmd::List } => {
            let doc = read_settings()?;
            for (name, what) in settings::SERVICES {
                let on = if settings::service(&doc, name) { "on" } else { "off" };
                println!("{name:<10} {on:<4} {what}");
            }
        }
        Cmd::Service { cmd } => {
            let (name, on) = match cmd {
                ServiceCmd::Enable { name } => (name, true),
                ServiceCmd::Disable { name } => (name, false),
                ServiceCmd::List => unreachable!(),
            };
            ensure_root()?;
            let mut doc = read_settings()?;
            if !settings::set_service(&mut doc, &name, on)? {
                eprintln!("{name} is already {}.", if on { "on" } else { "off" });
                return Ok(());
            }
            change_settings(&doc, &format!("{} {name}", if on { "Enable" } else { "Disable" }))?;
        }
        Cmd::Apply => {
            ensure_root()?;
            apply("Apply configuration")?;
        }
    }
    Ok(())
}
