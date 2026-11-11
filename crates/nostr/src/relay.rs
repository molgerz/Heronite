//! One relay, over a blocking WebSocket (NIP-01): `wss://` through rustls
//! with the Mozilla roots (`webpki-roots`, the same as the app's HTTP), or
//! `ws://` for a relay on this computer.
//!
//! [`Connection::recv`] waits at most [`POLL`], so a thread that owns a
//! connection can also look at its own queue in between. [`fetch`] and
//! [`publish_once`] are one-shot helpers that open short connections.

use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant};

use nostr::prelude::{ClientMessage, Event, EventId, Filter, RelayMessage, SubscriptionId, Url};
use tungstenite::client::IntoClientRequest;
use tungstenite::protocol::WebSocketConfig;
use tungstenite::{Message, WebSocket};

/// How long [`Connection::recv`] waits for a message.
pub const POLL: Duration = Duration::from_millis(200);
/// Connecting and the WebSocket handshake.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A relay's message is at most this big: big enough for a long note with
/// many tags, small enough that a broken relay cannot fill the memory.
const MAX_MESSAGE: usize = 4 << 20;

/// Why a relay could not be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Not a `ws://` or `wss://` address.
    BadUrl,
    /// It did not answer (refused, unknown host, timed out). The text is for
    /// the log and the Settings card.
    Unreachable(String),
    /// The connection broke or the relay closed it.
    Closed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::BadUrl => write!(f, "not a relay address (wss://…)"),
            Error::Unreachable(e) => write!(f, "the relay does not answer: {e}"),
            Error::Closed(e) => write!(f, "the connection closed: {e}"),
        }
    }
}

impl std::error::Error for Error {}

enum Stream {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Stream {
    fn tcp(&self) -> &TcpStream {
        match self {
            Stream::Plain(s) => s,
            Stream::Tls(s) => s.get_ref(),
        }
    }
}

impl Read for Stream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.read(buf),
            Stream::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Stream::Plain(s) => s.write(buf),
            Stream::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Stream::Plain(s) => s.flush(),
            Stream::Tls(s) => s.flush(),
        }
    }
}

/// rustls with ring (the provider the rest of the app links) and the
/// Mozilla roots, built once.
fn tls_config() -> Arc<rustls::ClientConfig> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
            let config = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
                .with_safe_default_protocol_versions()
                .expect("ring supports the default TLS versions")
                .with_root_certificates(roots)
                .with_no_client_auth();
            Arc::new(config)
        })
        .clone()
}

/// The normalised form of a relay address (`wss://host/path`, no trailing
/// slash), or `None` when it is not one.
pub fn normalize_url(input: &str) -> Option<String> {
    let input = input.trim();
    let with_scheme = if input.contains("://") { input.to_string() } else { format!("wss://{input}") };
    let url = Url::parse(&with_scheme).ok()?;
    if !matches!(url.scheme(), "ws" | "wss") || url.host_str().is_none_or(str::is_empty) {
        return None;
    }
    Some(url.as_str().trim_end_matches('/').to_string())
}

/// An open connection to one relay.
pub struct Connection {
    url: String,
    ws: WebSocket<Stream>,
}

impl Connection {
    /// Connects and does the WebSocket handshake.
    pub fn open(url: &str) -> Result<Self, Error> {
        let parsed = Url::parse(url).map_err(|_| Error::BadUrl)?;
        let tls = match parsed.scheme() {
            "wss" => true,
            "ws" => false,
            _ => return Err(Error::BadUrl),
        };
        let host = parsed.host_str().filter(|h| !h.is_empty()).ok_or(Error::BadUrl)?.to_string();
        let port = parsed.port_or_known_default().ok_or(Error::BadUrl)?;
        let unreachable = |e: io::Error| Error::Unreachable(e.to_string());
        let addrs: Vec<_> = (host.as_str(), port).to_socket_addrs().map_err(unreachable)?.collect();
        let mut last = io::Error::new(io::ErrorKind::NotFound, "no address");
        let tcp = addrs
            .iter()
            .find_map(|a| TcpStream::connect_timeout(a, CONNECT_TIMEOUT).map_err(|e| last = e).ok())
            .ok_or_else(|| unreachable(last))?;
        tcp.set_nodelay(true).ok();
        // The handshake waits for whole answers; `recv` polls afterwards.
        tcp.set_read_timeout(Some(CONNECT_TIMEOUT)).map_err(unreachable)?;
        tcp.set_write_timeout(Some(CONNECT_TIMEOUT)).map_err(unreachable)?;
        let stream = if tls {
            let name = rustls::pki_types::ServerName::try_from(host.clone()).map_err(|_| Error::BadUrl)?;
            let conn = rustls::ClientConnection::new(tls_config(), name).map_err(|e| Error::Unreachable(e.to_string()))?;
            Stream::Tls(Box::new(rustls::StreamOwned::new(conn, tcp)))
        } else {
            Stream::Plain(tcp)
        };
        let mut request = url.into_client_request().map_err(|_| Error::BadUrl)?;
        if let Ok(agent) = crate::USER_AGENT.parse() {
            request.headers_mut().insert("User-Agent", agent);
        }
        let config = WebSocketConfig::default().max_message_size(Some(MAX_MESSAGE)).max_frame_size(Some(MAX_MESSAGE));
        let (ws, _) =
            tungstenite::client::client_with_config(request, stream, Some(config)).map_err(|e| Error::Unreachable(e.to_string()))?;
        ws.get_ref().tcp().set_read_timeout(Some(POLL)).map_err(unreachable)?;
        Ok(Self { url: url.to_string(), ws })
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub fn send(&mut self, message: &ClientMessage<'_>) -> Result<(), Error> {
        self.ws.send(Message::text(message.as_json())).map_err(|e| Error::Closed(e.to_string()))
    }

    /// The next message, or `None` when nothing came within [`POLL`] (or it
    /// was not one this client understands).
    pub fn recv(&mut self) -> Result<Option<RelayMessage<'static>>, Error> {
        match self.ws.read() {
            Ok(Message::Text(text)) => match RelayMessage::from_json(text.as_str()) {
                Ok(msg) => Ok(Some(msg)),
                Err(e) => {
                    log::debug!("{}: unreadable message: {e}", self.url);
                    Ok(None)
                }
            },
            Ok(Message::Close(frame)) => Err(Error::Closed(frame.map(|f| f.reason.to_string()).unwrap_or_default())),
            Ok(_) => Ok(None),
            Err(tungstenite::Error::Io(e)) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => Ok(None),
            Err(e) => Err(Error::Closed(e.to_string())),
        }
    }

