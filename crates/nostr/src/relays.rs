//! The user's relay lists, published so other apps find them:
//! * NIP-65 (kind 10002): where the user writes (`r` tags, `write` /
//!   `read` markers, none = both).
//! * NIP-17 (kind 10050): where the user wants private messages delivered.

use nostr::prelude::{Event, EventBuilder, Kind, Tag};

use crate::prefs::RelayPref;

/// The NIP-65 list for `relays`.
pub fn relay_list(relays: &[RelayPref]) -> EventBuilder {
    let tags = relays.iter().filter(|r| r.read || r.write).map(|r| match (r.read, r.write) {
        (true, true) => Tag::custom("r", [r.url.clone()]),
        (true, false) => Tag::custom("r", [r.url.clone(), "read".to_string()]),
        _ => Tag::custom("r", [r.url.clone(), "write".to_string()]),
    });
    EventBuilder::new(Kind::RelayList, "").tags(tags)
}

/// The relays of a NIP-65 event, each once, normalised.
pub fn parse_relay_list(event: &Event) -> Vec<RelayPref> {
    let mut out: Vec<RelayPref> = Vec::new();
    if event.kind != Kind::RelayList {
        return out;
    }
    for tag in event.tags.iter() {
        let s = tag.as_slice();
        if s.first().map(String::as_str) != Some("r") {
            continue;
        }
        let Some(url) = s.get(1).and_then(|u| crate::relay::normalize_url(u)) else { continue };
        let (read, write) = match s.get(2).map(String::as_str) {
            Some("read") => (true, false),
            Some("write") => (false, true),
            _ => (true, true),
        };
        match out.iter_mut().find(|r| r.url == url) {
            Some(r) => {
                r.read |= read;
                r.write |= write;
            }
            None => out.push(RelayPref { url, read, write }),
        }
    }
    out
}

/// The NIP-17 inbox list (kind 10050).
pub fn inbox_list(urls: &[String]) -> EventBuilder {
    EventBuilder::new(Kind::InboxRelays, "").tags(urls.iter().map(|u| Tag::custom("relay", [u.clone()])))
}

/// The inbox relays of a kind-10050 event.
pub fn parse_inbox_list(event: &Event) -> Vec<String> {
    if event.kind != Kind::InboxRelays {
        return Vec::new();
    }
    let mut out: Vec<String> = Vec::new();
    for tag in event.tags.iter() {
        if let [kind, url, ..] = tag.as_slice() {
            if kind == "relay" {
                if let Some(url) = crate::relay::normalize_url(url) {
                    if !out.contains(&url) {
                        out.push(url);
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::prelude::{FinalizeEvent, Keys};

    #[test]
    fn nip65_round_trips_the_markers() {
        let keys = Keys::generate();
        let relays = vec![
            RelayPref { url: "wss://a.example".into(), read: true, write: true },
            RelayPref { url: "wss://b.example".into(), read: true, write: false },
            RelayPref { url: "wss://c.example".into(), read: false, write: true },
            RelayPref { url: "wss://off.example".into(), read: false, write: false },
        ];
        let event = relay_list(&relays).finalize(&keys).unwrap();
        assert_eq!(parse_relay_list(&event), relays[..3].to_vec());
    }

    #[test]
    fn nip65_merges_duplicates_and_skips_bad_urls() {
        let keys = Keys::generate();
        let event = EventBuilder::new(Kind::RelayList, "")
            .tags([
                Tag::custom("r", ["wss://a.example/", "read"]),
                Tag::custom("r", ["wss://a.example", "write"]),
                Tag::custom("r", ["https://not.a.relay"]),
            ])
            .finalize(&keys)
            .unwrap();
        assert_eq!(parse_relay_list(&event), vec![RelayPref::new("wss://a.example")]);
    }

    #[test]
    fn inbox_relays_round_trip() {
        let keys = Keys::generate();
        let urls = vec!["wss://inbox.example".to_string(), "wss://nos.lol".to_string()];
        let event = inbox_list(&urls).finalize(&keys).unwrap();
        assert_eq!(parse_inbox_list(&event), urls);
    }
}
