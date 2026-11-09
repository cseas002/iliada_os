//! Tier 2: one cloud-hypervisor micro-VM per app.
//!
//! Device model (nothing else is attached):
//! * virtio-fs `app`  — the `.app` bundle, **read-only** (virtiofsd `--readonly`)
//! * virtio-fs `data` — the app's private data directory, read-write
//! * virtio-vsock     — the only channel to the host; the guest agent dials
//!   `CID 2 : 7000` and cloud-hypervisor forwards it to the broker socket
//! * virtio-rng, a null serial console
//! * virtio-net       — only when the system policy grants NAT
//! * (Phase 2) virtio-gpu / Wayland cross-domain for display
//!
//! The runtime base is part of the guest initrd, so it is covered by the
//! same dm-verity root of trust as the host image.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use iliada_bundle::IsolationTier;
use iliada_ipc::{BROKER_VSOCK_PORT, transport};
use iliada_types::AppId;
use serde::Serialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};

use crate::{HostIsolationConfig, IsolationBackend, NetworkPolicy, SandboxHandle, SandboxSpec};

/// `PUT /api/v1/vm.create` body (cloud-hypervisor `VmConfig`, subset).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct VmConfig {
    pub payload: Payload,
    pub cpus: Cpus,
    pub memory: Memory,
    pub fs: Vec<Fs>,
    pub vsock: Vsock,
    pub rng: Rng,
    pub serial: ConsoleCfg,
    pub console: ConsoleCfg,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net: Option<Vec<Net>>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Payload {
    pub kernel: PathBuf,
    pub initramfs: PathBuf,
    pub cmdline: String,
}
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Cpus {
    pub boot_vcpus: u8,
    pub max_vcpus: u8,
}
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Memory {
    pub size: u64,
    /// Required for virtio-fs (vhost-user needs shared guest memory).
    pub shared: bool,
}
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Fs {
    pub tag: String,
    pub socket: PathBuf,
    pub num_queues: u8,
    pub queue_size: u16,
}
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Vsock {
    pub cid: u32,
    pub socket: PathBuf,
}
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Rng {
    pub src: PathBuf,
}
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ConsoleCfg {
    pub mode: String,
}
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Net {
    pub tap: String,
}

/// Per-app socket paths under the host socket directory.
#[derive(Debug, Clone)]
pub struct VmSockets {
    pub api: PathBuf,
    pub vsock: PathBuf,
    pub app_fs: PathBuf,
    pub data_fs: PathBuf,
}

impl VmSockets {
    pub fn new(dir: &Path, app: &AppId) -> Self {
        Self {
            api: dir.join(format!("{app}.api")),
            vsock: transport::vm_vsock_socket(dir, app),
            app_fs: dir.join(format!("{app}.app-fs")),
            data_fs: dir.join(format!("{app}.data-fs")),
        }
    }
}

/// Guest context ids 0–2 are reserved; derive a stable one from the app id
/// so restarts keep their CID (collisions are detected by the VMM).
pub fn guest_cid(app: &AppId) -> u32 {
    let h = app.as_str().bytes().fold(0x811c9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x01000193));
    3 + (h % 0x7fff_0000)
}

pub fn vm_config(spec: &SandboxSpec, host: &HostIsolationConfig, sockets: &VmSockets) -> VmConfig {
    let cmdline = format!(
        "console=hvc0 quiet panic=-1 iliada.app={} iliada.entry=/{} iliada.broker_port={}",
        spec.app_id, spec.entry, BROKER_VSOCK_PORT
    );
    VmConfig {
        payload: Payload { kernel: host.guest_kernel.clone(), initramfs: host.guest_initrd.clone(), cmdline },
        cpus: Cpus { boot_vcpus: spec.vcpus.max(1), max_vcpus: spec.vcpus.max(1) },
        memory: Memory { size: u64::from(spec.memory_mib) << 20, shared: true },
        fs: vec![
            Fs { tag: "app".into(), socket: sockets.app_fs.clone(), num_queues: 1, queue_size: 1024 },
            Fs { tag: "data".into(), socket: sockets.data_fs.clone(), num_queues: 1, queue_size: 1024 },
        ],
        vsock: Vsock { cid: guest_cid(&spec.app_id), socket: sockets.vsock.clone() },
        rng: Rng { src: "/dev/urandom".into() },
        serial: ConsoleCfg { mode: "Null".into() },
        console: ConsoleCfg { mode: "Off".into() },
        net: match spec.network {
            NetworkPolicy::None => None,
            NetworkPolicy::Nat => Some(vec![Net { tap: format!("il{:08x}", guest_cid(&spec.app_id)) }]),
        },
    }
}

