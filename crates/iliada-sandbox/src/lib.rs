//! The Iliada sandbox controller.
//!
//! Two isolation tiers, one interface ([`IsolationBackend`]):
//!
//! | Tier | Backend | Boundary | For |
//! |------|---------|----------|-----|
//! | 1 | [`namespace::NamespaceBackend`] (bubblewrap) | Linux namespaces + Landlock + seccomp | first-party / reviewed apps |
//! | 2 | [`cloud_hypervisor::CloudHypervisorBackend`] | KVM micro-VM, virtio only | everything ingested from upstream |
//!
//! In both tiers the app sees its bundle read-only, a private writable data
//! directory, the runtime base, and exactly one way out: the broker socket.
//! Network access is a sandbox-creation decision ([`NetworkPolicy`]), never
//! something the app can request later.
//!
//! Phase 1 status: configuration generation is complete and unit-tested on
//! every OS; actually launching requires Linux (KVM / user namespaces) and is
//! exercised in the Linux CI job and the dev VM.

pub mod cloud_hypervisor;
pub mod host;
pub mod namespace;

use std::path::PathBuf;

use anyhow::Result;
use async_trait::async_trait;
use iliada_bundle::{Bundle, IsolationTier};
use iliada_types::AppId;

pub use host::HostIsolationConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkPolicy {
    /// No network device / empty network namespace.
    None,
    /// Outbound-only NAT through the host (tap device / slirp).
    Nat,
}

/// Everything needed to start one app.
#[derive(Debug, Clone)]
pub struct SandboxSpec {
    pub app_id: AppId,
    pub bundle: PathBuf,
    /// Path inside `Contents/Root` of the real program.
    pub entry: String,
    /// Private, persistent, writable storage for the app (its "home").
    pub data_dir: PathBuf,
    pub network: NetworkPolicy,
    /// Host Wayland socket to give the app a window connection (GUI apps).
    pub display: Option<PathBuf>,
    /// Extra environment for the app (set by sandboxed triggers).
    pub env: Vec<(String, String)>,
    pub vcpus: u8,
    pub memory_mib: u32,
    pub args: Vec<String>,
}

impl SandboxSpec {
    /// Build a spec from a validated bundle using host defaults. `data_dir`
    /// is the app's private, persistent home (per user, like macOS containers).
    pub fn from_bundle(
        b: &Bundle,
        host: &HostIsolationConfig,
        network: NetworkPolicy,
        data_dir: PathBuf,
    ) -> Result<Self> {
        let entry = b
            .info
            .entry
            .clone()
            .ok_or_else(|| anyhow::anyhow!("{} has no Info.entry; cannot sandbox it", b.info.id))?;
        Ok(Self {
            app_id: b.info.id.clone(),
            bundle: b.layout.root().to_owned(),
            entry,
            data_dir,
            network,
            display: None,
            env: vec![],
            vcpus: host.default_vcpus,
            memory_mib: host.default_memory_mib,
            args: vec![],
        })
    }
}

#[async_trait]
pub trait IsolationBackend: Send + Sync {
    fn tier(&self) -> IsolationTier;
    async fn launch(&self, spec: &SandboxSpec) -> Result<Box<dyn SandboxHandle>>;
}

#[async_trait]
pub trait SandboxHandle: Send + std::fmt::Debug {
    fn app_id(&self) -> &AppId;
    /// Ask the sandbox to stop, then force it after a grace period.
    async fn stop(&mut self) -> Result<()>;
    /// Wait for the sandbox to exit; returns its exit status code if known.
    async fn wait(&mut self) -> Result<Option<i32>>;
}

/// Pick the backend for a bundle's declared tier.
pub fn backend_for(tier: IsolationTier, host: &HostIsolationConfig) -> Box<dyn IsolationBackend> {
    match tier {
        IsolationTier::Namespace => Box::new(namespace::NamespaceBackend::new(host.clone())),
        IsolationTier::MicroVm => Box::new(cloud_hypervisor::CloudHypervisorBackend::new(host.clone())),
    }
}
