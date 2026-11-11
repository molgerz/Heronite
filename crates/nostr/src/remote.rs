//! A remote signer (NIP-46, "Nostr Connect"): the secret key stays in an app
//! on the user's phone (Amber, nsec.app…), and Heronite asks it to sign and
//! to encrypt. Heronite has its own throwaway key for the session (the
//! client key); the requests and answers are kind-24133 events between the
//! two keys, NIP-44 encrypted, through the signer's relays.
//!
//! Two ways to pair:
//! * [`Pairing`]: Heronite shows a `nostrconnect://` QR code and waits for
//!   the phone to answer with the code's secret (the client starts).
//! * [`RemoteSigner::connect_bunker`]: the user pastes a `bunker://` address
//!   from the signer app (the signer starts).
//!
//! Every call blocks until the phone answers or [`TIMEOUT`] passes; the
//! user may have to approve on the phone first.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use nostr::nips::nip44::Nip44;
use nostr::nips::nip46::{
    NostrConnectEventBuilder, NostrConnectMessage, NostrConnectMethod, NostrConnectRequest, NostrConnectResponse, NostrConnectUri,
    ResponseResult,
};
use nostr::prelude::{
    ClientMessage, Event, Filter, FinalizeEvent, Keys, Kind, PublicKey, RelayMessage, RelayUrl, SubscriptionId, Timestamp, UnsignedEvent,
};

use crate::relay::Connection;

/// How long a request waits for the phone (the user may have to unlock it
/// and approve).
pub const TIMEOUT: Duration = Duration::from_secs(90);

/// The relays a `nostrconnect://` code names when the user has none that
/// suit: public ones that signer apps know.
pub const CONNECT_RELAYS: [&str; 2] = ["wss://relay.nsec.app", "wss://relay.damus.io"];

/// What Heronite asks the signer to allow, in NIP-46's `perms` form: the
/// event kinds the community layer signs, and NIP-44 for private messages.
const PERMISSIONS: &str = "nip44_encrypt,nip44_decrypt,sign_event:0,sign_event:3,sign_event:11,sign_event:13,sign_event:20,\
sign_event:21,sign_event:1111,sign_event:1984,sign_event:10000,sign_event:10002,sign_event:10050,sign_event:22242,\
sign_event:24242,sign_event:30315";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// None of the signer's relays could be reached.
    NoRelay(String),
    /// The phone did not answer in time.
    Timeout,
    /// The user has to approve on this web page first (the signer's
    /// `auth_url`); ask again afterwards.
    NeedsApproval(String),
    /// The signer said no, with its reason.
    Refused(String),
    /// The address or an answer could not be read.
    Invalid(String),
    Cancelled,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoRelay(e) => write!(f, "the signer's relays do not answer: {e}"),
            Error::Timeout => write!(f, "the signer did not answer in time"),
            Error::NeedsApproval(url) => write!(f, "approve the request first: {url}"),
            Error::Refused(e) => write!(f, "the signer refused: {e}"),
            Error::Invalid(e) => write!(f, "{e}"),
            Error::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::error::Error for Error {}

/// The open connections to the signer's relays, subscribed to the answers
/// for the client key.
struct Session {
    conns: Vec<Connection>,
    sub: SubscriptionId,
}

impl Session {
    fn open(relays: &[String], client: &PublicKey) -> Result<Self, Error> {
        let sub = SubscriptionId::generate();
        // A little in the past: an answer can come before the subscription.
        let since = Timestamp::from_secs(Timestamp::now().as_secs().saturating_sub(10));
        let filter = Filter::new().kind(Kind::NostrConnect).pubkey(*client).since(since);
        let mut conns = Vec::new();
        let mut last = String::from("no relay");
        for url in relays {
            match Connection::open(url).and_then(|mut c| c.send(&ClientMessage::req(sub.clone(), vec![filter.clone()])).map(|_| c)) {
                Ok(c) => conns.push(c),
                Err(e) => last = format!("{url}: {e}"),
            }
        }
        if conns.is_empty() {
            return Err(Error::NoRelay(last));
        }
        Ok(Self { conns, sub })
    }

    fn send(&mut self, event: &Event) {
        let msg = ClientMessage::event(event.clone());
        self.conns.retain_mut(|c| c.send(&msg).is_ok());
    }

