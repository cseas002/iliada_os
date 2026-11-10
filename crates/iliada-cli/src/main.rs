//! `iliada`: one command for apps, tools, services, updates and backups.
//!
//!   iliada list | install <app> | remove <app> | update
//!   iliada tool list | tool add <pkg>… | tool remove <pkg>…
//!   iliada service list | service enable <name> | service disable <name>
//!   iliada system update | system apply | system status
//!   iliada backup status | sign-in | enable <name> | now | list | restore <owner/iliada_name> | sign-out
//!   iliada ask "install firefox and turn on ssh"      (or just `iliada ask` for a conversation)
//!   iliada assistant status | on | off | mode ask|auto | model <id> | key
//!
//! Apps are `.app` bundles from the Store (no rebuild, no root). Everything
//! else goes through the Iliada system service, which checks that you are an
//! administrator; no sudo. Everything here is also in the Settings app.

use std::io::IsTerminal;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use iliada_store::{Store, StoreConfig};
use iliada_system::client;
use iliada_system::protocol::{Backup, Event, Request};

#[derive(Parser)]
#[command(name = "iliada", version, about = "Apps, tools, services, updates and backups")]
struct Cli {
    /// Show the full build output.
    #[arg(short, long, global = true)]
    verbose: bool,
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
    /// Iliada itself.
    System {
        #[command(subcommand)]
        cmd: SystemCmd,
    },
    /// Back up apps and settings to a private GitHub repository.
    Backup {
        #[command(subcommand)]
        cmd: BackupCmd,
    },
    /// Ask the assistant to do something (no text: a conversation).
    Ask { text: Vec<String> },
    /// The assistant's settings.
    Assistant {
        #[command(subcommand)]
        cmd: AssistantCmd,
    },
}

