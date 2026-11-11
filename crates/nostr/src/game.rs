//! How a game is named on Nostr, so the same game matches whether it came
//! from Steam, GOG, Epic, an emulator or a DRM-free folder.
//!
//! NIP-73 names external things with `i` tags (and their kind in `k`).
//! There is no game kind, so a game gets several names and two games match
//! when any name is shared:
//! * `#<title>`: NIP-73's hashtag form of the title, folded to lowercase
//!   letters and digits ("Hollow Knight™" → `#hollowknight`). Every game has
//!   it; it is what makes a Steam copy and a GOG copy meet.
//! * The Steam store page (`https://store.steampowered.com/app/<id>`) and
//!   the IGDB page (`https://www.igdb.com/games/<slug>`), as NIP-73 web URLs,
//!   when Heronite knows them: they tell apart games that share a title.
//!
//! A `t` tag with the folded title lets other Nostr apps find the event by
//! hashtag too.

use nostr::prelude::{Event, Tag};

/// What Heronite knows about a game, for naming it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GameRef {
    pub title: String,
    pub steam_app_id: Option<String>,
    pub igdb_slug: Option<String>,
}

impl GameRef {
    pub fn new(title: impl Into<String>) -> Self {
        Self { title: title.into(), ..Self::default() }
    }

    /// The title folded for the hashtag; empty for a title of only symbols.
    pub fn folded_title(&self) -> String {
        fold(&self.title)
    }

    /// Every name of the game, best first.
    pub fn ids(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Some(id) = self.steam_app_id.as_deref().map(str::trim).filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())) {
            out.push(format!("https://store.steampowered.com/app/{id}"));
        }
        if let Some(slug) = self.igdb_slug.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            out.push(format!("https://www.igdb.com/games/{slug}"));
        }
        let folded = self.folded_title();
        if !folded.is_empty() {
            out.push(format!("#{folded}"));
        }
        out
    }

    /// `i`, `k` and `t` tags for an event about the game.
    pub fn tags(&self) -> Vec<Tag> {
        let ids = self.ids();
        let mut tags: Vec<Tag> = ids.iter().map(|id| Tag::custom("i", [id.clone()])).collect();
        if ids.iter().any(|i| i.starts_with("http")) {
            tags.push(Tag::custom("k", ["web"]));
        }
        let folded = self.folded_title();
        if !folded.is_empty() {
            tags.push(Tag::custom("k", ["#"]));
            tags.push(Tag::hashtag(folded));
        }
        tags
    }
}

/// The `i` names on an event.
pub fn ids_of(event: &Event) -> Vec<String> {
    event
        .tags
        .iter()
        .filter_map(|t| match t.as_slice() {
            [k, v, ..] if k == "i" => Some(v.clone()),
            _ => None,
        })
        .collect()
}

/// Whether two lists of names share one.
pub fn same_game(a: &[String], b: &[String]) -> bool {
    a.iter().any(|x| b.contains(x))
}

/// Lowercase letters and digits only: "The Witcher 3: Wild Hunt" →
/// "thewitcher3wildhunt". Trademark signs and punctuation go; letters of
/// any script stay.
pub fn fold(title: &str) -> String {
    title.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::prelude::{EventBuilder, FinalizeEvent, Keys, Kind};

    #[test]
    fn titles_fold_to_one_hashtag() {
        assert_eq!(fold("Hollow Knight™"), "hollowknight");
        assert_eq!(fold("The Witcher 3: Wild Hunt"), "thewitcher3wildhunt");
        assert_eq!(fold("ÖKOSYSTEM"), "ökosystem");
        assert_eq!(fold("™ ®"), "");
    }

    #[test]
    fn a_steam_copy_and_a_gog_copy_share_a_name() {
        let steam = GameRef { title: "Hollow Knight".into(), steam_app_id: Some("367520".into()), igdb_slug: None };
        let gog = GameRef { title: "Hollow Knight™".into(), steam_app_id: None, igdb_slug: Some("hollow-knight".into()) };
        assert_eq!(steam.ids(), vec!["https://store.steampowered.com/app/367520", "#hollowknight"]);
        assert!(same_game(&steam.ids(), &gog.ids()));
        assert!(!same_game(&steam.ids(), &GameRef::new("Celeste").ids()));
    }

    #[test]
    fn tags_round_trip_through_an_event() {
        let game = GameRef { title: "Celeste".into(), steam_app_id: Some("504230".into()), igdb_slug: Some("celeste".into()) };
        let event = EventBuilder::new(Kind::TextNote, "").tags(game.tags()).finalize(&Keys::generate()).unwrap();
        assert_eq!(ids_of(&event), game.ids());
        assert!(event.tags.iter().any(|t| t.as_slice() == ["t", "celeste"]));
        assert!(event.tags.iter().any(|t| t.as_slice() == ["k", "web"]));
    }

    #[test]
    fn odd_ids_are_left_out() {
        let game = GameRef { title: "™".into(), steam_app_id: Some("abc".into()), igdb_slug: Some(" ".into()) };
        assert!(game.ids().is_empty());
        assert!(game.tags().is_empty());
    }
}
