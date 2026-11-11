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
    #[serde(default, rename = "request", skip_serializing_if = "Vec::is_empty", deserialize_with = "known_requests")]
    pub requests: Vec<PermissionRequest>,
}

/// Requests for capabilities this Iliada doesn't know (declared for a newer
/// one) are left out: never asked about, never granted. Anything else that
/// is malformed is still an error.
fn known_requests<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<PermissionRequest>, D::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Raw {
        capability: toml::Value,
        purpose: String,
    }
    let raw = Vec::<Raw>::deserialize(d)?;
    let mut out = Vec::with_capacity(raw.len());
    for r in raw {
        let known = r.capability.clone().try_into::<Capability>();
        match known {
            Ok(capability) => out.push(PermissionRequest { capability, purpose: r.purpose }),
            Err(_) if is_unknown_variant(&r.capability) => {}
            Err(e) => return Err(serde::de::Error::custom(e)),
        }
    }
    Ok(out)
}

/// A capability name (or a table keyed by one) that is well-formed but not
/// one this version knows.
fn is_unknown_variant(v: &toml::Value) -> bool {
    match v {
        toml::Value::String(s) => s.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
        toml::Value::Table(t) => t.len() == 1,
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionRequest {
    pub capability: Capability,
    /// Shown verbatim in the host-drawn permission prompt.
    pub purpose: String,
}

/// The reason shown for a permission Iliada worked out from what an app is
/// built with, rather than one its maker wrote.
pub const INFERRED_PURPOSE: &str = "The app is able to use this but didn't say why.";

impl PermissionRequest {
    /// The reason as it should be shown to people. Apps packaged before
    /// 0.2.1 carry a technical note ("Inferred: the app links …") instead.
    pub fn shown_purpose(&self) -> &str {
        if self.purpose.starts_with("Inferred:") { INFERRED_PURPOSE } else { &self.purpose }
    }
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
    fn old_technical_reasons_are_shown_plainly() {
        let old = PermissionRequest {
            capability: Capability::Microphone,
            purpose: "Inferred: the app links a audio (ALSA) library and may ask to use your microphone.".into(),
        };
        assert_eq!(old.shown_purpose(), INFERRED_PURPOSE);
        let own = PermissionRequest { capability: Capability::Microphone, purpose: "To record your voice.".into() };
        assert_eq!(own.shown_purpose(), "To record your voice.");
    }

    #[test]
    fn capabilities_from_newer_iliada_are_skipped() {
        let p: Permissions = toml::from_str(
            r#"
            [[request]]
            capability = "teleport"
            purpose = "From the future"
            [[request]]
            capability = "camera"
            purpose = "Video calls"
            "#,
        )
        .unwrap();
        assert_eq!(p.requests.len(), 1);
        assert_eq!(p.requests[0].capability, Capability::Camera);
        assert!(toml::from_str::<Permissions>("[[request]]\ncapability = 3\npurpose = \"x\"\n").is_err());
    }

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
