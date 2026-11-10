//! What the host tells a micro-VM's init to do (`spec.json` on the `spec`
//! virtio-fs share): which shares to mount where, and what to run.
//!
//! Shares are mounted at the same paths they have on the host, so the
//! sandbox inside the guest is assembled exactly like a Tier-1 one.

use serde::{Deserialize, Serialize};

/// The virtio-fs tag of the share that carries `spec.json`.
pub const SPEC_TAG: &str = "spec";
/// Where the guest mounts it.
pub const SPEC_DIR: &str = "/run/iliada/spec";
/// Guest port the display (waypipe) connects to on the host.
pub const DISPLAY_VSOCK_PORT: u32 = 7001;

/// The guest's network, when the app may use one (passt on the host: the
/// addresses are private to the VM, and passt carries its connections out
/// as the user's own).
pub const GUEST_ADDRESS: &str = "10.0.2.15/24";
pub const GUEST_GATEWAY: &str = "10.0.2.2";
/// passt forwards DNS queries sent here to the host's resolver.
pub const GUEST_DNS: &str = "10.0.2.3";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuestSpec {
    /// In mount order (the Nix store first: everything else runs from it).
    pub shares: Vec<Share>,
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Set when the VM has a network card (virtio-net, served by passt).
    #[serde(default)]
    pub network: Option<GuestNetwork>,
    /// Symlinks to create in the guest's root, `(path, target)`: the host's
    /// CA certificates in the store, for `/etc/ssl`.
    #[serde(default)]
    pub links: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuestNetwork {
    /// `address/prefix`.
    pub address: String,
    pub gateway: String,
    pub dns: String,
}

impl GuestNetwork {
    pub fn passt() -> Self {
        Self { address: GUEST_ADDRESS.into(), gateway: GUEST_GATEWAY.into(), dns: GUEST_DNS.into() }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Share {
    /// virtio-fs tag.
    pub tag: String,
    /// Absolute mount point in the guest (the host path).
    pub path: String,
    pub readonly: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_round_trip() {
        let s = GuestSpec {
            shares: vec![Share { tag: "store".into(), path: "/nix/store".into(), readonly: true }],
            program: "/nix/store/x-waypipe/bin/waypipe".into(),
            args: vec!["server".into()],
            env: vec![("A".into(), "b".into())],
            network: Some(GuestNetwork::passt()),
            links: vec![("/etc/ssl/certs/ca-certificates.crt".into(), "/nix/store/x-ca/ca.crt".into())],
        };
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<GuestSpec>(&j).unwrap(), s);
    }
}

#[cfg(test)]
mod compat {
    use super::*;

    #[test]
    fn a_spec_without_network_fields_still_parses() {
        let j = r#"{"shares":[],"program":"/bin/true","args":[],"env":[]}"#;
        let s: GuestSpec = serde_json::from_str(j).unwrap();
        assert_eq!(s.network, None);
        assert!(s.links.is_empty());
    }
}
