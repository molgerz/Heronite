//! A tiny relay on 127.0.0.1 for the tests: keeps every event it gets,
//! answers `REQ` with the stored matches and `EOSE`, then forwards new
//! matching events live, and answers `EVENT` with `OK`. No network beyond
//! the loopback.

use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use nostr::prelude::MatchEventOptions;
use nostr::prelude::{ClientMessage, Event, Filter, RelayMessage, SubscriptionId};
use tungstenite::Message;

type Subs = Vec<(usize, SubscriptionId, Vec<Filter>)>;

pub struct MockRelay {
    pub url: String,
    pub events: Arc<Mutex<Vec<Event>>>,
}

impl MockRelay {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let events: Arc<Mutex<Vec<Event>>> = Arc::default();
        let subs: Arc<Mutex<Subs>> = Arc::default();
        let outboxes: Arc<Mutex<Vec<std::sync::mpsc::Sender<String>>>> = Arc::default();
        let store = events.clone();
        std::thread::spawn(move || {
            for (client, stream) in listener.incoming().enumerate() {
                let Ok(stream) = stream else { continue };
                let (store, subs, outboxes) = (store.clone(), subs.clone(), outboxes.clone());
                std::thread::spawn(move || {
                    let Ok(mut ws) = tungstenite::accept(stream) else { return };
                    ws.get_ref().set_read_timeout(Some(Duration::from_millis(50))).ok();
                    let (tx, rx) = std::sync::mpsc::channel::<String>();
                    {
                        let mut o = outboxes.lock().unwrap();
                        while o.len() < client {
                            o.push(std::sync::mpsc::channel().0);
                        }
                        o.push(tx);
                    }
                    loop {
                        while let Ok(text) = rx.try_recv() {
                            if ws.send(Message::text(text)).is_err() {
                                return;
                            }
                        }
                        let msg = match ws.read() {
                            Ok(Message::Text(t)) => t,
                            Ok(Message::Close(_)) => return,
                            Ok(_) => continue,
                            Err(tungstenite::Error::Io(e))
                                if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) =>
                            {
                                continue
                            }
                            Err(_) => return,
                        };
                        let Ok(msg) = ClientMessage::from_json(msg.as_str()) else { continue };
                        match msg {
                            ClientMessage::Event(event) => {
                                let event = event.into_owned();
                                // Stored before the OK, as a relay does: a
                                // test that saw the OK finds the event.
                                store.lock().unwrap().push(event.clone());
                                let ok = RelayMessage::ok(event.id, true, "");
                                let _ = ws.send(Message::text(ok.as_json()));
                                for (c, id, filters) in subs.lock().unwrap().iter() {
                                    if filters.iter().any(|f| f.match_event(&event, MatchEventOptions::new())) {
                                        let msg = RelayMessage::event(id.clone(), event.clone()).as_json();
                                        if let Some(tx) = outboxes.lock().unwrap().get(*c) {
                                            let _ = tx.send(msg);
                                        }
                                    }
                                }
                            }
                            ClientMessage::Req { subscription_id, filters } => {
                                let id = subscription_id.into_owned();
                                let filters: Vec<Filter> = filters.into_iter().map(|f| f.into_owned()).collect();
                                for e in store.lock().unwrap().iter() {
                                    if filters.iter().any(|f| f.match_event(e, MatchEventOptions::new())) {
                                        let _ = ws.send(Message::text(RelayMessage::event(id.clone(), e.clone()).as_json()));
                                    }
                                }
                                let _ = ws.send(Message::text(RelayMessage::eose(id.clone()).as_json()));
                                let mut s = subs.lock().unwrap();
                                s.retain(|(c, i, _)| !(*c == client && *i == id));
                                s.push((client, id, filters));
                            }
                            ClientMessage::Close(id) => {
                                subs.lock().unwrap().retain(|(c, i, _)| !(*c == client && i == id.as_ref()));
                            }
                            _ => {}
                        }
                    }
                });
            }
        });
        Self { url, events }
    }

    pub fn stored(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }
}
