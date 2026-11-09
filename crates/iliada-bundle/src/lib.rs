//! The Iliada `.app` bundle format.
//!
//! ```text
//! Name.app/
//!   Contents/
//!     Info.toml          identity, entry point, isolation tier   (Info)
//!     Permissions.toml   capabilities the app may *ask* for      (Permissions)
//!     Provenance.toml    where every byte came from              (Provenance)
//!     Executables/       launcher entry points (what the shell runs)
//!     Libraries/         shared libraries bundled from dependencies
//!     Root/              the upstream FHS tree, verbatim (mounted at / in the micro-VM)
//!     Resources/         icons, .desktop files, localisations
//! ```
//!
//! A bundle is *data*: nothing in it is trusted until validated, and the
//! permission manifest is an upper bound — declaring a capability never grants
//! it, it only allows the app to prompt for it.

mod error;
mod info;
mod layout;
mod permissions;
mod provenance;

pub use error::BundleError;
pub use info::{Info, IsolationTier};
pub use layout::{Bundle, BundleLayout};
pub use permissions::{PermissionRequest, Permissions};
pub use provenance::{LibraryRecord, PackageRecord, PackageRole, Provenance, ResolutionStatus, SourceRecord};
