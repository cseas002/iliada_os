//! The Iliada app catalog (format version 1).
//!
//! `catalog.json` lists the apps the Store offers. It is signed as a whole
//! (`catalog.json.sig`: Ed25519 over the exact file bytes, base64); each app
//! archive is authenticated by the SHA-256 recorded in the signed catalog.
//! The public key is built into Iliada. See `iliada_apps/CATALOG.md`.

use anyhow::{Context, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use ed25519_dalek::{Signature, Signer as _, Verifier as _};
pub use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};

pub const FORMAT: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub format: u32,
    /// Seconds since the Unix epoch.
    pub generated: u64,
    pub apps: Vec<AppEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub category: String,
    pub version: String,
    #[serde(default)]
    pub network: bool,
    pub archive: Archive,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Archive {
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

/// `kind` of an Iliada release manifest (`release.json`).
pub const RELEASE_KIND: &str = "iliada-release";

/// An Iliada release: where its source is, signed like the catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub kind: String,
    pub version: String,
    /// URL of `iliada-<version>-source.tar.zst` (one folder: the source tree).
    pub source: String,
    pub sha256: String,
    pub size: u64,
}

pub fn parse_key(b64: &str) -> Result<VerifyingKey> {
    let bytes: [u8; 32] =
        B64.decode(b64.trim())?.try_into().map_err(|_| anyhow::anyhow!("public key must be 32 bytes"))?;
    Ok(VerifyingKey::from_bytes(&bytes)?)
}

pub fn parse_signing_key(b64: &str) -> Result<SigningKey> {
    let bytes: [u8; 32] =
        B64.decode(b64.trim())?.try_into().map_err(|_| anyhow::anyhow!("signing key must be 32 bytes"))?;
    Ok(SigningKey::from_bytes(&bytes))
}

pub fn encode_key(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

pub fn sign(key: &SigningKey, catalog_bytes: &[u8]) -> String {
    B64.encode(key.sign(catalog_bytes).to_bytes())
}

/// Check an Ed25519 signature (base64) over `bytes`.
pub fn verify_bytes(key: &VerifyingKey, bytes: &[u8], sig_b64: &str) -> Result<()> {
    let sig: [u8; 64] =
        B64.decode(sig_b64.trim())?.try_into().map_err(|_| anyhow::anyhow!("signature must be 64 bytes"))?;
    key.verify(bytes, &Signature::from_bytes(&sig)).context("signature is invalid")?;
    Ok(())
}

/// Verify `sig` over `catalog_bytes`, then parse. Nothing is parsed before
/// the signature checks out.
pub fn verify(key: &VerifyingKey, catalog_bytes: &[u8], sig_b64: &str) -> Result<Catalog> {
    verify_bytes(key, catalog_bytes, sig_b64).context("catalog signature is invalid")?;
    let catalog: Catalog = serde_json::from_slice(catalog_bytes).context("malformed catalog")?;
    if catalog.format != FORMAT {
        bail!("unsupported catalog format {}", catalog.format);
    }
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_verify_and_reject_tampering() {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let pk = sk.verifying_key();
        let cat = Catalog {
            format: FORMAT,
            generated: 1,
            apps: vec![AppEntry {
                id: "org.debian.jq".into(),
                name: "jq".into(),
                summary: "JSON".into(),
                category: "Utilities".into(),
                version: "1".into(),
                network: false,
                archive: Archive { url: "https://x/jq.app.tar.zst".into(), sha256: "00".into(), size: 1 },
                icon: None,
            }],
        };
        let bytes = serde_json::to_vec_pretty(&cat).unwrap();
        let sig = sign(&sk, &bytes);
        assert_eq!(verify(&pk, &bytes, &sig).unwrap(), cat);

        let mut evil = bytes.clone();
        let i = evil.windows(2).position(|w| w == b"00").unwrap();
        evil[i] = b'f';
        assert!(verify(&pk, &evil, &sig).is_err(), "a changed hash breaks the signature");
        let other = SigningKey::from_bytes(&[8u8; 32]).verifying_key();
        assert!(verify(&other, &bytes, &sig).is_err(), "wrong key");
        assert_eq!(parse_key(&encode_key(pk.as_bytes())).unwrap(), pk);
    }
}
