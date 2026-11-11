//! The identity cards of Settings → Community: setting one up (a new key,
//! an existing key, or a phone that keeps the key), then the account, the
//! profile and the key's export and removal.

use gpui_kit::prelude::*;
use gpui_kit::*;
use heronite_i18n::tr;
use heronite_nostr::social::Profile;

use super::super::SettingsPage;
use crate::icons::Icon;
use crate::services::nostr::{self as service, Pairing};
use crate::theme;
use crate::ui::controls::{button_sm, caption, card, help_text, icon_badge, row, row_lead, ButtonKind};
use crate::ui::fields::{input_box, labeled};
use crate::ui::icon;

fn error_line(text: impl Into<SharedString>) -> Div {
    div().text_size(px(12.5)).text_color(theme::danger_text()).child(text.into())
}

/// A long key or code with a copy button, in a monospace box.
fn copy_box(id: &'static str, text: String) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .px(px(10.))
                .py(px(8.))
                .rounded(px(10.))
                .bg(theme::ink_a(0.06))
                .font_family(theme::FONT_MONO)
                .text_size(px(12.))
                .text_color(theme::text_primary())
                .overflow_hidden()
                .text_ellipsis()
                .child(text.clone()),
        )
        .child(
            button_sm(id, ButtonKind::Glass, tr!("Copy"))
                .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))),
        )
}

pub(super) fn import(d: &mut SettingsPage, window: &mut Window, cx: &mut Context<SettingsPage>) {
    let s = &d.community;
    if s.importing {
        return;
    }
    // A key that asked for its passphrase is kept until the retry.
    let typed = s.import_key.read(cx).value().trim().to_string();
    let key = if typed.is_empty() { s.pending_key.clone().unwrap_or_default() } else { typed };
    if key.is_empty() {
        return;
    }
    let pass = s.import_pass.read(cx).value().to_string();
    let passphrase = (s.needs_pass && !pass.is_empty()).then_some(pass);
    // The secret leaves the fields at once.
    for input in [s.import_key.clone(), s.import_pass.clone()] {
        input.update(cx, |i, cx| i.set_value("", window, cx));
    }
    d.community.importing = true;
    d.community.import_error = None;
    let page = cx.entity().downgrade();
    let retry = key.clone();
    service::get(cx).update(cx, |svc, cx| {
        svc.import(key, passphrase, cx, move |problem, cx| {
            page.update(cx, |d, cx| {
                d.community.importing = false;
                d.community.pending_key = None;
                match problem {
                    None => d.community.needs_pass = false,
                    Some(service::ImportProblem::NeedsPassphrase) => {
                        d.community.needs_pass = true;
                        d.community.pending_key = Some(retry);
                        d.community.import_error = Some(tr!("This key is encrypted: type its passphrase.").into());
                    }
                    Some(service::ImportProblem::Other(e)) => d.community.import_error = Some(e),
                }
                cx.notify();
            })
            .ok();
        });
    });
}

