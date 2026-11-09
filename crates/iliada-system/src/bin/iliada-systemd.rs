//! `iliada-systemd`: the Iliada system service (runs as root).
//!
//!   iliada-systemd serve                 the service (systemd starts it)
//!   iliada-systemd init  [--root /mnt]…  create /etc/iliada (installer, first boot)
//!   iliada-systemd finish --root /mnt    after installing onto /mnt

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand, ValueEnum};
use iliada_system::protocol::{BackupInfo, Event, Request, ServiceInfo, Status, UpdateInfo};
use iliada_system::{apply, backup, github, init, settings, update, users};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};
use toml_edit::DocumentMut;

#[derive(Parser)]
#[command(name = "iliada-systemd", version, about = "The Iliada system service")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Clone, Copy, ValueEnum)]
enum ProfileArg {
    Hardware,
    Vm,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the service.
    Serve {
        /// Where backup state and the GitHub sign-in are kept.
        #[arg(long, default_value = "/var/lib/iliada-system")]
        state: PathBuf,
    },
    /// Create /etc/iliada (the system's configuration repository).
    Init {
        #[arg(long, default_value = "/")]
        root: PathBuf,
        /// The Iliada source to build the system from.
        #[arg(long)]
        source: PathBuf,
        #[arg(long, value_enum, default_value = "hardware")]
        profile: ProfileArg,
        #[arg(long, default_value = "iliada")]
        user: String,
        #[arg(long)]
        full_name: Option<String>,
        #[arg(long, default_value = "iliada")]
        hostname: String,
        #[arg(long, default_value = "UTC")]
        timezone: String,
        #[arg(long)]
        auto_login: bool,
        /// Do nothing if /etc/iliada is already set up (first boot).
        #[arg(long)]
        if_missing: bool,
    },
    /// After installing onto --root: point the configuration at the source
    /// where the installed system will see it.
    Finish {
        #[arg(long)]
        root: PathBuf,
    },
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|_| "iliada".into())
}

struct Service {
    dir: PathBuf,
    backups: backup::Store,
    github_client_id: Option<String>,
    version: String,
    update_url: String,
    /// One change at a time.
    job: tokio::sync::Mutex<()>,
    busy: Mutex<Option<&'static str>>,
    updates: Mutex<UpdateInfo>,
}

/// Clears "busy" when the job ends, however it ends.
struct Busy<'a>(&'a Mutex<Option<&'static str>>);
impl Drop for Busy<'_> {
    fn drop(&mut self) {
        *self.0.lock().expect("busy lock") = None;
    }
}

type Sink = UnboundedSender<Event>;

fn logger(tx: &Sink) -> impl Fn(String) + Sync + '_ {
    move |line| {
        let _ = tx.send(Event::Log { line });
    }
}

impl Service {
    fn settings_path(&self) -> PathBuf {
        self.dir.join("settings.toml")
    }

    fn read_settings(&self) -> Result<DocumentMut> {
        Ok(std::fs::read_to_string(self.settings_path()).unwrap_or_default().parse()?)
    }

    fn status(&self, uid: u32) -> Status {
        let doc = self.read_settings().unwrap_or_default();
        let b = self.backups.load();
        Status {
            version: self.version.clone(),
            hostname: hostname(),
            admin: users::is_admin(uid),
            busy: self.busy.lock().expect("busy lock").map(str::to_owned),
            tools: settings::tools(&doc),
            services: settings::SERVICES
                .iter()
                .map(|(n, d)| ServiceInfo {
                    name: (*n).into(),
                    description: (*d).into(),
                    on: settings::service(&doc, n),
                })
                .collect(),
            backup: BackupInfo {
                available: self.github_client_id.is_some(),
                account: b.account,
                repo: b.repo,
                last_backup: b.last_backup,
                last_error: b.last_error,
            },
            system_update: self.updates.lock().expect("updates lock").clone(),
        }
    }

