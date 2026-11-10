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

/// Format of `catalog-2.json`: one archive per processor (`archives`).
pub const FORMAT: u32 = 2;
/// Format of `catalog.json`, the only one Iliada 0.1 reads: x86-64 apps,
/// one `archive` each, no fields 0.1 does not know.
pub const LEGACY_FORMAT: u32 = 1;

/// File name of the format-2 catalog, published next to `catalog.json`.
pub const CATALOG_2: &str = "catalog-2.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub format: u32,
    /// Seconds since the Unix epoch.
    pub generated: u64,
    pub apps: Vec<AppEntry>,
}

/// One app. Unknown fields are ignored (the whole file is signed), so later
/// formats can add to an entry without breaking this reader.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppEntry {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub category: String,
    pub version: String,
    #[serde(default)]
    pub network: bool,
    /// The x86-64 archive: the only one format-1 readers (Iliada 0.1) know.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<Archive>,
    /// One archive per processor (format 2).
    #[serde(default, skip_serializing_if = "Archives::is_empty")]
    pub archives: Archives,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// Archives by processor, named as Rust names them (`std::env::consts::ARCH`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Archives {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x86_64: Option<Archive>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aarch64: Option<Archive>,
}

impl Archives {
    pub fn is_empty(&self) -> bool {
        self.x86_64.is_none() && self.aarch64.is_none()
    }
    pub fn get(&self, arch: &str) -> Option<&Archive> {
        match arch {
            "x86_64" => self.x86_64.as_ref(),
            "aarch64" => self.aarch64.as_ref(),
            _ => None,
        }
    }
    pub fn slot(&mut self, arch: &str) -> Option<&mut Option<Archive>> {
        match arch {
            "x86_64" => Some(&mut self.x86_64),
            "aarch64" => Some(&mut self.aarch64),
            _ => None,
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = (&'static str, &Archive)> {
        [("x86_64", self.x86_64.as_ref()), ("aarch64", self.aarch64.as_ref())]
            .into_iter()
            .filter_map(|(k, v)| v.map(|v| (k, v)))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Archive {
    pub url: String,
    pub sha256: String,
    pub size: u64,
    /// This build's version when it differs from the entry's (Debian
    /// sometimes rebuilds a package for one processor only: `+b1`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl AppEntry {
    /// The archive for a processor (`std::env::consts::ARCH` naming).
    pub fn archive_for(&self, arch: &str) -> Option<&Archive> {
        self.archives.get(arch).or(if arch == "x86_64" { self.archive.as_ref() } else { None })
    }

    /// The archive for this computer.
    pub fn local_archive(&self) -> Option<&Archive> {
        self.archive_for(std::env::consts::ARCH)
    }
}

impl Catalog {
    /// The apps this processor can run, each with its build's version.
    pub fn for_arch(mut self, arch: &str) -> Catalog {
        self.apps.retain(|a| a.archive_for(arch).is_some());
        for app in &mut self.apps {
            if let Some(v) = app.archive_for(arch).and_then(|a| a.version.clone()) {
                app.version = v;
            }
        }
        self
    }

    /// `catalog.json` for Iliada 0.1: the x86-64 apps, in exactly the shape
    /// 0.1 parses (it refuses fields it does not know).
    pub fn legacy(&self) -> Catalog {
        let apps = self
            .apps
            .iter()
            .filter_map(|a| {
                let mut x86 = a.archive_for("x86_64")?.clone();
                let version = x86.version.take().unwrap_or_else(|| a.version.clone());
                Some(AppEntry { version, archive: Some(x86), archives: Archives::default(), ..a.clone() })
            })
            .collect();
        Catalog { format: LEGACY_FORMAT, generated: self.generated, apps }
    }
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
    if catalog.format != FORMAT && catalog.format != LEGACY_FORMAT {
        bail!("unsupported catalog format {}", catalog.format);
    }
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive(url: &str) -> Archive {
        Archive { url: url.into(), sha256: "00".into(), size: 1, version: None }
    }

    fn entry(archive: Option<Archive>, archives: Archives) -> AppEntry {
        AppEntry {
            id: "org.debian.jq".into(),
            name: "jq".into(),
            summary: "JSON".into(),
            category: "Utilities".into(),
            version: "1".into(),
            network: false,
            archive,
            archives,
            icon: None,
        }
    }

    #[test]
    fn sign_verify_and_reject_tampering() {
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let pk = sk.verifying_key();
        let cat = Catalog {
            format: FORMAT,
            generated: 1,
            apps: vec![entry(None, Archives { x86_64: Some(archive("https://x/jq.app.tar.zst")), aarch64: None })],
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

    #[test]
    fn picks_the_archive_for_the_processor() {
        // A format-1 entry: its one archive is x86-64.
        let old = entry(Some(archive("x86")), Archives::default());
        assert_eq!(old.archive_for("x86_64").unwrap().url, "x86");
        assert!(old.archive_for("aarch64").is_none());

        let mut arm = archive("arm");
        arm.version = Some("1+b1".into());
        let both = entry(None, Archives { x86_64: Some(archive("x86")), aarch64: Some(arm) });
        let x86_only = AppEntry { id: "org.debian.x".into(), ..old.clone() };
        let cat = Catalog { format: FORMAT, generated: 5, apps: vec![both.clone(), x86_only] };

        let on_arm = cat.clone().for_arch("aarch64");
        assert_eq!(on_arm.apps.len(), 1, "apps without an ARM build are hidden on ARM");
        assert_eq!(on_arm.apps[0].version, "1+b1", "the ARM build's own version");
        assert_eq!(on_arm.apps[0].archive_for("aarch64").unwrap().url, "arm");
        assert_eq!(cat.clone().for_arch("x86_64").apps.len(), 2);
        assert!(cat.clone().for_arch("riscv64").apps.is_empty());
    }

    /// `catalog.json` must stay readable by Iliada 0.1, whose types refuse
    /// unknown fields.
    #[test]
    fn legacy_catalog_parses_with_iliada_0_1_types() {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct Catalog01 {
            format: u32,
            generated: u64,
            apps: Vec<Entry01>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct Entry01 {
            id: String,
            name: String,
            summary: String,
            category: String,
            version: String,
            #[serde(default)]
            network: bool,
            archive: Archive01,
            #[serde(default)]
            icon: Option<String>,
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        #[allow(dead_code)]
        struct Archive01 {
            url: String,
            sha256: String,
            size: u64,
        }
        let mut x86 = archive("x86");
        x86.version = Some("1+b2".into());
        let both = entry(None, Archives { x86_64: Some(x86), aarch64: Some(archive("arm")) });
        let arm_only = AppEntry {
            id: "org.debian.armonly".into(),
            ..entry(None, Archives { x86_64: None, aarch64: Some(archive("arm")) })
        };
        let cat = Catalog { format: FORMAT, generated: 5, apps: vec![both, arm_only] };
        let legacy = cat.legacy();
        let json = serde_json::to_vec(&legacy).unwrap();
        let old: Catalog01 = serde_json::from_slice(&json).unwrap();
        assert_eq!(old.format, 1);
        assert_eq!(old.apps.len(), 1, "only x86-64 apps");
        assert_eq!(old.apps[0].archive.url, "x86");
        assert_eq!(old.apps[0].version, "1+b2");
    }
}
