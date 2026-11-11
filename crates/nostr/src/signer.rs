//! What signs the user's events and opens their private messages: the key
//! on this computer, or a remote signer that holds it ([`crate::remote`]).
//! Everything that signs takes a [`Signer`], so it does not care which.
//!
//! It implements `nostr`'s blocking traits, so `nostr`'s builders (gift
//! wraps, NIP-46 messages) finalize with it directly. With a remote signer
//! each call is a round trip to the phone: call it off the UI thread.

use std::fmt;
use std::sync::Arc;

use nostr::nips::nip44::Nip44;
use nostr::prelude::{Event, EventBuilder, FinalizeEvent, GetPublicKey, Keys, PublicKey, SignEvent, UnsignedEvent};

use crate::remote::RemoteSigner;

#[derive(Clone)]
pub enum Signer {
    Local(Keys),
    Remote(Arc<RemoteSigner>),
}

impl fmt::Debug for Signer {
    // Never prints a key: `Keys`' own Debug hides the secret, but say so here.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Signer::Local(k) => write!(f, "Signer::Local({})", k.public_key()),
            Signer::Remote(r) => write!(f, "Signer::Remote({})", r.user_public_key()),
        }
    }
}

/// Why signing or encrypting failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl Signer {
    /// The user's public key (known without a round trip).
    pub fn public_key(&self) -> PublicKey {
        match self {
            Signer::Local(k) => k.public_key(),
            Signer::Remote(r) => r.user_public_key(),
        }
    }

    /// Builds and signs `builder` as the user.
    pub fn sign(&self, builder: EventBuilder) -> Result<Event, Error> {
        builder.finalize(self).map_err(|e| Error(e.to_string()))
    }

    pub fn is_remote(&self) -> bool {
        matches!(self, Signer::Remote(_))
    }
}

impl GetPublicKey for Signer {
    type Error = Error;

    fn get_public_key(&self) -> Result<PublicKey, Self::Error> {
        Ok(self.public_key())
    }
}

impl SignEvent for Signer {
    type Error = Error;

    fn sign_event(&self, unsigned: UnsignedEvent) -> Result<Event, Self::Error> {
        match self {
            Signer::Local(k) => k.sign_event(unsigned).map_err(|e| Error(e.to_string())),
            Signer::Remote(r) => r.sign_event(unsigned).map_err(|e| Error(e.to_string())),
        }
    }
}

impl Nip44 for Signer {
    type Error = Error;

    fn nip44_encrypt(&self, public_key: &PublicKey, content: &str) -> Result<String, Self::Error> {
        match self {
            Signer::Local(k) => k.nip44_encrypt(public_key, content).map_err(|e| Error(e.to_string())),
            Signer::Remote(r) => r.nip44_encrypt(public_key, content).map_err(|e| Error(e.to_string())),
        }
    }

    fn nip44_decrypt(&self, public_key: &PublicKey, payload: &str) -> Result<String, Self::Error> {
        match self {
            Signer::Local(k) => k.nip44_decrypt(public_key, payload).map_err(|e| Error(e.to_string())),
            Signer::Remote(r) => r.nip44_decrypt(public_key, payload).map_err(|e| Error(e.to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::prelude::Kind;

    #[test]
    fn a_local_signer_signs_and_encrypts() {
        let keys = Keys::generate();
        let signer = Signer::Local(keys.clone());
        let event = signer.sign(EventBuilder::new(Kind::TextNote, "hi")).unwrap();
        assert_eq!(event.pubkey, keys.public_key());
        assert!(event.verify().is_ok());
        let other = Keys::generate();
        let sealed = signer.nip44_encrypt(&other.public_key(), "secret").unwrap();
        assert!(!sealed.contains("secret"));
        assert_eq!(other.nip44_decrypt(&keys.public_key(), &sealed).unwrap(), "secret");
        assert!(!format!("{signer:?}").contains(&keys.secret_key().to_secret_hex()));
    }
}
