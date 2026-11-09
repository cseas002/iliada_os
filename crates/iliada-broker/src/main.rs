use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Result, bail};
use clap::{Parser, ValueEnum};
use iliada_broker::{AppContext, Broker, DenyAll, Engine, GrantStore, PromptFrontend, TerminalPrompt};
use iliada_bundle::Bundle;

#[derive(Parser, Debug)]
#[command(name = "iliada-broker", version, about = "Iliada runtime permission broker")]
struct Cli {
    /// A bundle to serve (repeatable).
    #[arg(long = "bundle")]
    bundles: Vec<PathBuf>,
    /// Serve every *.app in this directory (e.g. /Applications).
    #[arg(long)]
    applications: Option<PathBuf>,
    /// Where per-app sockets are created.
    #[arg(long, default_value = "/run/iliada/broker")]
    socket_dir: PathBuf,
    /// Grant store and audit log.
    #[arg(long, default_value = "/var/lib/iliada-broker")]
    state_dir: PathBuf,
    #[arg(long, value_enum, default_value_t = Frontend::Terminal)]
    frontend: Frontend,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Frontend {
    /// Ask on this terminal (development).
    Terminal,
    /// Deny every prompt (headless).
    Deny,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();

    if cli.bundles.is_empty() && cli.applications.is_none() {
        bail!("nothing to serve: pass --bundle <path> and/or --applications <dir>");
    }
    let mut apps = Vec::new();
    for p in &cli.bundles {
        match Bundle::open(p) {
            Ok(b) => apps.push(AppContext::from(&b)),
            Err(e) => bail!("invalid bundle {}: {e}", p.display()),
        }
    }

    let frontend: Arc<dyn PromptFrontend> = match cli.frontend {
        Frontend::Terminal => Arc::new(TerminalPrompt::default()),
        Frontend::Deny => Arc::new(DenyAll),
    };
    let engine = Engine::new(GrantStore::open(&cli.state_dir)?, frontend);
    let broker = Arc::new(Broker::new(engine, &cli.state_dir));
    let listeners = broker.listen(&cli.socket_dir, apps)?;
    for (path, _) in &listeners {
        tracing::info!(socket = %path.display(), "listening");
    }
    if let Some(dir) = cli.applications.clone() {
        // Apps may be installed after the broker starts (e.g. by the Package Translator).
        tracing::info!(dir = %dir.display(), "watching for bundles");
        tokio::spawn(broker.clone().watch_applications(dir, cli.socket_dir.clone(), std::time::Duration::from_secs(2)));
    }
    tracing::info!(audit = %broker.audit_path().display(), "audit log");
    tokio::signal::ctrl_c().await?;
    for (path, _) in &listeners {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}