    /// The next NIP-46 message from `from` (any sender when `None`) to
    /// `client`, until `deadline`.
    fn next(
        &mut self,
        client: &Keys,
        from: Option<&PublicKey>,
        deadline: Instant,
        cancel: Option<&AtomicBool>,
    ) -> Result<(PublicKey, NostrConnectMessage), Error> {
        while Instant::now() < deadline {
            if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                return Err(Error::Cancelled);
            }
            if self.conns.is_empty() {
                return Err(Error::NoRelay("every connection closed".into()));
            }
            let mut i = 0;
            while i < self.conns.len() {
                match self.conns[i].recv() {
                    Ok(Some(RelayMessage::Event { subscription_id, event })) if *subscription_id == self.sub => {
                        if from.is_some_and(|f| *f != event.pubkey) || event.verify().is_err() {
                            i += 1;
                            continue;
                        }
                        let Ok(json) = client.nip44_decrypt(&event.pubkey, &event.content) else {
                            i += 1;
                            continue;
                        };
                        if let Ok(msg) = NostrConnectMessage::from_json(json) {
                            return Ok((event.pubkey, msg));
                        }
                        i += 1;
                    }
                    Ok(_) => i += 1,
                    Err(e) => {
                        log::debug!("remote signer relay {}: {e}", self.conns[i].url());
                        self.conns.remove(i);
                    }
                }
            }
        }
        Err(Error::Timeout)
    }
}

/// Waiting for a phone to scan the `nostrconnect://` code.
pub struct Pairing {
    client: Keys,
    relays: Vec<String>,
    secret: String,
    uri: String,
}

impl Pairing {
    /// A new client key and the code to show. `relays`: where the phone
    /// should answer (empty: [`CONNECT_RELAYS`]).
    pub fn new(relays: &[String]) -> Self {
        let client = Keys::generate();
        let relays: Vec<String> = if relays.is_empty() { CONNECT_RELAYS.iter().map(|r| r.to_string()).collect() } else { relays.to_vec() };
        let urls: Vec<RelayUrl> = relays.iter().filter_map(|r| RelayUrl::parse(r).ok()).collect();
        let connect = NostrConnectUri::client(client.public_key(), urls, "Heronite");
        let secret = connect.secret().unwrap_or_default().to_string();
        let uri = format!("{connect}&perms={PERMISSIONS}");
        Self { client, relays, secret, uri }
    }

    /// `nostrconnect://…`, for the QR code (and a "copy" button).
    pub fn uri(&self) -> &str {
        &self.uri
    }

    /// Blocks until a signer answers with the code's secret, `cancel` is
    /// set or [`TIMEOUT`] passes, then asks it for the user's key.
    pub fn wait(self, cancel: &AtomicBool) -> Result<RemoteSigner, Error> {
        let mut session = Session::open(&self.relays, &self.client.public_key())?;
        let deadline = Instant::now() + TIMEOUT;
        let signer = loop {
            let (from, msg) = session.next(&self.client, None, deadline, Some(cancel))?;
            if let NostrConnectMessage::Response { result: Some(result), .. } = &msg {
                // NIP-46: the signer must echo the code's secret, so a
                // stranger who saw only the relays cannot pair.
                if *result == self.secret {
                    break from;
                }
            }
        };
        RemoteSigner::finish(self.client, signer, self.relays, session)
    }
}

/// A paired remote signer. Requests go one at a time.
pub struct RemoteSigner {
    client: Keys,
    signer: PublicKey,
    user: PublicKey,
    relays: Vec<String>,
    session: Mutex<Option<Session>>,
}

impl fmt::Debug for RemoteSigner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RemoteSigner").field("signer", &self.signer).field("user", &self.user).finish_non_exhaustive()
    }
}

impl RemoteSigner {
    /// Pairs through a `bunker://` address pasted from the signer app.
    pub fn connect_bunker(uri: &str) -> Result<Self, Error> {
        let parsed = NostrConnectUri::parse(uri.trim()).map_err(|e| Error::Invalid(e.to_string()))?;
        let NostrConnectUri::Bunker { remote_signer_public_key, relays, secret } = parsed else {
            return Err(Error::Invalid("not a bunker:// address".into()));
        };
        let client = Keys::generate();
        let relays: Vec<String> = relays.iter().map(|r| r.as_str_without_trailing_slash().to_string()).collect();
        let session = Session::open(&relays, &client.public_key())?;
        let mut this = Self {
            client,
            signer: remote_signer_public_key,
            // Replaced by `get_public_key` below; the signer key is a placeholder.
            user: remote_signer_public_key,
            relays,
            session: Mutex::new(Some(session)),
        };
        this.request(NostrConnectRequest::Connect { remote_signer_public_key, secret })?;
        this.user = this.request(NostrConnectRequest::GetPublicKey)?.to_get_public_key().map_err(|e| Error::Invalid(e.to_string()))?;
        Ok(this)
    }

