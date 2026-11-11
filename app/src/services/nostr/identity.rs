//! Creating, importing, pairing (NIP-46) and forgetting the identity.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use gpui_kit::*;
use heronite_i18n::tr;
use heronite_nostr::identity::{self, ImportError};
use heronite_nostr::prefs::IdentityRef;
use heronite_nostr::remote::{self, RemoteSigner};
use heronite_nostr::{Keys, Signer};

use super::{secret, Account, Nostr};

/// Pairing with a phone.
#[derive(Default)]
pub enum Pairing {
    #[default]
    None,
    /// Showing the `nostrconnect://` code until a phone answers.
    Waiting { uri: String, cancel: Arc<AtomicBool> },
    /// Connecting to a pasted `bunker://` address.
    Connecting,
}

/// Why an import did not happen, for the dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImportProblem {
    NeedsPassphrase,
    Other(String),
}

impl Nostr {
    pub fn pairing(&self) -> &Pairing {
        &self.pairing
    }

    /// The key pair when it is on this computer (export needs it).
    fn local_keys(&self) -> Option<&Keys> {
        match self.signer.as_ref()? {
            Signer::Local(k) => Some(k),
            Signer::Remote(_) => None,
        }
    }

    pub fn has_local_key(&self) -> bool {
        self.local_keys().is_some()
    }

    pub(super) fn signer_from_secret(&self, bytes: &[u8]) -> Option<Signer> {
        let keys = identity::from_secret_bytes(bytes)?;
        match self.prefs.identity.as_ref()? {
            IdentityRef::Local { public_key, .. } => (keys.public_key().to_hex() == *public_key).then_some(Signer::Local(keys)),
            IdentityRef::Remote { public_key, signer, relays, .. } => {
                let user = heronite_nostr::PublicKey::from_hex(public_key).ok()?;
                let signer = heronite_nostr::PublicKey::from_hex(signer).ok()?;
                Some(Signer::Remote(Arc::new(RemoteSigner::restore(keys, signer, user, relays.clone()))))
            }
        }
    }

    /// Makes a new key pair and turns the community layer on with it.
    pub fn create(&mut self, cx: &mut Context<Self>) {
        let keys = identity::generate();
        self.adopt(
            IdentityRef::Local { public_key: keys.public_key().to_hex(), sealed_secret: String::new() },
            Signer::Local(keys),
            true,
            cx,
        );
    }

