//! `iliada-store`: command-line front end of the Store (the GUI uses the same engine).

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use iliada_store::{Store, StoreConfig};

#[derive(Parser)]
#[command(name = "iliada-store", version, about = "Install and update Iliada apps")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Show the apps in the catalog.
    List,
    /// Install an app (by name or id).
    Install { app: String },
    /// Update every installed app that has a newer version.
    Update,
    /// Install the Store apps on the app list that are missing, and update
    /// the rest (what the background service runs).
    Sync,
    /// Remove an app.
    Remove { app: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()))
        .with_writer(std::io::stderr)
        .init();
    let store = Store::new(StoreConfig::from_env()?)?;
    match Cli::parse().cmd {
        Cmd::List => {
            let catalog = store.catalog().await?;
            let installed = store.installed();
            for app in &catalog.apps {
                let state = match installed.get(&app.id) {
                    Some(i) if i.version == app.version => "installed",
                    Some(_) => "update available",
                    None => "",
                };
                let net = if app.network { "  [uses the internet]" } else { "" };
                println!(
                    "{:<20} {:<10} {:>7.1} MB  {:<36} {state}{net}",
                    app.name,
                    app.category,
                    app.archive.size as f64 / 1e6,
                    app.summary
                );
            }
        }
        Cmd::Install { app } => {
            let catalog = store.catalog().await?;
            let needle = app.to_lowercase();
            let entry = catalog
                .apps
                .iter()
                .find(|a| a.id == app || a.name.to_lowercase() == needle)
                .with_context(|| format!("no app called {app:?} in the catalog"))?;
            if entry.network {
                eprintln!("{} uses the internet.", entry.name);
            }
            let path = store
                .install(entry, &|done, total| {
                    eprint!("\r  downloading {}: {:>3}%", entry.name, done * 100 / total.max(1))
                })
                .await?;
            eprintln!("\ninstalled {}", path.display());
        }
        Cmd::Update => {
            let catalog = store.catalog().await?;
            let installed = store.installed();
            let mut n = 0;
            for app in &catalog.apps {
                if installed.get(&app.id).is_some_and(|i| i.version != app.version) {
                    store.install(app, &|_, _| {}).await?;
                    eprintln!("updated {} to {}", app.name, app.version);
                    n += 1;
                }
            }
            eprintln!("{n} app(s) updated");
        }
        Cmd::Sync => {
            let r = store.sync().await?;
            for a in &r.installed {
                eprintln!("installed {a}");
            }
            for a in &r.updated {
                eprintln!("updated {a}");
            }
            for (a, e) in &r.failed {
                eprintln!("FAILED {a}: {e}");
            }
            eprintln!("sync: {} installed, {} updated, {} failed", r.installed.len(), r.updated.len(), r.failed.len());
            if !r.failed.is_empty() {
                std::process::exit(1);
            }
        }
        Cmd::Remove { app } => {
            let needle = app.to_lowercase();
            let installed = store.installed();
            let id = installed
                .values()
                .find(|i| i.id == app || i.name.to_lowercase() == needle)
                .map(|i| i.id.clone())
                .with_context(|| format!("{app:?} is not installed"))?;
            store.remove(&id)?;
            eprintln!("removed {app}");
        }
    }
    Ok(())
}
