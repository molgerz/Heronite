//! What the user is playing, as a NIP-38 user status (kind 30315): a
//! replaceable event per status type, here the `d` type `gaming`, with
//! the game's names ([`crate::game`]) and an expiration (NIP-40).
//!
//! The status is published when a game starts and refreshed while it runs,
//! each time expiring [`TTL`] later, so a crash or a computer switched off
//! does not leave "playing" behind for long. Closing the game publishes an
//! empty status, which NIP-38 reads as "cleared".

use std::time::Duration;

use nostr::prelude::{Event, EventBuilder, Kind, PublicKey, Tag, Timestamp};

use crate::game::{ids_of, GameRef};

/// The `d` type of Heronite's status.
pub const TYPE: &str = "gaming";
/// How long a published status lives without a refresh.
pub const TTL: Duration = Duration::from_secs(15 * 60);
/// How often a running game's status is published again.
pub const REFRESH: Duration = Duration::from_secs(10 * 60);

/// "Playing <title>", expiring [`TTL`] from now.
pub fn playing(game: &GameRef) -> EventBuilder {
    playing_at(game, Timestamp::now())
}

fn playing_at(game: &GameRef, now: Timestamp) -> EventBuilder {
    EventBuilder::new(Kind::UserStatus, game.title.trim())
        .tag(Tag::identifier(TYPE))
        .tags(game.tags())
        .tag(Tag::expiration(Timestamp::from_secs(now.as_secs() + TTL.as_secs())))
        .custom_created_at(now)
}

/// The empty status that clears "playing".
pub fn cleared() -> EventBuilder {
    EventBuilder::new(Kind::UserStatus, "").tag(Tag::identifier(TYPE))
}

/// A friend's current game.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Playing {
    pub who: PublicKey,
    pub title: String,
    /// The game's names, to match it with the user's own games.
    pub ids: Vec<String>,
    /// When the status was published.
    pub since: Timestamp,
}

/// What a kind-30315 event says, if it is a live `gaming` status: `None`
/// for another type, a cleared or an expired one (at `now`).
pub fn read(event: &Event, now: Timestamp) -> Option<Playing> {
    if event.kind != Kind::UserStatus || event.content.trim().is_empty() || event.is_expired_at(now) {
        return None;
    }
    let d = event.tags.iter().find_map(|t| match t.as_slice() {
        [k, v, ..] if k == "d" => Some(v.as_str()),
        _ => None,
    });
    (d == Some(TYPE)).then(|| Playing {
        who: event.pubkey,
        title: event.content.trim().to_string(),
        ids: ids_of(event),
        since: event.created_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::prelude::{FinalizeEvent, Keys};

    #[test]
    fn a_status_lives_until_cleared_or_expired() {
        let keys = Keys::generate();
        let game = GameRef { title: "Celeste".into(), steam_app_id: Some("504230".into()), igdb_slug: None };
        let now = Timestamp::now();
        let event = playing_at(&game, now).finalize(&keys).unwrap();
        let p = read(&event, now).unwrap();
        assert_eq!(p.title, "Celeste");
        assert_eq!(p.who, keys.public_key());
        assert_eq!(p.ids, game.ids());
        let later = Timestamp::from_secs(now.as_secs() + TTL.as_secs() + 1);
        assert!(read(&event, later).is_none(), "expired");
        let clear = cleared().finalize(&keys).unwrap();
        assert!(read(&clear, now).is_none());
    }

    #[test]
    fn other_status_types_are_not_games() {
        let keys = Keys::generate();
        let music = EventBuilder::new(Kind::UserStatus, "A song").tag(Tag::identifier("music")).finalize(&keys).unwrap();
        assert!(read(&music, Timestamp::now()).is_none());
    }
}