/// virtiofsd arguments for one share.
pub fn virtiofsd_args(socket: &Path, shared_dir: &Path, readonly: bool) -> Vec<String> {
    let mut a = vec![
        format!("--socket-path={}", socket.display()),
        format!("--shared-dir={}", shared_dir.display()),
        "--cache=never".into(),
        "--sandbox=namespace".into(),
        "--xattr".into(),
    ];
    if readonly {
        a.push("--readonly".into());
    }
    a
}

#[derive(Debug)]
pub struct CloudHypervisorBackend {
    host: HostIsolationConfig,
}

impl CloudHypervisorBackend {
    pub fn new(host: HostIsolationConfig) -> Self {
        Self { host }
    }
}

#[derive(Debug)]
struct VmHandle {
    app: AppId,
    api: PathBuf,
    children: Vec<Child>,
}

#[async_trait]
impl IsolationBackend for CloudHypervisorBackend {
    fn tier(&self) -> IsolationTier {
        IsolationTier::MicroVm
    }

    async fn launch(&self, spec: &SandboxSpec) -> Result<Box<dyn SandboxHandle>> {
        if !cfg!(target_os = "linux") || !Path::new("/dev/kvm").exists() {
            bail!("micro-VM sandboxes need Linux with /dev/kvm");
        }
        let sockets = VmSockets::new(&self.host.socket_dir, &spec.app_id);
        tokio::fs::create_dir_all(&self.host.socket_dir).await?;
        tokio::fs::create_dir_all(&spec.data_dir).await?;
        for s in [&sockets.api, &sockets.vsock, &sockets.app_fs, &sockets.data_fs] {
            let _ = tokio::fs::remove_file(s).await;
        }
        let spawn = |bin: &Path, args: Vec<String>| {
            Command::new(bin).args(args).stdin(Stdio::null()).kill_on_drop(true).spawn()
        };
        let mut children = vec![
            spawn(&self.host.virtiofsd, virtiofsd_args(&sockets.app_fs, &spec.bundle, true))
                .context("virtiofsd (app)")?,
            spawn(&self.host.virtiofsd, virtiofsd_args(&sockets.data_fs, &spec.data_dir, false))
                .context("virtiofsd (data)")?,
        ];
        children.push(
            spawn(&self.host.cloud_hypervisor, vec!["--api-socket".into(), format!("path={}", sockets.api.display())])
                .context("cloud-hypervisor")?,
        );
        wait_for_socket(&sockets.api).await?;
        let cfg = serde_json::to_vec(&vm_config(spec, &self.host, &sockets))?;
        api_put(&sockets.api, "vm.create", Some(&cfg)).await?;
        api_put(&sockets.api, "vm.boot", None).await?;
        tracing::info!(app = %spec.app_id, cid = guest_cid(&spec.app_id), "micro-VM booted");
        Ok(Box::new(VmHandle { app: spec.app_id.clone(), api: sockets.api, children }))
    }
}

