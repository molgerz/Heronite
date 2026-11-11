//! Private messages (NIP-17): gift wraps from the user's inbox relays are
//! opened off the UI thread (with a remote signer each one is a request to
//! the phone), and messages go to each recipient's inbox relays (their
//! kind-10050 list, read first when it is not known yet).

use std::collections::{BTreeMap, HashSet};
use std::time::Duration;

use gpui_kit::*;
use heronite_i18n::tr;
use heronite_nostr::chat::{self, Message};
use heronite_nostr::{Event, EventId, Filter, Kind, PublicKey, Timestamp};

use super::Nostr;

/// Everyone in a chat, sorted (the user included).
pub type ChatKey = Vec<PublicKey>;

/// Gift wraps asked for at most at once.
const INBOX_LIMIT: usize = 500;
/// A message older than this when it arrives is history, not news: no
/// desktop notification.
const FRESH: u64 = 10 * 60;

#[derive(Default)]
pub struct State {
    messages: BTreeMap<ChatKey, Vec<Message>>,
    unread: HashSet<ChatKey>,
    seen: HashSet<EventId>,
    opened_wraps: HashSet<EventId>,
    /// Wraps waiting to be opened, and whether they came live.
    queue: Vec<(Event, bool)>,
    opening: bool,
    /// Sending: shows "Sending…" in the chat.
    pub sending: bool,
}

impl Nostr {
    /// The chats, the newest message first.
    pub fn chats(&self) -> Vec<(&ChatKey, &Message)> {
        let mut out: Vec<(&ChatKey, &Message)> = self.chats.messages.iter().filter_map(|(k, m)| m.last().map(|last| (k, last))).collect();
        out.sort_by_key(|(_, m)| std::cmp::Reverse(m.at));
        out
    }

    pub fn messages(&self, chat: &ChatKey) -> &[Message] {
        self.chats.messages.get(chat).map(Vec::as_slice).unwrap_or_default()
    }

    pub fn is_unread(&self, chat: &ChatKey) -> bool {
        self.chats.unread.contains(chat)
    }

    pub fn unread_count(&self) -> usize {
        self.chats.unread.len()
    }

    pub fn mark_read(&mut self, chat: &ChatKey, cx: &mut Context<Self>) {
        if self.chats.unread.remove(chat) {
            cx.notify();
        }
    }

    pub fn is_sending(&self) -> bool {
        self.chats.sending
    }

    /// The chat with these people (the user is added).
    pub fn chat_with(&self, people: &[PublicKey]) -> ChatKey {
        let mut key: ChatKey = people.to_vec();
        key.extend(self.me());
        key.sort();
        key.dedup();
        key
    }

    pub(super) fn subscribe_inbox(&mut self) {
        let (Some(pool), Some(me)) = (self.pool.as_ref(), self.me()) else { return };
        pool.subscribe("inbox", vec![chat::inbox_filter(&me, INBOX_LIMIT)]);
        // What the cache kept from earlier starts opens again.
        let kept: Vec<Event> = self.cache.wraps().to_vec();
        self.chats.queue.extend(kept.into_iter().map(|w| (w, false)));
    }

    /// Opens `wraps` (in the background). `live`: they came now, so a new
    /// message from someone else may notify.
    pub(super) fn open_wraps(&mut self, wraps: Vec<Event>, live: bool, cx: &mut Context<Self>) {
        let fresh: Vec<(Event, bool)> = wraps.into_iter().filter(|w| !self.chats.opened_wraps.contains(&w.id)).map(|w| (w, live)).collect();
        self.chats.queue.extend(fresh);
        self.open_queue(cx);
    }

