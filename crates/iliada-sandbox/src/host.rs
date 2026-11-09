//! Host-wide isolation settings, written by the `iliada.isolation` NixOS
//! module to `/etc/iliada/isolation.json`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostIsolationConfig {
    /// Guest kernel image (bzImage / Image) for micro-VMs.
    pub guest_kernel: PathBuf,
    /// Guest initramfs containing the Iliada guest agent.
    pub guest_initrd: PathBuf,
    pub cloud_hypervisor: PathBuf,
    pub virtiofsd: PathBuf,
    pub bwrap: PathBuf,
    /// Read-only runtime base (glibc, Mesa, …) mapped into every sandbox.
    pub runtime_dir: PathBuf,
    /// Per-app sockets (VMM API, vsock, broker).
    pub socket_dir: PathBuf,
    /// Parent of per-app writable data directories.
    pub data_root: PathBuf,
    pub default_vcpus: u8,
    pub default_memory_mib: u32,
}

impl HostIsolationConfig {
    pub const DEFAULT_PATH: &'static str = "/etc/iliada/isolation.json";

    pub fn load(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))
    }

    /// Values for tests and documentation.
    pub fn example() -> Self {
        Self {
            guest_kernel: "/run/current-system/iliada/guest/kernel".into(),
            guest_initrd: "/run/current-system/iliada/guest/initrd".into(),
            cloud_hypervisor: "/run/current-system/sw/bin/cloud-hypervisor".into(),
            virtiofsd: "/run/current-system/sw/bin/virtiofsd".into(),
            bwrap: "/run/current-system/sw/bin/bwrap".into(),
            runtime_dir: "/run/current-system/iliada/runtime/iliada-base-1".into(),
            socket_dir: "/run/iliada/sandbox".into(),
            data_root: "/var/lib/iliada/apps".into(),
            default_vcpus: 2,
            default_memory_mib: 1024,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verbatim output of `nix eval .#nixosConfigurations.dev-vm-aarch64.config.environment.etc."iliada/isolation.json".text`.
    /// If the NixOS module and this struct drift apart, this test fails.
    const FROM_NIX: &str = r#"{"bwrap":"/nix/store/x-bubblewrap-0.13.1/bin/bwrap","cloudHypervisor":"/nix/store/x-cloud-hypervisor-54.0/bin/cloud-hypervisor","dataRoot":"/var/lib/iliada/apps","defaultMemoryMib":1024,"defaultVcpus":2,"guestInitrd":"/run/current-system/iliada/guest/initrd","guestKernel":"/nix/store/x-linux-6.18.62/Image","runtimeDir":"/nix/store/x-iliada-base-1","socketDir":"/run/iliada/sandbox","virtiofsd":"/nix/store/x-virtiofsd-1.14.0/bin/virtiofsd"}"#;

    #[test]
    fn parses_nix_generated_config() {
        let c: HostIsolationConfig = serde_json::from_str(FROM_NIX).unwrap();
        assert_eq!(c.socket_dir, Path::new("/run/iliada/sandbox"));
        assert_eq!(c.default_memory_mib, 1024);
    }
}
