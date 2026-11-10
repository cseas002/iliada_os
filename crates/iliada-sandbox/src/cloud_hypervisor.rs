//! Tier 2: one cloud-hypervisor micro-VM per app.
//!
//! The app gets its own kernel. Inside the VM it runs in the same bubblewrap
//! sandbox as Tier 1 (defence in depth, and one way of assembling the app's
//! world): the guest's `iliada-guest-init` mounts the host's shares at their
//! host paths, so the Tier-1 bubblewrap arguments work unchanged.
//!
//! Devices (nothing else is attached):
//! * virtio-fs `store` — the host's `/nix/store`, **read-only** (runtime base,
//!   bubblewrap, waypipe, Mesa)
//! * virtio-fs `app`   — the `.app` bundle, **read-only**
//! * virtio-fs `data`  — the app's private data directory
//! * virtio-fs `f<n>`  — folders the user allowed (Pictures, …)
//! * virtio-fs `spec`  — what to run (`spec.json`), read-only
//! * virtio-vsock      — the only channel to the host: the app's window
//!   (waypipe, guest port 7001) and the broker (port 7000)
//! * virtio-net       — only for apps that declare a network: served by
//!   passt, which runs as the user and carries the VM's connections out as
//!   the user's own sockets (no root, no bridge; the host's own local
//!   services are not reachable from the VM)
//! * virtio-rng, the serial console logged to a file
//!
//! Sound, for apps allowed it, goes over vsock too: the guest's init
//! listens where the sandbox expects the PulseAudio socket and relays each
//! connection to guest port 7002; on the host, `iliada-run` relays those to
//! the user's sound server. No GPU yet: apps render in software (llvmpipe).

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use iliada_bundle::IsolationTier;
use iliada_ipc::guest::{
    DISPLAY_VSOCK_PORT, GUEST_ADDRESS, GUEST_DNS, GUEST_GATEWAY, GUEST_PULSE_SOCKET, GuestNetwork, GuestSpec,
    SOUND_VSOCK_PORT, SPEC_TAG, Share,
};
use iliada_types::AppId;
use tokio::process::{Child, Command};

use crate::namespace::{HostDesktop, bwrap_args};
use crate::{HostIsolationConfig, IsolationBackend, NetworkPolicy, SandboxHandle, SandboxSpec};

/// Where the guest's waypipe creates the app's Wayland socket.
const GUEST_RUNTIME: &str = "/run/user/0";
const GUEST_DISPLAY: &str = "wayland-iliada";

/// A per-app vsock context id (3 and up; 0–2 are reserved).
pub fn guest_cid(app: &AppId) -> u32 {
    let h = app.as_str().bytes().fold(2166136261u32, |h, b| (h ^ b as u32).wrapping_mul(16777619));
    3 + h % 0x00ff_ffff
}

/// One virtio-fs share: a virtiofsd serving `host` under `tag`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostShare {
    pub tag: String,
    pub host: PathBuf,
    pub readonly: bool,
}

/// Everything needed to start one app's VM.
#[derive(Debug, Clone)]
pub struct VmPlan {
    pub shares: Vec<HostShare>,
    pub guest: GuestSpec,
    pub vmm_args: Vec<String>,
    /// The vsock Unix socket (cloud-hypervisor's "hybrid vsock").
    pub vsock: PathBuf,
    pub console_log: PathBuf,
    /// The passt (vhost-user) socket, when the VM has a network.
    pub net: Option<PathBuf>,
    /// The user's sound server (PulseAudio socket), when the app may use it.
    pub sound: Option<PathBuf>,
}

impl VmPlan {
    /// The socket virtiofsd serves a share on.
    pub fn fs_socket(dir: &Path, tag: &str) -> PathBuf {
        dir.join(format!("fs-{tag}.sock"))
    }

    /// Where guest connections to the display port arrive on the host.
    pub fn display_socket(&self) -> PathBuf {
        PathBuf::from(format!("{}_{DISPLAY_VSOCK_PORT}", self.vsock.display()))
    }

    /// Where guest connections to the sound port arrive on the host.
    pub fn sound_socket(&self) -> PathBuf {
        PathBuf::from(format!("{}_{SOUND_VSOCK_PORT}", self.vsock.display()))
    }
}