    pub fn close(mut self) {
        let _ = self.ws.close(None);
        let _ = self.ws.flush();
    }
}

/// Every event matching `filters` that the relays in `urls` have, each
/// once, read in parallel until every relay sent its stored events or
/// `timeout` passed. Relays that fail are skipped.
pub fn fetch(urls: &[String], filters: Vec<Filter>, timeout: Duration) -> Vec<Event> {
    let (tx, rx) = mpsc::channel::<Option<Event>>();
    let deadline = Instant::now() + timeout;
    for url in urls {
        let (tx, url, filters) = (tx.clone(), url.clone(), filters.clone());
        std::thread::Builder::new()
            .name("nostr-fetch".into())
            .spawn(move || {
                if let Err(e) = fetch_one(&url, filters, deadline, &tx) {
                    log::debug!("{url}: {e}");
                }
                let _ = tx.send(None);
            })
            .ok();
    }
    drop(tx);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut open = urls.len();
    while open > 0 {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(Some(event)) => {
                if seen.insert(event.id) {
                    out.push(event);
                }
            }
            Ok(None) => open -= 1,
            Err(_) => break,
        }
    }
    out
}

fn fetch_one(url: &str, filters: Vec<Filter>, deadline: Instant, tx: &mpsc::Sender<Option<Event>>) -> Result<(), Error> {
    let mut conn = Connection::open(url)?;
    let id = SubscriptionId::generate();
    conn.send(&ClientMessage::req(id.clone(), filters))?;
    while Instant::now() < deadline {
        match conn.recv()? {
            Some(RelayMessage::Event { subscription_id, event }) if *subscription_id == id => {
                if event.verify().is_ok() {
                    let _ = tx.send(Some(event.into_owned()));
                }
            }
            Some(RelayMessage::EndOfStoredEvents(sub)) if *sub == id => break,
            Some(RelayMessage::Closed { subscription_id, .. }) if *subscription_id == id => break,
            _ => {}
        }
    }
    let _ = conn.send(&ClientMessage::close(id));
    conn.close();
    Ok(())
}

/// What one relay answered to a published event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Published {
    pub url: String,
    /// `Ok(())` when it took the event; otherwise its reason (or why it
    /// could not be reached).
    pub result: Result<(), String>,
}

/// Sends `event` to each relay in `urls` (in parallel) and waits for their
/// `OK` until `timeout`. For relays the user does not keep open, such as a
/// friend's inbox relays.
pub fn publish_once(urls: &[String], event: &Event, timeout: Duration) -> Vec<Published> {
    let deadline = Instant::now() + timeout;
    let handles: Vec<_> = urls
        .iter()
        .map(|url| {
            let (url, event) = (url.clone(), event.clone());
            std::thread::spawn(move || {
                let result = publish_one(&url, &event, deadline);
                Published { url, result }
            })
        })
        .collect();
    handles.into_iter().filter_map(|h| h.join().ok()).collect()
}

fn publish_one(url: &str, event: &Event, deadline: Instant) -> Result<(), String> {
    let mut conn = Connection::open(url).map_err(|e| e.to_string())?;
    conn.send(&ClientMessage::event(event.clone())).map_err(|e| e.to_string())?;
    let answer = wait_ok(&mut conn, event.id, deadline);
    conn.close();
    answer
}

fn wait_ok(conn: &mut Connection, id: EventId, deadline: Instant) -> Result<(), String> {
    while Instant::now() < deadline {
        if let Some(RelayMessage::Ok { event_id, status, message }) = conn.recv().map_err(|e| e.to_string())? {
            if event_id == id {
                return if status { Ok(()) } else { Err(message.into_owned()) };
            }
        }
    }
    Err("no answer".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_addresses_are_normalised() {
        assert_eq!(normalize_url("relay.damus.io").as_deref(), Some("wss://relay.damus.io"));
        assert_eq!(normalize_url(" wss://nos.lol/ ").as_deref(), Some("wss://nos.lol"));
        assert_eq!(normalize_url("ws://127.0.0.1:7777").as_deref(), Some("ws://127.0.0.1:7777"));
        assert_eq!(normalize_url("wss://relay.example/inbox").as_deref(), Some("wss://relay.example/inbox"));
        assert_eq!(normalize_url("https://relay.example"), None);
        assert_eq!(normalize_url("wss://"), None);
        assert_eq!(normalize_url(""), None);
    }

    #[test]
    fn a_bad_address_fails_without_the_network() {
        assert_eq!(Connection::open("https://example.com").err(), Some(Error::BadUrl));
        assert_eq!(Connection::open("nonsense").err(), Some(Error::BadUrl));
    }
}
