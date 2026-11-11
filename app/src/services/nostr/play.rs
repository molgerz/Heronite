//! "Playing" while a game runs (NIP-38), when the user shares it: published
//! when the game starts, refreshed while it runs (each status expires on its
//! own, so a crash does not leave it behind), cleared when it ends.
//!
//! The running game is kept whether it is shared or not, so the Profile tab
//! can say what friends see, and so turning sharing on (or a game back in)
//! while it runs publishes it at once.

use gpui_kit::*;
use heronite_nostr::game::GameRef;
use heronite_nostr::status;

use super::Nostr;

#[derive(Default)]
pub struct State {
    running: Option<Running>,
    /// The Collection key of the game whose status is out.
    published: Option<String>,
    refresh: Option<Task<()>>,
}

struct Running {
    /// The Collection key.
    key: String,
    /// The launch name, as the launcher reports the game's end.
    app_name: String,
    game: GameRef,
}

/// What friends see of the user's play, for the Profile tab.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MyStatus {
    /// No game runs.
    Idle,
    /// Friends see this game.
    Shared(String),
    /// This game runs, and friends do not see it.
    Hidden(String, Hidden),
}

/// Why a running game is not shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hidden {
    /// "Share what I'm playing" is off.
    SharingOff,
    /// The game is left out in its Discussions dialog.
    LeftOut,
    /// Without a Steam app id or IGDB slug, friends' apps could not match it.
    Unknown,
    /// The community layer is off or has no identity.
    Offline,
}

/// A game started: kept, and published when the user shares it.
pub fn game_started(runner: &str, app_name: &str, title: &str, steam_app_id: Option<&str>, cx: &mut App) {
    let Some(service) = super::started(cx) else { return };
    let key = format!("{runner}_{app_name}");
    let fallback = GameRef { title: title.to_string(), steam_app_id: steam_app_id.map(str::to_string), igdb_slug: None };
    service.update(cx, |s, cx| {
        let game = s.games.get(&key).cloned().unwrap_or(fallback);
        s.play.running = Some(Running { key, app_name: app_name.to_string(), game });
        s.sync_playing(cx);
        cx.notify();
    });
}

/// A game ended (by its launch name, as the launcher reports it): clear
/// its status.
pub fn game_ended(app_name: &str, cx: &mut App) {
    let Some(service) = super::started(cx) else { return };
    service.update(cx, |s, cx| {
        if s.play.running.as_ref().is_some_and(|r| r.app_name == app_name) {
            s.play.running = None;
            s.sync_playing(cx);
            cx.notify();
        }
    });
}

impl Nostr {
    /// Why the running game is not shared; `None` when it is (or none runs).
    fn hidden_because(&self) -> Option<Hidden> {
        let running = self.play.running.as_ref()?;
        if !self.ready() {
            Some(Hidden::Offline)
        } else if !self.prefs.share_playing {
            Some(Hidden::SharingOff)
        } else if !self.prefs.may_share(&running.key) {
            Some(Hidden::LeftOut)
        } else if running.game.ids().is_empty() {
            Some(Hidden::Unknown)
        } else {
            None
        }
    }

    /// What friends see of the user's play now.
    pub fn my_status(&self) -> MyStatus {
        let Some(running) = &self.play.running else { return MyStatus::Idle };
        let title = running.game.title.clone();
        match self.hidden_because() {
            None => MyStatus::Shared(title),
            Some(why) => MyStatus::Hidden(title, why),
        }
    }

    /// Publishes the running game's status, or clears it, to match the
    /// running game and the sharing settings.
    pub(super) fn sync_playing(&mut self, cx: &mut Context<Self>) {
        let share = match (&self.play.running, self.hidden_because()) {
            (Some(running), None) => Some((running.key.clone(), running.game.clone())),
            _ => None,
        };
        let Some((key, game)) = share else {
            self.stop_playing(cx);
            return;
        };
        if self.play.published.as_deref() == Some(key.as_str()) {
            return;
        }
        self.play.published = Some(key);
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

    /// Clears the published status (the running game is kept).
    pub(super) fn stop_playing(&mut self, cx: &mut Context<Self>) {
        self.play.refresh = None;
        if self.play.published.take().is_some() && self.ready() {
            self.publish(status::cleared(), cx);
        }
    }

    /// Whether a game runs now, shared or not.
    pub fn in_game(&self) -> bool {
        self.play.running.is_some()
    }

    /// Never (or again) publish playing this game.
    pub fn set_never_share(&mut self, key: &str, never: bool, cx: &mut Context<Self>) {
        let key = key.to_string();
        self.set_prefs(
            |p| {
                p.never_share.retain(|k| *k != key);
                if never {
                    p.never_share.push(key.clone());
                }
            },
            cx,
        );
    }
}
