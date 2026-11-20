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
use x25519_dalek::{PublicKey, SharedSecret, StaticSecret};

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
        grouped(&self.0)
    }

    /// Seals this key for the new computer whose one-time public key this
    /// is (see [`Receiver`]): only that computer can open it.
    pub fn seal_for(&self, public_key: &[u8; 32]) -> Result<Vec<u8>> {
        let mine = StaticSecret::from(random::<32>()?);
        let mine_public = PublicKey::from(&mine);
        let cipher = handover_cipher(&mine.diffie_hellman(&PublicKey::from(*public_key)), public_key, &mine_public)?;
        let nonce = random::<NONCE_LEN>()?;
        let sealed = cipher
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: &self.0, aad: HANDOVER })
            .map_err(|_| anyhow!("encryption failed"))?;
        Ok([mine_public.as_bytes().as_slice(), &nonce, &sealed].concat())
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

/// Bytes as `ABCD-EFGH-…` in [`ALPHABET`] (5 bits a character).
fn grouped(bytes: &[u8]) -> String {
    let mut chars = Vec::with_capacity(bytes.len() * 8 / 5 + 1);
    let (mut acc, mut bits) = (0u32, 0);
    for &b in bytes {
        acc = (acc << 8) | u32::from(b);
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            chars.push(ALPHABET[((acc >> bits) & 31) as usize] as char);
        }
    }
    chars.chunks(4).map(|c| c.iter().collect::<String>()).collect::<Vec<_>>().join("-")
}

fn random<const N: usize>() -> Result<[u8; N]> {
    let mut b = [0; N];
    getrandom::fill(&mut b).map_err(|e| anyhow!("no randomness: {e}"))?;
    Ok(b)
}

// ── Handing the key to a new computer ────────────────────────────────
//
// The new computer makes a one-time X25519 key pair and leaves the public
// half with the server; a computer that has the account key seals it to
// that public half (its own one-time X25519 key, then XChaCha20-Poly1305).
// Both screens show `match_code` of the public half: if the server swapped
// in its own, the codes differ and the person says no.

const HANDOVER: &[u8] = b"iliada account key handover v1";

fn handover_cipher(shared: &SharedSecret, receiver: &[u8; 32], sender: &PublicKey) -> Result<XChaCha20Poly1305> {
    if !shared.was_contributory() {
        bail!("that computer's public key is not usable");
    }
    let k = Sha256::new()
        .chain_update(HANDOVER)
        .chain_update(shared.as_bytes())
        .chain_update(receiver)
        .chain_update(sender.as_bytes())
        .finalize();
    Ok(XChaCha20Poly1305::new(Key::from_slice(&k)))
}

/// The code both computers show for a request (`ABCD-EFGH`, 40 bits of its
/// public key's hash): the person checks they are the same.
pub fn match_code(public_key: &[u8; 32]) -> String {
    let h = Sha256::new().chain_update(b"iliada key request code").chain_update(public_key).finalize();
    grouped(&h[..5])
}

/// A new computer's one-time key pair, to receive the account key.
#[derive(Clone)]
pub struct Receiver {
    secret: StaticSecret,
}

impl std::fmt::Debug for Receiver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Receiver(…)")
    }
}

impl Receiver {
    pub fn new() -> Result<Self> {
        Ok(Receiver { secret: StaticSecret::from(random::<32>()?) })
    }

    pub fn public_key(&self) -> [u8; 32] {
        PublicKey::from(&self.secret).to_bytes()
    }

    /// Opens what [`AccountKey::seal_for`] sealed for this computer.
    pub fn open(&self, sealed: &[u8]) -> Result<AccountKey> {
        let bad = || anyhow!("the other computer sent something that doesn't open here");
        if sealed.len() < 32 + NONCE_LEN {
            return Err(bad());
        }
        let (sender, rest) = sealed.split_at(32);
        let (nonce, data) = rest.split_at(NONCE_LEN);
        let sender = PublicKey::from(<[u8; 32]>::try_from(sender).expect("32 bytes"));
        let cipher = handover_cipher(&self.secret.diffie_hellman(&sender), &self.public_key(), &sender)?;
        let key = cipher.decrypt(XNonce::from_slice(nonce), Payload { msg: data, aad: HANDOVER }).map_err(|_| bad())?;
        Ok(AccountKey(key.try_into().map_err(|_| bad())?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_reaches_only_the_computer_that_asked() {
        let key = AccountKey::generate().unwrap();
        let (new, other) = (Receiver::new().unwrap(), Receiver::new().unwrap());
        let sealed = key.seal_for(&new.public_key()).unwrap();
        assert_eq!(new.open(&sealed).unwrap(), key);
        assert!(other.open(&sealed).is_err());
        let mut bent = sealed.clone();
        *bent.last_mut().unwrap() ^= 1;
        assert!(new.open(&bent).is_err());
        assert!(new.open(&sealed[..40]).is_err());
        // A low-order public key (one a server might send to learn nothing
        // useful) is refused rather than used.
        assert!(key.seal_for(&[0; 32]).is_err());
        // The code: short, readable, the same for the same public key.
        let code = match_code(&new.public_key());
        assert_eq!(code.len(), 9, "{code}");
        assert_eq!(code, match_code(&new.public_key()));
        assert_ne!(code, match_code(&other.public_key()));
    }

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