#[derive(Subcommand)]
enum AssistantCmd {
    Status,
    On,
    Off,
    /// ask: every action needs your OK; auto: it acts on its own.
    Mode {
        mode: String,
    },
    /// The Claude model it uses.
    Model {
        model: String,
    },
    /// Set your API key (read from the terminal, not the command line).
    Key,
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

#[derive(Subcommand)]
enum SystemCmd {
    /// Version, updates, backup.
    Status,
    /// Install the newest Iliada release.
    Update,
    /// Apply /etc/iliada (after editing settings.toml or configuration.nix).
    Apply,
}

#[derive(Subcommand)]
enum BackupCmd {
    Status,
    /// Sign in to GitHub (you approve a code in your browser).
    SignIn,
    /// Back up to the private repository iliada_<name>.
    Enable {
        name: String,
    },
    /// Back up now.
    Now,
    /// Your backups on GitHub.
    List,
    /// Restore apps and settings from a backup.
    Restore {
        repo: String,
    },
    /// Stop backing up and forget the sign-in.
    SignOut,
}

/// Run a request on the system service, showing its progress.
async fn system(request: Request, verbose: bool) -> Result<serde_json::Value> {
    let interactive = std::io::stderr().is_terminal();
    let what = request.activity();
    if !what.is_empty() {
        eprintln!("{what}…");
    }
    let mut on_event = |e: &Event| match e {
        Event::Log { line } if verbose => eprintln!("{line}"),
        Event::Log { line } if interactive => {
            let short: String = line.chars().take(100).collect();
            eprint!("\r\x1b[2K  {short}");
        }
        Event::SignInCode { code, url } => {
            eprintln!("\nOpen {url} and enter the code:  {code}\n");
            eprintln!("Waiting for you to approve it on GitHub…");
        }
        _ => {}
    };
    let r = client::call(&request, &mut on_event).await;
    if interactive && !verbose {
        eprint!("\r\x1b[2K");
    }
    r
}

fn changed(v: &serde_json::Value) -> bool {
    v.get("changed").and_then(|c| c.as_bool()).unwrap_or(true)
}

fn ago(t: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(t);
    match now.saturating_sub(t) {
        s if s < 90 => "just now".into(),
        s if s < 90 * 60 => format!("{} minutes ago", s / 60),
        s if s < 36 * 3600 => format!("{} hours ago", s / 3600),
        s => format!("{} days ago", s / 86400),
    }
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
    let cli = Cli::parse();
    let v = cli.verbose;
    match cli.cmd {
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
            let last = std::sync::atomic::AtomicU64::new(u64::MAX);
            s.install(entry, &|d, t| {
                let pct = d * 100 / t.max(1);
                if last.swap(pct, std::sync::atomic::Ordering::Relaxed) != pct {
                    eprint!("\rDownloading {}… {pct:>3}%", entry.name);
                }
            })
            .await?;
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
            for t in client::status().await?.tools {
                println!("{t}");
            }
        }
        Cmd::Tool { cmd: ToolCmd::Add { packages } } => {
            let r = system(Request::AddTools { packages }, v).await?;
            eprintln!("{}", if changed(&r) { "Installed." } else { "Already installed." });
        }
        Cmd::Tool { cmd: ToolCmd::Remove { packages } } => {
            let r = system(Request::RemoveTools { packages }, v).await?;
            eprintln!("{}", if changed(&r) { "Removed." } else { "None of those are installed." });
        }
        Cmd::Service { cmd: ServiceCmd::List } => {
            for s in client::status().await?.services {
                println!("{:<10} {:<4} {}", s.name, if s.on { "on" } else { "off" }, s.description);
            }
        }
        Cmd::Service { cmd } => {
            let (name, on) = match cmd {
                ServiceCmd::Enable { name } => (name, true),
                ServiceCmd::Disable { name } => (name, false),
                ServiceCmd::List => unreachable!(),
            };
            let r = system(Request::SetService { name: name.clone(), on }, v).await?;
            let state = if on { "on" } else { "off" };
            eprintln!(
                "{}",
                if changed(&r) { format!("{name} is {state}.") } else { format!("{name} is already {state}.") }
            );
        }
        Cmd::System { cmd: SystemCmd::Status } => {
            let s = client::status().await?;
            println!("Iliada {} on {}", s.version, s.hostname);
            match (&s.system_update.latest, s.system_update.last_check) {
                (Some(l), _) if iliada_system::update::newer(l, &s.version) => {
                    println!("Iliada {l} is available: iliada system update")
                }
                (_, Some(t)) => println!("Up to date (checked {})", ago(t)),
                _ => println!("Not checked for updates yet"),
            }
            if let Some(b) = &s.busy {
                println!("Busy: {b}");
            }
        }
        Cmd::System { cmd: SystemCmd::Update } => {
            let r = system(Request::UpdateSystem, v).await?;
            if r["updated"].as_bool() == Some(true) {
                eprintln!("Iliada {} is installed. Restart to finish.", r["version"].as_str().unwrap_or("?"));
            } else {
                eprintln!("Iliada is up to date.");
            }
        }
        Cmd::System { cmd: SystemCmd::Apply } => {
            system(Request::Apply, v).await?;
            eprintln!("Done.");
        }
        Cmd::Backup { cmd } => backup(cmd, v).await?,
        Cmd::Ask { text } => ask(text.join(" ")).await?,
        Cmd::Assistant { cmd } => assistant(cmd, v).await?,
    }
    Ok(())
}

struct Terminal;

#[async_trait::async_trait]
impl iliada_assistant::Frontend for Terminal {
    async fn approve(&self, action: &str) -> bool {
        eprint!("  The assistant wants to: {action}. Allow? [y/N] ");
        let mut line = String::new();
        let _ = std::io::stdin().read_line(&mut line);
        matches!(line.trim(), "y" | "Y" | "yes" | "Yes")
    }
    fn progress(&self, text: &str) {
        eprintln!("  {text}");
    }
}

async fn ask(text: String) -> Result<()> {
    let settings = client::status().await?.assistant;
    let mut a = iliada_assistant::Assistant::new(&settings)?;
    if !text.trim().is_empty() {
        println!("{}", a.send(&text, &Terminal).await?);
        return Ok(());
    }
    let mode = match a.mode() {
        iliada_system::protocol::AssistantMode::Ask => "it asks before doing anything",
        iliada_system::protocol::AssistantMode::Auto => "it acts on its own",
    };
    eprintln!("Iliada assistant ({}; {mode}). An empty line ends.", settings.model);
    loop {
        eprint!("> ");
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 || line.trim().is_empty() {
            return Ok(());
        }
        match a.send(line.trim(), &Terminal).await {
            Ok(answer) => println!("{answer}\n"),
            Err(e) => eprintln!("{e:#}\n"),
        }
    }
}

