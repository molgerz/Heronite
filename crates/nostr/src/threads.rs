//! Discussions around a game: threads (NIP-7D, kind 11, with a `title`)
//! named by the game's `i` tags ([`crate::game`]), and comments on them
//! (NIP-22, kind 1111, with the thread as the upper-case root and the
//! comment answered as the lower-case parent). Other Nostr apps that read
//! NIP-7D and NIP-22 show the same discussion.
//!
//! Muting (NIP-51 mute list, kind 10000) hides people and threads in
//! Heronite and in every app that reads the list; reporting (NIP-56, kind
//! 1984) tells relays and moderators.

use nostr::prelude::{Event, EventBuilder, EventId, Filter, Kind, PublicKey, SingleLetterTag, Tag, Timestamp};

use crate::game::{ids_of, GameRef};

/// NIP-7D thread.
pub const THREAD: Kind = Kind::Custom(11);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    pub id: EventId,
    pub author: PublicKey,
    pub title: String,
    pub body: String,
    pub at: Timestamp,
    pub game_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub id: EventId,
    pub author: PublicKey,
    pub body: String,
    pub at: Timestamp,
    /// The thread it belongs to.
    pub root: EventId,
    /// What it answers: the thread or another comment.
    pub parent: EventId,
}

/// A new thread about `game`.
pub fn new_thread(game: &GameRef, title: &str, body: &str) -> EventBuilder {
    EventBuilder::new(THREAD, body.trim()).tag(Tag::custom("title", [title.trim().to_string()])).tags(game.tags())
}

/// A comment on `thread`, answering `parent` (the thread itself or a
/// comment in it).
pub fn new_comment(thread: &Thread, parent: Option<&Comment>, body: &str) -> EventBuilder {
    let (pid, pkind, pauthor) = match parent {
        Some(c) => (c.id, "1111", c.author),
        None => (thread.id, "11", thread.author),
    };
    EventBuilder::new(Kind::Comment, body.trim()).tags([
        Tag::custom("E", [thread.id.to_hex(), String::new(), thread.author.to_hex()]),
        Tag::custom("K", ["11"]),
        Tag::custom("P", [thread.author.to_hex()]),
        Tag::custom("e", [pid.to_hex(), String::new(), pauthor.to_hex()]),
        Tag::custom("k", [pkind]),
        Tag::custom("p", [pauthor.to_hex()]),
    ])
}

/// The threads about `game` (any of its names).
pub fn threads_filter(game: &GameRef, limit: usize) -> Filter {
    Filter::new().kind(THREAD).custom_tags(letter('i'), game.ids()).limit(limit)
}

/// The comments of a thread.
pub fn comments_filter(thread: &EventId) -> Filter {
    Filter::new().kind(Kind::Comment).custom_tag(letter('E'), thread.to_hex())
}

fn letter(c: char) -> SingleLetterTag {
    SingleLetterTag::from_char(c).expect("a single ASCII letter")
}

fn tag_value<'a>(event: &'a Event, name: &str) -> Option<&'a str> {
    event.tags.iter().find_map(|t| match t.as_slice() {
        [k, v, ..] if k == name => Some(v.as_str()),
        _ => None,
    })
}

pub fn read_thread(event: &Event) -> Option<Thread> {
    if event.kind != THREAD {
        return None;
    }
    let title = tag_value(event, "title").unwrap_or_default().trim().to_string();
    if title.is_empty() && event.content.trim().is_empty() {
        return None;
    }
    Some(Thread {
        id: event.id,
        author: event.pubkey,
        title,
        body: event.content.trim().to_string(),
        at: event.created_at,
        game_ids: ids_of(event),
    })
}

pub fn read_comment(event: &Event) -> Option<Comment> {
    if event.kind != Kind::Comment {
        return None;
    }
    let root = EventId::from_hex(tag_value(event, "E")?).ok()?;
    let parent = tag_value(event, "e").and_then(|v| EventId::from_hex(v).ok()).unwrap_or(root);
    Some(Comment { id: event.id, author: event.pubkey, body: event.content.trim().to_string(), at: event.created_at, root, parent })
}

/// Who and what the user muted (the public part of a NIP-51 mute list).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mutes {
    pub people: Vec<PublicKey>,
    pub events: Vec<EventId>,
}

impl Mutes {
    pub fn from_event(event: &Event) -> Self {
        let mut m = Self::default();
        if event.kind != Kind::MuteList {
            return m;
        }
        for t in event.tags.iter() {
            match t.as_slice() {
                [k, v, ..] if k == "p" => m.people.extend(PublicKey::from_hex(v).ok()),
                [k, v, ..] if k == "e" => m.events.extend(EventId::from_hex(v).ok()),
                _ => {}
            }
        }
        m
    }

    pub fn hides(&self, author: &PublicKey, id: &EventId) -> bool {
        self.people.contains(author) || self.events.contains(id)
    }
}

/// What can be muted or reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Person(PublicKey),
    Post(EventId, PublicKey),
}

