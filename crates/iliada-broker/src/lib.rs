//! The Iliada permission broker.
//!
//! The broker is the *only* path from a sandboxed app to sensitive host
//! resources. Sandboxes have no camera, microphone, location or home
//! directory of their own; they get a socket to the broker and nothing else.
//!
//! Decision order for `Capability` requests (see [`engine::Engine`]):
//! 1. Not declared in the bundle's `Permissions.toml` → deny, no prompt.
//! 2. System policy forbids it → deny, no prompt.
//! 3. A persistent ("Always") grant exists → allow.
//! 4. A "While running" grant exists in this sandbox's session → allow.
//! 5. Otherwise ask the user through the [`prompt::PromptFrontend`] — a
//!    host-drawn, compositor-owned dialog in production (the guest can never
//!    draw or spoof it), a terminal prompt in development.
//!
//! Every decision is appended to an audit log.

pub mod audit;
pub mod engine;
pub mod prompt;
pub mod server;
pub mod store;

pub use engine::{AppContext, Decision, DecisionSource, Engine};
pub use prompt::{DenyAll, PromptAnswer, PromptFrontend, PromptRequest, TerminalPrompt};
pub use server::Broker;
pub use store::GrantStore;
