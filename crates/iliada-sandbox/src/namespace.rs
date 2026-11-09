//! Tier 1: bubblewrap namespace sandbox.
//!
//! bubblewrap starts from an empty tmpfs root; the app's world is assembled
//! from binds:
//!
//! | Path in sandbox | Source | Mode |
//! |---|---|---|
//! | `/usr`, `/opt`, `/etc/*`, … | bundle `Contents/Root/*` | ro |
//! | `/lib64`, `/lib` | runtime base `lib/` (ELF loader, glibc) | ro |
//! | `/run/iliada/runtime` | runtime base (fonts, Mesa, Wayland client libs, …) | ro |
//! | `/app/lib` | bundle `Contents/Libraries` | ro |
//! | `/nix/store` | host store (runtime base symlinks resolve here; immutable) | ro |
//! | `/home/app` | the app's private data directory | **rw** |
//! | `/tmp` | fresh tmpfs | rw |
//! | `/run/user/app/wayland-0` | host Wayland socket (GUI apps only) | socket |
//! | `/run/iliada/broker.sock` | the app's broker socket | socket |
//! | `/etc/fonts`, `/run/host/share/icons`, `/dev/dri`, `/run/opengl-driver` | host fonts, icons, GPU | ro |
//!
//! All namespaces are unshared (no network unless the policy grants NAT),
//! all capabilities are dropped, and the environment is cleared. The app sees
//! no host home directory, no other apps, and no D-Bus session bus. Phase 2
//! adds a seccomp filter and a filtered D-Bus proxy for portals.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result};
use async_trait::async_trait;
use iliada_bundle::IsolationTier;
use iliada_ipc::transport;
use iliada_types::AppId;
use tokio::process::{Child, Command};

use crate::{HostIsolationConfig, IsolationBackend, NetworkPolicy, SandboxHandle, SandboxSpec};

pub const GUEST_BROKER_SOCKET: &str = "/run/iliada/broker.sock";
pub const GUEST_RUNTIME_DIR: &str = "/run/iliada/runtime";
const GUEST_XDG_RUNTIME_DIR: &str = "/run/user/app";

/// Host paths that make GUI apps usable; each is bound only if it exists.
#[derive(Debug, Clone)]
pub struct HostDesktop {
    pub fonts: PathBuf,
    pub icons: PathBuf,
    pub opengl_driver: PathBuf,
}

impl Default for HostDesktop {
    fn default() -> Self {
        Self {
            fonts: "/etc/fonts".into(),
            icons: "/run/current-system/sw/share/icons".into(),
            opengl_driver: "/run/opengl-driver".into(),
        }
    }
}

/// GNU multiarch triplet of the machine we run on.
pub fn host_triplet() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "aarch64-linux-gnu",
        "x86" => "i386-linux-gnu",
        "riscv64" => "riscv64-linux-gnu",
        _ => "x86_64-linux-gnu",
    }
}

/// Build the full bubblewrap argument vector (everything after `bwrap`).
pub fn bwrap_args(spec: &SandboxSpec, host: &HostIsolationConfig, desktop: &HostDesktop) -> Vec<String> {
    let mut a = bwrap_prefix(spec, host, desktop);
    a.push(format!("/{}", spec.entry));
    a.extend(spec.args.iter().cloned());
    a
}