/// Host paths the sandbox binds, resolved into the Nix store so they exist
/// in the guest too (the guest sees only the store and the shares).
fn resolved_desktop() -> HostDesktop {
    let d = HostDesktop::default();
    let r = |p: PathBuf| std::fs::canonicalize(&p).unwrap_or(p);
    HostDesktop { fonts: r(d.fonts), icons: r(d.icons), mime: r(d.mime), opengl_driver: r(d.opengl_driver) }
}

/// The host's CA certificates as `(guest path, store path)` symlinks, so
/// apps in a VM can check TLS certificates like apps outside one.
pub fn cert_links() -> Vec<(String, String)> {
    let mut links = Vec::new();
    let Ok(dir) = std::fs::read_dir("/etc/ssl/certs") else { return links };
    for e in dir.flatten() {
        if let Ok(target) = std::fs::canonicalize(e.path())
            && target.starts_with("/nix/store")
        {
            links.push((e.path().display().to_string(), target.display().to_string()));
        }
    }
    links.sort();
    links
}

/// passt arguments: a vhost-user network for one VM. The guest gets fixed
/// private addresses; nothing is forwarded into it, and the gateway is not
/// mapped to the host's loopback, so the VM can reach the internet and the
/// local network, but not services listening only on this computer.
pub fn passt_args(socket: &Path) -> Vec<String> {
    let (addr, prefix) = GUEST_ADDRESS.split_once('/').expect("address/prefix");
    [
        "--vhost-user",
        "--socket",
        &socket.display().to_string(),
        "--foreground",
        "--quiet",
        "--one-off",
        "--address",
        addr,
        "--netmask",
        prefix,
        "--gateway",
        GUEST_GATEWAY,
        "--dns-forward",
        GUEST_DNS,
        "--no-map-gw",
        "--tcp-ports",
        "none",
        "--udp-ports",
        "none",
    ]
    .map(str::to_owned)
    .to_vec()
}