    async fn dispatch(self: &Arc<Self>, uid: u32, req: Request, tx: &Sink) -> Result<Value> {
        if !req.read_only() && !users::is_admin(uid) {
            bail!("Only an administrator can change this computer's settings.");
        }
        match &req {
            Request::Status => return Ok(serde_json::to_value(self.status(uid))?),
            // These wait on GitHub, not on the system: no job lock.
            Request::BackupSignIn => return self.sign_in(tx).await,
            Request::BackupList => return self.list_backups().await,
            Request::CheckSystemUpdate => {
                let r = self.check_update().await?;
                return Ok(json!({ "latest": r.version, "newer": update::newer(&r.version, &self.version) }));
            }
            _ => {}
        }
        let _job = self.job.try_lock().map_err(|_| {
            let what = self.busy.lock().expect("busy lock").unwrap_or("working");
            anyhow!("Iliada is busy ({}). Try again when it has finished.", what.to_lowercase())
        })?;
        *self.busy.lock().expect("busy lock") = Some(req.activity());
        let _busy = Busy(&self.busy);
        let log = logger(tx);
        match req {
            Request::AddTools { packages } => {
                for p in &packages {
                    if !settings::valid_tool_name(p) {
                        bail!("{p:?} is not a valid package name");
                    }
                    log(format!("Looking up {p}"));
                    if !apply::package_exists(&self.dir, p).await {
                        bail!("there is no package called {p:?}");
                    }
                }
                let mut doc = self.read_settings()?;
                let added = settings::add_tools(&mut doc, &packages)?;
                if added.is_empty() {
                    return Ok(json!({ "changed": false }));
                }
                self.change_settings(&doc, &format!("Add tools: {}", added.join(", ")), &log).await
            }
            Request::RemoveTools { packages } => {
                let mut doc = self.read_settings()?;
                let removed = settings::remove_tools(&mut doc, &packages);
                if removed.is_empty() {
                    return Ok(json!({ "changed": false }));
                }
                self.change_settings(&doc, &format!("Remove tools: {}", removed.join(", ")), &log).await
            }
            Request::SetService { name, on } => {
                let mut doc = self.read_settings()?;
                if !settings::set_service(&mut doc, &name, on)? {
                    return Ok(json!({ "changed": false }));
                }
                self.change_settings(&doc, &format!("{} {name}", if on { "Enable" } else { "Disable" }), &log).await
            }
            Request::Apply => {
                apply::rebuild(&self.dir, &log).await?;
                init::commit_all(&self.dir, "Apply configuration")?;
                self.back_up_quietly().await;
                Ok(json!({ "changed": true }))
            }
            Request::UpdateApps => {
                apply::run_logged("systemctl", &["start", "iliada-store-sync.service"], &log)
                    .await
                    .context("updating apps failed (see Settings → Apps for details)")?;
                Ok(json!({}))
            }
            Request::UpdateSystem => self.update_system(&log).await,
            Request::BackupEnable { name } => self.enable_backup(&name, &log).await,
            Request::BackupNow => {
                self.back_up().await?;
                Ok(json!({}))
            }
            Request::Restore { repo } => self.restore(&repo, &log).await,
            Request::BackupSignOut => {
                self.backups.set_token(None)?;
                self.backups.save(&backup::State::default())?;
                Ok(json!({}))
            }
            Request::Status | Request::BackupSignIn | Request::BackupList | Request::CheckSystemUpdate => {
                unreachable!("handled above")
            }
        }
    }

    /// Write new settings and apply them; put the old ones back on failure.
    async fn change_settings(&self, doc: &DocumentMut, message: &str, log: &(dyn Fn(String) + Sync)) -> Result<Value> {
        let path = self.settings_path();
        let previous = std::fs::read_to_string(&path).unwrap_or_default();
        std::fs::write(&path, doc.to_string()).with_context(|| format!("writing {}", path.display()))?;
        if let Err(e) = apply::rebuild(&self.dir, log).await {
            std::fs::write(&path, previous)?;
            return Err(e.context("nothing was changed"));
        }
        init::commit_all(&self.dir, message)?;
        self.back_up_quietly().await;
        Ok(json!({ "changed": true }))
    }

    // ── Updates ────────────────────────────────────────────────────────

    async fn check_update(&self) -> Result<update::Release> {
        let r = update::latest(&self.update_url, iliada_store_key()).await;
        let mut u = self.updates.lock().expect("updates lock");
        u.last_check = Some(now());
        match &r {
            Ok(rel) => {
                u.latest = Some(rel.version.clone());
                u.last_error = None;
            }
            Err(e) => u.last_error = Some(format!("{e:#}")),
        }
        r
    }

    async fn update_system(&self, log: &(dyn Fn(String) + Sync)) -> Result<Value> {
        log("Checking for a new version of Iliada".into());
        let rel = self.check_update().await?;
        if !update::newer(&rel.version, &self.version) {
            return Ok(json!({ "updated": false, "version": self.version }));
        }
        log(format!("Downloading Iliada {}", rel.version));
        let staging = self.dir.join(".source-new");
        let new = update::fetch_source(&rel, &staging).await?;
        let (source, old) = (self.dir.join("source"), self.dir.join(".source-old"));
        let _ = std::fs::remove_dir_all(&old);
        std::fs::rename(&source, &old)?;
        std::fs::rename(&new, &source)?;
        let _ = std::fs::remove_dir_all(&staging);
        let undo = |dir: &Path| {
            let _ = std::fs::remove_dir_all(&source);
            let _ = std::fs::rename(&old, &source);
            let _ = init::git(dir, &["checkout", "--", "flake.lock"]);
        };
        let url = format!("git+file://{}", self.dir.display());
        let locked = apply::run_logged(
            "nix",
            &["--extra-experimental-features", "nix-command flakes", "flake", "update", "iliada", "--flake", &url],
            log,
        )
        .await;
        if let Err(e) = locked.and(apply::rebuild(&self.dir, log).await) {
            undo(&self.dir);
            return Err(e.context(format!("Iliada {} could not be installed; nothing was changed", rel.version)));
        }
        let _ = std::fs::remove_dir_all(&old);
        init::commit_all(&self.dir, &format!("Update Iliada to {}", rel.version))?;
        self.back_up_quietly().await;
        Ok(json!({ "updated": true, "version": rel.version, "restart": true }))
    }

