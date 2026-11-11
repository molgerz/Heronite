//! The Chats tab: the chats, newest first, and one open chat with its
//! messages and a line to write. Messages are end-to-end encrypted
//! (NIP-17); relays see neither the text nor who writes to whom.

use gpui_kit::prelude::*;
use gpui_kit::*;
use heronite_i18n::tr;
use heronite_nostr::chat::Message;

use super::friends::avatar;
use super::CommunityPage;
use crate::icons::Icon;
use crate::pages::{empty_row, icon_action, list_item};
use crate::services::nostr::{ChatKey, Nostr};
use crate::theme;
use crate::ui::controls::{button_sm, caption, card, help_text, ButtonKind};
use crate::ui::fields::input_box;
use crate::ui::icon;

/// "Ana, Bo" for the people of a chat other than the user.
fn chat_name(chat: &ChatKey, svc: &Nostr) -> String {
    let me = svc.me();
    let names: Vec<String> = chat.iter().filter(|p| Some(**p) != me).map(|p| svc.label(p)).collect();
    if names.is_empty() {
        tr!("Notes to myself").into()
    } else {
        names.join(", ")
    }
}

fn time(m: &Message) -> String {
    chrono::DateTime::from_timestamp(m.at.as_secs() as i64, 0)
        .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

impl CommunityPage {
    pub(super) fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(chat) = self.chat.clone() else { return };
        let text = self.compose.read(cx).value().trim().to_string();
        if text.is_empty() || self.svc.read(cx).is_sending() {
            return;
        }
        let me = self.svc.read(cx).me();
        let to: Vec<_> = chat.into_iter().filter(|p| Some(*p) != me).collect();
        self.svc.update(cx, |s, cx| s.send_message(to, text, None, cx));
        self.compose.update(cx, |i, cx| i.set_value("", window, cx));
    }

    pub(super) fn render_chats(&mut self, cx: &mut Context<Self>) -> Div {
        match self.chat.clone() {
            Some(chat) => self.render_chat(chat, cx),
            None => self.render_chat_list(cx),
        }
    }

    fn render_chat_list(&mut self, cx: &mut Context<Self>) -> Div {
        let chats: Vec<(ChatKey, Message, String, bool)> = {
            let svc = self.svc.read(cx);
            svc.chats().into_iter().map(|(k, m)| (k.clone(), m.clone(), chat_name(k, svc), svc.is_unread(k))).collect()
        };
        let rows: Vec<AnyElement> = if chats.is_empty() {
            vec![empty_row(tr!("No chats yet. Start one from a friend's chat button.")).into_any_element()]
        } else {
            chats
                .into_iter()
                .enumerate()
                .map(|(i, (chat, last, name, unread))| {
                    let content = div()
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(
                            div()
                                .text_size(px(14.))
                                .text_color(theme::text_primary())
                                .when(unread, |d| d.font_weight(FontWeight::SEMIBOLD))
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(name.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(theme::text_muted())
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(last.text.clone()),
                        );
                    let trailing = div().text_size(px(11.5)).text_color(theme::text_dim()).child(time(&last));
                    list_item(Some(avatar(&name, 36.).into_any_element()), content, trailing)
                        .id(("chat-row", i))
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| this.open_chat(chat.clone(), cx)))
                        .into_any_element()
                })
                .collect()
        };
        div()
            .flex()
            .flex_col()
            .child(caption(tr!("Chats")))
            .child(card(rows))
            .child(div().mt(px(6.)).child(help_text(tr!("End-to-end encrypted: relays see neither the text nor who writes to whom."))))
    }

    fn render_chat(&mut self, chat: ChatKey, cx: &mut Context<Self>) -> Div {
        if self.svc.read(cx).is_unread(&chat) {
            let key = chat.clone();
            self.svc.update(cx, |s, cx| s.mark_read(&key, cx));
        }
        let svc = self.svc.read(cx);
        let me = svc.me();
        let name = chat_name(&chat, svc);
        let group = chat.len() > 2;
        let messages: Vec<AnyElement> = if svc.messages(&chat).is_empty() {
            vec![empty_row(tr!("No messages yet. Say hello!")).into_any_element()]
        } else {
            svc.messages(&chat)
                .iter()
                .map(|m| {
                    let mine = Some(m.from) == me;
                    let who = if group && !mine { Some(svc.label(&m.from)) } else { None };
                    let bubble = div()
                        .max_w(px(480.))
                        .px(px(12.))
                        .py(px(8.))
                        .rounded(px(14.))
                        .bg(if mine { theme::accent_tint() } else { theme::ink_a(0.07) })
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .children(who.map(|w| div().text_size(px(11.5)).text_color(theme::text_dim()).child(w)))
                        .child(div().text_size(px(13.5)).text_color(theme::text_primary()).child(m.text.clone()))
                        .child(div().text_size(px(10.5)).text_color(theme::text_dim()).child(time(m)));
                    div().flex().w_full().py(px(3.)).when(mine, |d| d.justify_end()).child(bubble).into_any_element()
                })
                .collect()
        };
        let sending = svc.is_sending();
        let header = div()
            .flex()
            .items_center()
            .gap(px(10.))
            .py(px(10.))
            .child(icon_action("chat-back", Icon::ChevronLeft, true).on_click(cx.listener(|this, _, _, cx| {
                this.chat = None;
                cx.notify();
            })))
            .child(avatar(&name, 30.))
            .child(
                div().flex_1().min_w_0().text_size(px(15.)).text_color(theme::text_primary()).overflow_hidden().text_ellipsis().child(name),
            )
            .child(icon(Icon::Lock, px(14.), theme::text_dim()));
        let compose = div().flex().gap(px(10.)).py(px(12.)).child(div().flex_1().min_w_0().child(input_box(&self.compose, sending))).child(
            button_sm("chat-send", ButtonKind::Primary, if sending { tr!("Sending…") } else { tr!("Send") })
                .when(!sending, |b| b.on_click(cx.listener(|this, _, window, cx| this.send(window, cx)))),
        );
        let list = div().flex().flex_col().py(px(8.)).children(messages);
        div().flex().flex_col().child(card([header.into_any_element(), list.into_any_element(), compose.into_any_element()]))
    }
}
