//! What the relays sent last, kept in `AppPaths::nostr_cache()` so the
//! Friends page shows names, pictures and lists at once and offline:
//! * the newest replaceable event per author and kind (profiles, follow
//!   lists, mute lists, relay lists);
//! * when the user keeps chat history, the gift wraps of their private
//!   messages. They stay as they came, encrypted for the user's key: the
//!   history on disk opens only with the same key, and is never stored in
//!   the clear.
//!
//! It is a cache: losing it costs one more read from the relays.

use std::collections::HashMap;

use anyhow::Result;
use heronite_data::json_store;
use heronite_data::paths::AppPaths;
use nostr::prelude::{Event, Kind, PublicKey};
use serde_json::{json, Value};

/// Kinds kept newest-per-author.
const REPLACEABLE: [Kind; 5] = [Kind::Metadata, Kind::ContactList, Kind::MuteList, Kind::RelayList, Kind::InboxRelays];
/// Gift wraps kept at most (the oldest go first).
const MAX_WRAPS: usize = 5000;

#[derive(Debug, Default, Clone)]
pub struct Cache {
    latest: HashMap<(u16, PublicKey), Event>,
    wraps: Vec<Event>,
}

impl Cache {
    /// A missing or unreadable file is an empty cache.
    pub fn load(paths: &AppPaths) -> Self {
        let v = json_store::read_value(&paths.nostr_cache()).unwrap_or(Value::Null);
        let events = |k: &str| -> Vec<Event> {
            v.get(k)
                .and_then(Value::as_array)
                .map(|a| a.iter().filter_map(|e| Event::from_json(e.to_string()).ok()).filter(|e| e.verify().is_ok()).collect())
                .unwrap_or_default()
        };
        let mut cache = Self::default();
        for e in events("latest") {
            cache.put(e);
        }
        for w in events("wraps") {
            cache.add_wrap(w);
        }
        cache
    }

    pub fn save(&self, paths: &AppPaths) -> Result<()> {
        let as_json = |e: &Event| serde_json::from_str::<Value>(&e.as_json()).unwrap_or(Value::Null);
        let mut latest: Vec<&Event> = self.latest.values().collect();
        latest.sort_by_key(|e| (e.kind.as_u16(), e.pubkey));
        json_store::write(
            &paths.nostr_cache(),
            &json!({
                "latest": latest.into_iter().map(as_json).collect::<Vec<_>>(),
                "wraps": self.wraps.iter().map(as_json).collect::<Vec<_>>(),
            }),
        )
    }

    /// Keeps `event` when it is a replaceable kind and newer than the one
    /// kept; `true` when it changed the cache.
    pub fn put(&mut self, event: Event) -> bool {
        if !REPLACEABLE.contains(&event.kind) {
            return false;
        }
        let key = (event.kind.as_u16(), event.pubkey);
        match self.latest.get(&key) {
            Some(old) if (old.created_at, std::cmp::Reverse(old.id)) >= (event.created_at, std::cmp::Reverse(event.id)) => false,
            _ => {
                self.latest.insert(key, event);
                true
            }
        }
    }

    pub fn latest(&self, kind: Kind, author: &PublicKey) -> Option<&Event> {
        self.latest.get(&(kind.as_u16(), *author))
    }

    /// Keeps a gift wrap; `false` when it was kept already.
    pub fn add_wrap(&mut self, wrap: Event) -> bool {
        if wrap.kind != Kind::GiftWrap || self.wraps.iter().any(|w| w.id == wrap.id) {
            return false;
        }
        self.wraps.push(wrap);
        if self.wraps.len() > MAX_WRAPS {
            self.wraps.sort_by_key(|w| w.created_at);
            let extra = self.wraps.len() - MAX_WRAPS;
            self.wraps.drain(..extra);
        }
        true
    }

    pub fn wraps(&self) -> &[Event] {
        &self.wraps
    }

    pub fn clear_wraps(&mut self) {
        self.wraps.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::prelude::{EventBuilder, FinalizeEvent, Keys, Timestamp};

    fn paths(dir: &std::path::Path) -> AppPaths {
        AppPaths { config: dir.join("config"), cache: dir.join("cache") }
    }

    #[test]
    fn keeps_the_newest_replaceable_event_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        let keys = Keys::generate();
        let at = |s: u64, c: &str| EventBuilder::new(Kind::Metadata, c).custom_created_at(Timestamp::from_secs(s)).finalize(&keys).unwrap();
        let mut cache = Cache::load(&p);
        assert!(cache.put(at(200, "{\"name\":\"new\"}")));
        assert!(!cache.put(at(100, "{\"name\":\"old\"}")));
        assert!(!cache.put(EventBuilder::new(Kind::TextNote, "note").finalize(&keys).unwrap()));
        let wrap = crate::chat::wrap(&crate::Signer::Local(keys.clone()), &[Keys::generate().public_key()], "hi", None).unwrap();
        assert!(cache.add_wrap(wrap.wraps[0].1.clone()));
        assert!(!cache.add_wrap(wrap.wraps[0].1.clone()));
        std::fs::create_dir_all(&p.cache).unwrap();
        cache.save(&p).unwrap();
        let back = Cache::load(&p);
        assert_eq!(back.latest(Kind::Metadata, &keys.public_key()).unwrap().content, "{\"name\":\"new\"}");
        assert_eq!(back.wraps().len(), 1);
        let raw = std::fs::read_to_string(p.nostr_cache()).unwrap();
        assert!(!raw.contains("\"hi\""), "messages stay encrypted on disk");
    }

    #[test]
    fn a_tampered_event_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        let keys = Keys::generate();
        let e = EventBuilder::new(Kind::Metadata, "{}").finalize(&keys).unwrap();
        let mut v: Value = serde_json::from_str(&e.as_json()).unwrap();
        v["content"] = json!("{\"name\":\"forged\"}");
        std::fs::create_dir_all(&p.cache).unwrap();
        std::fs::write(p.nostr_cache(), json!({"latest": [v]}).to_string()).unwrap();
        assert!(Cache::load(&p).latest(Kind::Metadata, &keys.public_key()).is_none());
    }
}
