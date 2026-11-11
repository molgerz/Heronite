//! The user's key pair: made here, or brought from another Nostr app.
//!
//! A secret key comes in as `nsec1…` (NIP-19), as 64 hex digits, or
//! encrypted with a passphrase as `ncryptsec1…` (NIP-49); a `nostr:` prefix
//! (NIP-21) is accepted. It goes out as `nsec1…`, or as `ncryptsec1…` when
//! the user gives a passphrase, which is the safer way to move it.
//!
//! The secret bytes this module hands out are for the keyring or
//! [`crate::prefs::seal`] only: never log them or write them in the clear.

use std::fmt;

use nostr::nips::nip19::{FromBech32, ToBech32};
use nostr::nips::nip49::{EncryptedSecretKey, KeySecurity};
use nostr::prelude::{Keys, PublicKey, SecretKey};

/// scrypt's work factor for an exported key: NIP-49's suggested 16
/// (64 MiB, about a second).
const EXPORT_LOG_N: u8 = 16;

/// Why a typed or pasted key was not taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportError {
    Empty,
    /// An `npub` or another public form: it cannot sign.
    PublicKey,
    /// An `ncryptsec`: ask for its passphrase and try again.
    NeedsPassphrase,
    WrongPassphrase,
    /// Not a key in any of the accepted forms.
    NotAKey,
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ImportError::Empty => "no key given",
            ImportError::PublicKey => "a public key cannot sign",
            ImportError::NeedsPassphrase => "the key is encrypted with a passphrase",
            ImportError::WrongPassphrase => "wrong passphrase",
            ImportError::NotAKey => "not a secret key",
        })
    }
}

impl std::error::Error for ImportError {}

/// A new random key pair.
pub fn generate() -> Keys {
    Keys::generate()
}

/// The key pair of a secret key typed or pasted by the user. `passphrase`
/// opens an `ncryptsec1…`.
pub fn import(input: &str, passphrase: Option<&str>) -> Result<Keys, ImportError> {
    let input = input.trim();
    let input = input.strip_prefix("nostr:").unwrap_or(input);
    if input.is_empty() {
        return Err(ImportError::Empty);
    }
    let lower = input.to_ascii_lowercase();
    if lower.starts_with("npub1") || lower.starts_with("nprofile1") {
        return Err(ImportError::PublicKey);
    }
    if lower.starts_with("ncryptsec1") {
        let encrypted = EncryptedSecretKey::from_bech32(input).map_err(|_| ImportError::NotAKey)?;
        let passphrase = passphrase.filter(|p| !p.is_empty()).ok_or(ImportError::NeedsPassphrase)?;
        let secret = encrypted.decrypt(passphrase).map_err(|_| ImportError::WrongPassphrase)?;
        return Ok(Keys::new(secret));
    }
    SecretKey::parse(input).map(Keys::new).map_err(|_| ImportError::NotAKey)
}

/// The key pair of secret bytes read back from the keyring or a sealed blob.
pub fn from_secret_bytes(bytes: &[u8]) -> Option<Keys> {
    SecretKey::from_slice(bytes).ok().map(Keys::new)
}

/// The 32 secret bytes, for the keyring or [`crate::prefs::seal`].
pub fn secret_bytes(keys: &Keys) -> [u8; 32] {
    keys.secret_key().to_secret_bytes()
}

/// `nsec1…`, shown only after the user confirmed.
pub fn export_nsec(keys: &Keys) -> String {
    keys.secret_key().to_bech32().unwrap_or_default()
}

/// `ncryptsec1…`: the secret key encrypted with `passphrase` (NIP-49).
/// Blocks for about a second (scrypt).
pub fn export_encrypted(keys: &Keys, passphrase: &str) -> anyhow::Result<String> {
    anyhow::ensure!(!passphrase.is_empty(), "an empty passphrase protects nothing");
    let encrypted = EncryptedSecretKey::new(keys.secret_key(), passphrase, EXPORT_LOG_N, KeySecurity::Medium)?;
    Ok(encrypted.to_bech32()?)
}

