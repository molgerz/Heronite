//! The user's relays, kept open while the community layer is on: one thread
//! per relay that connects, reconnects with a growing pause, sends the open
//! subscriptions again after each reconnect, and answers a relay's AUTH
//! challenge (NIP-42) when a signer is set.
//!
//! What the relays send comes out of one channel ([`PoolEvent`]); an event
//! that several relays send comes out once. Dropping the [`Pool`] (or
//! [`Pool::stop`]) closes every connection and ends the threads.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use nostr::prelude::{ClientMessage, Event, EventBuilder, EventId, Filter, Kind, RelayMessage, RelayUrl, SubscriptionId, Tag};

use crate::relay::Connection;
use crate::signer::Signer;

/// Pause before the first reconnect; it doubles up to [`MAX_BACKOFF`].
const FIRST_BACKOFF: Duration = Duration::from_secs(2);
const MAX_BACKOFF: Duration = Duration::from_secs(5 * 60);
/// Events kept to send once a relay is back (published while it was down).
const MAX_PENDING: usize = 200;
/// Event ids remembered to pass each event on once.
const SEEN: usize = 20_000;

/// A relay's connection, for Settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelayStatus {
    Connecting,
    Connected,
    /// Waiting before the next try; the text is the last error.
    Failed(String),
}

/// What comes out of the pool.
#[derive(Debug, Clone)]
pub enum PoolEvent {
    /// A new event (verified) for one of the subscriptions.
    Event {
        relay: String,
        subscription: String,
        event: Box<Event>,
    },
    /// The relay sent every stored event it had for the subscription.
    EndOfStored {
        relay: String,
        subscription: String,
    },
    Status {
        relay: String,
        status: RelayStatus,
    },
    /// A relay's answer to a published event.
    Published {
        relay: String,
        event: EventId,
        accepted: bool,
        message: String,
    },
}

enum Command {
    Send(Box<ClientMessage<'static>>),
    Stop,
}

struct Worker {
    tx: Sender<Command>,
    read: bool,
    write: bool,
    handle: Option<JoinHandle<()>>,
}

/// State the relay threads share.
struct Shared {
    subs: Mutex<HashMap<String, Vec<Filter>>>,
    seen: Mutex<(HashSet<EventId>, VecDeque<EventId>)>,
    signer: Mutex<Option<Signer>>,
}

impl Shared {
    /// Whether `id` comes out for the first time.
    fn first_time(&self, id: EventId) -> bool {
        let mut seen = self.seen.lock().unwrap_or_else(|p| p.into_inner());
        let (set, order) = &mut *seen;
        if !set.insert(id) {
            return false;
        }
        order.push_back(id);
        if order.len() > SEEN {
            if let Some(old) = order.pop_front() {
                set.remove(&old);
            }
        }
        true
    }
}

pub struct Pool {
    workers: HashMap<String, Worker>,
    shared: Arc<Shared>,
    out: Sender<PoolEvent>,
}

impl Pool {
    /// An empty pool and the channel its events come out of.
    pub fn new() -> (Self, Receiver<PoolEvent>) {
        let (out, rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            subs: Mutex::new(HashMap::new()),
            seen: Mutex::new((HashSet::new(), VecDeque::new())),
            signer: Mutex::new(None),
        });
        (Self { workers: HashMap::new(), shared, out }, rx)
    }

    /// The signer that answers AUTH challenges; `None` stops answering.
    pub fn set_signer(&self, signer: Option<Signer>) {
        *self.shared.signer.lock().unwrap_or_else(|p| p.into_inner()) = signer;
    }

