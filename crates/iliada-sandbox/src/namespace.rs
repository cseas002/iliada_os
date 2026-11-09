//! Tier 1: bubblewrap namespace sandbox.
//!
//! The app gets a fresh mount, PID, IPC, UTS, cgroup, user (and, unless the
//! policy grants NAT, network) namespace. Its view of the filesystem is
//! assembled from read-only binds: bundle `Root` at `/`, the runtime base at
//! `/lib` + `/usr/lib/iliada-runtime`, bundled libraries at `/app/lib`; the
//! only writable paths are its data dir and a tmpfs. The broker socket is
//! bound at `/run/iliada/broker.sock`.
//!
//! Phase 2 adds a seccomp filter (`--seccomp <fd>`) and Landlock via the
//! guest agent.

use std::path::Path;
use std::process::Stdio;

use anyhow::{Context, Result};
use async_trait::async_trait;
use iliada_bundle::IsolationTier;
use iliada_ipc::transport;
use iliada_types::AppId;
use tokio::process::{Child, Command};

use crate::{HostIsolationConfig, IsolationBackend, NetworkPolicy, SandboxHandle, SandboxSpec};

pub const GUEST_BROKER_SOCKET: &str = "/run/iliada/broker.sock";

/// Build the full bubblewrap argument vector (everything after `bwrap`).
pub fn bwrap_args(spec: &SandboxSpec, host: &HostIsolationConfig) -> Vec<String> {
    let contents = spec.bundle.join("Contents");
    let p = |x: &Path| x.display().to_string();
    let mut a: Vec<String> = vec!["--unshare-all".into()];
    if spec.network == NetworkPolicy::Nat {
        a.push("--share-net".into());
    }
    a.extend(
        [
            "--die-with-parent",
            "--new-session",
            "--cap-drop",
            "ALL",
            "--clearenv",
            "--ro-bind",
            &p(&contents.join("Root")),
            "/",
            "--ro-bind",
            &p(&host.runtime_dir.join("lib")),
            "/lib",
            "--ro-bind",
            &p(&host.runtime_dir),
            "/usr/lib/iliada-runtime",
            "--ro-bind",
            &p(&contents.join("Libraries")),
            "/app/lib",
            "--bind",
            &p(&spec.data_dir),
            "/home/app",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--tmpfs",
            "/tmp",
            "--ro-bind",
            &p(&transport::broker_socket(&host.socket_dir, &spec.app_id)),
            GUEST_BROKER_SOCKET,
            "--setenv",
            "HOME",
            "/home/app",
            "--setenv",
            "LD_LIBRARY_PATH",
            "/app/lib:/usr/lib/iliada-runtime/lib",
            "--setenv",
            "ILIADA_BROKER",
            GUEST_BROKER_SOCKET,
            "--setenv",
            "ILIADA_APP_ID",
            spec.app_id.as_str(),
            "--",
        ]
        .map(str::to_owned),
    );
    a.push(format!("/{}", spec.entry));
    a.extend(spec.args.iter().cloned());
    a
}

#[derive(Debug)]
pub struct NamespaceBackend {
    host: HostIsolationConfig,
}

impl NamespaceBackend {
    pub fn new(host: HostIsolationConfig) -> Self {
        Self { host }
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
        let child = Command::new(&self.host.bwrap)
            .args(bwrap_args(spec, &self.host))
            .stdin(Stdio::null())
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

    fn spec(network: NetworkPolicy) -> SandboxSpec {
        SandboxSpec {
            app_id: AppId::new("org.debian.jq").unwrap(),
            bundle: "/Applications/jq.app".into(),
            entry: "usr/bin/jq".into(),
            data_dir: "/var/lib/iliada/apps/org.debian.jq".into(),
            network,
            vcpus: 1,
            memory_mib: 512,
            args: vec!["--version".into()],
        }
    }

    fn pair(args: &[String], flag: &str, src: &str) -> Option<String> {
        args.windows(3).find(|w| w[0] == flag && w[1] == src).map(|w| w[2].clone())
    }

    #[test]
    fn isolates_by_default() {
        let a = bwrap_args(&spec(NetworkPolicy::None), &HostIsolationConfig::example());
        assert_eq!(a[0], "--unshare-all");
        assert!(!a.contains(&"--share-net".to_string()));
        assert_eq!(pair(&a, "--ro-bind", "/Applications/jq.app/Contents/Root").as_deref(), Some("/"));
        assert_eq!(pair(&a, "--bind", "/var/lib/iliada/apps/org.debian.jq").as_deref(), Some("/home/app"));
        assert_eq!(
            pair(&a, "--ro-bind", "/run/iliada/sandbox/org.debian.jq.vsock_7000").as_deref(),
            Some(GUEST_BROKER_SOCKET)
        );
        assert_eq!(&a[a.len() - 2..], ["/usr/bin/jq", "--version"]);
        // The only writable bind is the data dir.
        assert_eq!(a.iter().filter(|x| *x == "--bind").count(), 1);
    }

    #[test]
    fn nat_shares_network() {
        let a = bwrap_args(&spec(NetworkPolicy::Nat), &HostIsolationConfig::example());
        assert!(a.contains(&"--share-net".to_string()));
    }
}