/// `npub1…`, the form people share.
pub fn npub(key: &PublicKey) -> String {
    key.to_bech32().unwrap_or_else(|_| key.to_hex())
}

/// The first and last characters of the `npub`, for tight places.
pub fn short_npub(key: &PublicKey) -> String {
    let full = npub(key);
    if full.len() <= 20 {
        return full;
    }
    format!("{}…{}", &full[..12], &full[full.len() - 6..])
}

/// A public key in any common form: `npub1…`, `nprofile1…`, hex, with or
/// without `nostr:`. NIP-05 addresses need the network: see
/// [`crate::social::resolve_nip05`].
pub fn parse_public_key(input: &str) -> Option<PublicKey> {
    let input = input.trim();
    let input = input.strip_prefix("nostr:").unwrap_or(input);
    if input.to_ascii_lowercase().starts_with("nprofile1") {
        return nostr::nips::nip19::Nip19Profile::from_bech32(input).ok().map(|p| p.public_key);
    }
    PublicKey::parse(input).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_nsec_hex_and_the_nostr_prefix() {
        let keys = generate();
        let nsec = export_nsec(&keys);
        assert!(nsec.starts_with("nsec1"));
        assert_eq!(import(&nsec, None).unwrap().public_key(), keys.public_key());
        assert_eq!(import(&format!(" nostr:{nsec} \n"), None).unwrap().public_key(), keys.public_key());
        let hex = keys.secret_key().to_secret_hex();
        assert_eq!(import(&hex, None).unwrap().public_key(), keys.public_key());
    }

    #[test]
    fn refuses_what_cannot_sign() {
        let keys = generate();
        assert_eq!(import("", None).unwrap_err(), ImportError::Empty);
        assert_eq!(import(&npub(&keys.public_key()), None).unwrap_err(), ImportError::PublicKey);
        assert_eq!(import("nsec1notreally", None).unwrap_err(), ImportError::NotAKey);
        assert_eq!(import("hello", None).unwrap_err(), ImportError::NotAKey);
    }

    #[test]
    fn ncryptsec_round_trips_and_needs_the_right_passphrase() {
        let keys = generate();
        let encrypted = export_encrypted(&keys, "correct horse").unwrap();
        assert!(encrypted.starts_with("ncryptsec1"));
        assert!(!encrypted.contains(&keys.secret_key().to_secret_hex()));
        assert_eq!(import(&encrypted, None).unwrap_err(), ImportError::NeedsPassphrase);
        assert_eq!(import(&encrypted, Some("")).unwrap_err(), ImportError::NeedsPassphrase);
        assert_eq!(import(&encrypted, Some("wrong")).unwrap_err(), ImportError::WrongPassphrase);
        assert_eq!(import(&encrypted, Some("correct horse")).unwrap().public_key(), keys.public_key());
        assert!(export_encrypted(&keys, "").is_err());
    }

    #[test]
    fn secret_bytes_round_trip() {
        let keys = generate();
        let back = from_secret_bytes(&secret_bytes(&keys)).unwrap();
        assert_eq!(back.public_key(), keys.public_key());
        assert!(from_secret_bytes(b"short").is_none());
    }

    #[test]
    fn public_keys_in_every_common_form() {
        let keys = generate();
        let pk = keys.public_key();
        assert_eq!(parse_public_key(&npub(&pk)), Some(pk));
        assert_eq!(parse_public_key(&format!("nostr:{}", npub(&pk))), Some(pk));
        assert_eq!(parse_public_key(&pk.to_hex()), Some(pk));
        let profile = nostr::nips::nip19::Nip19Profile::new(pk, []);
        assert_eq!(parse_public_key(&profile.to_bech32().unwrap()), Some(pk));
        assert_eq!(parse_public_key("someone@example.com"), None);
        assert!(short_npub(&pk).contains('…'));
    }
}
