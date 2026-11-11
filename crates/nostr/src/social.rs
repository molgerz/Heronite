//! The profile (NIP-01 kind 0), the follow list that is the Friends list
//! (NIP-02 kind 3) and NIP-05 addresses (`name@domain`).
//!
//! Both events are replaceable: the newest one of an author wins. Editing
//! keeps what Heronite does not show (a banner, a lightning address, a
//! petname, the old relay map in kind 3's content), so another Nostr app's
//! data survives.

use std::time::Duration;

use nostr::prelude::{Event, EventBuilder, Kind, PublicKey, Tag};
use serde_json::{json, Map, Value};

/// The fields Heronite shows and edits; `raw` holds them all.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Profile {
    pub name: String,
    pub display_name: String,
    pub about: String,
    /// An image URL.
    pub picture: String,
    /// `name@domain`, as the author claims it (check with [`verify_nip05`]).
    pub nip05: String,
    raw: Map<String, Value>,
}

impl Profile {
    /// From a kind-0 event; `None` for another kind or unreadable content.
    pub fn from_event(event: &Event) -> Option<Self> {
        if event.kind != Kind::Metadata {
            return None;
        }
        let raw: Map<String, Value> = serde_json::from_str(&event.content).ok()?;
        let text = |k: &str| raw.get(k).and_then(Value::as_str).unwrap_or_default().trim().to_string();
        Some(Self {
            name: text("name"),
            // `displayName` is an older spelling some apps still write.
            display_name: Some(text("display_name")).filter(|s| !s.is_empty()).unwrap_or_else(|| text("displayName")),
            about: text("about"),
            picture: text("picture"),
            nip05: text("nip05"),
            raw,
        })
    }

    /// What to call the person: the display name, the name, or a short npub.
    pub fn label(&self, key: &PublicKey) -> String {
        [&self.display_name, &self.name].into_iter().find(|s| !s.is_empty()).cloned().unwrap_or_else(|| crate::identity::short_npub(key))
    }

    /// The kind-0 event with these fields, keeping every other one.
    pub fn to_builder(&self) -> EventBuilder {
        let mut raw = self.raw.clone();
        for (k, v) in [
            ("name", &self.name),
            ("display_name", &self.display_name),
            ("about", &self.about),
            ("picture", &self.picture),
            ("nip05", &self.nip05),
        ] {
            if v.trim().is_empty() {
                raw.remove(k);
            } else {
                raw.insert(k.to_string(), json!(v.trim()));
            }
        }
        EventBuilder::new(Kind::Metadata, Value::Object(raw).to_string())
    }
}

/// The people a kind-3 event follows, in its order, each once.
pub fn follows(event: &Event) -> Vec<PublicKey> {
    let mut out: Vec<PublicKey> = Vec::new();
    for tag in event.tags.iter() {
        if let [kind, key, ..] = tag.as_slice() {
            if kind == "p" {
                if let Ok(pk) = PublicKey::from_hex(key) {
                    if !out.contains(&pk) {
                        out.push(pk);
                    }
                }
            }
        }
    }
    out
}

/// The follow list with `key` added (`follow`) or removed, built on the
/// newest kind-3 event (`current`, `None` when there is none yet), keeping
/// its other tags, relay hints, petnames and content.
pub fn change_follow(current: Option<&Event>, key: &PublicKey, follow: bool) -> EventBuilder {
    let hex = key.to_hex();
    let mut tags: Vec<Tag> = current.map(|e| e.tags.iter().cloned().collect()).unwrap_or_default();
    let has = |t: &Tag| matches!(t.as_slice(), [k, v, ..] if k == "p" && *v == hex);
    if follow {
        if !tags.iter().any(has) {
            tags.push(Tag::public_key(*key));
        }
    } else {
        tags.retain(|t| !has(t));
    }
    let content = current.map(|e| e.content.clone()).unwrap_or_default();
    EventBuilder::new(Kind::ContactList, content).tags(tags)
}

/// The newest event of `events` (by `created_at`, then id, as NIP-01 says
/// for replaceable events).
pub fn newest(events: impl IntoIterator<Item = Event>) -> Option<Event> {
    events.into_iter().max_by(|a, b| a.created_at.cmp(&b.created_at).then_with(|| b.id.cmp(&a.id)))
}

/// `name@domain` (or `domain`, meaning `_@domain`), lowercased; `None` when
/// it is not one.
pub fn parse_nip05(input: &str) -> Option<(String, String)> {
    let input = input.trim().to_ascii_lowercase();
    let (name, domain) = match input.split_once('@') {
        Some((n, d)) => (if n.is_empty() { "_".to_string() } else { n.to_string() }, d.to_string()),
        None => ("_".to_string(), input),
    };
    let name_ok = name.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
    let domain_ok = domain.contains('.') && domain.chars().all(|c| c.is_ascii_alphanumeric() || "-.:".contains(c));
    (name_ok && domain_ok && !domain.starts_with('.')).then_some((name, domain))
}