/// No identity yet: the three ways to get one.
pub(super) fn setup_card(d: &SettingsPage, cx: &mut Context<SettingsPage>) -> Div {
    let s = &d.community;
    let pairing = {
        let svc = service::get(cx);
        match svc.read(cx).pairing() {
            Pairing::None => None,
            Pairing::Waiting { uri, .. } => Some(Some(uri.clone())),
            Pairing::Connecting => Some(None),
        }
    };
    let create = row_lead(
        icon_badge(Icon::AutoAwesome),
        tr!("Create a new identity"),
        Some(tr!("A new key pair, kept on this computer. You can give yourself a name afterwards.")),
        button_sm("nostr-create", ButtonKind::Primary, tr!("Create"))
            .on_click(cx.listener(|_, _, _, cx| service::get(cx).update(cx, |s, cx| s.create(cx)))),
    );
    let import_form = div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .py(px(12.))
        .child(labeled(tr!("Secret key (nsec1… or ncryptsec1…)"), input_box(&s.import_key, s.importing)))
        .when(s.needs_pass, |c| c.child(labeled(tr!("Passphrase"), input_box(&s.import_pass, s.importing))))
        .children(s.import_error.clone().map(error_line))
        .child(
            div().flex().child(
                button_sm("nostr-import", ButtonKind::Glass, if s.importing { tr!("Opening…") } else { tr!("Use this key") })
                    .when(!s.importing, |b| b.on_click(cx.listener(|d, _, window, cx| import(d, window, cx)))),
            ),
        );
    let import = row_lead(
        icon_badge(Icon::VpnKey),
        tr!("Use an existing key"),
        Some(tr!("From another Nostr app. It is kept in your desktop's keyring (on Windows, encrypted for your user).")),
        div(),
    );
    let phone_body = match pairing {
        None => div().flex().flex_col().gap(px(10.)).py(px(12.)).child(
            div()
                .flex()
                .flex_wrap()
                .gap(px(10.))
                .child(
                    button_sm("nostr-pair", ButtonKind::Glass, tr!("Show a QR code"))
                        .on_click(cx.listener(|_, _, _, cx| service::get(cx).update(cx, |s, cx| s.start_pairing(cx)))),
                )
                .child(div().flex_1().min_w(px(220.)).child(input_box(&s.bunker, false)))
                .child(button_sm("nostr-bunker", ButtonKind::Glass, tr!("Connect")).on_click(cx.listener(|d, _, window, cx| {
                    let uri = d.community.bunker.read(cx).value().trim().to_string();
                    if uri.is_empty() {
                        return;
                    }
                    d.community.bunker.update(cx, |i, cx| i.set_value("", window, cx));
                    service::get(cx).update(cx, |s, cx| s.connect_bunker(uri, cx));
                }))),
        ),
        Some(Some(uri)) => div()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(10.))
            .py(px(12.))
            .child(help_text(tr!("Scan this code with your signer app (for example Amber or nsec.app), then approve Heronite there.")))
            .children(crate::ui::qr::qr_code(&uri, 4.))
            .child(div().w_full().child(copy_box("nostr-pair-copy", uri)))
            .child(button_sm("nostr-pair-cancel", ButtonKind::Ghost, tr!("Cancel")).on_click(cx.listener(|_, _, _, cx| {
                service::get(cx).update(cx, |s, cx| {
                    s.cancel_pairing();
                    cx.notify();
                })
            }))),
        Some(None) => div().py(px(12.)).child(help_text(tr!("Connecting to your signer… approve Heronite on your phone."))),
    };
    let phone = row_lead(
        icon_badge(Icon::QrCode),
        tr!("Sign in with your phone"),
        Some(tr!("Your key stays in a signer app on your phone; Heronite asks it to sign. Scan a QR code, or paste a bunker:// address from the app.")),
        div(),
    );
    div().flex().flex_col().child(caption(tr!("Your identity"))).child(card([
        create.into_any_element(),
        import.into_any_element(),
        import_form.into_any_element(),
        phone.into_any_element(),
        phone_body.into_any_element(),
    ]))
}

/// Fills the profile fields once from the profile the relays sent.
pub(super) fn fill_profile(d: &mut SettingsPage, window: &mut Window, cx: &mut Context<SettingsPage>) {
    let (me, profile) = {
        let svc = service::get(cx);
        let svc = svc.read(cx);
        (svc.me().map(|k| k.to_hex()), svc.my_profile())
    };
    let Some(me) = me else { return };
    // Once per identity, when its profile came (the fields are the user's afterwards).
    if d.community.profile_of.as_deref() == Some(me.as_str()) || profile == Profile::default() {
        return;
    }
    d.community.profile_of = Some(me);
    let name = if profile.display_name.is_empty() { profile.name.clone() } else { profile.display_name.clone() };
    d.community.name.update(cx, |i, cx| i.set_value(name, window, cx));
    d.community.about.update(cx, |i, cx| i.set_value(profile.about.clone(), window, cx));
    d.community.picture.update(cx, |i, cx| i.set_value(profile.picture.clone(), window, cx));
}

pub(super) fn account_card(_d: &SettingsPage, cx: &mut Context<SettingsPage>) -> Div {
    let (npub, remote) = {
        let svc = service::get(cx);
        let svc = svc.read(cx);
        (svc.me().map(|k| heronite_nostr::identity::npub(&k)).unwrap_or_default(), svc.is_remote())
    };
    let kind = if remote {
        tr!("Signed by the app on your phone: the key never comes to this computer.")
    } else {
        tr!("The key is on this computer, in your desktop's keyring (on Windows, encrypted for your user).")
    };
    div().flex().flex_col().child(caption(tr!("Your identity"))).child(card([
        row(tr!("Your public key"), Some(tr!("Give it to friends so they can add you.")), div()).into_any_element(),
        div().pb(px(12.)).child(copy_box("nostr-npub-copy", npub)).into_any_element(),
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .pb(px(12.))
            .text_size(px(12.))
            .text_color(theme::text_dim())
            .child(icon(Icon::Lock, px(13.), theme::text_dim()))
            .child(kind)
            .into_any_element(),
    ]))
}

