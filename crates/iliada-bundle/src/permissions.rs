use std::collections::BTreeSet;

use iliada_types::Capability;
use serde::{Deserialize, Serialize};

use crate::BundleError;

/// `Contents/Permissions.toml`: the *upper bound* of what an app may request.
///
/// ```toml
/// [[request]]
/// capability = "microphone"
/// purpose = "Record voice memos"
///
/// [[request]]
/// capability = { files = "user_picked" }
/// purpose = "Open documents you choose"
/// ```
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Permissions {
    #[serde(default, rename = "request", skip_serializing_if = "Vec::is_empty")]
    pub requests: Vec<PermissionRequest>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionRequest {
    pub capability: Capability,
    /// Shown verbatim in the host-drawn permission prompt.
    pub purpose: String,
}

impl Permissions {
    /// The declared request that covers `requested`, if any.
    pub fn declaration_for(&self, requested: &Capability) -> Option<&PermissionRequest> {
        self.requests.iter().find(|r| r.capability.covers(requested))
    }

    pub fn validate(&self) -> Result<(), BundleError> {
        let mut seen = BTreeSet::new();
        for r in &self.requests {
            if !seen.insert(r.capability) {
                return Err(BundleError::Invalid(format!("capability {:?} declared twice", r.capability)));
            }
            if r.purpose.trim().is_empty() {
                return Err(BundleError::Invalid(format!("capability {:?} has an empty purpose", r.capability)));
            }
            if r.purpose.len() > 300 {
                return Err(BundleError::Invalid(format!("purpose for {:?} exceeds 300 bytes", r.capability)));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iliada_types::{FileScope, LocationPrecision};

    #[test]
    fn parses_documented_example() {
        let p: Permissions = toml::from_str(
            r#"
            [[request]]
            capability = "microphone"
            purpose = "Record voice memos"

            [[request]]
            capability = { files = "user_picked" }
            purpose = "Open documents you choose"
            "#,
        )
        .unwrap();
        p.validate().unwrap();
        assert!(p.declaration_for(&Capability::Microphone).is_some());
        assert!(p.declaration_for(&Capability::Files(FileScope::UserPicked)).is_some());
        assert!(p.declaration_for(&Capability::Camera).is_none());
    }

    #[test]
    fn precise_declaration_covers_coarse_request() {
        let p = Permissions {
            requests: vec![PermissionRequest {
                capability: Capability::Location(LocationPrecision::Precise),
                purpose: "Maps".into(),
            }],
        };
        assert!(p.declaration_for(&Capability::Location(LocationPrecision::Coarse)).is_some());
    }

    #[test]
    fn rejects_duplicates() {
        let r = PermissionRequest { capability: Capability::Camera, purpose: "x".into() };
        assert!(Permissions { requests: vec![r.clone(), r] }.validate().is_err());
    }
}
