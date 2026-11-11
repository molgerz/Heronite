//! "Share" of a screenshot or a replay (the viewer's share button): a
//! caption, and who sees it, everyone (a public picture or video post) or
//! one friend (a private message). Never automatic. The upload is
//! `services::nostr`'s, after the file's metadata is removed.

use std::path::PathBuf;

use gpui_kit::component::input::InputState;
use gpui_kit::prelude::*;
use gpui_kit::*;
use heronite_i18n::tr;
use heronite_nostr::game::GameRef;
use heronite_nostr::PublicKey;

use super::DialogEvent;
use crate::services::nostr::{self as service, Nostr, ShareKind, ShareState};
use crate::theme;
use crate::ui::controls::{button, button_sm, caption, help_text, ButtonKind};
use crate::ui::dialog::{dialog, DialogProps};
use crate::ui::fields::{input_box, labeled, new_input};

pub struct ShareCaptureDialog {
    svc: Entity<Nostr>,
    path: PathBuf,
    kind: ShareKind,
    game: GameRef,
    caption: Entity<InputState>,
    /// `None`: everyone.
    to: Option<PublicKey>,
    console_list: crate::console::list::ListNav,
    _sub: Subscription,
}

impl EventEmitter<DialogEvent> for ShareCaptureDialog {}

impl ShareCaptureDialog {
    pub fn new(path: PathBuf, game_key: &str, title: &str, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let svc = service::get(cx);
        let sub = cx.observe(&svc, |_, _, cx| cx.notify());
        svc.update(cx, |s, cx| s.reset_share(cx));
        let game = svc.read(cx).game(game_key).cloned().unwrap_or_else(|| GameRef::new(title));
        let kind = if heronite_media::is_video(&path) { ShareKind::Replay } else { ShareKind::Screenshot };
        Self {
            svc,
            path,
            kind,
            game,
            caption: new_input(window, cx, "", tr!("Say something about it (optional)")),
            to: None,
            console_list: Default::default(),
            _sub: sub,
        }
    }

    fn share(&mut self, cx: &mut Context<Self>) {
        let text = self.caption.read(cx).value().trim().to_string();
        let (path, kind, game, to) = (self.path.clone(), self.kind, self.game.clone(), self.to);
        self.svc.update(cx, |s, cx| s.share(path, kind, game, text, to, cx));
    }

    fn console_rows(&self, cx: &App) -> Vec<crate::console::list::Row> {
        use crate::console::list::Row;
        let busy = *self.svc.read(cx).share_state() == ShareState::Uploading;
        vec![
            Row::action("share", if busy { tr!("Uploading…") } else { tr!("Share with everyone") }).enabled(!busy),
            Row::action("close", tr!("Close")),
        ]
    }
}

impl Render for ShareCaptureDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let heading = match self.kind {
            ShareKind::Screenshot => tr!("Share screenshot"),
            ShareKind::Replay => tr!("Share replay"),
        };
        if crate::console::active() {
            let rows = self.console_rows(cx);
            return crate::console::list::simple_dialog(&mut self.console_list, "console-share", heading, &self.game.title, None, &rows);
        }
        let (state, ready, friends) = {
            let svc = self.svc.read(cx);
            let friends: Vec<(PublicKey, String)> = svc.friends().iter().map(|f| (*f, svc.label(f))).collect();
            (svc.share_state().clone(), svc.ready(), friends)
        };
        let busy = state == ShareState::Uploading;
        let who = div()
            .flex()
            .flex_wrap()
            .gap(px(8.))
            .child(
                button_sm("share-everyone", if self.to.is_none() { ButtonKind::Primary } else { ButtonKind::Glass }, tr!("Everyone"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.to = None;
                        cx.notify();
                    })),
            )
            .children(friends.into_iter().enumerate().map(|(i, (key, name))| {
                button_sm(("share-friend", i), if self.to == Some(key) { ButtonKind::Primary } else { ButtonKind::Glass }, name).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.to = Some(key);
                        cx.notify();
                    }),
                )
            }));
        let status: Option<(String, bool)> = match &state {
            ShareState::Idle => None,
            ShareState::Uploading => Some((tr!("Uploading…").into(), false)),
            ShareState::Done(url) => Some((tr!("Shared: {url}", url = url.as_str()), false)),
            ShareState::Failed(e) => Some((e.clone(), true)),
        };
        let body = if !ready {
            div().child(help_text(tr!("Turn on the community layer in Settings → Community to share.")))
        } else {
            div()
                .flex()
                .flex_col()
                .gap(px(14.))
                .child(div().text_size(px(13.)).text_color(theme::text_muted()).child(self.game.title.clone()))
                .child(labeled(tr!("Caption"), input_box(&self.caption, busy)))
                .child(caption(tr!("Who sees it")))
                .child(who)
                .child(help_text(if self.to.is_none() {
                    tr!("A public post: anyone on Nostr can see it, with the game's name.")
                } else {
                    tr!("Sent as a private, encrypted message. The file itself is on the media server, reachable by its link.")
                }))
                .children(status.map(|(text, error)| {
                    div().text_size(px(12.5)).text_color(if error { theme::danger_text() } else { theme::text_muted() }).child(text)
                }))
        };
        let done = matches!(state, ShareState::Done(_));
        let footer = div()
            .flex()
            .gap(px(10.))
            .child(
                button("share-close", ButtonKind::Ghost, if done { tr!("Close") } else { tr!("Cancel") })
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(DialogEvent::Close))),
            )
            .when(ready && !done, |f| {
                f.child(
                    button("share-go", ButtonKind::Primary, if busy { tr!("Uploading…") } else { tr!("Share") })
                        .when(!busy, |b| b.on_click(cx.listener(|this, _, _, cx| this.share(cx)))),
                )
            });
        let entity = cx.entity().downgrade();
        dialog(
            DialogProps { id: "share-capture".into(), title: heading.into(), width: 520., height: None },
            body.into_any_element(),
            Some(footer.into_any_element()),
            move |_, cx| {
                entity.update(cx, |_, cx| cx.emit(DialogEvent::Close)).ok();
            },
        )
        .into_any_element()
    }
}

impl crate::console::PadTarget for ShareCaptureDialog {
    fn on_pad(&mut self, pad: crate::console::Pad, _window: &mut Window, cx: &mut Context<Self>) -> bool {
        use crate::console::list::Outcome;
        let rows = self.console_rows(cx);
        match self.console_list.on_pad(pad, &rows) {
            Outcome::Activate(id) if id.as_ref() == "share" => self.share(cx),
            Outcome::Activate(id) if id.as_ref() == "close" => cx.emit(DialogEvent::Close),
            Outcome::Ignored => return false,
            _ => {}
        }
        cx.notify();
        true
    }

    fn hints(&self, _cx: &App) -> Vec<crate::console::Hint> {
        vec![crate::console::hint(crate::console::Btn::A, tr!("Select"))]
    }
}