    // ── Backups ────────────────────────────────────────────────────────

    fn client_id(&self) -> Result<&str> {
        self.github_client_id.as_deref().context("backups to GitHub are not set up on this system")
    }

    fn token(&self) -> Result<String> {
        self.backups.token().context("sign in to GitHub first")
    }

    async fn sign_in(&self, tx: &Sink) -> Result<Value> {
        let id = self.client_id()?;
        let code = github::device_code(id).await?;
        let _ = tx.send(Event::SignInCode { code: code.user_code.clone(), url: code.verification_uri.clone() });
        let token = github::wait_for_token(id, &code).await?;
        let account = github::GitHub::new(token.clone()).login().await?;
        self.backups.set_token(Some(&token))?;
        let mut s = self.backups.load();
        if s.account.as_deref() != Some(&account) {
            s = backup::State { account: Some(account.clone()), ..Default::default() };
        }
        self.backups.save(&s)?;
        Ok(json!({ "account": account }))
    }

    async fn list_backups(&self) -> Result<Value> {
        let gh = github::GitHub::new(self.token()?);
        Ok(serde_json::to_value(gh.backups().await?)?)
    }

    async fn enable_backup(&self, name: &str, log: &(dyn Fn(String) + Sync)) -> Result<Value> {
        if !github::valid_name(name) {
            bail!("{name:?} can't be a backup name (letters, digits, - _ and .)");
        }
        let gh = github::GitHub::new(self.token()?);
        let account = gh.login().await?;
        log(format!("Creating {account}/{}{name}", github::PREFIX));
        let repo = gh.ensure_repo(&account, name).await?;
        let mut s = self.backups.load();
        s.account = Some(account);
        s.repo = Some(repo.clone());
        s.last_pushed = None;
        self.backups.save(&s)?;
        self.back_up().await?;
        Ok(json!({ "repo": repo }))
    }

    /// Push /etc/iliada if a backup is set up and anything changed.
    async fn back_up(&self) -> Result<()> {
        let mut s = self.backups.load();
        let Some(repo) = s.repo.clone() else { return Ok(()) };
        let token = self.token()?;
        let dir = self.dir.clone();
        let result =
            tokio::task::spawn_blocking(move || backup::push(&dir, &backup::github_url(&repo), &token, &hostname()))
                .await?;
        match result {
            Ok(commit) => {
                s.last_backup = Some(now());
                s.last_error = None;
                s.last_pushed = Some(commit);
                self.backups.save(&s)?;
                Ok(())
            }
            Err(e) => {
                s.last_error = Some(format!("{e:#}"));
                self.backups.save(&s)?;
                Err(e)
            }
        }
    }

    async fn back_up_quietly(&self) {
        if let Err(e) = self.back_up().await {
            tracing::warn!("backup failed: {e:#}");
        }
    }

    async fn restore(&self, repo: &str, log: &(dyn Fn(String) + Sync)) -> Result<Value> {
        if !github::valid_repo(repo) {
            bail!("{repo:?} is not an Iliada backup");
        }
        let token = self.token()?;
        log(format!("Restoring {repo}"));
        let (dir, r, t) = (self.dir.clone(), repo.to_owned(), token.clone());
        let previous =
            tokio::task::spawn_blocking(move || backup::restore(&dir, &backup::github_url(&r), &t, &hostname()))
                .await??;
        fix_app_list(&self.dir.join("apps.toml"));
        let mut s = self.backups.load();
        s.repo = Some(repo.to_owned());
        s.last_pushed = None;
        self.backups.save(&s)?;
        log("Applying the restored settings".into());
        let applied = apply::rebuild(&self.dir, log).await;
        if let Err(e) = &applied {
            tracing::warn!("restored settings failed to apply: {e:#}");
            std::fs::write(self.settings_path(), previous)?;
            init::commit_all(&self.dir, "Keep this computer's settings (the restored ones did not apply)")?;
        }
        log("Installing your apps".into());
        let _ = tokio::process::Command::new("systemctl")
            .args(["start", "--no-block", "iliada-store-sync.service"])
            .status()
            .await;
        self.back_up_quietly().await;
        match applied {
            Ok(()) => Ok(json!({ "repo": repo })),
            Err(e) => Err(e.context("your apps are being restored, but the restored settings could not be applied")),
        }
    }