    /// A signer paired before: its keys come from the settings and the
    /// keyring. Connects on the first request.
    pub fn restore(client: Keys, signer: PublicKey, user: PublicKey, relays: Vec<String>) -> Self {
        Self { client, signer, user, relays, session: Mutex::new(None) }
    }

    fn finish(client: Keys, signer: PublicKey, relays: Vec<String>, session: Session) -> Result<Self, Error> {
        let mut this = Self { client, signer, user: signer, relays, session: Mutex::new(Some(session)) };
        this.user = this.request(NostrConnectRequest::GetPublicKey)?.to_get_public_key().map_err(|e| Error::Invalid(e.to_string()))?;
        Ok(this)
    }

    pub fn user_public_key(&self) -> PublicKey {
        self.user
    }

    pub fn signer_public_key(&self) -> PublicKey {
        self.signer
    }

    pub fn relays(&self) -> &[String] {
        &self.relays
    }

    /// This session's client key, kept like a secret key (keyring or DPAPI).
    pub fn client_keys(&self) -> &Keys {
        &self.client
    }

    /// Whether the signer still answers.
    pub fn ping(&self) -> Result<(), Error> {
        self.request(NostrConnectRequest::Ping).map(|_| ())
    }

    pub fn sign_event(&self, unsigned: UnsignedEvent) -> Result<Event, Error> {
        let event =
            self.request(NostrConnectRequest::SignEvent(unsigned.clone()))?.to_sign_event().map_err(|e| Error::Invalid(e.to_string()))?;
        // Signing must not change what was asked for.
        if event.pubkey != self.user || event.kind != unsigned.kind || event.content != unsigned.content || event.verify().is_err() {
            return Err(Error::Invalid("the signer returned another event".into()));
        }
        Ok(event)
    }

    pub fn nip44_encrypt(&self, public_key: &PublicKey, text: &str) -> Result<String, Error> {
        self.request(NostrConnectRequest::Nip44Encrypt { public_key: *public_key, text: text.to_string() })?
            .to_nip44_encrypt()
            .map_err(|e| Error::Invalid(e.to_string()))
    }

    pub fn nip44_decrypt(&self, public_key: &PublicKey, ciphertext: &str) -> Result<String, Error> {
        self.request(NostrConnectRequest::Nip44Decrypt { public_key: *public_key, ciphertext: ciphertext.to_string() })?
            .to_nip44_decrypt()
            .map_err(|e| Error::Invalid(e.to_string()))
    }

    fn request(&self, req: NostrConnectRequest) -> Result<ResponseResult, Error> {
        let method: NostrConnectMethod = req.method();
        let msg = NostrConnectMessage::request(&req);
        let id = msg.id().to_string();
        let event = NostrConnectEventBuilder::new(self.signer, msg).finalize(&self.client).map_err(|e| Error::Invalid(e.to_string()))?;
        let mut guard = self.session.lock().unwrap_or_else(|p| p.into_inner());
        if guard.as_ref().is_none_or(|s| s.conns.is_empty()) {
            *guard = Some(Session::open(&self.relays, &self.client.public_key())?);
        }
        let session = guard.as_mut().expect("opened above");
        session.send(&event);
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let (_, answer) = session.next(&self.client, Some(&self.signer), deadline, None)?;
            let NostrConnectMessage::Response { id: answer_id, result, error } = answer else { continue };
            if answer_id != id {
                continue;
            }
            let response = NostrConnectResponse::parse(method, result, error.clone()).map_err(|e| Error::Invalid(e.to_string()))?;
            if response.is_auth_url() {
                return Err(Error::NeedsApproval(error.unwrap_or_default()));
            }
            return match (response.result, response.error) {
                (Some(ResponseResult::Error), e) | (None, e) => Err(Error::Refused(e.unwrap_or_default())),
                (Some(result), _) => Ok(result),
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pairing_code_names_the_client_relays_secret_and_permissions() {
        let pairing = Pairing::new(&[]);
        let uri = pairing.uri();
        assert!(uri.starts_with("nostrconnect://"));
        assert!(uri.contains(&pairing.client.public_key().to_hex()));
        assert!(uri.contains(&pairing.secret));
        assert!(uri.contains("perms=nip44_encrypt"));
        assert!(uri.contains("relay.nsec.app"));
        // The code parses back as a client URI with the same secret.
        let back = NostrConnectUri::parse(uri).unwrap();
        assert_eq!(back.secret(), Some(pairing.secret.as_str()));
    }

    #[test]
    fn a_bunker_address_is_checked_before_connecting() {
        assert!(matches!(RemoteSigner::connect_bunker("nostrconnect://x"), Err(Error::Invalid(_))));
        assert!(matches!(RemoteSigner::connect_bunker("hello"), Err(Error::Invalid(_))));
    }
}