    /// Opens the relays that are new in `relays` (`(url, read, write)`),
    /// closes those no longer in it, and updates the flags of the others.
    pub fn set_relays(&mut self, relays: &[(String, bool, bool)]) {
        let wanted: HashSet<&str> = relays.iter().map(|(u, ..)| u.as_str()).collect();
        let gone: Vec<String> = self.workers.keys().filter(|u| !wanted.contains(u.as_str())).cloned().collect();
        for url in gone {
            if let Some(w) = self.workers.remove(&url) {
                stop_worker(w);
            }
        }
        for (url, read, write) in relays {
            if let Some(w) = self.workers.get_mut(url) {
                w.read = *read;
                w.write = *write;
                continue;
            }
            let (tx, rx) = mpsc::channel();
            let (shared, out, name) = (self.shared.clone(), self.out.clone(), url.clone());
            let read_flag = *read;
            let handle =
                std::thread::Builder::new().name("nostr-relay".into()).spawn(move || run(&name, read_flag, &rx, &shared, &out)).ok();
            self.workers.insert(url.clone(), Worker { tx, read: *read, write: *write, handle });
        }
    }

    pub fn relays(&self) -> Vec<String> {
        self.workers.keys().cloned().collect()
    }

    /// Opens (or replaces) the subscription `id` on every read relay.
    pub fn subscribe(&self, id: &str, filters: Vec<Filter>) {
        self.shared.subs.lock().unwrap_or_else(|p| p.into_inner()).insert(id.to_string(), filters.clone());
        let msg = ClientMessage::req(SubscriptionId::new(id), filters);
        for w in self.workers.values().filter(|w| w.read) {
            let _ = w.tx.send(Command::Send(Box::new(msg.clone())));
        }
    }

    pub fn unsubscribe(&self, id: &str) {
        if self.shared.subs.lock().unwrap_or_else(|p| p.into_inner()).remove(id).is_some() {
            let msg = ClientMessage::close(SubscriptionId::new(id));
            for w in self.workers.values().filter(|w| w.read) {
                let _ = w.tx.send(Command::Send(Box::new(msg.clone())));
            }
        }
    }

    /// Sends `event` to every write relay; their answers come out as
    /// [`PoolEvent::Published`]. Returns how many relays it went to.
    pub fn publish(&self, event: &Event) -> usize {
        let msg = ClientMessage::event(event.clone());
        let mut n = 0;
        for w in self.workers.values().filter(|w| w.write) {
            if w.tx.send(Command::Send(Box::new(msg.clone()))).is_ok() {
                n += 1;
            }
        }
        n
    }

    /// Closes every relay and waits for the threads.
    pub fn stop(&mut self) {
        for (_, w) in self.workers.drain() {
            stop_worker(w);
        }
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.stop();
    }
}

fn stop_worker(mut w: Worker) {
    let _ = w.tx.send(Command::Stop);
    if let Some(h) = w.handle.take() {
        // A thread in the middle of connecting ends once that times out;
        // nothing waits on it.
        drop(h);
    }
}

