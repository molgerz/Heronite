//! Settings → Community: the optional community layer on Nostr. The switch
//! and the identity (`identity.rs`), the relays, and what is shared
//! (`options.rs`). Everything goes through `services::nostr`, which saves
//! the settings and talks to the relays.

mod identity;
mod options;

use gpui_kit::component::input::{InputEvent, InputState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use heronite_i18n::tr;

use super::SettingsPage;
use crate::icons::Icon;
use crate::pages::status_pill;
use crate::services::nostr::{self as service, Account};
use crate::theme;
use crate::ui::controls::{card, help_text, icon_badge, row_lead, toggle};
use crate::ui::fields::new_input;

pub struct State {
    import_key: Entity<InputState>,
    import_pass: Entity<InputState>,
    bunker: Entity<InputState>,
    export_pass: Entity<InputState>,
    name: Entity<InputState>,
    about: Entity<InputState>,
    picture: Entity<InputState>,
    relay: Entity<InputState>,
    server: Entity<InputState>,
    /// The import asked for the key's passphrase.
    needs_pass: bool,
    /// The key waiting for its passphrase (its field is cleared at once).
    pending_key: Option<String>,
    import_error: Option<String>,
    importing: bool,
    /// "Forget this identity" waits for its second click.
    confirm_forget: bool,
    /// The secret key is on screen (after "Show my secret key").
    shown_secret: Option<String>,
    /// The `ncryptsec1…` made by "Export with a passphrase".
    exported: Option<String>,
    /// What the profile fields were filled with (see `Filled`).
    filled: crate::pages::community::profile::Filled,
}

impl State {
    pub fn new(window: &mut Window, cx: &mut Context<SettingsPage>) -> Self {
        let svc = service::get(cx);
        cx.observe(&svc, |_, _, cx| cx.notify()).detach();
        let import_key = cx.new(|cx| InputState::new(window, cx).masked(true));
        let import_pass = cx.new(|cx| InputState::new(window, cx).masked(true));
        let export_pass = cx.new(|cx| InputState::new(window, cx).masked(true));
        for input in [&import_key, &import_pass] {
            cx.subscribe_in(input, window, |d: &mut SettingsPage, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::PressEnter { .. }) {
                    identity::import(d, window, cx);
                }
            })
            .detach();
        }
        let relay = new_input(window, cx, "", "wss://relay.example.com");
        cx.subscribe_in(&relay, window, |d: &mut SettingsPage, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                options::add_relay(d, window, cx);
            }
        })
        .detach();
        let server = new_input(window, cx, "", "https://blossom.example.com");
        cx.subscribe_in(&server, window, |d: &mut SettingsPage, _, ev: &InputEvent, window, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                options::add_server(d, window, cx);
            }
        })
        .detach();
        Self {
            import_key,
            import_pass,
            bunker: new_input(window, cx, "", "bunker://…"),
            export_pass,
            name: new_input(window, cx, "", tr!("Your name")),
            about: new_input(window, cx, "", tr!("A line about you")),
            picture: new_input(window, cx, "", "https://…/me.png"),
            relay,
            server,
            needs_pass: false,
            pending_key: None,
            import_error: None,
            importing: false,
            confirm_forget: false,
            shown_secret: None,
            exported: None,
            filled: Default::default(),
        }
    }

    /// Drops an open question or a shown secret; true when there was one.
    pub(super) fn dismiss(&mut self) -> bool {
        let had = self.confirm_forget || self.shown_secret.is_some() || self.exported.is_some();
        self.confirm_forget = false;
        self.shown_secret = None;
        self.exported = None;
        had
    }
}

/// The pill next to the switch.
fn status(svc: &service::Nostr) -> (SharedString, Rgba) {
    if !svc.prefs().enabled {
        return (tr!("Off").into(), theme::text_dim());
    }
    match svc.account() {
        Account::None => (tr!("No identity").into(), theme::status_color("on-hold")),
        Account::Opening => (tr!("Opening…").into(), theme::status_color("on-hold")),
        Account::Failed(_) => (tr!("Not connected").into(), theme::status_color("abandoned")),
        Account::Ready => match svc.connected_relays() {
            0 => (tr!("Connecting…").into(), theme::status_color("on-hold")),
            n => (heronite_i18n::trn!(n, "Connected to {n} relay", "Connected to {n} relays").into(), theme::status_color("playing")),
        },
    }
}

fn switch_card(cx: &mut Context<SettingsPage>) -> Div {
    let svc = service::get(cx);
    let svc = svc.read(cx);
    let (pill, colour) = status(svc);
    let enabled = svc.prefs().enabled;
    let main = row_lead(
        icon_badge(Icon::Group),
        tr!("Turn on the community layer"),
        Some(tr!("Friends, what they play, sharing, game discussions and private chats, over Nostr, an open protocol. Off by default.")),
        div().flex().items_center().gap(px(10.)).child(status_pill(pill, colour)).child(toggle("nostr-enabled-sw", enabled, false)),
    )
    .id("nostr-enabled")
    .cursor_pointer()
    .on_click(cx.listener(|_, _, _, cx| {
        service::get(cx).update(cx, |s, cx| s.set_prefs(|p| p.enabled = !p.enabled, cx));
    }));
    let notice = match svc.account() {
        Account::Failed(e) => Some((e.clone(), true)),
        _ => svc.notice().cloned(),
    };
    let mut rows = vec![main.into_any_element()];
    if let Some((text, error)) = notice {
        rows.push(
            div()
                .py(px(10.))
                .text_size(px(12.5))
                .text_color(if error { theme::danger_text() } else { theme::text_muted() })
                .child(text)
                .into_any_element(),
        );
    }
    card(rows)
}

pub fn render(d: &mut SettingsPage, window: &mut Window, cx: &mut Context<SettingsPage>) -> AnyElement {
    if !service::available() {
        return help_text(tr!("The community layer is not available on this system.")).into_any_element();
    }
    let (has_identity, enabled) = {
        let svc = service::get(cx);
        let svc = svc.read(cx);
        (svc.prefs().identity.is_some(), svc.prefs().enabled)
    };
    let mut col = div().flex().flex_col().gap(px(16.)).child(switch_card(cx));
    if has_identity {
        identity::fill_profile(d, window, cx);
        col = col.child(identity::account_card(d, cx)).child(identity::profile_card(d, cx)).child(identity::keys_card(d, cx));
    } else {
        col = col.child(identity::setup_card(d, cx));
    }
    col.child(options::sharing_card(cx))
        .child(options::relays_card(d, cx))
        .child(options::servers_card(d, cx))
        .when(!enabled, |c| c.child(help_text(tr!("Nothing connects and nothing is shared while the switch above is off."))))
        .into_any_element()
}
