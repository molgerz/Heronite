//! Private messages, NIP-17: the message (an unsigned kind-14 "rumor") is
//! sealed for each recipient with NIP-44 (kind 13, signed by the sender)
//! and the seal is gift-wrapped (kind 1059) by a throwaway key, with a
//! timestamp moved back at random (NIP-59). Relays see neither the text nor
//! who writes to whom; only the recipient's key opens it. The sender keeps a
//! copy wrapped for themselves, so their other apps show it too.
//!
//! A chat is the set of everyone in a message (sender and `p` tags): two
//! people, or a small group.

use nostr::nips::nip59::{extract_rumor, GiftWrapBuilder};
use nostr::prelude::{Event, EventBuilder, EventId, Filter, FinalizeEvent, FinalizeUnsignedEvent, Kind, PublicKey, Tag, Timestamp};

use crate::signer::Signer;

/// Groups bigger than this are refused: every message is wrapped once per
/// person, and NIP-17 means it for small groups.
pub const MAX_GROUP: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// The rumor's id, the same in every copy.
    pub id: EventId,
    pub from: PublicKey,
    /// Everyone else in the chat.
    pub to: Vec<PublicKey>,
    pub text: String,
    /// NIP-17's optional `subject` (a group chat's name).
    pub subject: Option<String>,
    pub at: Timestamp,
}

impl Message {
    /// Everyone in the chat, sorted: the chat's key.
    pub fn chat(&self) -> Vec<PublicKey> {
        let mut all = self.to.clone();
        all.push(self.from);
        all.sort();
        all.dedup();
        all
    }
}

/// A message on its way: what it says, and the gift wraps that carry it.
#[derive(Debug, Clone)]
pub struct Outgoing {
    pub message: Message,
    /// One per recipient and one for the sender, as `(who it is for, wrap)`.
    pub wraps: Vec<(PublicKey, Event)>,
}

/// Wraps `text` for `to` and for the sender. Signing goes through `signer`
/// (the phone, for a remote signer: one approval per seal).
pub fn wrap(signer: &Signer, to: &[PublicKey], text: &str, subject: Option<&str>) -> anyhow::Result<Outgoing> {
    let me = signer.public_key();
    let mut people: Vec<PublicKey> = to.iter().copied().filter(|p| *p != me).collect();
    people.sort();
    people.dedup();
    anyhow::ensure!(!people.is_empty(), "nobody to write to");
    anyhow::ensure!(people.len() <= MAX_GROUP, "a chat has at most {MAX_GROUP} people");
    let subject = subject.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    let mut rumor = EventBuilder::new(Kind::PrivateDirectMessage, text)
        .tags(people.iter().map(|p| Tag::public_key(*p)))
        .tag_maybe(subject.clone().map(|s| Tag::custom("subject", [s])))
        .finalize_unsigned(me);
    let message = Message { id: rumor.id(), from: me, to: people.clone(), text: text.to_string(), subject, at: rumor.created_at };
    let mut out = Vec::with_capacity(people.len() + 1);
    for receiver in people.iter().copied().chain(std::iter::once(me)) {
        let wrap = GiftWrapBuilder::new(receiver, rumor.clone()).finalize(signer)?;
        out.push((receiver, wrap));
    }
    Ok(Outgoing { message, wraps: out })
}

/// Opens a gift wrap addressed to the user. `None` when it is not a
/// private message, does not open with the user's key, or its rumor
/// claims another author than the seal (a forgery).
pub fn open(signer: &Signer, wrap: &Event) -> Option<Message> {
    if wrap.kind != Kind::GiftWrap {
        return None;
    }
    let gift = extract_rumor(signer, wrap).ok()?;
    let mut rumor = gift.rumor;
    if rumor.pubkey != gift.sender || rumor.kind != Kind::PrivateDirectMessage {
        return None;
    }
    let to = rumor
        .tags
        .iter()
        .filter_map(|t| match t.as_slice() {
            [k, v, ..] if k == "p" => PublicKey::from_hex(v).ok(),
            _ => None,
        })
        .filter(|p| *p != gift.sender)
        .collect();
    let subject = rumor.tags.iter().find_map(|t| match t.as_slice() {
        [k, v, ..] if k == "subject" => Some(v.clone()),
        _ => None,
    });
    Some(Message { id: rumor.id(), from: gift.sender, to, text: rumor.content.clone(), subject, at: rumor.created_at })
}