/// Everything up to and including `--`; the program and its arguments follow.
fn bwrap_prefix(spec: &SandboxSpec, host: &HostIsolationConfig, desktop: &HostDesktop) -> Vec<String> {
    let contents = spec.bundle.join("Contents");
    let root = contents.join("Root");
    let p = |x: &Path| x.display().to_string();
    let mut a: Vec<String> = Vec::new();
    let mut push = |items: &[&str]| a.extend(items.iter().map(|s| (*s).to_owned()));

    push(&["--unshare-all"]);
    if spec.network == NetworkPolicy::Nat {
        push(&["--share-net"]);
    }
    push(&["--die-with-parent", "--new-session", "--cap-drop", "ALL", "--clearenv"]);

    // The upstream FHS tree, one top-level entry at a time so the runtime
    // base can supply /lib and /lib64 (the ELF loader) alongside it.
    let (mut has_lib, mut has_bin) = (false, false);
    for name in top_level_entries(&root) {
        match name.as_str() {
            "lib" | "lib64" => has_lib = true,
            "bin" => has_bin = true,
            "etc" | "run" | "proc" | "dev" | "tmp" | "home" | "app" | "nix" => continue,
            _ => {}
        }
        push(&["--ro-bind", &p(&root.join(&name)), &format!("/{name}")]);
    }
    // /etc is composed: upstream entries plus host fonts.
    push(&["--dir", "/etc"]);
    for name in top_level_entries(&root.join("etc")) {
        if name != "fonts" {
            push(&["--ro-bind", &p(&root.join("etc").join(&name)), &format!("/etc/{name}")]);
        }
    }
    push(&["--ro-bind-try", &p(&desktop.fonts), "/etc/fonts"]);
    if spec.network == NetworkPolicy::Nat {
        // Name resolution and TLS roots from the host, read-only.
        for f in ["resolv.conf", "hosts", "ssl"] {
            push(&["--ro-bind-try", &format!("/etc/{f}"), &format!("/etc/{f}")]);
        }
    }

    let runtime = &host.runtime_dir;
    push(&["--ro-bind", "/nix/store", "/nix/store"]);
    push(&["--ro-bind", &p(runtime), GUEST_RUNTIME_DIR]);
    if !has_lib {
        push(&["--ro-bind", &p(&runtime.join("lib")), "/lib64"]);
        push(&["--ro-bind", &p(&runtime.join("lib")), "/lib"]);
    }
    if !has_bin {
        // A shell and core utilities for the app's own launcher scripts.
        push(&["--ro-bind-try", &p(&runtime.join("bin")), "/bin"]);
    }
    push(&["--ro-bind", &p(&contents.join("Libraries")), "/app/lib"]);
    push(&["--bind", &p(&spec.data_dir), "/home/app"]);
    push(&["--proc", "/proc", "--dev", "/dev", "--tmpfs", "/tmp"]);
    push(&["--perms", "0700", "--dir", GUEST_XDG_RUNTIME_DIR]);
    push(&["--ro-bind-try", &p(&transport::broker_socket(&host.socket_dir, &spec.app_id)), GUEST_BROKER_SOCKET]);

    if let Some(wayland) = &spec.display {
        push(&["--ro-bind", &p(wayland), &format!("{GUEST_XDG_RUNTIME_DIR}/wayland-0")]);
        push(&["--setenv", "WAYLAND_DISPLAY", "wayland-0"]);
        push(&["--dev-bind-try", "/dev/dri", "/dev/dri"]);
        push(&["--ro-bind-try", &p(&desktop.opengl_driver), "/run/opengl-driver"]);
        push(&["--ro-bind-try", &p(&desktop.icons), "/run/host/share/icons"]);
        push(&["--setenv", "__EGL_VENDOR_LIBRARY_DIRS", "/run/opengl-driver/share/glvnd/egl_vendor.d"]);
    }

    let triplet = host_triplet();
    let ld_path = format!("/app/lib:/usr/lib/{triplet}:/usr/lib:{GUEST_RUNTIME_DIR}/lib");
    let env: [(&str, &str); 9] = [
        ("HOME", "/home/app"),
        ("XDG_RUNTIME_DIR", GUEST_XDG_RUNTIME_DIR),
        ("XDG_DATA_DIRS", "/usr/share:/run/host/share"),
        ("GSETTINGS_SCHEMA_DIR", "/usr/share/glib-2.0/schemas"),
        ("LD_LIBRARY_PATH", &ld_path),
        ("PATH", "/usr/bin:/bin"),
        ("LANG", "C.UTF-8"),
        ("ILIADA_BROKER", GUEST_BROKER_SOCKET),
        ("ILIADA_APP_ID", spec.app_id.as_str()),
    ];
    for (k, v) in env {
        push(&["--setenv", k, v]);
    }
    for (k, v) in &spec.env {
        push(&["--setenv", k, v]);
    }
    push(&["--"]);
    a
}

