use iliada_types::{AppId, Capability};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;
/// vsock port the broker listens on (guest connects to CID 2, this port).
pub const BROKER_VSOCK_PORT: u32 = 7000;
/// The host's vsock context id.
pub const HOST_CID: u32 = 2;
/// Upper bound for one encoded frame (1 MiB).
pub const MAX_FRAME_LEN: u32 = 1 << 20;
/// Upper bound for one `Read` chunk.
pub const MAX_READ_CHUNK: u32 = 256 * 1024;

/// Guest → host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum GuestMessage {
    Hello { version: u16, client: String },
    Request { id: u32, body: Request },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Request {
    /// Ask for a capability. `reason` is shown to the user *labelled as coming
    /// from the app*; the declared purpose from Permissions.toml is shown too.
    Capability { capability: Capability, reason: Option<String> },
    /// Give up a capability (and any persistent grant for it).
    Revoke { capability: Capability },
    /// What does this app currently hold?
    ListGrants,
    /// Read from a resource handle (e.g. a user-picked file).
    Read { handle: Handle, offset: u64, len: u32 },
    /// Release a handle.
    Close { handle: Handle },
}

/// Host → guest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum HostMessage {
    Welcome { version: u16, app_id: AppId, session: u64 },
    Reply { id: u32, body: Reply },
    Event(Event),
    Goodbye { reason: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Reply {
    Granted(Grant),
    Denied { reason: DenyReason },
    Grants(Vec<GrantInfo>),
    Data { bytes: Vec<u8>, eof: bool },
    Ok,
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Grant {
    pub handle: Handle,
    pub capability: Capability,
    pub scope: GrantScope,
    pub resource: Resource,
}

/// What the guest receives with a grant. The guest never gets host device
/// nodes or host paths; only broker-mediated handles.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Resource {
    /// Pure permission (notifications, clipboard): use the corresponding request.
    None,
    /// A file the user chose; read it with `Request::Read`.
    File { name: String, size: u64 },
    /// A media stream. Phase 1 returns `endpoint: None` (decision only);
    /// Phase 2 returns the vsock port of a PipeWire stream bridged by the host.
    Media { kind: MediaKind, endpoint: Option<u32> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    AudioCapture,
    VideoCapture,
    ScreenCapture,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum GrantScope {
    /// This request only.
    Once,
    /// Until the app's sandbox stops.
    WhileRunning,
    /// Persisted until the user revokes it in Settings.
    Always,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DenyReason {
    /// Not declared in Permissions.toml — denied without prompting.
    NotDeclared,
    UserDenied,
    /// System policy (MDM, parental controls, tier restrictions).
    PolicyDenied,
    /// The host cannot provide this capability.
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GrantInfo {
    pub capability: Capability,
    pub scope: GrantScope,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Handle(pub u64);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    /// The user revoked a grant (e.g. from Settings) while the app runs.
    Revoked { capability: Capability, handles: Vec<Handle> },
}
