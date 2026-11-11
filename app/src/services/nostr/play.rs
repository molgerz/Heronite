//! "Playing" while a game runs (NIP-38), when the user shares it: published
//! when the game starts, refreshed while it runs (each status expires on its
//! own, so a crash does not leave it behind), cleared when it ends.

use gpui_kit::*;
use heronite_nostr::game::GameRef;
use heronite_nostr::status;

use super::Nostr;

#[derive(Default)]
pub struct State {
    /// The Collection key and launch name of the game whose status is out.
    game: Option<(String, String)>,
    refresh: Option<Task<()>>,
}

/// A game started: publish it when the user shares what they play.
pub fn game_started(runner: &str, app_name: &str, title: &str, steam_app_id: Option<&str>, cx: &mut App) {
    let Some(service) = super::started(cx) else { return };
    let key = format!("{runner}_{app_name}");
    let fallback = GameRef { title: title.to_string(), steam_app_id: steam_app_id.map(str::to_string), igdb_slug: None };
    service.update(cx, |s, cx| {
        let game = s.games.get(&key).cloned().unwrap_or(fallback);
        s.start_playing(key, app_name.to_string(), game, cx);
    });
}

/// A game ended (by its launch name, as the launcher reports it): clear
/// its status.
pub fn game_ended(app_name: &str, cx: &mut App) {
    let Some(service) = super::started(cx) else { return };
    service.update(cx, |s, cx| {
        if s.play.game.as_ref().is_some_and(|(_, a)| a == app_name) {
            s.stop_playing(cx);
        }
    });
}

impl Nostr {
    fn start_playing(&mut self, key: String, app_name: String, game: GameRef, cx: &mut Context<Self>) {
        if !self.ready() || !self.prefs.may_share(&key) || game.ids().is_empty() {
            return;
        }
        self.play.game = Some((key, app_name));
        self.publish(status::playing(&game), cx);
        self.play.refresh = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(status::REFRESH).await;
            let alive = this
                .update(cx, |s, cx| {
                    s.expire_statuses();
                    s.publish(status::playing(&game), cx);
                })
                .is_ok();
            if !alive {
                break;
            }
        }));
    }

    pub(super) fn stop_playing(&mut self, cx: &mut Context<Self>) {
        self.play.refresh = None;
        if self.play.game.take().is_some() && self.ready() {
            self.publish(status::cleared(), cx);
        }
    }

    /// The game whose status is out now (its Collection key).
    pub fn sharing_game(&self) -> Option<&str> {
        self.play.game.as_ref().map(|(k, _)| k.as_str())
    }

    /// Never (or again) publish playing this game.
    pub fn set_never_share(&mut self, key: &str, never: bool, cx: &mut Context<Self>) {
        let key = key.to_string();
        let playing_it = self.sharing_game() == Some(key.as_str());
        self.set_prefs(
            |p| {
                p.never_share.retain(|k| *k != key);
                if never {
                    p.never_share.push(key.clone());
                }
            },
            cx,
        );
        if never && playing_it {
            self.stop_playing(cx);
        }
    }
}
