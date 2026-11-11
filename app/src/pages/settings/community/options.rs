//! What is shared and where: the sharing switches, the relays and the media
//! servers of Settings → Community.

use gpui_kit::prelude::*;
use gpui_kit::*;
use heronite_i18n::tr;
use heronite_nostr::pool::RelayStatus;

use super::super::SettingsPage;
use crate::icons::Icon;
use crate::pages::{icon_action, status_pill};
use crate::services::nostr as service;
use crate::theme;
use crate::ui::controls::{button_sm, caption, card, help_text, sub_rows, toggle_row, ButtonKind};
use crate::ui::fields::input_box;

fn set(cx: &mut Context<SettingsPage>, change: impl FnOnce(&mut heronite_nostr::prefs::Prefs)) {
    service::get(cx).update(cx, |s, cx| s.set_prefs(change, cx));
}

pub(super) fn add_relay(d: &mut SettingsPage, window: &mut Window, cx: &mut Context<SettingsPage>) {
    let text = d.community.relay.read(cx).value().trim().to_string();
    if text.is_empty() {
        return;
    }
    let added = service::get(cx).update(cx, |s, cx| s.add_relay(&text, cx));
    if added {
        d.community.relay.update(cx, |i, cx| i.set_value("", window, cx));
    }
}

pub(super) fn add_server(d: &mut SettingsPage, window: &mut Window, cx: &mut Context<SettingsPage>) {
    let text = d.community.server.read(cx).value().trim().trim_end_matches('/').to_string();
    if !text.starts_with("https://") {
        return;
    }
    d.community.server.update(cx, |i, cx| i.set_value("", window, cx));
    set(cx, |p| {
        if !p.media_servers.contains(&text) {
            p.media_servers.push(text);
        }
    });
}

pub(super) fn sharing_card(cx: &mut Context<SettingsPage>) -> Div {
    let prefs = service::get(cx).read(cx).prefs().clone();
    let notify = toggle_row(
        "nostr-notify",
        tr!("Notify me about new messages"),
        Some(tr!("A desktop notification names who wrote; the text stays in Heronite.")),
        prefs.notify_messages,
    )
    .on_click(cx.listener(|_, _, _, cx| set(cx, |p| p.notify_messages = !p.notify_messages)));
    let while_playing = toggle_row("nostr-notify-playing", tr!("Also while a game runs"), None, prefs.notify_while_playing)
        .on_click(cx.listener(|_, _, _, cx| set(cx, |p| p.notify_while_playing = !p.notify_while_playing)));
    let rows = [
        toggle_row(
            "nostr-share-playing",
            tr!("Share what I'm playing"),
            Some(tr!("Friends see the game while it runs, from any store or emulator. A game's menu can leave it out.")),
            prefs.share_playing,
        )
        .on_click(cx.listener(|_, _, _, cx| set(cx, |p| p.share_playing = !p.share_playing)))
        .into_any_element(),
        notify.into_any_element(),
        sub_rows(vec![while_playing.into_any_element()]).when(!prefs.notify_messages, |r| r.opacity(0.42)).into_any_element(),
        toggle_row(
            "nostr-history",
            tr!("Keep chat history on this computer"),
            Some(tr!("Kept as it arrived, encrypted for your key. Otherwise chats are read from the relays again at each start.")),
            prefs.keep_history,
        )
        .on_click(cx.listener(|_, _, _, cx| set(cx, |p| p.keep_history = !p.keep_history)))
        .into_any_element(),
    ];
    div().flex().flex_col().child(caption(tr!("Sharing"))).child(card(rows))
}

fn relay_pill(status: Option<&RelayStatus>) -> Div {
    match status {
        Some(RelayStatus::Connected) => status_pill(tr!("Connected"), theme::status_color("playing")),
        Some(RelayStatus::Connecting) => status_pill(tr!("Connecting…"), theme::status_color("on-hold")),
        Some(RelayStatus::Failed(_)) => status_pill(tr!("Not reachable"), theme::status_color("abandoned")),
        None => status_pill(tr!("Off"), theme::text_dim()),
    }
}

