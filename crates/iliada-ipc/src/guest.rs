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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GuestSpec {
    /// In mount order (the Nix store first: everything else runs from it).
    pub shares: Vec<Share>,
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
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
        };
        let j = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<GuestSpec>(&j).unwrap(), s);
    }
}