/// Plan a VM for `spec`. `dir`: a private directory for its sockets and
/// spec; `certs`: [`cert_links`].
pub fn plan(
    spec: &SandboxSpec,
    host: &HostIsolationConfig,
    dir: &Path,
    desktop: &HostDesktop,
    certs: &[(String, String)],
) -> VmPlan {
    let p = |x: &Path| x.display().to_string();
    let mut shares = vec![
        HostShare { tag: "store".into(), host: "/nix/store".into(), readonly: true },
        HostShare { tag: "app".into(), host: spec.bundle.clone(), readonly: true },
        HostShare { tag: "data".into(), host: spec.data_dir.clone(), readonly: false },
    ];
    for (i, (folder, _)) in spec.grants.folders.iter().enumerate() {
        shares.push(HostShare { tag: format!("f{i}"), host: folder.clone(), readonly: false });
    }

    let net = (spec.network == NetworkPolicy::Nat && host.passt.is_some()).then(|| dir.join("net.sock"));

    // The sandbox inside the guest: Tier 1's, minus what the VM doesn't have.
    let mut inner = spec.clone();
    if net.is_none() {
        inner.network = NetworkPolicy::None;
    }
    inner.grants.camera = false;
    // Sound: the guest's relay socket stands in for the host's.
    let sound = spec.grants.sound.as_ref().and_then(|s| s.pulse.clone());
    inner.grants.sound =
        sound.as_ref().map(|_| crate::SoundSockets { pipewire: None, pulse: Some(GUEST_PULSE_SOCKET.into()) });
    inner.files.clear();
    inner.display = Some(Path::new(GUEST_RUNTIME).join(GUEST_DISPLAY));
    inner.env.extend([
        ("LIBGL_ALWAYS_SOFTWARE".to_owned(), "1".to_owned()),
        ("GSK_RENDERER".to_owned(), "cairo".to_owned()),
    ]);
    let mut args = vec![
        "--vsock".to_owned(),
        "--no-gpu".into(),
        "--compress".into(),
        "none".into(),
        "-s".into(),
        format!("2:{DISPLAY_VSOCK_PORT}"),
        "--display".into(),
        GUEST_DISPLAY.into(),
        "server".into(),
        "--".into(),
        p(&host.bwrap),
    ];
    args.extend(bwrap_args(&inner, host, desktop));
    let guest = GuestSpec {
        shares: shares.iter().map(|s| Share { tag: s.tag.clone(), path: p(&s.host), readonly: s.readonly }).collect(),
        program: p(&host.waypipe),
        args,
        env: vec![
            ("PATH".into(), "/bin".into()),
            ("XDG_RUNTIME_DIR".into(), GUEST_RUNTIME.into()),
            ("HOME".into(), "/tmp".into()),
        ],
        network: net.as_ref().map(|_| GuestNetwork::passt()),
        links: if net.is_some() { certs.to_vec() } else { vec![] },
        sound: sound.as_ref().map(|_| GUEST_PULSE_SOCKET.to_owned()),
        user: None,
    };

    let vsock = dir.join("vsock.sock");
    let console_log = dir.join("console.log");
    let mut fs = vec!["--fs".to_owned()];
    for tag in shares.iter().map(|s| s.tag.as_str()).chain([SPEC_TAG]) {
        fs.push(format!("tag={tag},socket={}", p(&VmPlan::fs_socket(dir, tag))));
    }
    let mut vmm_args = vec![
        "--kernel".to_owned(),
        p(&host.guest_kernel),
        "--initramfs".into(),
        p(&host.guest_initrd),
        "--cmdline".into(),
        // The serial port is built into the kernel (virtio-console is a
        // module, absent while it boots), so the whole boot is logged.
        format!("console={} panic=-1 loglevel=4", if cfg!(target_arch = "aarch64") { "ttyAMA0" } else { "ttyS0" }),
        "--cpus".into(),
        format!("boot={}", spec.vcpus.max(1)),
        "--memory".into(),
        // virtio-fs (vhost-user) needs memory shared with virtiofsd.
        format!("size={}M,shared=on", spec.memory_mib.max(256)),
        "--vsock".into(),
        format!("cid={},socket={}", guest_cid(&spec.app_id), p(&vsock)),
        "--rng".into(),
        "src=/dev/urandom".into(),
        "--serial".into(),
        format!("file={}", p(&console_log)),
        "--console".into(),
        "off".into(),
    ];
    vmm_args.extend(fs);
    if let Some(sock) = &net {
        vmm_args.push("--net".into());
        vmm_args.push(format!("vhost_user=true,socket={},vhost_mode=client", p(sock)));
    }
    VmPlan { shares, guest, vmm_args, vsock, console_log, net, sound }
}

/// virtiofsd arguments for one share. virtiofsd runs as the user, in a
/// user namespace where that user is root; it translates the guest's
/// `owner` (the app runs as the user's own uid in the VM) to that root, so
/// the guest reads and writes the user's files as the user. `owner` is the
/// user's `(uid, gid)`.
pub fn virtiofsd_args(socket: &Path, shared_dir: &Path, readonly: bool, owner: (u32, u32)) -> Vec<String> {
    let mut a = vec![
        format!("--socket-path={}", socket.display()),
        format!("--shared-dir={}", shared_dir.display()),
        format!("--cache={}", if readonly { "auto" } else { "never" }),
        "--sandbox=namespace".into(),
        format!("--uid-map=:0:{}:1:", owner.0),
        format!("--gid-map=:0:{}:1:", owner.1),
        format!("--translate-uid=map:{}:0:1", owner.0),
        format!("--translate-gid=map:{}:0:1", owner.1),
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

    /// Whether micro-VMs can start here (KVM, the VMM, a guest kernel and initrd).
    pub fn available(host: &HostIsolationConfig) -> bool {
        Path::new("/dev/kvm").exists()
            && host.guest_kernel.exists()
            && host.guest_initrd.exists()
            && host.cloud_hypervisor.exists()
            && host.waypipe.exists()
    }
}

#[derive(Debug)]
struct VmHandle {
    app: AppId,
    vmm: Child,
    /// virtiofsd and waypipe: stopped when the VM is.
    helpers: Vec<Child>,
    dir: PathBuf,
}

/// A private directory for one VM's sockets, under the user's runtime dir.
fn vm_dir(app: &AppId) -> Result<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    Ok(base.join("iliada-vm").join(app.as_str()))
}

