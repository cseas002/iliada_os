use std::fmt;

use serde::{Deserialize, Serialize};

/// A sensitive resource an application may ask the host for at runtime.
///
/// Network access is intentionally *not* a capability: it is decided when the
/// sandbox is created (VMM/netns configuration), not brokered per request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Camera,
    Microphone,
    Location(LocationPrecision),
    Clipboard(ClipboardAccess),
    Files(FileScope),
    Notifications,
    ScreenCapture,
    /// Play sound. Granted without asking (as on macOS).
    Sound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocationPrecision {
    Coarse,
    Precise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipboardAccess {
    Read,
    Write,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileScope {
    /// A single file the user chooses in a host-drawn picker (portal semantics).
    UserPicked,
    /// Ongoing access to one well-known user directory.
    KnownDir(KnownDir),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnownDir {
    Documents,
    Downloads,
    Music,
    Pictures,
    Videos,
}

impl KnownDir {
    pub const ALL: [KnownDir; 5] =
        [KnownDir::Documents, KnownDir::Downloads, KnownDir::Music, KnownDir::Pictures, KnownDir::Videos];

    /// The folder's name in a home directory (and in the XDG user-dirs key).
    pub fn name(self) -> &'static str {
        match self {
            KnownDir::Documents => "Documents",
            KnownDir::Downloads => "Downloads",
            KnownDir::Music => "Music",
            KnownDir::Pictures => "Pictures",
            KnownDir::Videos => "Videos",
        }
    }
}

impl Capability {
    /// Whether a declared capability `self` covers a requested capability.
    /// Precise location implies coarse; everything else must match exactly.
    pub fn covers(&self, requested: &Capability) -> bool {
        match (self, requested) {
            (Capability::Location(LocationPrecision::Precise), Capability::Location(_)) => true,
            (a, b) => a == b,
        }
    }
}

impl fmt::Display for Capability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Capability::Camera => f.write_str("use your camera"),
            Capability::Microphone => f.write_str("use your microphone"),
            Capability::Location(LocationPrecision::Coarse) => f.write_str("know your approximate location"),
            Capability::Location(LocationPrecision::Precise) => f.write_str("know your precise location"),
            Capability::Clipboard(ClipboardAccess::Read) => f.write_str("read your clipboard"),
            Capability::Clipboard(ClipboardAccess::Write) => f.write_str("write to your clipboard"),
            Capability::Files(FileScope::UserPicked) => f.write_str("open a file you choose"),
            Capability::Files(FileScope::KnownDir(d)) => write!(f, "access your {} folder", d.name()),
            Capability::Notifications => f.write_str("show notifications"),
            Capability::ScreenCapture => f.write_str("record your screen"),
            Capability::Sound => f.write_str("play sound"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    struct Wrapper {
        capability: Capability,
    }

    #[test]
    fn toml_shapes_are_readable() {
        let unit: Wrapper = toml::from_str(r#"capability = "microphone""#).unwrap();
        assert_eq!(unit.capability, Capability::Microphone);
        let nested: Wrapper = toml::from_str(r#"capability = { files = { known_dir = "downloads" } }"#).unwrap();
        assert_eq!(nested.capability, Capability::Files(FileScope::KnownDir(KnownDir::Downloads)));
    }

    #[test]
    fn postcard_round_trip() {
        for cap in [
            Capability::Camera,
            Capability::Location(LocationPrecision::Coarse),
            Capability::Files(FileScope::KnownDir(KnownDir::Pictures)),
        ] {
            let bytes = postcard::to_stdvec(&cap).unwrap();
            assert_eq!(postcard::from_bytes::<Capability>(&bytes).unwrap(), cap);
        }
    }

    #[test]
    fn precise_location_covers_coarse() {
        let precise = Capability::Location(LocationPrecision::Precise);
        assert!(precise.covers(&Capability::Location(LocationPrecision::Coarse)));
        assert!(!Capability::Location(LocationPrecision::Coarse).covers(&precise));
    }
}