    fn open_queue(&mut self, cx: &mut Context<Self>) {
        if self.chats.opening || self.chats.queue.is_empty() {
            return;
        }
        let Some(signer) = self.signer.clone() else { return };
        let batch = std::mem::take(&mut self.chats.queue);
        self.chats.opening = true;
        cx.spawn(async move |this, cx| {
            let opened = cx
                .background_spawn(async move {
                    batch.into_iter().map(|(wrap, live)| (chat::open(&signer, &wrap), wrap, live)).collect::<Vec<_>>()
                })
                .await;
            this.update(cx, |s, cx| {
                s.chats.opening = false;
                let mut kept = false;
                for (message, wrap, live) in opened {
                    s.chats.opened_wraps.insert(wrap.id);
                    let Some(message) = message else { continue };
                    if s.prefs.keep_history && s.cache.add_wrap(wrap) {
                        kept = true;
                    }
                    s.add_message(message, live);
                }
                if kept {
                    s.cache_changed(cx);
                }
                s.open_queue(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn add_message(&mut self, message: Message, live: bool) {
        if !self.chats.seen.insert(message.id) || self.mutes.people.contains(&message.from) {
            return;
        }
        let key = message.chat();
        let mine = Some(message.from) == self.me();
        let fresh = Timestamp::now().as_secs().saturating_sub(message.at.as_secs()) < FRESH;
        if live && !mine && fresh {
            self.chats.unread.insert(key.clone());
            let gaming = self.sharing_game().is_some();
            if self.prefs.notify_messages && (!gaming || self.prefs.notify_while_playing) {
                // The text stays in Heronite: the notification names the sender only.
                crate::system::notify::send(tr!("New message"), tr!("From {name}", name = self.label(&message.from)));
            }
        }
        let list = self.chats.messages.entry(key).or_default();
        list.push(message);
        list.sort_by_key(|m| m.at);
    }

    /// Sends `text` to `to` (one friend, or a small group with `subject`).
    pub fn send_message(&mut self, to: Vec<PublicKey>, text: String, subject: Option<String>, cx: &mut Context<Self>) {
        let Some(signer) = self.signer.clone().filter(|_| self.ready()) else { return };
        if text.trim().is_empty() || self.chats.sending {
            return;
        }
        let mine = self.prefs.write_relays();
        let readable = self.prefs.read_relays();
        // Inbox lists already known; the others are read before sending.
        let mut inboxes: Vec<(PublicKey, Vec<String>)> = to
            .iter()
            .map(|p| (*p, self.cache.latest(Kind::InboxRelays, p).map(heronite_nostr::relays::parse_inbox_list).unwrap_or_default()))
            .collect();
        self.chats.sending = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let unknown: Vec<PublicKey> = inboxes.iter().filter(|(_, r)| r.is_empty()).map(|(p, _)| *p).collect();
                    let mut found = Vec::new();
                    if !unknown.is_empty() {
                        found = heronite_nostr::relay::fetch(
                            &readable,
                            vec![Filter::new().kind(Kind::InboxRelays).authors(unknown)],
                            Duration::from_secs(8),
                        );
                        for (p, relays) in inboxes.iter_mut().filter(|(_, r)| r.is_empty()) {
                            if let Some(e) = heronite_nostr::social::newest(found.iter().filter(|e| e.pubkey == *p).cloned()) {
                                *relays = heronite_nostr::relays::parse_inbox_list(&e);
                            }
                        }
                    }
                    let outgoing = chat::wrap(&signer, &to, &text, subject.as_deref()).map_err(|e| e.to_string())?;
                    let me = signer.public_key();
                    let mut delivered = 0;
                    let mut own = None;
                    for (receiver, wrap) in &outgoing.wraps {
                        if *receiver == me {
                            own = Some(wrap.clone());
                            continue;
                        }
                        // NIP-17: to the recipient's inbox relays; without
                        // a list, the user's own write relays are the best guess.
                        let relays = inboxes.iter().find(|(p, _)| p == receiver).map(|(_, r)| r.clone()).filter(|r| !r.is_empty());
                        let report = heronite_nostr::relay::publish_once(relays.as_ref().unwrap_or(&mine), wrap, Duration::from_secs(10));
                        if report.iter().any(|r| r.result.is_ok()) {
                            delivered += 1;
                        }
                    }
                    Ok::<_, String>((outgoing.message, own, delivered, found))
                })
                .await;
            this.update(cx, |s, cx| {
                s.chats.sending = false;
                match result {
                    Ok((message, own, delivered, found)) => {
                        for e in found {
                            s.cache.put(e);
                        }
                        if let Some(own) = own {
                            // The user's copy goes to their own relays, for their other apps.
                            if let Some(pool) = s.pool.as_ref() {
                                pool.publish(&own);
                            }
                            s.chats.opened_wraps.insert(own.id);
                            if s.prefs.keep_history && s.cache.add_wrap(own) {
                                s.cache_changed(cx);
                            }
                        }
                        if delivered == 0 {
                            s.set_notice(tr!("The message could not be delivered: no relay took it."), true, cx);
                        }
                        s.add_message(message, false);
                    }
                    Err(e) => s.set_notice(tr!("Not sent: {error}", error = e.as_str()), true, cx),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}
