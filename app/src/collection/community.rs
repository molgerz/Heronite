//! Where the Collection reaches the community layer (`services::nostr`):
//! a game's "Discussions" and the viewer's "Share" of a capture. Both show
//! only while the layer is on and has an identity.

use gpui_kit::*;

use super::CollectionView;

/// The community layer is on and signs.
pub(super) fn ready(cx: &App) -> bool {
    crate::services::nostr::started(cx).is_some_and(|s| s.read(cx).ready())
}

impl CollectionView {
    pub(super) fn open_game_community(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(game) = self.lib.as_ref().and_then(|l| l.get(key)).cloned() else { return };
        let view = cx.new(|cx| crate::dialogs::game_community::GameCommunityDialog::new(game.key.clone(), game.title.clone(), window, cx));
        self.open_dialog(view, cx);
    }

    /// Shares the viewer's capture `i` (a screenshot or a replay).
    pub(super) fn open_share_capture(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.shots.items.get(i).map(|s| s.path.clone()) else { return };
        let Some(key) = self.shots.game.clone() else { return };
        let title = self.lib.as_ref().and_then(|l| l.get(&key)).map(|g| g.title.clone()).unwrap_or_default();
        let view = cx.new(|cx| crate::dialogs::share_capture::ShareCaptureDialog::new(path, &key, &title, window, cx));
        self.open_dialog(view, cx);
    }
}