/// The public key a NIP-05 address names, read from
/// `https://<domain>/.well-known/nostr.json?name=<name>`.
pub fn resolve_nip05(address: &str) -> anyhow::Result<PublicKey> {
    let (name, domain) = parse_nip05(address).ok_or_else(|| anyhow::anyhow!("not a name@domain address"))?;
    let doc = well_known(&name, &domain)?;
    pick_nip05(&doc, &name).ok_or_else(|| anyhow::anyhow!("{domain} does not know {name}"))
}

/// Whether `address` names `key` (NIP-05 verification for a profile).
pub fn verify_nip05(address: &str, key: &PublicKey) -> bool {
    resolve_nip05(address).is_ok_and(|k| k == *key)
}

fn well_known(name: &str, domain: &str) -> anyhow::Result<Value> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        // NIP-05: "fetchers MUST ignore any HTTP redirects".
        .max_redirects(0)
        .user_agent(crate::USER_AGENT)
        .build()
        .new_agent();
    let mut resp = agent.get(format!("https://{domain}/.well-known/nostr.json")).query("name", name).call()?;
    Ok(serde_json::from_str(&resp.body_mut().with_config().limit(1 << 20).read_to_string()?)?)
}

fn pick_nip05(doc: &Value, name: &str) -> Option<PublicKey> {
    let hex = doc.get("names")?.get(name)?.as_str()?;
    PublicKey::from_hex(hex).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::prelude::{FinalizeEvent, Keys, Timestamp};

    fn signed(keys: &Keys, b: EventBuilder) -> Event {
        b.finalize(keys).unwrap()
    }

    #[test]
    fn editing_a_profile_keeps_the_fields_heronite_does_not_show() {
        let keys = Keys::generate();
        let first =
            signed(&keys, EventBuilder::new(Kind::Metadata, r#"{"name":"m","displayName":"Old","lud16":"m@ln.example","banner":"b.png"}"#));
        let mut p = Profile::from_event(&first).unwrap();
        assert_eq!(p.display_name, "Old");
        assert_eq!(p.label(&keys.public_key()), "Old");
        p.display_name = "New".into();
        p.about = "  ".into();
        let next = signed(&keys, p.to_builder());
        let raw: Value = serde_json::from_str(&next.content).unwrap();
        assert_eq!(raw["display_name"], "New");
        assert_eq!(raw["lud16"], "m@ln.example");
        assert_eq!(raw["banner"], "b.png");
        assert!(raw.get("about").is_none());
        assert!(Profile::from_event(&signed(&keys, EventBuilder::new(Kind::TextNote, "{}"))).is_none());
        assert!(Profile::from_event(&signed(&keys, EventBuilder::new(Kind::Metadata, "nope"))).is_none());
    }

    #[test]
    fn a_profile_without_names_is_called_by_its_npub() {
        let key = Keys::generate().public_key();
        assert!(Profile::default().label(&key).starts_with("npub1"));
    }

    #[test]
    fn following_keeps_other_tags_and_content() {
        let keys = Keys::generate();
        let (a, b) = (Keys::generate().public_key(), Keys::generate().public_key());
        let petname = Tag::parse(["p", &a.to_hex(), "wss://r.example", "alice"]).unwrap();
        let first =
            signed(&keys, EventBuilder::new(Kind::ContactList, "{\"wss://r.example\":{}}").tags([petname.clone(), Tag::hashtag("games")]));
        assert_eq!(follows(&first), vec![a]);
        let added = signed(&keys, change_follow(Some(&first), &b, true));
        assert_eq!(follows(&added), vec![a, b]);
        assert!(added.tags.contains(&petname));
        assert_eq!(added.content, first.content);
        // Following twice adds nothing.
        assert_eq!(follows(&signed(&keys, change_follow(Some(&added), &b, true))), vec![a, b]);
        let removed = signed(&keys, change_follow(Some(&added), &a, false));
        assert_eq!(follows(&removed), vec![b]);
        assert!(removed.tags.iter().any(|t| t.kind() == "t"));
        assert_eq!(follows(&signed(&keys, change_follow(None, &a, true))), vec![a]);
    }

    #[test]
    fn the_newest_replaceable_event_wins() {
        let keys = Keys::generate();
        let at = |s: u64| signed(&keys, EventBuilder::new(Kind::Metadata, "{}").custom_created_at(Timestamp::from_secs(s)));
        let (old, new) = (at(100), at(200));
        assert_eq!(newest([old, new.clone()]).unwrap().id, new.id);
        assert!(newest(Vec::new()).is_none());
    }

    #[test]
    fn nip05_addresses() {
        assert_eq!(parse_nip05("Bob@Example.com"), Some(("bob".into(), "example.com".into())));
        assert_eq!(parse_nip05("example.com"), Some(("_".into(), "example.com".into())));
        assert_eq!(parse_nip05("@example.com"), Some(("_".into(), "example.com".into())));
        assert_eq!(parse_nip05("bob@localhost"), None);
        assert_eq!(parse_nip05("b o b@example.com"), None);
        assert_eq!(parse_nip05("npub1xyz"), None);
        let key = Keys::generate().public_key();
        let doc = json!({"names": {"bob": key.to_hex()}});
        assert_eq!(pick_nip05(&doc, "bob"), Some(key));
        assert_eq!(pick_nip05(&doc, "alice"), None);
    }
}