    /// Hourly: back up app changes; daily: look for Iliada updates.
    async fn background(self: Arc<Self>) {
        let mut tick = 0u64;
        loop {
            tokio::time::sleep(Duration::from_secs(60 * 60)).await;
            tick += 1;
            if let Ok(_job) = self.job.try_lock() {
                self.back_up_quietly().await;
            }
            if tick % 24 == 1 {
                let _ = self.check_update().await;
            }
        }
    }
}

/// The app list stays writable by the Store and the app service.
fn fix_app_list(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let group = std::fs::read_to_string("/etc/group").unwrap_or_default();
    if let Some(gid) = users::gid(&group, "iliada-apps") {
        let _ = std::os::unix::fs::chown(path, Some(0), Some(gid));
    }
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o664));
}

fn iliada_store_key() -> &'static str {
    // The key the app catalog is signed with also signs Iliada releases.
    include_str!("../../../iliada-store/catalog.pub")
}

async fn handle(svc: Arc<Service>, stream: UnixStream) -> Result<()> {
    let uid = stream.peer_cred()?.uid();
    let (read, mut write) = stream.into_split();
    let mut line = String::new();
    BufReader::new(read.take(64 << 10)).read_line(&mut line).await?;
    let (tx, mut rx) = unbounded_channel::<Event>();
    let writer = tokio::spawn(async move {
        while let Some(e) = rx.recv().await {
            let mut l = serde_json::to_string(&e).expect("events serialize");
            l.push('\n');
            if write.write_all(l.as_bytes()).await.is_err() {
                break;
            }
        }
    });
    let result = match serde_json::from_str::<Request>(&line) {
        Ok(req) => svc.dispatch(uid, req, &tx).await,
        Err(e) => Err(anyhow!("bad request: {e}")),
    };
    let _ = tx.send(match result {
        Ok(result) => Event::Done { result },
        Err(e) => Event::Failed { error: format!("{e:#}") },
    });
    drop(tx);
    let _ = writer.await;
    Ok(())
}

/// After a system update the service's own binary may be new: exit (systemd
/// restarts it) once idle, so the new one runs.
fn stale() -> bool {
    let current = std::fs::canonicalize("/run/current-system/sw/bin/iliada-systemd").ok();
    let me = std::env::current_exe().ok().and_then(|p| std::fs::canonicalize(p).ok());
    matches!((current, me), (Some(c), Some(m)) if c != m)
}

async fn serve(state: PathBuf) -> Result<()> {
    let svc = Arc::new(Service {
        dir: iliada_system::config_dir(),
        backups: backup::Store { dir: state },
        github_client_id: std::env::var("ILIADA_GITHUB_CLIENT_ID").ok().filter(|s| !s.is_empty()),
        version: std::env::var("ILIADA_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").into()),
        update_url: std::env::var("ILIADA_UPDATE_URL").unwrap_or_else(|_| update::DEFAULT_URL.into()),
        job: tokio::sync::Mutex::new(()),
        busy: Mutex::new(None),
        updates: Mutex::new(UpdateInfo::default()),
    });
    svc.updates.lock().expect("updates lock").current = svc.version.clone();
    let path = iliada_system::client::socket_path();
    let _ = std::fs::remove_file(&path);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let listener = UnixListener::bind(&path).with_context(|| format!("binding {}", path.display()))?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666))?;
    }
    tracing::info!("listening on {}", path.display());
    tokio::spawn(svc.clone().background());
    loop {
        let (stream, _) = listener.accept().await?;
        let svc = svc.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(svc.clone(), stream).await {
                tracing::warn!("request failed: {e:#}");
            }
            if svc.job.try_lock().is_ok() && stale() {
                tracing::info!("a new version is installed; restarting");
                std::process::exit(0);
            }
        });
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .init();
    match Cli::parse().cmd {
        Cmd::Serve { state } => serve(state).await,
        Cmd::Init { root, source, profile, user, full_name, hostname, timezone, auto_login, if_missing } => {
            if if_missing && root.join("etc/iliada/flake.nix").exists() {
                return Ok(());
            }
            let o = init::InitOptions {
                full_name: full_name.unwrap_or_else(|| user.clone()),
                root,
                source,
                profile: match profile {
                    ProfileArg::Hardware => init::Profile::Hardware,
                    ProfileArg::Vm => init::Profile::Vm,
                },
                user,
                hostname,
                timezone,
                auto_login,
                system: init::host_system(),
            };
            init::init(&o, &mut |l| eprintln!("{l}"))
        }
        Cmd::Finish { root } => init::finish(&root),
    }
}
