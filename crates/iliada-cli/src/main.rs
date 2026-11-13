//! `iliada`: one command for apps, tools, services, updates and backups.
//!
//!   iliada list | search <word> | install <name>… | remove <name>… | update
//!   iliada tool list | tool add <pkg>… | tool remove <pkg>…
//!   iliada service list | service enable <name> | service disable <name>
//!   iliada system update | system apply | system status
//!   iliada backup status | sign-in | enable <name> | now | list | restore <owner/iliada_name> | sign-out
//!   iliada ask "install firefox and turn on ssh"      (or just `iliada ask` for a conversation)
//!   iliada assistant status | on | off | mode ask|auto | model <id> | key
//!   iliada assistant email status | set <address> [--server …] | test <to> | remove
//!
//! Apps are `.app` bundles from the Store (no rebuild, no root). Everything
//! else goes through the Iliada system service, which checks that you are an
//! administrator; no sudo. Everything here is also in the Settings app.

use std::io::IsTerminal;

use anyhow::Result;
use clap::{Parser, Subcommand};
use iliada_store::{Store, StoreConfig, translate};
use iliada_system::client;
use iliada_system::protocol::{Backup, Event, Request};

const EXAMPLES: &str = "\
Examples:
  iliada install firefox          an app (it goes into Applications)
  iliada install python git       software for the terminal
  iliada install ssh              turn on a service (here: log in from other computers)
  iliada search database          find something
  iliada remove python            remove it again
  iliada system update            update Iliada itself
  iliada ask \"turn on dark mode\"  ask the assistant

Everything here is also in the Store and in Iliada Settings.";

