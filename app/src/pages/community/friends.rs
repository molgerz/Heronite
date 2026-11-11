//! The Friends tab: adding a friend, and the list with what each plays.

use gpui_kit::prelude::*;
use gpui_kit::*;
use heronite_i18n::tr;
use heronite_nostr::PublicKey;

use super::CommunityPage;
use crate::icons::Icon;
use crate::pages::{empty_row, icon_action, list_item, status_pill};
use crate::services::nostr::Nostr;
use crate::theme;
use crate::ui::controls::{button_sm, caption, card, ButtonKind};
use crate::ui::fields::input_box;

/// A round badge with the first letter of the name (pictures are links to
/// anywhere, so they are not loaded).
pub(super) fn avatar(name: &str, size: f32) -> Div {
    let letter = name.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded_full()
        .bg(theme::accent_tint())
        .text_size(px(size * 0.45))
        .text_color(theme::text_primary())
        .child(letter)
}

/// The friends to list (who plays first, muted ones left out) as
/// `(key, name, game)`, and the user's npub.
fn snapshot(svc: &Nostr) -> (Vec<(PublicKey, String, Option<String>)>, String) {
    let mut friends: Vec<(PublicKey, String, Option<String>)> = svc
        .friends()
        .iter()
        .filter(|f| !svc.mutes().people.contains(f))
        .map(|f| (*f, svc.label(f), svc.playing(f).map(|p| p.title.clone())))
        .collect();
    friends.sort_by_key(|(_, _, playing)| playing.is_none());
    let me = svc.me().map(|k| heronite_nostr::identity::npub(&k)).unwrap_or_default();
    (friends, me)
}

impl CommunityPage {
    pub(super) fn add_friend(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.add.read(cx).value().trim().to_string();
        if input.is_empty() || self.adding {
            return;
        }
        self.adding = true;
        self.add_error = None;
        let page = cx.entity().downgrade();
        self.svc.update(cx, |s, cx| {
            s.follow(input, cx, move |problem, cx| {
                page.update(cx, |p, cx| {
                    p.adding = false;
                    p.add_error = problem;
                    cx.notify();
                })
                .ok();
            })
        });
        self.add.update(cx, |i, cx| i.set_value("", window, cx));
        cx.notify();
    }

    fn friend_row(&self, i: usize, key: PublicKey, name: String, playing: Option<String>, cx: &mut Context<Self>) -> Div {
        let line: Div = match playing {
            Some(title) => div()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(status_pill(tr!("Playing"), theme::status_color("playing")))
                .child(div().text_size(px(12.5)).text_color(theme::text_muted()).overflow_hidden().text_ellipsis().child(title)),
            None => div().text_size(px(12.)).text_color(theme::text_dim()).child(heronite_nostr::identity::short_npub(&key)),
        };
        let content = div()
            .flex()
            .flex_col()
            .gap(px(3.))
            .child(div().text_size(px(14.)).text_color(theme::text_primary()).overflow_hidden().text_ellipsis().child(name.clone()))
            .child(line);
        let confirming = self.confirm_remove == Some(key);
        let trailing = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(icon_action(("friend-chat", i), Icon::Chat, true).on_click(cx.listener(move |this, _, _, cx| {
                let chat = this.svc.read(cx).chat_with(&[key]);
                this.open_chat(chat, cx);
            })))
            .child(
                button_sm(
                    ("friend-remove", i),
                    if confirming { ButtonKind::Danger } else { ButtonKind::Ghost },
                    if confirming { tr!("Remove?") } else { tr!("Remove") },
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    if this.confirm_remove == Some(key) {
                        this.confirm_remove = None;
                        this.svc.update(cx, |s, cx| s.unfollow(key, cx));
                    } else {
                        this.confirm_remove = Some(key);
                    }
                    cx.notify();
                })),
            );
        list_item(Some(avatar(&name, 36.).into_any_element()), content, trailing)
    }

    pub(super) fn render_friends(&mut self, cx: &mut Context<Self>) -> Div {
        let (friends, me) = snapshot(self.svc.read(cx));
        let rows: Vec<AnyElement> = if friends.is_empty() {
            vec![empty_row(tr!("No friends yet. Add someone by their npub or name@domain address.")).into_any_element()]
        } else {
            friends
                .into_iter()
                .enumerate()
                .map(|(i, (key, name, playing))| self.friend_row(i, key, name, playing, cx).into_any_element())
                .collect()
        };
        let add = div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .py(px(12.))
            .child(
                div().flex().gap(px(10.)).child(div().flex_1().min_w_0().child(input_box(&self.add, self.adding))).child(
                    button_sm("friend-add", ButtonKind::Primary, if self.adding { tr!("Looking…") } else { tr!("Add") })
                        .when(!self.adding, |b| b.on_click(cx.listener(|this, _, window, cx| this.add_friend(window, cx)))),
                ),
            )
            .children(self.add_error.clone().map(|e| div().text_size(px(12.5)).text_color(theme::danger_text()).child(e)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .text_size(px(12.))
                    .text_color(theme::text_dim())
                    .child(tr!("Your key:"))
                    .child(div().flex_1().min_w_0().overflow_hidden().text_ellipsis().font_family(theme::FONT_MONO).child(me.clone()))
                    .child(
                        button_sm("friend-copy-me", ButtonKind::Ghost, tr!("Copy"))
                            .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(me.clone()))),
                    ),
            );
        div()
            .flex()
            .flex_col()
            .child(caption(tr!("Add a friend")))
            .child(card([add.into_any_element()]))
            .child(caption(tr!("Friends")))
            .child(card(rows))
    }
}