#[async_trait]
impl IsolationBackend for CloudHypervisorBackend {
    fn tier(&self) -> IsolationTier {
        IsolationTier::MicroVm
    }

    async fn launch(&self, spec: &SandboxSpec) -> Result<Box<dyn SandboxHandle>> {
        if !Self::available(&self.host) {
            bail!("micro-VMs are not available on this computer (needs KVM and the Iliada guest image)");
        }
        let dir = vm_dir(&spec.app_id)?;
        let _ = tokio::fs::remove_dir_all(&dir).await;
        tokio::fs::create_dir_all(dir.join("spec")).await?;
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
        }
        tokio::fs::create_dir_all(&spec.data_dir).await?;
        crate::namespace::write_identity(&spec.data_dir)?;
        // The user running the app owns its data directory. Inside the VM
        // the app runs as that user too, not as root: an app's own sandbox
        // (Firefox's, Chromium's) can then create its user namespaces,
        // which the kernel refuses when they would map real root.
        let owner = {
            use std::os::unix::fs::MetadataExt;
            let m = std::fs::metadata(&spec.data_dir)?;
            (m.uid(), m.gid())
        };
        let mut plan = plan(spec, &self.host, &dir, &resolved_desktop(), &cert_links());
        plan.guest.user = Some(owner);
        tokio::fs::write(dir.join("spec/spec.json"), serde_json::to_vec_pretty(&plan.guest)?).await?;

        let spawn = |bin: &Path, args: Vec<String>, what: &str| {
            Command::new(bin)
                .args(args)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .kill_on_drop(true)
                .spawn()
                .with_context(|| format!("starting {what}"))
        };
        let mut helpers = Vec::new();
        let spec_share = HostShare { tag: SPEC_TAG.into(), host: dir.join("spec"), readonly: true };
        for s in plan.shares.iter().chain([&spec_share]) {
            let sock = VmPlan::fs_socket(&dir, &s.tag);
            helpers.push(spawn(&self.host.virtiofsd, virtiofsd_args(&sock, &s.host, s.readonly, owner), "virtiofsd")?);
        }
        // The app's window: the guest's waypipe dials host port 7001, which
        // cloud-hypervisor delivers to this Unix socket.
        helpers.push(spawn(
            &self.host.waypipe,
            vec![
                "--no-gpu".into(),
                "--compress".into(),
                "none".into(),
                "-s".into(),
                plan.display_socket().display().to_string(),
                "client".into(),
            ],
            "waypipe",
        )?);
        if let (Some(sock), Some(passt)) = (&plan.net, &self.host.passt) {
            helpers.push(spawn(passt, passt_args(sock), "passt (the app's network)")?);
        }
        for s in plan.shares.iter().chain([&spec_share]) {
            wait_for_socket(&VmPlan::fs_socket(&dir, &s.tag)).await?;
        }
        if let Some(sock) = &plan.net {
            wait_for_socket(sock).await?;
        }
        if let Some(server) = &plan.sound {
            relay_sound(plan.sound_socket(), server.clone())?;
        }
        wait_for_socket(&plan.display_socket()).await?;
        let vmm = spawn(&self.host.cloud_hypervisor, plan.vmm_args.clone(), "cloud-hypervisor")?;
        tracing::info!(app = %spec.app_id, cid = guest_cid(&spec.app_id), "micro-VM started");
        Ok(Box::new(VmHandle { app: spec.app_id.clone(), vmm, helpers, dir }))
    }
}

#[async_trait]
impl SandboxHandle for VmHandle {
    fn app_id(&self) -> &AppId {
        &self.app
    }
    async fn stop(&mut self) -> Result<()> {
        let _ = self.vmm.kill().await;
        for c in &mut self.helpers {
            let _ = c.kill().await;
        }
        Ok(())
    }
    async fn wait(&mut self) -> Result<Option<i32>> {
        // The guest powers off when the app exits; the VMM then exits.
        let status = self.vmm.wait().await?;
        for c in &mut self.helpers {
            let _ = c.kill().await;
        }
        if !status.success() {
            eprintln!(
                "iliada-run: the micro-VM stopped ({status}); its console log is {}",
                self.dir.join("console.log").display()
            );
        }
        Ok(status.code())
    }
}