#[async_trait]
impl SandboxHandle for VmHandle {
    fn app_id(&self) -> &AppId {
        &self.app
    }
    async fn stop(&mut self) -> Result<()> {
        let _ = api_put(&self.api, "vm.shutdown", None).await;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        for c in &mut self.children {
            let _ = c.kill().await;
        }
        Ok(())
    }
    async fn wait(&mut self) -> Result<Option<i32>> {
        // The VMM is the last child.
        let status = self.children.last_mut().context("no VMM process")?.wait().await?;
        Ok(status.code())
    }
}

async fn wait_for_socket(p: &Path) -> Result<()> {
    for _ in 0..100 {
        if p.exists() {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    bail!("{} did not appear", p.display())
}

/// Minimal HTTP/1.1 PUT over the VMM's Unix API socket.
pub async fn api_put(socket: &Path, endpoint: &str, body: Option<&[u8]>) -> Result<()> {
    let mut s = tokio::net::UnixStream::connect(socket).await?;
    let body = body.unwrap_or_default();
    let head = format!(
        "PUT /api/v1/{endpoint} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    s.write_all(head.as_bytes()).await?;
    s.write_all(body).await?;
    let mut resp = Vec::new();
    s.read_to_end(&mut resp).await?;
    let status = String::from_utf8_lossy(&resp).lines().next().unwrap_or_default().to_owned();
    if !(status.contains(" 200") || status.contains(" 204")) {
        bail!("cloud-hypervisor {endpoint} failed: {status}");
    }
    Ok(())
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
            display: None,
            vcpus: 1,
            memory_mib: 512,
            args: vec![],
        }
    }

    #[test]
    fn config_is_minimal_and_wired_to_the_broker() {
        let host = HostIsolationConfig::example();
        let s = spec(NetworkPolicy::None);
        let socks = VmSockets::new(&host.socket_dir, &s.app_id);
        let cfg = vm_config(&s, &host, &socks);
        let json = serde_json::to_value(&cfg).unwrap();

        assert_eq!(json["memory"]["size"], 512u64 << 20);
        assert_eq!(json["memory"]["shared"], true, "virtio-fs needs shared memory");
        assert!(json.get("net").is_none(), "no network device unless policy says so");
        assert_eq!(json["fs"][0]["tag"], "app");
        assert_eq!(json["vsock"]["socket"], "/run/iliada/sandbox/org.debian.jq.vsock");
        // cloud-hypervisor forwards guest port 7000 to "<vsock socket>_7000" — the broker's socket.
        assert_eq!(
            transport::broker_socket(&host.socket_dir, &s.app_id),
            PathBuf::from(format!("{}_{}", cfg.vsock.socket.display(), BROKER_VSOCK_PORT))
        );
        assert!(cfg.payload.cmdline.contains("iliada.entry=/usr/bin/jq"));
        assert!(cfg.vsock.cid >= 3);
    }

    #[test]
    fn nat_adds_exactly_one_nic() {
        let host = HostIsolationConfig::example();
        let s = spec(NetworkPolicy::Nat);
        let cfg = vm_config(&s, &host, &VmSockets::new(&host.socket_dir, &s.app_id));
        assert_eq!(cfg.net.unwrap().len(), 1);
    }

    #[test]
    fn app_share_is_readonly_data_share_is_not() {
        assert!(virtiofsd_args(Path::new("/s"), Path::new("/a"), true).contains(&"--readonly".to_string()));
        assert!(!virtiofsd_args(Path::new("/s"), Path::new("/d"), false).contains(&"--readonly".to_string()));
    }

    #[tokio::test]
    async fn api_put_speaks_http() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("api");
        let l = tokio::net::UnixListener::bind(&sock).unwrap();
        let server = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 4096];
            let n = s.read(&mut buf).await.unwrap();
            s.write_all(b"HTTP/1.1 204 No Content\r\n\r\n").await.unwrap();
            String::from_utf8_lossy(&buf[..n]).into_owned()
        });
        api_put(&sock, "vm.boot", None).await.unwrap();
        assert!(server.await.unwrap().starts_with("PUT /api/v1/vm.boot HTTP/1.1"));
    }
}
