//! `iliada-run`: start an app from /Applications inside its sandbox.
//!
//!     iliada-run /Applications/Calculator.app
//!     iliada-run org.debian.jq -- --version
//!
//! This is what desktop entries exported by `iliada-appd` execute, so apps
//! are never started outside a sandbox.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use iliada_bundle::decisions::{Decision, Decisions};
use iliada_bundle::{Bundle, IsolationTier, PermissionRequest};
use iliada_sandbox::{Grants, HostIsolationConfig, NetworkPolicy, SandboxSpec, SoundSockets, backend_for};
use iliada_types::{Capability, FileScope, KnownDir};

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
        if let Ok(b) = Bundle::open(e.path())
            && b.info.id.as_str() == target
        {
            return Ok(b);
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

fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// A user folder, as `~/.config/user-dirs.dirs` names it (default `~/<Name>`).
fn user_dir(d: KnownDir) -> Option<PathBuf> {
    let home = home()?;
    let key = format!("XDG_{}_DIR=", d.name().to_uppercase());
    let cfg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
    let configured = std::fs::read_to_string(cfg.join("user-dirs.dirs"))
        .ok()
        .and_then(|t| t.lines().find_map(|l| l.trim().strip_prefix(&key).map(|v| v.trim_matches('"').to_owned())));
    let path = match configured {
        Some(v) => PathBuf::from(v.replace("$HOME", &home.display().to_string())),
        None => home.join(d.name()),
    };
    // A folder outside the home (or the home itself) is never handed out.
    (path.starts_with(&home) && path != home).then_some(path)
}

fn sound_sockets() -> Option<SoundSockets> {
    let run = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?);
    let (pw, pulse) = (run.join("pipewire-0"), run.join("pulse/native"));
    let s = SoundSockets { pipewire: pw.exists().then_some(pw), pulse: pulse.exists().then_some(pulse) };
    (s.pipewire.is_some() || s.pulse.is_some()).then_some(s)
}

/// Ask the user, in a window Iliada draws (never the app). `None`: no
/// answer (closed, or no display): denied this time, asked again next time.
fn ask(bundle: &Bundle, req: &PermissionRequest) -> Option<Decision> {
    let mut cmd = std::process::Command::new("iliada-prompt");
    cmd.args(["--app", &bundle.info.name, "--wants", &req.capability.to_string(), "--purpose", &req.purpose]);
    if let Some(icon) = &bundle.info.icon {
        cmd.arg("--icon").arg(bundle.layout.contents().join(icon));
    }
    match cmd.status().ok()?.code() {
        Some(0) => Some(Decision::Allow),
        Some(1) => Some(Decision::Deny),
        _ => None,
    }
}

/// What this app may use: its declared permissions, as the user decided
/// them (asking now for any not decided yet).
fn grants(bundle: &Bundle) -> Grants {
    let mut g = Grants::default();
    let app = bundle.info.id.as_str();
    let path = Decisions::default_path();
    let mut decisions = path.as_deref().map(Decisions::load).unwrap_or_default();
    for req in &bundle.permissions.requests {
        let cap = &req.capability;
        // Only what a namespace sandbox can hand over is asked about.
        let supported =
            matches!(cap, Capability::Camera | Capability::Microphone | Capability::Files(FileScope::KnownDir(_)));
        if *cap == Capability::Sound {
            g.sound = g.sound.take().or_else(sound_sockets);
            continue;
        }
        if !supported {
            continue;
        }
        let decision = match decisions.get(app, cap) {
            Some(d) => d,
            None => match ask(bundle, req) {
                Some(d) => {
                    decisions.set(app, cap, d);
                    if let Some(p) = &path
                        && let Err(e) = decisions.save(p)
                    {
                        eprintln!("iliada-run: could not save the permission decision: {e}");
                    }
                    d
                }
                None => Decision::Deny,
            },
        };
        if decision == Decision::Deny {
            continue;
        }
        match cap {
            Capability::Camera => g.camera = true,
            // The microphone comes through the sound server.
            Capability::Microphone => g.sound = g.sound.take().or_else(sound_sockets),
            Capability::Files(FileScope::KnownDir(d)) => {
                if let Some(dir) = user_dir(*d) {
                    let _ = std::fs::create_dir_all(&dir);
                    g.folders.push((dir, d.name().to_owned()));
                }
            }
            _ => {}
        }
    }
    g
}

/// Arguments that are files the user opened (absolute paths, or file://
/// URLs, that exist): the app gets exactly those. System paths never.
fn opened_files(args: &[String]) -> Vec<PathBuf> {
    args.iter()
        .filter_map(|a| {
            let p = PathBuf::from(a.strip_prefix("file://").unwrap_or(a));
            let system = ["/nix", "/proc", "/sys", "/dev", "/run", "/etc", "/boot", "/Applications"];
            (p.is_absolute() && p.exists() && !system.iter().any(|s| p.starts_with(s))).then_some(p)
        })
        .collect()
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
    spec.files = opened_files(&args);
    // file:// URLs become plain paths (the file is at the same path inside).
    let args: Vec<String> =
        args.into_iter().map(|a| a.strip_prefix("file://").map(str::to_owned).unwrap_or(a)).collect();
    spec.args = bundle.info.args.iter().cloned().chain(args).collect();
    spec.grants = grants(&bundle);
    spec.env.extend(bundle.info.env.iter().map(|(k, v)| (k.clone(), v.clone())));

    let backend = backend_for(tier, &host);
    let mut handle = backend.launch(&spec).await?;
    let code = handle.wait().await?.unwrap_or(1);
    std::process::exit(code);
}