/// The mute list with `target` added or removed, built on the newest list
/// (`current`), keeping its other tags (words, hashtags) and its encrypted
/// private part in `content`.
pub fn change_mute(current: Option<&Event>, target: Target, mute: bool) -> EventBuilder {
    let (kind, value) = match target {
        Target::Person(pk) => ("p", pk.to_hex()),
        Target::Post(id, _) => ("e", id.to_hex()),
    };
    let mut tags: Vec<Tag> = current.map(|e| e.tags.iter().cloned().collect()).unwrap_or_default();
    let same = |t: &Tag| matches!(t.as_slice(), [k, v, ..] if k == kind && *v == value);
    if mute {
        if !tags.iter().any(same) {
            tags.push(Tag::custom(kind, [value.clone()]));
        }
    } else {
        tags.retain(|t| !same(t));
    }
    EventBuilder::new(Kind::MuteList, current.map(|e| e.content.clone()).unwrap_or_default()).tags(tags)
}

/// Why something is reported (NIP-56's types).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Spam,
    Illegal,
    Nudity,
    Profanity,
    Impersonation,
    Other,
}

impl Reason {
    pub const ALL: [Reason; 6] = [Reason::Spam, Reason::Illegal, Reason::Nudity, Reason::Profanity, Reason::Impersonation, Reason::Other];

    pub fn as_str(self) -> &'static str {
        match self {
            Reason::Spam => "spam",
            Reason::Illegal => "illegal",
            Reason::Nudity => "nudity",
            Reason::Profanity => "profanity",
            Reason::Impersonation => "impersonation",
            Reason::Other => "other",
        }
    }
}

/// A NIP-56 report of `target`, with the user's words in `note`.
pub fn report(target: Target, reason: Reason, note: &str) -> EventBuilder {
    let r = reason.as_str();
    let tags = match target {
        Target::Person(pk) => vec![Tag::custom("p", [pk.to_hex(), r.to_string()])],
        Target::Post(id, author) => vec![Tag::custom("e", [id.to_hex(), r.to_string()]), Tag::custom("p", [author.to_hex()])],
    };
    EventBuilder::new(Kind::Reporting, note.trim()).tags(tags)
}

#[cfg(test)]
mod tests {
    use nostr::prelude::filter::MatchEventOptions;
    use nostr::prelude::{FinalizeEvent, Keys};

    use super::*;

    #[test]
    fn a_thread_and_its_comments_link_up() {
        let (alice, bob) = (Keys::generate(), Keys::generate());
        let game = GameRef { title: "Celeste".into(), steam_app_id: Some("504230".into()), igdb_slug: None };
        let event = new_thread(&game, " Best route? ", "Any tips").finalize(&alice).unwrap();
        let thread = read_thread(&event).unwrap();
        assert_eq!(thread.title, "Best route?");
        assert_eq!(thread.game_ids, game.ids());
        // A GOG copy of the same game finds the thread by its title name.
        let gog = GameRef::new("Celeste™");
        assert!(threads_filter(&gog, 50).match_event(&event, MatchEventOptions::new()));
        assert!(!threads_filter(&GameRef::new("Hades"), 50).match_event(&event, MatchEventOptions::new()));

        let first = read_comment(&new_comment(&thread, None, "Dash early").finalize(&bob).unwrap()).unwrap();
        assert_eq!((first.root, first.parent), (thread.id, thread.id));
        let answer_event = new_comment(&thread, Some(&first), "Thanks").finalize(&alice).unwrap();
        assert!(comments_filter(&thread.id).match_event(&answer_event, MatchEventOptions::new()));
        let answer = read_comment(&answer_event).unwrap();
        assert_eq!((answer.root, answer.parent), (thread.id, first.id));
        assert!(answer_event.tags.iter().any(|t| t.as_slice() == ["p", bob.public_key().to_hex().as_str()]));
    }

    #[test]
    fn empty_threads_and_other_kinds_are_skipped() {
        let keys = Keys::generate();
        assert!(read_thread(&EventBuilder::new(THREAD, " ").finalize(&keys).unwrap()).is_none());
        assert!(read_comment(&EventBuilder::new(Kind::Comment, "no root").finalize(&keys).unwrap()).is_none());
    }

    #[test]
    fn muting_keeps_the_rest_of_the_list() {
        let keys = Keys::generate();
        let spammer = Keys::generate().public_key();
        let first =
            EventBuilder::new(Kind::MuteList, "encrypted-private-part").tag(Tag::custom("word", ["spoiler"])).finalize(&keys).unwrap();
        let muted = change_mute(Some(&first), Target::Person(spammer), true).finalize(&keys).unwrap();
        let m = Mutes::from_event(&muted);
        assert_eq!(m.people, vec![spammer]);
        assert!(m.hides(&spammer, &first.id));
        assert_eq!(muted.content, "encrypted-private-part");
        assert!(muted.tags.iter().any(|t| t.kind() == "word"));
        let again = change_mute(Some(&muted), Target::Person(spammer), true).finalize(&keys).unwrap();
        assert_eq!(Mutes::from_event(&again).people.len(), 1);
        let unmuted = change_mute(Some(&again), Target::Person(spammer), false).finalize(&keys).unwrap();
        assert!(Mutes::from_event(&unmuted).people.is_empty());
    }

    #[test]
    fn reports_name_the_reason() {
        let keys = Keys::generate();
        let (id, author) = (EventId::from_hex(&"11".repeat(32)).unwrap(), Keys::generate().public_key());
        let r = report(Target::Post(id, author), Reason::Spam, " ads ").finalize(&keys).unwrap();
        assert_eq!(r.kind, Kind::Reporting);
        assert_eq!(r.content, "ads");
        assert!(r.tags.iter().any(|t| t.as_slice() == ["e", id.to_hex().as_str(), "spam"]));
    }
}
