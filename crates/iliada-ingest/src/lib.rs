//! The Iliada Package Translator: upstream packages in, `.app` bundles out.
//!
//! ```text
//!  source ──fetch──▶ bytes+sha256 ──open_deb──▶ control + data.tar
//!     │                                              │ extract (confined, path-checked)
//!     │                                              ▼
//!     │                                         Contents/Root  ──scan ELF──▶ NEEDED sonames
//!     │                                                                          │
//!     │   Debian Packages index ◀── Depends walk (lib* only) ◀───────────────────┤
//!     │            │                                                             │
//!     │            ▼                                                             ▼
//!     │   dependency .debs (sha256-verified) ──▶ available sonames ──▶ closure: provided / runtime /
//!     │                                                                bundled / unresolved
//!     │                                                                          │ unresolved
//!     │                                       resolvers: naming rules, then optional AI
//!     │                                       suggestions are *verified* by fetching the package
//!     ▼                                       and checking it really ships the soname
//!  Name.app/Contents/{Info,Permissions,Provenance}.toml, Executables/, Libraries/, Root/, Resources/
//! ```
//!
//! Maintainer scripts are never executed. Prebuilt binaries are never
//! "statically relinked" — that is not possible after the fact; libraries are
//! bundled next to them and found via RUNPATH (patchelf, when available) or a
//! launcher that sets `LD_LIBRARY_PATH`.

pub mod closure;
pub mod control;
pub mod deb;
pub mod desktop;
pub mod elf;
pub mod extract;
pub mod fetch;
pub mod index;
pub mod infer;
pub mod launcher;
pub mod pipeline;
pub mod resolvers;
pub mod runtime;
pub mod sandbox;
pub mod serve;

pub use pipeline::{IngestConfig, IngestReport, Ingestor, Source};