/// The gift wraps addressed to `me`. No `since`: a wrap's timestamp is
/// moved back by up to two days, so a window would miss some.
pub fn inbox_filter(me: &PublicKey, limit: usize) -> Filter {
    Filter::new().kind(Kind::GiftWrap).pubkey(*me).limit(limit)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::prelude::Keys;

    #[test]
    fn a_message_opens_for_its_people_only() {
        let (alice, bob, eve) = (Keys::generate(), Keys::generate(), Keys::generate());
        let a = Signer::Local(alice.clone());
        let sent = wrap(&a, &[bob.public_key()], "Want to play Celeste?", None).unwrap();
        let wraps = &sent.wraps;
        assert_eq!(wraps.len(), 2, "one for Bob, one for Alice's other apps");
        for (_, w) in wraps {
            assert_eq!(w.kind, Kind::GiftWrap);
            assert_ne!(w.pubkey, alice.public_key(), "wrapped by a throwaway key");
            assert!(!w.content.contains("Celeste"));
        }
        let wrap_bob = &wraps.iter().find(|(r, _)| *r == bob.public_key()).unwrap().1;
        let msg = open(&Signer::Local(bob.clone()), wrap_bob).unwrap();
        assert_eq!(msg.text, "Want to play Celeste?");
        assert_eq!(msg.from, alice.public_key());
        assert_eq!(msg.to, vec![bob.public_key()]);
        assert!(open(&Signer::Local(eve), wrap_bob).is_none());
        // Alice's own copy is the same message.
        let mine = open(&a, &wraps.iter().find(|(r, _)| *r == alice.public_key()).unwrap().1).unwrap();
        assert_eq!(mine.id, msg.id);
        assert_eq!(sent.message, msg, "what was sent is what arrives");
        assert_eq!(mine.chat(), msg.chat());
    }

    #[test]
    fn a_group_chat_has_a_subject_and_one_key() {
        let people: Vec<Keys> = (0..3).map(|_| Keys::generate()).collect();
        let me = Signer::Local(people[0].clone());
        let to: Vec<PublicKey> = people[1..].iter().map(Keys::public_key).collect();
        let wraps = wrap(&me, &to, "gg", Some("Friday raid")).unwrap().wraps;
        assert_eq!(wraps.len(), 3);
        let for_ = |k: &Keys| &wraps.iter().find(|(r, _)| *r == k.public_key()).unwrap().1;
        let a = open(&Signer::Local(people[1].clone()), for_(&people[1])).unwrap();
        let b = open(&Signer::Local(people[2].clone()), for_(&people[2])).unwrap();
        assert_eq!(a.subject.as_deref(), Some("Friday raid"));
        assert_eq!(a.chat(), b.chat());
        assert_eq!(a.chat().len(), 3);
    }

    #[test]
    fn nobody_or_only_myself_is_refused() {
        let me = Keys::generate();
        let s = Signer::Local(me.clone());
        assert!(wrap(&s, &[], "x", None).is_err());
        assert!(wrap(&s, &[me.public_key()], "x", None).is_err());
        let crowd: Vec<PublicKey> = (0..=MAX_GROUP).map(|_| Keys::generate().public_key()).collect();
        assert!(wrap(&s, &crowd, "x", None).is_err());
    }

    #[test]
    fn other_kinds_are_not_messages() {
        let keys = Keys::generate();
        let note = EventBuilder::new(Kind::TextNote, "hi").finalize(&keys).unwrap();
        assert!(open(&Signer::Local(keys), &note).is_none());
    }
}
