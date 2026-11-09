use serde::{Deserialize, Serialize};

/// `Contents/Provenance.toml`: an auditable record of how the bundle was made.
///
/// Every package whose files ended up in the bundle is listed with its
/// download URL and SHA-256, and every soname the executables need is listed
/// with how it was satisfied — including any suggestion that came from the
/// AI library resolver, so a reviewer can see exactly what the model influenced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    /// Version of the tool that produced the bundle.
    pub generator: String,
    /// Seconds since the Unix epoch.
    pub created_at: u64,
    pub source: SourceRecord,
    #[serde(default, rename = "package", skip_serializing_if = "Vec::is_empty")]
    pub packages: Vec<PackageRecord>,
    #[serde(default, rename = "library", skip_serializing_if = "Vec::is_empty")]
    pub libraries: Vec<LibraryRecord>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRecord {
    pub url: String,
    pub sha256: String,
    /// `deb`, later `appimage`, `flatpak`, `tarball`.
    pub format: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageRecord {
    pub name: String,
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub role: PackageRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackageRole {
    Primary,
    Dependency,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryRecord {
    pub soname: String,
    pub status: ResolutionStatus,
    /// Package that supplied the library (bundled / suggested).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
    /// Which resolver proposed the package (e.g. `ai (claude-opus-5-5)`), when status = suggested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggested_by: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionStatus {
    /// Shipped inside the upstream package itself.
    Provided,
    /// Provided by the Iliada runtime base; not bundled.
    Runtime,
    /// Copied from a dependency package found via the package index.
    Bundled,
    /// Copied from a package a resolver suggested, after mechanical verification.
    Suggested,
    /// Nothing satisfied it; the app may fail to start.
    Unresolved,
}