#[derive(Parser)]
#[command(
    name = "iliada",
    version,
    about = "Apps, software, services, updates and backups",
    after_help = EXAMPLES,
    arg_required_else_help = true
)]
struct Cli {
    /// Show the full build output.
    #[arg(short, long, global = true)]
    verbose: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Everything in the Store: apps, software and services.
    List,
    /// Find something in the Store ("iliada search database").
    Search { word: String },
    /// Install apps, software (python, node, ffmpeg…) or turn on services (ssh, nginx…).
    Install {
        #[arg(required = true)]
        names: Vec<String>,
        /// Build apps that aren't in the Store from Debian's package without asking.
        #[arg(long)]
        build: bool,
    },
    /// Remove apps or software, or turn services off.
    Remove {
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// Update apps (and install any on your app list that are missing).
    Update,
    /// Any command-line program from nixpkgs, by its package name.
    Tool {
        #[command(subcommand)]
        cmd: ToolCmd,
    },
    /// Services: list them; enable/disable starting them at boot.
    Service {
        #[command(subcommand)]
        cmd: ServiceCmd,
    },
    /// What changed on this computer, and when (every change is recorded).
    History {
        /// How many changes to show.
        #[arg(short = 'n', long, default_value_t = 20)]
        count: usize,
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
    /// The model it uses: a Claude id, or an OpenRouter one ("deepseek/deepseek-v4-flash").
    Model {
        model: String,
    },
    /// on: it may also run commands as you; off: only Iliada's own actions.
    FullAccess {
        on: String,
    },
    /// Set your API key (read from the terminal, not the command line).
    Key,
    /// Your email account, which the assistant sends email from.
    Email {
        #[command(subcommand)]
        cmd: EmailCmd,
    },
}

#[derive(Subcommand)]
enum EmailCmd {
    Status,
    /// Set the account (the password is read from the terminal). Common
    /// providers (Gmail, Outlook, iCloud, …) need only the address.
    Set {
        address: String,
        /// The name shown with your address.
        #[arg(long)]
        name: Option<String>,
        /// Outgoing (SMTP) server.
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        port: Option<u16>,
        /// tls (port 465), starttls (587), or none (a server on this computer only).
        #[arg(long)]
        security: Option<String>,
        /// Sign-in name, if it isn't the address.
        #[arg(long)]
        username: Option<String>,
    },
    /// Send a test email.
    Test {
        to: String,
    },
    Remove,
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
            println!("Apps (iliada install <name>)");
            for a in &catalog.apps {
                let mark = match installed.get(&a.id) {
                    Some(i) if i.version == a.version => "installed",
                    Some(_) => "update available",
                    None => "",
                };
                println!("  {:<20} {:<10} {:<34} {mark}", a.name, a.category, a.summary);
            }
            let st = client::status().await.ok();
            use iliada_system::software::{CATALOG, Kind};
            for (kind, title) in [
                (Kind::Software, "Software for the terminal (iliada install <id>)"),
                (Kind::Service, "Services (iliada install <id> turns one on)"),
            ] {
                println!("\n{title}");
                for i in CATALOG.iter().filter(|i| i.kind == kind) {
                    let on = st.as_ref().is_some_and(|st| match kind {
                        Kind::Software => st.tools.iter().any(|t| t == i.id),
                        Kind::Service => st.services.iter().any(|x| x.name == i.id && x.on),
                    });
                    let mark = match (kind, on) {
                        (_, false) => "",
                        (Kind::Software, true) => "installed",
                        (Kind::Service, true) => "on",
                    };
                    println!("  {:<12} {:<16} {:<44} {mark}", i.id, i.name, i.summary);
                }
            }
        }
        Cmd::Search { word } => search(&word).await?,
        Cmd::Install { names, build } => install(names, build, v).await?,
        Cmd::Remove { names } => remove(names, v).await?,
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
            // Installed and started at boot are settings (kept after a
            // restart); "now" is what is running (`systemctl start/stop`).
            println!("{:<12} {:<10} {:<9} {:<9} ", "SERVICE", "INSTALLED", "AT BOOT", "NOW");
            for s in client::status().await?.services {
                let now = iliada_system::settings::unit_of_service(&s.name)
                    .filter(|_| s.on)
                    .map(|u| {
                        let active = std::process::Command::new("systemctl")
                            .args(["is-active", "--quiet", u])
                            .stdin(std::process::Stdio::null())
                            .status()
                            .is_ok_and(|st| st.success());
                        if active { "running" } else { "stopped" }
                    })
                    .unwrap_or("");
                let (installed, boot) = match (s.on, s.boot) {
                    (false, _) => ("no", ""),
                    (true, true) => ("yes", "yes"),
                    (true, false) => ("yes", "no"),
                };
                println!("{:<12} {:<10} {:<9} {:<9} {}", s.name, installed, boot, now, s.description);
            }
        }
        Cmd::History { count } => {
            for c in iliada_system::changes(count)? {
                println!("{:<16} {}", ago(c.time), c.what);
            }
        }
        Cmd::Service { cmd } => {
            let (name, boot) = match cmd {
                ServiceCmd::Enable { name } => (name, true),
                ServiceCmd::Disable { name } => (name, false),
                ServiceCmd::List => unreachable!(),
            };
            let r = system(Request::SetServiceBoot { name: name.clone(), boot }, v).await?;
            let what = if boot { "starts at every boot" } else { "doesn't start at boot (systemctl start starts it)" };
            eprintln!("{name} {}{what}.", if changed(&r) { "now " } else { "already " });
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

/// The name people type for an app: "LibreOffice Writer" is `libreoffice-writer`.
fn short_name(name: &str) -> String {
    name.to_lowercase().replace(' ', "-")
}

fn is_app(id: &str, name: &str, typed: &str) -> bool {
    let t = typed.to_lowercase();
    id == typed || name.to_lowercase() == t || short_name(name) == t
}

fn not_in_store(name: &str) -> anyhow::Error {
    anyhow::anyhow!(
        "there is nothing called \"{name}\" in the Store. Look for it with `iliada search {name}`. \
         Any other command-line program from nixpkgs can be installed with `iliada tool add {name}`."
    )
}

fn names(items: &[&iliada_system::software::Item]) -> String {
    items.iter().map(|i| i.name).collect::<Vec<_>>().join(", ")
}

/// Install apps and software, turn on services: whatever each name is.
async fn install(names_typed: Vec<String>, build: bool, v: bool) -> Result<()> {
    use iliada_system::software::{Kind, lookup};
    let (mut software, mut services, mut apps) = (Vec::new(), Vec::new(), Vec::new());
    for n in &names_typed {
        match lookup(n) {
            Some(i) if i.kind == Kind::Software => software.push(i),
            Some(i) => services.push(i),
            None => apps.push(n.as_str()),
        }
    }
    // Every name is checked before anything is installed.
    let mut entries = Vec::new();
    // Not in the Store, but Debian has it: Iliada can build it.
    let mut to_build = Vec::new();
    let debian = if apps.is_empty() { Vec::new() } else { translate::debian_apps() };
    let s = if apps.is_empty() { None } else { Some(store().await?) };
    if let Some(s) = &s {
        let catalog = s.catalog().await?;
        for n in apps {
            match catalog.apps.iter().find(|a| is_app(&a.id, &a.name, n)) {
                Some(e) => entries.push(e.clone()),
                None => to_build.push(translate::find(&debian, n).cloned().ok_or_else(|| not_in_store(n))?),
            }
        }
    }
    if !software.is_empty() {
        let packages = software.iter().map(|i| i.id.to_owned()).collect();
        let r = system(Request::AddTools { packages }, v).await?;
        if changed(&r) {
            eprintln!("{} installed. Open a new terminal to use it.", names(&software));
        } else {
            eprintln!("{} already installed.", names(&software));
        }
    }
    for i in services {
        let r = system(Request::SetService { name: i.id.into(), on: true }, v).await?;
        eprintln!(
            "{} {}.",
            i.name,
            if changed(&r) { "is installed and running; it starts at every boot" } else { "was already installed" }
        );
    }
    if let Some(s) = &s {
        for entry in &entries {
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
    }
    for app in &to_build {
        build_from_debian(app, build).await?;
    }
    Ok(())
}

/// An app that isn't in the Store: built from Debian's package on this
/// computer, after asking (or with --build).
async fn build_from_debian(app: &translate::DebianApp, yes: bool) -> Result<()> {
    eprintln!(
        "{} isn't in the Iliada Store. Iliada can build it from Debian's package \"{}\": it becomes \
         an ordinary sandboxed app, but a few apps don't work this way.",
        app.name, app.package
    );
    if !yes {
        anyhow::ensure!(std::io::stdin().is_terminal(), "run `iliada install --build {}` to build it", app.package);
        eprint!("Build it? [y/N] ");
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if !matches!(line.trim(), "y" | "Y" | "yes" | "Yes") {
            eprintln!("Not built.");
            return Ok(());
        }
    }
    let list = StoreConfig::from_env()?.app_list.unwrap_or_else(|| "/etc/iliada/apps.toml".into());
    translate::request(&list, &app.package)?;
    let name = app.name.clone();
    let r = translate::wait(&app.package, std::time::Duration::from_secs(30 * 60), &move |state| match state {
        translate::BuildState::Queued => eprintln!("Waiting to build {name}…"),
        translate::BuildState::Building => eprintln!("Building {name} (this takes a minute or two)…"),
        _ => {}
    })
    .await;
    match r {
        Ok(()) => {
            eprintln!("{} is in Applications.", app.name);
            Ok(())
        }
        Err(e) => anyhow::bail!("{} could not be built: {e:#}", app.name),
    }
}

/// Remove apps and software, turn services off.
async fn remove(names_typed: Vec<String>, v: bool) -> Result<()> {
    use iliada_system::software::{Kind, lookup};
    let s = store().await?;
    let installed = s.installed();
    let tools = client::status().await.map(|st| st.tools).unwrap_or_default();
    let (mut packages, mut shown, mut services, mut apps) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for n in &names_typed {
        if let Some(app) = installed.values().find(|i| is_app(&i.id, &i.name, n)) {
            apps.push(app.clone());
            continue;
        }
        match lookup(n) {
            Some(i) if i.kind == Kind::Software => {
                packages.push(i.id.to_owned());
                shown.push(i.name.to_owned());
            }
            Some(i) => services.push(i),
            None if tools.contains(n) => {
                packages.push(n.clone());
                shown.push(n.clone());
            }
            None => anyhow::bail!("\"{n}\" is not installed (`iliada list` shows what is)"),
        }
    }
    if !packages.is_empty() {
        let r = system(Request::RemoveTools { packages }, v).await?;
        let what = shown.join(", ");
        eprintln!("{}", if changed(&r) { format!("Removed {what}.") } else { format!("{what}: not installed.") });
    }
    for i in services {
        let r = system(Request::SetService { name: i.id.into(), on: false }, v).await?;
        eprintln!("{} {}.", i.name, if changed(&r) { "is removed" } else { "wasn't installed" });
    }
    for app in apps {
        s.remove(&app.id)?;
        eprintln!("Removed {}.", app.name);
    }
    Ok(())
}

/// Apps, software and services that mention `word`.
async fn search(word: &str) -> Result<()> {
    use iliada_system::software::Kind;
    let w = word.to_lowercase();
    let mut found = false;
    // Apps the Store has aren't offered again from Debian.
    let mut in_store: Vec<String> = Vec::new();
    match async { store().await?.catalog().await }.await {
        Ok(catalog) => {
            in_store = catalog.apps.iter().map(|a| a.name.to_lowercase()).collect();
            let apps: Vec<_> = catalog
                .apps
                .iter()
                .filter(|a| [&a.id, &a.name, &a.summary, &a.category].iter().any(|f| f.to_lowercase().contains(&w)))
                .collect();
            if !apps.is_empty() {
                found = true;
                println!("Apps");
                for a in apps {
                    println!("  {:<22} {:<36} iliada install {}", a.name, a.summary, short_name(&a.name));
                }
            }
        }
        Err(e) => eprintln!("(The Store's app list could not be reached: {e:#})"),
    }
    let items = iliada_system::software::search(word);
    for (kind, title) in [(Kind::Software, "Software for the terminal"), (Kind::Service, "Services")] {
        let list: Vec<_> = items.iter().filter(|i| i.kind == kind).collect();
        if !list.is_empty() {
            found = true;
            println!("{title}");
            for i in list {
                println!("  {:<22} {:<36} iliada install {}", i.name, i.summary, i.id);
            }
        }
    }
    // Not in the Store, but Debian has it (Iliada builds it on this computer).
    let debian = translate::debian_apps();
    let more: Vec<_> = translate::search(&debian, word)
        .into_iter()
        .filter(|a| !in_store.contains(&a.name.to_lowercase()))
        .take(12)
        .collect();
    if !more.is_empty() {
        found = true;
        println!("Not in the Store yet: Iliada can build these from Debian's package");
        for a in more {
            let summary: String = a.summary.chars().take(36).collect();
            println!(
                "  {:<22} {:<36} iliada install {}",
                a.name.chars().take(22).collect::<String>(),
                summary,
                a.package
            );
        }
    }
    if !found {
        println!("Nothing in the Store matches \"{word}\".");
        println!("Command-line programs from nixpkgs (search.nixos.org/packages): iliada tool add <name>");
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
    use iliada_system::protocol::{AssistantMode, Provider};
    if let AssistantCmd::Email { cmd } = cmd {
        return email(cmd).await;
    }
    let mut s = client::status().await?.assistant;
    match cmd {
        AssistantCmd::Status => {
            let provider = Provider::of(&s.model);
            let key = if iliada_assistant::api::load_key(provider).is_some() { "set" } else { "not set" };
            let mode = match s.effective_mode() {
                AssistantMode::Auto => "auto (acts on its own)",
                AssistantMode::Ask => "ask (every action needs your OK)",
            };
            println!("Assistant:   {}", if s.enabled { "on" } else { "off" });
            println!("Mode:        {mode}");
            println!("Full access: {}", if s.full_access { "on (may run commands as you)" } else { "off" });
            println!("Model:       {} ({})", s.model, provider.name());
            println!("API key:     {key}");
            println!("Spent:       ${:.4}", iliada_assistant::total_spent());
            return Ok(());
        }
        AssistantCmd::Key => {
            // Typed or pasted at the keyboard, or read from a file
            // (`iliada assistant key < key.txt`, a .env file works too).
            let mut text = String::new();
            if std::io::stdin().is_terminal() {
                eprint!("API key (Anthropic sk-ant-… or OpenRouter sk-or-…): ");
                std::io::stdin().read_line(&mut text)?;
            } else {
                std::io::Read::read_to_string(&mut std::io::stdin(), &mut text)?;
            }
            let key = iliada_assistant::api::find_key(&text).unwrap_or(text.trim());
            let p = iliada_assistant::api::save_key(key)?;
            eprintln!("{} key saved (only you can read it).", p.name());
            return Ok(());
        }
        AssistantCmd::Email { .. } => unreachable!("handled above"),
        AssistantCmd::On => s.enabled = true,
        AssistantCmd::Off => s.enabled = false,
        AssistantCmd::Mode { mode } => {
            s.mode = match mode.as_str() {
                "ask" => AssistantMode::Ask,
                "auto" => AssistantMode::Auto,
                _ => anyhow::bail!("mode is \"ask\" or \"auto\""),
            };
            if s.mode == AssistantMode::Auto && !s.recommended_for_auto() {
                eprintln!("Note: {} may make more mistakes acting on its own than Opus, Fable or Sonnet.", s.model);
            }
        }
        AssistantCmd::Model { model } => s.model = model,
        AssistantCmd::FullAccess { on } => {
            s.full_access = match on.as_str() {
                "on" => true,
                "off" => false,
                _ => anyhow::bail!("full-access is \"on\" or \"off\""),
            }
        }
    }
    system(Request::SetAssistant { settings: s }, v).await?;
    eprintln!("Saved.");
    Ok(())
}

async fn email(cmd: EmailCmd) -> Result<()> {
    use iliada_assistant::email::{self, Email, EmailAccount, Security};
    match cmd {
        EmailCmd::Status => match email::load() {
            None => println!("No email account (iliada assistant email set you@example.com)."),
            Some(a) => {
                println!("Address:  {}", a.address);
                println!("Server:   {}:{} ({:?})", a.server, a.port(), a.security);
                println!("Password: {}", if email::has_password() { "saved" } else { "not set" });
            }
        },
        EmailCmd::Set { address, name, server, port, security, username } => {
            let mut a = EmailAccount::suggested(&address)
                .unwrap_or_else(|| EmailAccount { address: address.clone(), ..Default::default() });
            if let Some(s) = server {
                a.server = s;
            }
            anyhow::ensure!(!a.server.is_empty(), "give the outgoing mail server with --server");
            if let Some(s) = security {
                a.security = match s.as_str() {
                    "tls" => Security::Tls,
                    "starttls" => Security::Starttls,
                    "none" => Security::None,
                    _ => anyhow::bail!("security is tls, starttls or none"),
                };
            }
            a.port = port.or(a.port);
            a.name = name.unwrap_or_default();
            a.username = username.unwrap_or_default();
            a.validate()?;
            let password = if a.security == Security::None {
                None
            } else {
                eprint!("Password for {} (for Gmail, iCloud and others: an app password): ", a.username());
                let mut p = String::new();
                std::io::stdin().read_line(&mut p)?;
                Some(p.trim_end_matches(['\n', '\r']).to_owned())
            };
            email::save(&a, password.as_deref())?;
            eprintln!("Saved (only you can read it).");
        }
        EmailCmd::Test { to } => {
            let e = Email {
                to: vec![to.clone()],
                subject: "Iliada test email".into(),
                body: "This is a test email sent by Iliada.".into(),
                ..Default::default()
            };
            email::send(&e).await?;
            eprintln!("Sent to {to}.");
        }
        EmailCmd::Remove => {
            email::remove()?;
            eprintln!("Removed.");
        }
    }
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