/// The thread of one relay. Ends on [`Command::Stop`] or when the pool is
/// gone.
fn run(url: &str, read: bool, rx: &Receiver<Command>, shared: &Shared, out: &Sender<PoolEvent>) {
    let status = |s: RelayStatus| out.send(PoolEvent::Status { relay: url.to_string(), status: s }).is_ok();
    let mut backoff = FIRST_BACKOFF;
    let mut pending: VecDeque<ClientMessage<'static>> = VecDeque::new();
    loop {
        if !status(RelayStatus::Connecting) {
            return;
        }
        let mut conn = match Connection::open(url) {
            Ok(c) => c,
            Err(e) => {
                if !status(RelayStatus::Failed(e.to_string())) {
                    return;
                }
                if wait_or_stop(rx, backoff, &mut pending) {
                    return;
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };
        status(RelayStatus::Connected);
        backoff = FIRST_BACKOFF;
        let resend = |conn: &mut Connection| -> bool {
            if !read {
                return true;
            }
            let subs = shared.subs.lock().unwrap_or_else(|p| p.into_inner()).clone();
            subs.into_iter().all(|(id, filters)| conn.send(&ClientMessage::req(SubscriptionId::new(id), filters)).is_ok())
        };
        let mut alive = resend(&mut conn);
        while alive {
            if let Some(msg) = pending.front() {
                if conn.send(msg).is_err() {
                    break;
                }
                pending.pop_front();
                continue;
            }
            match rx.try_recv() {
                Ok(Command::Send(msg)) => {
                    if conn.send(&msg).is_err() {
                        keep(&mut pending, *msg);
                        break;
                    }
                }
                Ok(Command::Stop) | Err(TryRecvError::Disconnected) => {
                    conn.close();
                    return;
                }
                Err(TryRecvError::Empty) => {}
            }
            match conn.recv() {
                Ok(Some(msg)) => alive = handle(url, msg, &mut conn, shared, out, &resend),
                Ok(None) => {}
                Err(e) => {
                    log::debug!("{url}: {e}");
                    alive = false;
                }
            }
        }
        if !status(RelayStatus::Failed("the connection closed".into())) {
            return;
        }
        if wait_or_stop(rx, backoff, &mut pending) {
            return;
        }
    }
}

/// Handles one message; `false` when the connection should be dropped.
fn handle(
    url: &str,
    msg: RelayMessage<'static>,
    conn: &mut Connection,
    shared: &Shared,
    out: &Sender<PoolEvent>,
    resend: &dyn Fn(&mut Connection) -> bool,
) -> bool {
    match msg {
        RelayMessage::Event { subscription_id, event } => {
            if event.verify().is_ok() && shared.first_time(event.id) {
                let _ = out.send(PoolEvent::Event {
                    relay: url.to_string(),
                    subscription: subscription_id.to_string(),
                    event: Box::new(event.into_owned()),
                });
            }
            true
        }
        RelayMessage::EndOfStoredEvents(sub) => {
            let _ = out.send(PoolEvent::EndOfStored { relay: url.to_string(), subscription: sub.to_string() });
            true
        }
        RelayMessage::Ok { event_id, status, message } => {
            let _ =
                out.send(PoolEvent::Published { relay: url.to_string(), event: event_id, accepted: status, message: message.into_owned() });
            true
        }
        RelayMessage::Auth { challenge } => {
            let signer = shared.signer.lock().unwrap_or_else(|p| p.into_inner()).clone();
            let Some(signer) = signer else { return true };
            let Ok(relay) = RelayUrl::parse(url) else { return true };
            let builder = EventBuilder::new(Kind::Authentication, "")
                .tag(Tag::custom("relay", [relay.as_str().to_string()]))
                .tag(Tag::custom("challenge", [challenge.to_string()]));
            match signer.sign(builder) {
                // Subscriptions refused before AUTH are asked again after it.
                Ok(auth) => conn.send(&ClientMessage::auth(auth)).is_ok() && resend(conn),
                Err(e) => {
                    log::debug!("{url}: AUTH not signed: {e}");
                    true
                }
            }
        }
        RelayMessage::Notice(text) => {
            log::debug!("{url}: notice: {text}");
            true
        }
        _ => true,
    }
}

fn keep(pending: &mut VecDeque<ClientMessage<'static>>, msg: ClientMessage<'static>) {
    // Only events are worth keeping: subscriptions are sent again anyway.
    if msg.is_event() {
        pending.push_back(msg);
        while pending.len() > MAX_PENDING {
            pending.pop_front();
        }
    }
}

/// Waits `pause` while queueing what comes in; `true` when the pool stops.
fn wait_or_stop(rx: &Receiver<Command>, pause: Duration, pending: &mut VecDeque<ClientMessage<'static>>) -> bool {
    let deadline = std::time::Instant::now() + pause;
    loop {
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return false;
        }
        match rx.recv_timeout(left) {
            Ok(Command::Send(msg)) => keep(pending, *msg),
            Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => return true,
            Err(RecvTimeoutError::Timeout) => return false,
        }
    }
}

#[cfg(test)]
mod tests;