pub(super) fn relays_card(d: &SettingsPage, cx: &mut Context<SettingsPage>) -> Div {
    let (relays, status) = {
        let svc = service::get(cx);
        let svc = svc.read(cx);
        (svc.prefs().relays.clone(), svc.relay_status().clone())
    };
    let mut rows: Vec<AnyElement> =
        relays
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let url = r.url.clone();
                let error = match status.get(&r.url) {
                    Some(RelayStatus::Failed(e)) => Some(e.clone()),
                    _ => None,
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .py(px(10.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(13.5))
                                    .text_color(theme::text_primary())
                                    .overflow_hidden()
                                    .text_ellipsis()
                                    .child(r.url.clone()),
                            )
                            .children(error.map(|e| {
                                div().text_size(px(11.5)).text_color(theme::text_dim()).overflow_hidden().text_ellipsis().child(e)
                            })),
                    )
                    .child(relay_pill(status.get(&r.url)))
                    .child(
                        icon_action(("nostr-relay-remove", i), Icon::Close, true)
                            .on_click(cx.listener(move |_, _, _, cx| service::get(cx).update(cx, |s, cx| s.remove_relay(&url, cx)))),
                    )
                    .into_any_element()
            })
            .collect();
    if relays.is_empty() {
        rows.push(div().py(px(12.)).child(help_text(tr!("No relay: nothing can be read or published."))).into_any_element());
    }
    rows.push(
        div()
            .flex()
            .gap(px(10.))
            .py(px(12.))
            .child(div().flex_1().min_w_0().child(input_box(&d.community.relay, false)))
            .child(
                button_sm("nostr-relay-add", ButtonKind::Glass, tr!("Add"))
                    .on_click(cx.listener(|d, _, window, cx| add_relay(d, window, cx))),
            )
            .child(
                button_sm("nostr-relay-reset", ButtonKind::Ghost, tr!("Reset"))
                    .on_click(cx.listener(|_, _, _, cx| service::get(cx).update(cx, |s, cx| s.reset_relays(cx)))),
            )
            .into_any_element(),
    );
    div().flex().flex_col().child(caption(tr!("Relays"))).child(card(rows)).child(
        div()
            .mt(px(6.))
            .child(help_text(tr!("Relays carry your events. Anyone can run one; your friends may use others, and that works too."))),
    )
}

pub(super) fn servers_card(d: &SettingsPage, cx: &mut Context<SettingsPage>) -> Div {
    let servers = service::get(cx).read(cx).prefs().media_servers.clone();
    let mut rows: Vec<AnyElement> = servers
        .iter()
        .enumerate()
        .map(|(i, url)| {
            let gone = url.clone();
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .py(px(10.))
                .child(div().flex_1().min_w_0().text_size(px(13.5)).text_color(theme::text_primary()).child(url.clone()))
                .child(icon_action(("nostr-server-remove", i), Icon::Close, true).on_click(cx.listener(move |_, _, _, cx| {
                    let gone = gone.clone();
                    set(cx, move |p| p.media_servers.retain(|s| *s != gone));
                })))
                .into_any_element()
        })
        .collect();
    rows.push(
        div()
            .flex()
            .gap(px(10.))
            .py(px(12.))
            .child(div().flex_1().min_w_0().child(input_box(&d.community.server, false)))
            .child(
                button_sm("nostr-server-add", ButtonKind::Glass, tr!("Add"))
                    .on_click(cx.listener(|d, _, window, cx| add_server(d, window, cx))),
            )
            .into_any_element(),
    );
    div()
        .flex()
        .flex_col()
        .child(caption(tr!("Media servers")))
        .child(card(rows))
        .child(div().mt(px(6.)).child(help_text(tr!(
            "Shared screenshots and replays are uploaded to the first of these Blossom servers that takes them. Their text, EXIF and XMP metadata are removed first."
        ))))
}
