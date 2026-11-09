use iliada_types::AppId;
use serde::{Deserialize, Serialize};

use crate::BundleError;

/// `Contents/Info.toml`: who the app is and how to start it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Info {
    pub id: AppId,
    /// Human-readable name shown in the shell.
    pub name: String,
    pub version: String,
    /// File name inside `Contents/Executables/` launched by the shell.
    pub executable: String,
    /// The real program, relative to `Contents/Root` (e.g. `usr/bin/jq`).
    /// Sandboxes exec this directly with `Root` mounted at `/`; the launcher
    /// in `Executables/` is the host-side equivalent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry: Option<String>,
    /// Arguments always passed to `entry` (e.g. `--writer`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
    /// Path relative to `Contents/`, e.g. `Resources/icon.png`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// The runtime base ABI this bundle was built against (sonames it may
    /// assume the platform provides), e.g. `iliada-base-1`.
    pub runtime: String,
    #[serde(default)]
    pub isolation: IsolationTier,
    /// The app needs the internet. Shown before installation; sandboxes of
    /// apps without it have no network at all.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub network: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub categories: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mime_types: Vec<String>,
}

/// How strongly the app is separated from the host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum IsolationTier {
    /// Tier 1 (default): namespaces + Landlock + seccomp, like macOS's App
    /// Sandbox. Native speed, no extra memory.
    #[default]
    Namespace,
    /// Tier 2 (opt-in): a dedicated micro-VM with its own kernel. Strongest
    /// isolation, at a cost in start-up time, memory and graphics speed. Chosen
    /// per app ("Extra isolation") or for every app ("High security mode").
    MicroVm,
}

impl Info {
    pub fn validate(&self) -> Result<(), BundleError> {
        if self.name.trim().is_empty() {
            return Err(BundleError::Invalid("Info.name is empty".into()));
        }
        if self.version.trim().is_empty() {
            return Err(BundleError::Invalid("Info.version is empty".into()));
        }
        if !is_plain_file_name(&self.executable) {
            return Err(BundleError::Invalid(format!(
                "Info.executable {:?} must be a plain file name inside Contents/Executables",
                self.executable
            )));
        }
        if let Some(entry) = &self.entry {
            if !is_safe_relative(entry) {
                return Err(BundleError::Invalid(format!(
                    "Info.entry {entry:?} must be a relative path inside Contents/Root"
                )));
            }
        }
        if let Some(icon) = &self.icon {
            if !is_safe_relative(icon) {
                return Err(BundleError::Invalid(format!(
                    "Info.icon {icon:?} must be a relative path inside Contents"
                )));
            }
        }
        Ok(())
    }
}

pub(crate) fn is_plain_file_name(s: &str) -> bool {
    !s.is_empty() && s != "." && s != ".." && !s.contains('/') && !s.contains('\\') && !s.contains('\0')
}

pub(crate) fn is_safe_relative(s: &str) -> bool {
    !s.is_empty() && !s.starts_with('/') && !s.contains('\0') && s.split('/').all(|c| !c.is_empty() && c != "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Info {
        Info {
            id: AppId::new("org.debian.jq").unwrap(),
            name: "jq".into(),
            version: "1.7.1".into(),
            executable: "jq".into(),
            entry: Some("usr/bin/jq".into()),
            args: vec![],
            icon: None,
            runtime: "iliada-base-1".into(),
            isolation: IsolationTier::MicroVm,
            network: false,
            categories: vec![],
            mime_types: vec![],
        }
    }

    #[test]
    fn round_trips_through_toml() {
        let info = sample();
        let text = toml::to_string(&info).unwrap();
        assert_eq!(toml::from_str::<Info>(&text).unwrap(), info);
        assert!(text.contains(r#"isolation = "micro-vm""#));
    }

    #[test]
    fn rejects_escaping_paths() {
        let mut info = sample();
        info.executable = "../../bin/sh".into();
        assert!(info.validate().is_err());
        let mut info = sample();
        info.icon = Some("../secret".into());
        assert!(info.validate().is_err());
    }
}