/// Sandboxed triggers.
///
/// Debian runs some generators at install time, as root (`postinst`
/// triggers). Iliada never runs app code outside the app's sandbox, so these
/// run *inside* it, on first launch and whenever the bundle changes. Their
/// output lives in the app's private data directory.
///
/// Currently: `gdk-pixbuf-query-loaders` (GTK 3 needs it to load any image,
/// including its own icons).
pub fn run_triggers(spec: &mut SandboxSpec, host: &HostIsolationConfig, desktop: &HostDesktop) -> Result<()> {
    let triplet = host_triplet();
    let rel = format!("usr/lib/{triplet}/gdk-pixbuf-2.0/gdk-pixbuf-query-loaders");
    let tool = spec.bundle.join("Contents/Root").join(&rel);
    if !tool.is_file() {
        return Ok(());
    }
    let cache_dir = spec.data_dir.join(".cache/iliada");
    let cache = cache_dir.join("gdk-pixbuf-loaders.cache");
    let stamp = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let bundle_time = stamp(&spec.bundle.join("Contents/Info.toml"));
    let fresh = matches!((stamp(&cache), bundle_time), (Some(c), Some(b)) if c >= b);
    if !fresh {
        std::fs::create_dir_all(&cache_dir)?;
        let mut args = bwrap_prefix(spec, host, desktop);
        args.push(format!("/{rel}"));
        let out = std::process::Command::new(&host.bwrap)
            .args(&args)
            .stdin(Stdio::null())
            .output()
            .context("running gdk-pixbuf-query-loaders in the sandbox")?;
        if out.status.success() && !out.stdout.is_empty() {
            std::fs::write(&cache, &out.stdout)?;
        } else {
            tracing::warn!(app = %spec.app_id, "gdk-pixbuf-query-loaders failed; images may not load");
            return Ok(());
        }
    }
    spec.env.push(("GDK_PIXBUF_MODULE_FILE".into(), "/home/app/.cache/iliada/gdk-pixbuf-loaders.cache".into()));
    Ok(())
}