    /// Imports a secret key typed or pasted by the user. `done` gets `None`
    /// on success; NIP-49 decryption takes a second, so it runs off the UI
    /// thread.
    pub fn import(
        &mut self,
        input: String,
        passphrase: Option<String>,
        cx: &mut Context<Self>,
        done: impl FnOnce(Option<ImportProblem>, &mut App) + 'static,
    ) {
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { identity::import(&input, passphrase.as_deref()) }).await;
            let problem = match result {
                Ok(keys) => {
                    this.update(cx, |s, cx| {
                        s.adopt(
                            IdentityRef::Local { public_key: keys.public_key().to_hex(), sealed_secret: String::new() },
                            Signer::Local(keys),
                            false,
                            cx,
                        )
                    })
                    .ok();
                    None
                }
                Err(ImportError::NeedsPassphrase) => Some(ImportProblem::NeedsPassphrase),
                Err(e) => Some(ImportProblem::Other(import_text(&e))),
            };
            cx.update(|cx| done(problem, cx));
        })
        .detach();
    }

    /// Shows a `nostrconnect://` code and waits for a phone to pair.
    pub fn start_pairing(&mut self, cx: &mut Context<Self>) {
        self.cancel_pairing();
        let pairing = remote::Pairing::new(&[]);
        let cancel = Arc::new(AtomicBool::new(false));
        self.pairing = Pairing::Waiting { uri: pairing.uri().to_string(), cancel: cancel.clone() };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let flag = cancel.clone();
            let result = cx.background_spawn(async move { pairing.wait(&flag) }).await;
            this.update(cx, |s, cx| s.paired(result, cx)).ok();
        })
        .detach();
    }

    /// Pairs through a `bunker://` address from the signer app.
    pub fn connect_bunker(&mut self, uri: String, cx: &mut Context<Self>) {
        self.cancel_pairing();
        self.pairing = Pairing::Connecting;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx.background_spawn(async move { RemoteSigner::connect_bunker(&uri) }).await;
            this.update(cx, |s, cx| s.paired(result, cx)).ok();
        })
        .detach();
    }

    pub fn cancel_pairing(&mut self) {
        if let Pairing::Waiting { cancel, .. } = &self.pairing {
            cancel.store(true, Ordering::Relaxed);
        }
        self.pairing = Pairing::None;
    }

    fn paired(&mut self, result: Result<RemoteSigner, remote::Error>, cx: &mut Context<Self>) {
        self.pairing = Pairing::None;
        match result {
            Ok(rs) => {
                let identity = IdentityRef::Remote {
                    public_key: rs.user_public_key().to_hex(),
                    signer: rs.signer_public_key().to_hex(),
                    relays: rs.relays().to_vec(),
                    sealed_secret: String::new(),
                };
                self.adopt(identity, Signer::Remote(Arc::new(rs)), false, cx);
            }
            Err(remote::Error::Cancelled) => cx.notify(),
            Err(e) => self.set_notice(remote_text(&e), true, cx),
        }
    }

    /// Keeps the identity's secret (the key, or the client key of a remote
    /// signer), then turns on and connects. `new`: a fresh key, whose relay
    /// lists are published so others can reach it.
    fn adopt(&mut self, identity: IdentityRef, signer: Signer, new: bool, cx: &mut Context<Self>) {
        let secret = match &signer {
            Signer::Local(k) => identity::secret_bytes(k),
            Signer::Remote(r) => identity::secret_bytes(r.client_keys()),
        };
        self.forget_session(cx);
        // Another identity of this data folder: its key goes.
        if let Some(old) = self.prefs.identity.as_ref().map(|i| i.public_key().to_string()) {
            if old != identity.public_key() {
                secret::forget(&old, cx).detach();
            }
        }
        self.prefs.identity = Some(identity);
        self.prefs.enabled = true;
        let stored = secret::store(&mut self.prefs, &secret, cx);
        self.account = Account::Opening;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = stored.await;
            this.update(cx, |s, cx| match result {
                Ok(()) => {
                    s.save_prefs(cx);
                    s.signer = Some(signer.clone());
                    s.account = Account::Ready;
                    s.notice = None;
                    s.start_pool(signer, cx);
                    if new {
                        s.publish_relay_lists(cx);
                    }
                }
                Err(e) => {
                    // Not kept: not used either, so nothing works half-saved.
                    s.prefs.identity = None;
                    s.prefs.enabled = false;
                    s.save_prefs(cx);
                    s.account = Account::Failed(tr!("The key could not be kept safely: {error}", error = e.as_str()));
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Forgets the identity on this computer: the key, the settings that
    /// name it and the cache. What was published stays on the relays.
    pub fn forget(&mut self, cx: &mut Context<Self>) {
        self.forget_session(cx);
        if let Some(public_key) = self.prefs.identity.take().map(|i| i.public_key().to_string()) {
            secret::forget(&public_key, cx).detach();
        }
        self.prefs.enabled = false;
        self.save_prefs(cx);
        let path = self.app_paths.nostr_cache();
        cx.background_spawn(async move {
            let _ = std::fs::remove_file(path);
        })
        .detach();
    }

    /// Closes the connection and drops everything tied to the identity.
    fn forget_session(&mut self, cx: &mut Context<Self>) {
        self.cancel_pairing();
        self.close(cx);
        self.signer = None;
        self.account = Account::None;
        self.cache = Default::default();
        self.profiles.clear();
        self.friends.clear();
        self.mutes = Default::default();
        self.chats = Default::default();
        self.notice = None;
    }

    /// `nsec1…`, for "Show my secret key".
    pub fn export_nsec(&self) -> Option<String> {
        self.local_keys().map(identity::export_nsec)
    }

    /// `ncryptsec1…` with `passphrase` (NIP-49); takes about a second.
    pub fn export_encrypted(
        &self,
        passphrase: String,
        cx: &mut Context<Self>,
        done: impl FnOnce(Result<String, String>, &mut App) + 'static,
    ) {
        let Some(keys) = self.local_keys().cloned() else { return };
        cx.spawn(async move |_, cx| {
            let result =
                cx.background_spawn(async move { identity::export_encrypted(&keys, &passphrase).map_err(|e| e.to_string()) }).await;
            cx.update(|cx| done(result, cx));
        })
        .detach();
    }
}

fn import_text(e: &ImportError) -> String {
    match e {
        ImportError::Empty => tr!("Paste a secret key (nsec1…).").into(),
        ImportError::PublicKey => tr!("That is a public key (npub). Signing needs the secret key (nsec1…).").into(),
        ImportError::NeedsPassphrase => tr!("This key is encrypted: type its passphrase.").into(),
        ImportError::WrongPassphrase => tr!("Wrong passphrase.").into(),
        ImportError::NotAKey => tr!("That is not a secret key. It starts with nsec1 or ncryptsec1.").into(),
    }
}

pub(super) fn remote_text(e: &remote::Error) -> String {
    match e {
        remote::Error::NoRelay(_) => tr!("The signer's relays do not answer.").into(),
        remote::Error::Timeout => tr!("The signer did not answer. Is the app open on your phone?").into(),
        remote::Error::NeedsApproval(url) => tr!("Approve the request in your signer first: {url}", url = url.as_str()),
        remote::Error::Refused(why) => tr!("The signer refused: {why}", why = why.as_str()),
        remote::Error::Invalid(why) => tr!("That did not work: {why}", why = why.as_str()),
        remote::Error::Cancelled => String::new(),
    }
}
