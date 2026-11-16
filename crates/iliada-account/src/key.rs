//! The account key: made on the first computer, never sent to the server.
//! Everything the account keeps is sealed with it here, on the computer.
//!
//! People see it once, as the **recovery key**: 32 letters and digits in
//! groups of four (`ABCD-EFGH-…`), with nothing that looks alike (no 0, O,
//! 1 or I). It is how a new computer gets the key when no other computer
//! can hand it over.

use anyhow::{Result, anyhow, bail};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use sha2::{Digest, Sha256};

/// 160 random bits: 32 characters of the alphabet below.
const SECRET_LEN: usize = 20;
const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
/// Sealed data starts with this, then the nonce, then the ciphertext.
const MAGIC: &[u8; 8] = b"ILIADA1\0";
const NONCE_LEN: usize = 24;

#[derive(Clone, PartialEq, Eq)]
pub struct AccountKey([u8; SECRET_LEN]);

impl std::fmt::Debug for AccountKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AccountKey(…)")
    }
}

impl AccountKey {
    pub fn generate() -> Result<Self> {
        let mut b = [0; SECRET_LEN];
        getrandom::fill(&mut b).map_err(|e| anyhow!("no randomness: {e}"))?;
        Ok(AccountKey(b))
    }

    /// `ABCD-EFGH-…`: what people write down.
    pub fn recovery_text(&self) -> String {
        let mut chars = Vec::with_capacity(SECRET_LEN * 8 / 5);
        let (mut acc, mut bits) = (0u32, 0);
        for &b in &self.0 {
            acc = (acc << 8) | u32::from(b);
            bits += 8;
            while bits >= 5 {
                bits -= 5;
                chars.push(ALPHABET[((acc >> bits) & 31) as usize] as char);
            }
        }
        chars.chunks(4).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join("-")
    }

    /// Reads a recovery key as people type it: any case, with or without
    /// dashes and spaces.
    pub fn from_recovery_text(text: &str) -> Result<Self> {
        let mut out = Vec::with_capacity(SECRET_LEN);
        let (mut acc, mut bits, mut n) = (0u32, 0, 0);
        for c in text.chars().filter(|c| !c.is_whitespace() && *c != '-') {
            let up = c.to_ascii_uppercase() as u8;
            let Some(v) = ALPHABET.iter().position(|&a| a == up) else {
                bail!("A recovery key has no {c:?}: it uses letters and the digits 2 to 9 (no 0, O, 1 or I).");
            };
            acc = (acc << 5) | v as u32;
            bits += 5;
            n += 1;
            if bits >= 8 {
                bits -= 8;
                out.push((acc >> bits) as u8);
            }
        }
        if n != SECRET_LEN * 8 / 5 {
            bail!("A recovery key has 32 letters and digits; this has {n}.");
        }
        Ok(AccountKey(out.try_into().expect("20 bytes")))
    }

    fn cipher(&self) -> XChaCha20Poly1305 {
        let k = Sha256::new().chain_update(b"iliada account key v1").chain_update(self.0).finalize();
        XChaCha20Poly1305::new(Key::from_slice(&k))
    }

    /// Encrypts `plain` to be kept under `name` (the name is authenticated
    /// too, so the server can't swap one thing for another).
    pub fn seal(&self, name: &str, plain: &[u8]) -> Result<Vec<u8>> {
        let mut nonce = [0; NONCE_LEN];
        getrandom::fill(&mut nonce).map_err(|e| anyhow!("no randomness: {e}"))?;
        let sealed = self
            .cipher()
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: plain, aad: name.as_bytes() })
            .map_err(|_| anyhow!("encryption failed"))?;
        Ok([MAGIC.as_slice(), &nonce, &sealed].concat())
    }

    /// Decrypts what `seal` made for `name`; fails for any other key or name,
    /// or if a single byte changed.
    pub fn open(&self, name: &str, sealed: &[u8]) -> Result<Vec<u8>> {
        let rest = sealed.strip_prefix(MAGIC.as_slice()).ok_or_else(|| anyhow!("not something Iliada sealed"))?;
        if rest.len() < NONCE_LEN {
            bail!("not something Iliada sealed");
        }
        let (nonce, data) = rest.split_at(NONCE_LEN);
        self.cipher()
            .decrypt(XNonce::from_slice(nonce), Payload { msg: data, aad: name.as_bytes() })
            .map_err(|_| anyhow!("this needs a different account key"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_text_round_trips_as_typed() {
        let k = AccountKey::generate().unwrap();
        let t = k.recovery_text();
        assert_eq!(t.len(), 32 + 7, "{t}");
        assert!(t.split('-').all(|g| g.len() == 4));
        assert_eq!(AccountKey::from_recovery_text(&t).unwrap(), k);
        let typed = t.to_lowercase().replace('-', " ");
        assert_eq!(AccountKey::from_recovery_text(&typed).unwrap(), k);
        assert!(AccountKey::from_recovery_text(&t[..20]).is_err());
        assert!(AccountKey::from_recovery_text(&t.replace(&t[..1], "0")).is_err());
    }

    #[test]
    fn sealed_opens_only_with_its_key_and_name() {
        let (a, b) = (AccountKey::generate().unwrap(), AccountKey::generate().unwrap());
        let s = a.seal("settings", b"dark mode").unwrap();
        assert_eq!(a.open("settings", &s).unwrap(), b"dark mode");
        assert!(b.open("settings", &s).is_err());
        assert!(a.open("other", &s).is_err());
        let mut bent = s.clone();
        *bent.last_mut().unwrap() ^= 1;
        assert!(a.open("settings", &bent).is_err());
        assert_ne!(a.seal("settings", b"dark mode").unwrap(), s, "a fresh nonce each time");
    }
}
