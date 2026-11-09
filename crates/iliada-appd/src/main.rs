use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "iliada-appd", version, about = "Export /Applications bundles to the desktop")]
struct Cli {
    #[arg(long, default_value = "/Applications")]
    applications: PathBuf,
    /// An XDG data dir; entries go to <export>/applications.
    #[arg(long, default_value = "/var/lib/iliada-desktop")]
    export: PathBuf,
    /// The launcher every exported entry executes.
    #[arg(long, default_value = "/run/current-system/sw/bin/iliada-run")]
    iliada_run: PathBuf,
    /// Sync once and exit instead of watching.
    #[arg(long)]
    once: bool,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let cli = Cli::parse();
    loop {
        match iliada_appd::sync(&cli.applications, &cli.export, &cli.iliada_run) {
            Ok(r) => {
                for f in &r.written {
                    tracing::info!(entry = %f, "exported");
                }
                for f in &r.removed {
                    tracing::info!(entry = %f, "removed");
                }
            }
            Err(e) => tracing::error!(error = %format!("{e:#}"), "sync failed"),
        }
        if cli.once {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}