/// Relay the guest's sound connections to the user's sound server, for as
/// long as `iliada-run` runs. Plain bytes: PipeWire's PulseAudio server
/// never uses shared memory, so nothing but the socket stream is needed.
fn relay_sound(listen: PathBuf, server: PathBuf) -> Result<()> {
    let listener =
        tokio::net::UnixListener::bind(&listen).with_context(|| format!("listening on {}", listen.display()))?;
    tokio::spawn(async move {
        while let Ok((mut guest, _)) = listener.accept().await {
            let server = server.clone();
            tokio::spawn(async move {
                match tokio::net::UnixStream::connect(&server).await {
                    Ok(mut host) => {
                        let _ = tokio::io::copy_bidirectional(&mut guest, &mut host).await;
                    }
                    Err(e) => tracing::warn!("sound server {}: {e}", server.display()),
                }
            });
        }
    });
    Ok(())
}

async fn wait_for_socket(p: &Path) -> Result<()> {
    for _ in 0..250 {
        if p.exists() {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    bail!("{} did not appear", p.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> SandboxSpec {
        SandboxSpec {
            app_id: AppId::new("org.debian.jq").unwrap(),
            bundle: "/Applications/jq.app".into(),
            entry: "usr/bin/jq".into(),
            data_dir: "/home/alex/.local/share/iliada/apps/org.debian.jq".into(),
            network: NetworkPolicy::Nat,
            display: Some("/run/user/1000/wayland-1".into()),
            env: vec![],
            vcpus: 1,
            memory_mib: 512,
            args: vec![],
            grants: Default::default(),
            files: vec![],
        }
    }

    fn value_after<'a>(a: &'a [String], flag: &str) -> Option<&'a str> {
        a.windows(2).find(|w| w[0] == flag).map(|w| w[1].as_str())
    }

    #[test]
    fn plan_shares_store_app_and_data_and_runs_the_tier1_sandbox_through_waypipe() {
        let mut s = spec();
        s.grants.folders.push(("/home/alex/Pictures".into(), "Pictures".into()));
        let host = HostIsolationConfig::example();
        s.network = NetworkPolicy::None;
        let p = plan(&s, &host, Path::new("/run/user/1000/iliada-vm/org.debian.jq"), &HostDesktop::default(), &[]);

        let tags: Vec<_> = p.shares.iter().map(|s| (s.tag.as_str(), s.readonly)).collect();
        assert_eq!(tags, [("store", true), ("app", true), ("data", false), ("f0", false)]);
        assert_eq!(p.guest.shares[1].path, "/Applications/jq.app", "mounted at the host path");
        assert_eq!(value_after(&p.vmm_args, "--memory"), Some("size=512M,shared=on"));
        assert!(p.vmm_args.iter().any(|a| a.starts_with("tag=spec,socket=")));
        assert!(!p.vmm_args.iter().any(|a| a == "--net"), "no network unless declared");
        assert_eq!(p.guest.network, None);
        assert_eq!((p.sound.as_ref(), p.guest.sound.as_ref()), (None, None), "no sound unless allowed");
        assert_eq!(p.display_socket(), Path::new("/run/user/1000/iliada-vm/org.debian.jq/vsock.sock_7001"));

        // waypipe server -- bwrap <Tier-1 args>, without network or host sockets.
        let a = &p.guest.args;
        assert_eq!(value_after(a, "-s"), Some("2:7001"));
        let bwrap = a.iter().position(|x| x == "--").unwrap() + 1;
        assert_eq!(a[bwrap], host.bwrap.display().to_string());
        assert!(!a.contains(&"--share-net".to_owned()));
        assert!(a.windows(3).any(|w| w[0] == "--ro-bind" && w[1] == "/run/user/0/wayland-iliada"));
        assert!(a.windows(3).any(|w| w[0] == "--bind" && w[1] == "/home/alex/Pictures"));
        assert!(a.contains(&"LIBGL_ALWAYS_SOFTWARE".to_owned()));
    }

    #[test]
    fn an_app_with_a_network_gets_passt_and_the_sandbox_shares_it() {
        let host = HostIsolationConfig::example();
        let certs = [("/etc/ssl/certs/ca-certificates.crt".to_owned(), "/nix/store/x-nss-cacert/ca.crt".to_owned())];
        let p =
            plan(&spec(), &host, Path::new("/run/user/1000/iliada-vm/org.debian.jq"), &HostDesktop::default(), &certs);
        assert_eq!(p.net.as_deref(), Some(Path::new("/run/user/1000/iliada-vm/org.debian.jq/net.sock")));
        assert_eq!(
            value_after(&p.vmm_args, "--net"),
            Some("vhost_user=true,socket=/run/user/1000/iliada-vm/org.debian.jq/net.sock,vhost_mode=client")
        );
        assert_eq!(p.guest.network, Some(GuestNetwork::passt()));
        assert_eq!(p.guest.links, certs);
        assert!(p.guest.args.contains(&"--share-net".to_owned()), "the inner sandbox uses the VM's network");

        // Without passt on the host, the VM stays offline.
        let mut offline = host.clone();
        offline.passt = None;
        let p = plan(&spec(), &offline, Path::new("/d"), &HostDesktop::default(), &certs);
        assert!(p.net.is_none() && p.guest.network.is_none() && p.guest.links.is_empty());
        assert!(!p.guest.args.contains(&"--share-net".to_owned()));
    }

    #[test]
    fn sound_goes_through_the_guests_relay_socket() {
        let mut s = spec();
        s.grants.sound = Some(crate::SoundSockets {
            pipewire: Some("/run/user/1000/pipewire-0".into()),
            pulse: Some("/run/user/1000/pulse/native".into()),
        });
        let p = plan(&s, &HostIsolationConfig::example(), Path::new("/d"), &HostDesktop::default(), &[]);
        assert_eq!(p.sound.as_deref(), Some(Path::new("/run/user/1000/pulse/native")));
        assert_eq!(p.sound_socket(), Path::new("/d/vsock.sock_7002"));
        assert_eq!(p.guest.sound.as_deref(), Some(GUEST_PULSE_SOCKET));
        // Inside the guest the sandbox binds the relay, not a host socket.
        let a = &p.guest.args;
        assert!(a.windows(3).any(|w| w[0] == "--ro-bind" && w[1] == GUEST_PULSE_SOCKET));
        assert!(!a.iter().any(|x| x.contains("/run/user/1000/p")), "no host sound sockets in the guest");
    }

    #[test]
    fn passt_forwards_nothing_in_and_hides_the_hosts_loopback() {
        let a = passt_args(Path::new("/d/net.sock"));
        assert_eq!(value_after(&a, "--socket"), Some("/d/net.sock"));
        assert_eq!(value_after(&a, "--tcp-ports"), Some("none"));
        assert_eq!(value_after(&a, "--udp-ports"), Some("none"));
        assert!(a.contains(&"--no-map-gw".to_owned()));
        assert!(a.contains(&"--vhost-user".to_owned()) && a.contains(&"--foreground".to_owned()));
    }

    #[test]
    fn cids_are_stable_and_not_reserved() {
        let a = AppId::new("org.debian.jq").unwrap();
        assert_eq!(guest_cid(&a), guest_cid(&a));
        assert!(guest_cid(&a) >= 3);
    }

    #[test]
    fn app_share_is_readonly_data_share_is_not() {
        let a = virtiofsd_args(Path::new("/s"), Path::new("/a"), true, (1000, 100));
        assert!(a.contains(&"--readonly".to_string()));
        assert!(a.contains(&"--uid-map=:0:1000:1:".to_string()), "virtiofsd's root is the user");
        assert!(a.contains(&"--translate-uid=map:1000:0:1".to_string()), "the guest's user is the user");
        assert!(a.contains(&"--translate-gid=map:100:0:1".to_string()));
        assert!(
            !virtiofsd_args(Path::new("/s"), Path::new("/d"), false, (1000, 100)).contains(&"--readonly".to_string())
        );
    }
}
