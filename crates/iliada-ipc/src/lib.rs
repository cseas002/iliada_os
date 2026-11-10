//! The Iliada guest↔host IPC protocol.
//!
//! * **Transport.** A byte stream. In production that is virtio-vsock from
//!   the app's micro-VM to the host (cloud-hypervisor's "hybrid vsock": a
//!   guest `connect(CID 2, port 7000)` arrives on the host as a connection to
//!   the Unix socket `<vm vsock socket>_7000`). Tier-1 namespace sandboxes and
//!   development use a plain Unix socket bind-mounted into the sandbox.
//! * **Identity.** The broker binds *one socket per app*. The app identity
//!   is therefore a property of the socket the connection arrived on, chosen
//!   by the host when it created the sandbox. Nothing the guest says can
//!   change it; the `Hello` message carries only a protocol version and a
//!   free-form client string for logs.
//! * **Framing.** Each message is a big-endian `u32` length followed by that
//!   many bytes of [postcard](https://docs.rs/postcard)-encoded data, capped
//!   at [`MAX_FRAME_LEN`].
//! * **Flow.** Guest sends `Hello`; host answers `Welcome` (or `Goodbye`).
//!   Then the guest sends `Request{id, ..}` and the host answers
//!   `Reply{id, ..}` in order. The host may also push `Event`s at any time.
//!
//! See `docs/ipc-protocol.md` for the full specification.

mod frame;
pub mod guest;
mod proto;
pub mod transport;

pub use frame::{Channel, FrameError, read_frame, write_frame};
pub use proto::*;