async fn assistant(cmd: AssistantCmd, v: bool) -> Result<()> {
    use iliada_system::protocol::{AUTO_MODELS, AssistantMode};
    let mut s = client::status().await?.assistant;
    match cmd {
        AssistantCmd::Status => {
            let key = if iliada_assistant::api::load_key().is_some() { "set" } else { "not set" };
            let mode = match (s.mode, s.effective_mode()) {
                (AssistantMode::Auto, AssistantMode::Auto) => "auto (acts on its own)",
                (AssistantMode::Auto, AssistantMode::Ask) => "ask (auto needs a stronger model)",
                _ => "ask (every action needs your OK)",
            };
            println!("Assistant: {}", if s.enabled { "on" } else { "off" });
            println!("Mode:      {mode}");
            println!("Model:     {}", s.model);
            println!("API key:   {key}");
            return Ok(());
        }
        AssistantCmd::Key => {
            eprint!("Anthropic API key (from console.anthropic.com): ");
            let mut k = String::new();
            std::io::stdin().read_line(&mut k)?;
            anyhow::ensure!(k.trim().starts_with("sk-"), "that doesn't look like an API key");
            iliada_assistant::api::save_key(&k)?;
            eprintln!("Saved (only you can read it).");
            return Ok(());
        }
        AssistantCmd::On => s.enabled = true,
        AssistantCmd::Off => s.enabled = false,
        AssistantCmd::Mode { mode } => {
            s.mode = match mode.as_str() {
                "ask" => AssistantMode::Ask,
                "auto" => AssistantMode::Auto,
                _ => anyhow::bail!("mode is \"ask\" or \"auto\""),
            };
            if s.mode == AssistantMode::Auto && !AUTO_MODELS.contains(&s.model.as_str()) {
                eprintln!("Note: {} is not trusted to act on its own; it stays in ask mode.", s.model);
            }
        }
        AssistantCmd::Model { model } => s.model = model,
    }
    system(Request::SetAssistant { settings: s }, v).await?;
    eprintln!("Saved.");
    Ok(())
}

async fn backup(cmd: BackupCmd, v: bool) -> Result<()> {
    match cmd {
        BackupCmd::Status => {
            let b = client::status().await?.backup;
            if !b.available {
                println!("Backups to GitHub are not set up on this system.");
                return Ok(());
            }
            match (&b.account, &b.repo) {
                (None, _) => println!("Not signed in. Sign in with: iliada backup sign-in"),
                (Some(a), None) => println!("Signed in as {a}; not backing up yet: iliada backup enable <name>"),
                (Some(_), Some(r)) => {
                    let when = b.last_backup.map(ago).unwrap_or_else(|| "not yet".into());
                    println!("Backing up to github.com/{r} (last backup: {when})");
                }
            }
            if let Some(e) = &b.last_error {
                println!("Last backup failed: {e}");
            }
        }
        BackupCmd::SignIn => {
            let r = system(Request::BackupSignIn, v).await?;
            eprintln!("Signed in as {}.", r["account"].as_str().unwrap_or("?"));
        }
        BackupCmd::Enable { name } => {
            let r = system(Request::BackupEnable { name }, v).await?;
            eprintln!("Backing up to github.com/{}.", r["repo"].as_str().unwrap_or("?"));
        }
        BackupCmd::Now => {
            system(Request::BackupNow, v).await?;
            eprintln!("Backed up.");
        }
        BackupCmd::List => {
            let list: Vec<Backup> = client::call_as(&Request::BackupList, &mut |_| {}).await?;
            for b in list {
                println!("{:<40} {}", b.repo, b.updated);
            }
        }
        BackupCmd::Restore { repo } => {
            system(Request::Restore { repo }, v).await?;
            eprintln!("Restored. Your apps are being installed in the background.");
        }
        BackupCmd::SignOut => {
            system(Request::BackupSignOut, v).await?;
            eprintln!("Signed out; backups are off.");
        }
    }
    Ok(())
}
