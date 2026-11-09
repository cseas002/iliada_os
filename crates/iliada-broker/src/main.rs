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

    let mut paths = cli.bundles.clone();
    if let Some(dir) = &cli.applications {
        for e in std::fs::read_dir(dir)?.flatten() {
            if e.path().extension().is_some_and(|x| x == "app") {
                paths.push(e.path());
            }
        }
    }
    let mut apps = Vec::new();
    for p in &paths {
        match Bundle::open(p) {
            Ok(b) => apps.push(AppContext::from(&b)),
            Err(e) => tracing::warn!(bundle = %p.display(), error = %e, "skipping invalid bundle"),
        }
    }
    if apps.is_empty() {
        bail!("no valid bundles to serve (use --bundle or --applications)");
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
    tracing::info!(audit = %broker.audit_path().display(), "audit log");
    tokio::signal::ctrl_c().await?;
    for (path, _) in &listeners {
        let _ = std::fs::remove_file(path);
    }
    Ok(())
}