pub(super) fn profile_card(d: &SettingsPage, cx: &mut Context<SettingsPage>) -> Div {
    let s = &d.community;
    let form = div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .py(px(12.))
        .child(labeled(tr!("Name"), input_box(&s.name, false)))
        .child(labeled(tr!("About"), input_box(&s.about, false)))
        .child(labeled(tr!("Picture (a link to an image)"), input_box(&s.picture, false)))
        .child(div().flex().child(button_sm("nostr-profile-save", ButtonKind::Primary, tr!("Save profile")).on_click(cx.listener(
            |d, _, _, cx| {
                let s = &d.community;
                let svc = service::get(cx);
                let mut profile = svc.read(cx).my_profile();
                profile.display_name = s.name.read(cx).value().trim().to_string();
                if profile.name.is_empty() {
                    profile.name = profile.display_name.clone();
                }
                profile.about = s.about.read(cx).value().trim().to_string();
                profile.picture = s.picture.read(cx).value().trim().to_string();
                svc.update(cx, |svc, cx| svc.save_profile(profile, cx));
            },
        ))))
        .child(help_text(tr!("Your profile is public: every Nostr app shows it.")));
    div().flex().flex_col().child(caption(tr!("Profile"))).child(card([form.into_any_element()]))
}

pub(super) fn keys_card(d: &SettingsPage, cx: &mut Context<SettingsPage>) -> Div {
    let s = &d.community;
    let local = service::get(cx).read(cx).has_local_key();
    let mut rows: Vec<AnyElement> = Vec::new();
    if local {
        let show = row(
            tr!("Show my secret key"),
            Some(tr!("Anyone with it can act as you. Keep it like a password; never paste it into a website.")),
            button_sm("nostr-show-secret", ButtonKind::Glass, if s.shown_secret.is_some() { tr!("Hide") } else { tr!("Show") }).on_click(
                cx.listener(|d, _, _, cx| {
                    d.community.shown_secret = match d.community.shown_secret.take() {
                        Some(_) => None,
                        None => service::get(cx).read(cx).export_nsec(),
                    };
                    cx.notify();
                }),
            ),
        );
        rows.push(show.into_any_element());
        if let Some(nsec) = s.shown_secret.clone() {
            rows.push(div().pb(px(12.)).child(copy_box("nostr-secret-copy", nsec)).into_any_element());
        }
        let export = div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .py(px(12.))
            .child(labeled(tr!("Export with a passphrase (ncryptsec, NIP-49)"), input_box(&s.export_pass, false)))
            .child(div().flex().child(button_sm("nostr-export", ButtonKind::Glass, tr!("Export")).on_click(cx.listener(
                |d, _, window, cx| {
                    let pass = d.community.export_pass.read(cx).value().to_string();
                    if pass.is_empty() {
                        return;
                    }
                    d.community.export_pass.update(cx, |i, cx| i.set_value("", window, cx));
                    let page = cx.entity().downgrade();
                    service::get(cx).update(cx, |svc, cx| {
                        svc.export_encrypted(pass, cx, move |result, cx| {
                            page.update(cx, |d, cx| {
                                d.community.exported = result.ok();
                                cx.notify();
                            })
                            .ok();
                        })
                    });
                },
            ))))
            .children(s.exported.clone().map(|e| copy_box("nostr-export-copy", e)));
        rows.push(export.into_any_element());
    }
    let forget = row(
        tr!("Forget this identity"),
        Some(if local {
            tr!("Removes the key from this computer. Without a copy of it, this identity is gone for good. What you published stays on the relays.")
        } else {
            tr!("Disconnects your phone's signer from Heronite. What you published stays on the relays.")
        }),
        button_sm("nostr-forget", ButtonKind::Danger, if s.confirm_forget { tr!("Click again to forget") } else { tr!("Forget") })
            .on_click(cx.listener(|d, _, _, cx| {
                if d.community.confirm_forget {
                    d.community.confirm_forget = false;
                    d.community.profile_of = None;
                    service::get(cx).update(cx, |s, cx| s.forget(cx));
                } else {
                    d.community.confirm_forget = true;
                }
                cx.notify();
            })),
    );
    rows.push(forget.into_any_element());
    div().flex().flex_col().child(caption(tr!("Key"))).child(card(rows))
}