fn top_level_entries(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

#[derive(Debug)]
pub struct NamespaceBackend {
    host: HostIsolationConfig,
    desktop: HostDesktop,
}

impl NamespaceBackend {
    pub fn new(host: HostIsolationConfig) -> Self {
        Self { host, desktop: HostDesktop::default() }
    }
}

#[derive(Debug)]
struct NsHandle {
    app: AppId,
    child: Child,
}

#[async_trait]
impl IsolationBackend for NamespaceBackend {
    fn tier(&self) -> IsolationTier {
        IsolationTier::Namespace
    }
    async fn launch(&self, spec: &SandboxSpec) -> Result<Box<dyn SandboxHandle>> {
        anyhow::ensure!(cfg!(target_os = "linux"), "namespace sandboxes need Linux");
        tokio::fs::create_dir_all(&spec.data_dir).await?;
        let mut spec = spec.clone();
        run_triggers(&mut spec, &self.host, &self.desktop)?;
        let child = Command::new(&self.host.bwrap)
            .args(bwrap_args(&spec, &self.host, &self.desktop))
            .stdin(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .context("spawning bwrap")?;
        Ok(Box::new(NsHandle { app: spec.app_id.clone(), child }))
    }
}

#[async_trait]
impl SandboxHandle for NsHandle {
    fn app_id(&self) -> &AppId {
        &self.app
    }
    async fn stop(&mut self) -> Result<()> {
        self.child.kill().await.map_err(Into::into)
    }
    async fn wait(&mut self) -> Result<Option<i32>> {
        Ok(self.child.wait().await?.code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(network: NetworkPolicy, display: bool) -> (tempfile::TempDir, SandboxSpec) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("jq.app/Contents/Root");
        std::fs::create_dir_all(root.join("usr/bin")).unwrap();
        std::fs::create_dir_all(root.join("etc/jq")).unwrap();
        std::fs::create_dir_all(root.join("etc/fonts")).unwrap();
        let spec = SandboxSpec {
            app_id: AppId::new("org.debian.jq").unwrap(),
            bundle: dir.path().join("jq.app"),
            entry: "usr/bin/jq".into(),
            data_dir: "/home/alice/.local/share/iliada/apps/org.debian.jq".into(),
            network,
            display: display.then(|| PathBuf::from("/run/user/1000/wayland-1")),
            env: vec![],
            vcpus: 1,
            memory_mib: 512,
            args: vec!["--version".into()],
        };
        (dir, spec)
    }

    fn after<'a>(args: &'a [String], flag: &str, src: &str) -> Option<&'a str> {
        args.windows(3).find(|w| w[0] == flag && w[1] == src).map(|w| w[2].as_str())
    }

    #[test]
    fn isolates_by_default() {
        let (dir, spec) = fixture(NetworkPolicy::None, false);
        let host = HostIsolationConfig::example();
        let a = bwrap_args(&spec, &host, &HostDesktop::default());
        let root = dir.path().join("jq.app/Contents/Root");
        assert_eq!(a[0], "--unshare-all");
        assert!(!a.contains(&"--share-net".to_string()));
        assert_eq!(after(&a, "--ro-bind", &root.join("usr").display().to_string()), Some("/usr"));
        assert_eq!(after(&a, "--ro-bind", &root.join("etc/jq").display().to_string()), Some("/etc/jq"));
        assert!(after(&a, "--ro-bind", &root.join("etc/fonts").display().to_string()).is_none(), "host fonts win");
        assert_eq!(after(&a, "--ro-bind", &host.runtime_dir.join("lib").display().to_string()), Some("/lib64"));
        assert_eq!(after(&a, "--ro-bind-try", &host.runtime_dir.join("bin").display().to_string()), Some("/bin"));
        assert_eq!(after(&a, "--bind", "/home/alice/.local/share/iliada/apps/org.debian.jq"), Some("/home/app"));
        assert_eq!(
            after(&a, "--ro-bind-try", "/run/iliada/sandbox/org.debian.jq.vsock_7000"),
            Some(GUEST_BROKER_SOCKET)
        );
        assert_eq!(&a[a.len() - 2..], ["/usr/bin/jq", "--version"]);
        // The only writable bind is the data dir, and no display without a socket.
        assert_eq!(a.iter().filter(|x| *x == "--bind").count(), 1);
        assert!(!a.iter().any(|x| x.contains("wayland")));
    }

    #[test]
    fn gui_apps_get_only_the_wayland_socket() {
        let (_d, spec) = fixture(NetworkPolicy::None, true);
        let a = bwrap_args(&spec, &HostIsolationConfig::example(), &HostDesktop::default());
        assert_eq!(after(&a, "--ro-bind", "/run/user/1000/wayland-1"), Some("/run/user/app/wayland-0"));
        assert!(!a.iter().any(|x| x.contains("/run/user/1000/bus")), "no host D-Bus session bus");
    }

    #[test]
    fn nat_shares_network() {
        let (_d, spec) = fixture(NetworkPolicy::Nat, false);
        let a = bwrap_args(&spec, &HostIsolationConfig::example(), &HostDesktop::default());
        assert!(a.contains(&"--share-net".to_string()));
        assert_eq!(after(&a, "--ro-bind-try", "/etc/resolv.conf"), Some("/etc/resolv.conf"));
        let (_d, spec) = fixture(NetworkPolicy::None, false);
        let a = bwrap_args(&spec, &HostIsolationConfig::example(), &HostDesktop::default());
        assert!(!a.iter().any(|x| x == "/etc/resolv.conf"), "no DNS without network");
    }
}
