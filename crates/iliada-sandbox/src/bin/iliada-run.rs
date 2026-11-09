//! `iliada-run`: start an app from /Applications inside its sandbox.
//!
//!     iliada-run /Applications/Calculator.app
//!     iliada-run org.debian.jq -- --version
//!
//! This is what desktop entries exported by `iliada-appd` execute, so apps
//! are never started outside a sandbox.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use iliada_bundle::{Bundle, IsolationTier};
use iliada_sandbox::{HostIsolationConfig, NetworkPolicy, SandboxSpec, backend_for};

const APPLICATIONS: &str = "/Applications";

fn usage() -> ! {
    eprintln!("usage: iliada-run [--config <isolation.json>] <bundle.app | app-id> [args...]");
    std::process::exit(2);
}

fn find_bundle(target: &str) -> Result<Bundle> {
    if Path::new(target).exists() {
        return Ok(Bundle::open(target)?);
    }
    for e in std::fs::read_dir(APPLICATIONS).with_context(|| format!("reading {APPLICATIONS}"))?.flatten() {
        if let Ok(b) = Bundle::open(e.path()) {
            if b.info.id.as_str() == target {
                return Ok(b);
            }
        }
    }
    bail!("no bundle {target:?} (not a path, and no app with that id in {APPLICATIONS})")
}

/// Per-user private storage, like macOS `~/Library/Containers/<id>`.
fn data_dir(app: &str) -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_DATA_HOME") {
        Some(x) => PathBuf::from(x),
        None => PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".local/share"),
    };
    Ok(base.join("iliada/apps").join(app))
}

fn wayland_socket() -> Option<PathBuf> {
    let display = std::env::var_os("WAYLAND_DISPLAY")?;
    let path = PathBuf::from(display);
    let path = if path.is_absolute() { path } else { PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join(path) };
    path.exists().then_some(path)
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut config = PathBuf::from(HostIsolationConfig::DEFAULT_PATH);
    if args.first().map(String::as_str) == Some("--config") {
        if args.len() < 2 {
            usage();
        }
        config = PathBuf::from(args.remove(1));
        args.remove(0);
    }
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        usage();
    }
    let target = args.remove(0);
    if args.first().map(String::as_str) == Some("--") {
        args.remove(0);
    }

    let host = HostIsolationConfig::load(&config)?;
    let bundle = find_bundle(&target)?;
    // The app's own setting, unless High security mode puts every app in a VM.
    let mut tier = if host.high_security { IsolationTier::MicroVm } else { bundle.info.isolation };
    if tier == IsolationTier::MicroVm {
        // Tier 2 is configured but not yet bootable (no guest image yet).
        eprintln!(
            "iliada-run: micro-VM sandboxes are not available yet; running {} in a namespace sandbox",
            bundle.info.id
        );
        tier = IsolationTier::Namespace;
    }
    // Only apps that declare it (and were approved for it at install) get a network.
    let network = if bundle.info.network { NetworkPolicy::Nat } else { NetworkPolicy::None };
    let mut spec = SandboxSpec::from_bundle(&bundle, &host, network, data_dir(bundle.info.id.as_str())?)?;
    spec.display = wayland_socket();
    spec.args = bundle.info.args.iter().cloned().chain(args).collect();
    spec.env.extend(bundle.info.env.iter().map(|(k, v)| (k.clone(), v.clone())));

    let backend = backend_for(tier, &host);
    let mut handle = backend.launch(&spec).await?;
    let code = handle.wait().await?.unwrap_or(1);
    std::process::exit(code);
}
