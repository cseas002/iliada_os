//! Shared vocabulary for Iliada.
//!
//! These types appear in three places that must agree byte-for-byte on meaning:
//! the on-disk bundle manifests (`Permissions.toml`), the guest↔host IPC wire
//! protocol (postcard), and the broker's persisted grant store (JSON). They are
//! therefore deliberately *externally tagged* serde enums, which every one of
//! those formats supports (postcard cannot decode internally tagged enums).

mod app_id;
mod capability;

pub use app_id::{AppId, AppIdError};
pub use capability::{Capability, ClipboardAccess, FileScope, KnownDir, LocationPrecision};
