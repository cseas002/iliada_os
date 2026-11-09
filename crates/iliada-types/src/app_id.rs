use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A reverse-DNS application identifier, e.g. `org.debian.jq`.
///
/// Rules: 2–8 dot-separated segments, each 1–63 chars of `[a-z0-9-]`, not
/// starting or ending with `-`; total length ≤ [`AppId::MAX_LEN`]. The
/// identifier is used as a path component (socket names, state directories),
/// so the charset is intentionally narrow: no `/`, no `..`, no uppercase, no
/// whitespace. The length cap keeps per-app Unix socket paths such as
/// `/run/iliada/sandbox/<id>.vsock_7000` under the 104-byte `sun_path` limit.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct AppId(String);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AppIdError {
    #[error("app id is empty or longer than 64 bytes")]
    Length,
    #[error("app id must have between 2 and 8 dot-separated segments")]
    SegmentCount,
    #[error("app id segment {0:?} is invalid (allowed: a-z, 0-9, '-', 1-63 chars, no leading/trailing '-')")]
    Segment(String),
}

impl AppId {
    pub const MAX_LEN: usize = 64;

    pub fn new(s: impl Into<String>) -> Result<Self, AppIdError> {
        let s = s.into();
        if s.is_empty() || s.len() > Self::MAX_LEN {
            return Err(AppIdError::Length);
        }
        let segments: Vec<&str> = s.split('.').collect();
        if !(2..=8).contains(&segments.len()) {
            return Err(AppIdError::SegmentCount);
        }
        for seg in segments {
            let ok = !seg.is_empty()
                && seg.len() <= 63
                && !seg.starts_with('-')
                && !seg.ends_with('-')
                && seg.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            if !ok {
                return Err(AppIdError::Segment(seg.to_owned()));
            }
        }
        Ok(Self(s))
    }

    /// Build an id from an arbitrary upstream name (e.g. a Debian package
    /// name like `libfoo++` or `Foo_Bar`) under a vendor prefix, normalising
    /// characters that are not allowed.
    pub fn from_upstream(prefix: &str, name: &str) -> Result<Self, AppIdError> {
        let mut seg: String =
            name.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        while seg.contains("--") {
            seg = seg.replace("--", "-");
        }
        let seg = seg.trim_matches('-');
        let room = Self::MAX_LEN.saturating_sub(prefix.len() + 1).min(63);
        let seg = seg[..seg.len().min(room)].trim_end_matches('-');
        Self::new(format!("{prefix}.{seg}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for AppId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for AppId {
    type Err = AppIdError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl TryFrom<String> for AppId {
    type Error = AppIdError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(s)
    }
}

impl From<AppId> for String {
    fn from(id: AppId) -> Self {
        id.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_reverse_dns() {
        assert!(AppId::new("org.debian.jq").is_ok());
        assert!(AppId::new("com.example.my-app2").is_ok());
    }

    #[test]
    fn rejects_path_like_or_malformed() {
        for bad in ["", "jq", "org..jq", "org/debian.jq", "../etc.passwd", "Org.Debian", "org.-jq", "a.b.c.d.e.f.g.h.i"]
        {
            assert!(AppId::new(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn normalises_upstream_names() {
        assert_eq!(AppId::from_upstream("org.debian", "libstdc++6").unwrap().as_str(), "org.debian.libstdc-6");
        assert_eq!(AppId::from_upstream("org.debian", "Foo_Bar").unwrap().as_str(), "org.debian.foo-bar");
        let long = AppId::from_upstream("org.debian", &"x".repeat(200)).unwrap();
        assert_eq!(long.as_str().len(), AppId::MAX_LEN);
    }
}
