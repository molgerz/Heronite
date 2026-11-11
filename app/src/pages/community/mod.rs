//! The Community page (☰ → Community): friends from any store or emulator,
//! what they play now, and private chats, over Nostr (`services::nostr`).
//! Off until the user turns the community layer on in Settings →
//! Community; until then the page says so and links there.
//!
//! * `friends.rs`: adding friends, the list with what each one plays.
//! * `chats.rs`: the chats and one open chat.
//! * `profile.rs`: what friends see of the user: the game now, name and bio.

mod chats;
mod friends;
pub(crate) mod profile;

use gpui_kit::component::input::{InputEvent, InputState, TextareaState};
use gpui_kit::prelude::*;
use gpui_kit::*;
use heronite_i18n::tr;

use super::commands::{CommandCtx, PageCommand, PaletteCommands};
use super::{page_close, sheet, Page, PageCtx, PageEvent};
use crate::console::list::{Outcome, Row};
use crate::console::{hint, Btn, Hint, Pad, PadTarget};
use crate::icons::Icon;
use crate::services::nostr::{self as service, ChatKey, Nostr};
use crate::ui::controls::{button, help_text, tabs, ButtonKind};
use crate::ui::fields::{new_input, new_textarea};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tab {
    Friends,
    Chats,
    Profile,
}

pub struct CommunityPage {
    svc: Entity<Nostr>,
    tab: Tab,
    add: Entity<InputState>,
    add_error: Option<String>,
    adding: bool,
    /// The friend whose "Remove" waits for its second click.
    confirm_remove: Option<heronite_nostr::PublicKey>,
    chat: Option<ChatKey>,
    compose: Entity<InputState>,
    /// The Profile tab's name and bio.
    name: Entity<InputState>,
    about: Entity<TextareaState>,
    filled: profile::Filled,
    console_list: crate::console::list::ListNav,
    _subs: Vec<Subscription>,
}

impl EventEmitter<PageEvent> for CommunityPage {}

const KEYWORDS: &str = "community friends nostr chat messages playing profile status comunidade amigos conversa mensagens perfil comunidad amigos chat mensajes perfil estado gemeinschaft freunde nachrichten profil";

impl CommunityPage {
    pub fn new(_ctx: PageCtx, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let svc = service::get(cx);
        let add = new_input(window, cx, "", tr!("npub1… or name@domain"));
        let compose = new_input(window, cx, "", tr!("Write a message"));
        let name = new_input(window, cx, "", tr!("Your name"));
        let about = new_textarea(window, cx, "", profile::ABOUT_ROWS);
        let subs = vec![
            cx.observe(&svc, |_, _, cx| cx.notify()),
            cx.subscribe_in(&add, window, |this: &mut Self, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::PressEnter { .. }) {
                    this.add_friend(window, cx);
                }
            }),
            cx.subscribe_in(&compose, window, |this: &mut Self, _, ev: &InputEvent, window, cx| {
                if matches!(ev, InputEvent::PressEnter { .. }) {
                    this.send(window, cx);
                }
            }),
        ];
        Self {
            svc,
            tab: Tab::Friends,
            add,
            add_error: None,
            adding: false,
            confirm_remove: None,
            chat: None,
            compose,
            name,
            about,
            filled: Default::default(),
            console_list: Default::default(),
            _subs: subs,
        }
    }

    /// The page was left: an open question goes.
    pub fn hidden(&mut self) {
        self.confirm_remove = None;
    }

    fn open_settings(&mut self, cx: &mut Context<Self>) {
        crate::pages::settings::request_section(crate::pages::settings::Section::Community, cx);
        cx.emit(PageEvent::Navigate(Page::Settings));
    }

    fn open_chat(&mut self, chat: ChatKey, cx: &mut Context<Self>) {
        self.svc.update(cx, |s, cx| s.mark_read(&chat, cx));
        self.chat = Some(chat);
        self.tab = Tab::Chats;
        cx.notify();
    }

    /// The page while the community layer is off or has no identity.
    fn render_off(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .items_start()
            .gap(px(14.))
            .py(px(24.))
            .child(help_text(tr!(
                "See what your friends play in any store or emulator, share screenshots and chat, over Nostr, an open protocol. It is optional and off until you turn it on."
            )))
            .child(
                button("community-settings", ButtonKind::Primary, tr!("Set up in Settings"))
                    .on_click(cx.listener(|this, _, _, cx| this.open_settings(cx))),
            )
    }

    fn console_rows(&self, cx: &App) -> Vec<Row> {
        let svc = self.svc.read(cx);
        if !svc.ready() {
            return vec![Row::action("settings", tr!("Set up in Settings")).icon(Icon::Settings)];
        }
        let (head, _, line) = profile::status_text(&svc.my_status());
        let mut rows = vec![Row::header(tr!("You")), Row::info("me", head, line).icon(Icon::AccountCircle), Row::header(tr!("Friends"))];
        for (i, f) in svc.friends().iter().enumerate() {
            let what = svc.playing(f).map(|p| tr!("Playing {game}", game = p.title.as_str())).unwrap_or_default();
            rows.push(Row::info(SharedString::from(format!("friend-{i}")), svc.label(f), what).icon(Icon::AccountCircle));
        }
        if svc.friends().is_empty() {
            rows.push(Row::info("none", tr!("No friends yet"), tr!("Add them on the desktop.")));
        }
        rows
    }
}

impl Render for CommunityPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if crate::console::active() {
            let rows = self.console_rows(cx);
            return div().size_full().p(px(40.)).child(self.console_list.render("community-console", &rows, true)).into_any_element();
        }
        let ready = self.svc.read(cx).ready();
        let unread = self.svc.read(cx).unread_count();
        let body = if !ready {
            self.render_off(cx).into_any_element()
        } else {
            let chats_label: SharedString = if unread > 0 { format!("{} ({unread})", tr!("Chats")).into() } else { tr!("Chats").into() };
            let switcher = tabs(
                "community-tabs",
                &[
                    (Tab::Friends, SharedString::from(tr!("Friends"))),
                    (Tab::Chats, chats_label),
                    (Tab::Profile, SharedString::from(tr!("Profile"))),
                ],
                self.tab,
                {
                    let page = cx.entity().downgrade();
                    move |tab, _, cx| {
                        page.update(cx, |p, cx| {
                            p.tab = tab;
                            cx.notify();
                        })
                        .ok();
                    }
                },
            );
            let content = match self.tab {
                Tab::Friends => self.render_friends(cx).into_any_element(),
                Tab::Chats => self.render_chats(cx).into_any_element(),
                Tab::Profile => self.render_profile(window, cx).into_any_element(),
            };
            div().flex().flex_col().gap(px(16.)).child(div().flex().child(switcher)).child(content).into_any_element()
        };
        sheet(
            "community-page",
            Page::Community.title(),
            Some(tr!("Your friends from any store or emulator, over the open Nostr protocol.").into()),
            None,
            760.,
            window,
            page_close(cx),
            body,
        )
        .into_any_element()
    }
}

impl PadTarget for CommunityPage {
    fn on_pad(&mut self, pad: Pad, _window: &mut Window, cx: &mut Context<Self>) -> bool {
        let rows = self.console_rows(cx);
        let used = match self.console_list.on_pad(pad, &rows) {
            Outcome::Activate(id) if id.as_ref() == "settings" => {
                self.open_settings(cx);
                true
            }
            Outcome::Ignored => false,
            _ => true,
        };
        cx.notify();
        used
    }

    fn hints(&self, _cx: &App) -> Vec<Hint> {
        vec![hint(Btn::A, tr!("Select"))]
    }
}

impl PaletteCommands for CommunityPage {
    fn palette_commands(_page: Option<&Self>, _ctx: &CommandCtx, cx: &App) -> Vec<PageCommand> {
        if !service::available() {
            return Vec::new();
        }
        let ready = service::started(cx).is_some_and(|s| s.read(cx).ready());
        let mut out =
            vec![PageCommand::new("settings", tr!("Community settings"), tr!("Identity, relays and what is shared"), Icon::Settings)
                .keywords(KEYWORDS)];
        if ready {
            out.push(PageCommand::new("friends", tr!("Friends"), tr!("Who plays what now"), Icon::Group).keywords(KEYWORDS));
            out.push(PageCommand::new("chats", tr!("Chats"), tr!("Private messages"), Icon::Chat).keywords(KEYWORDS));
            out.push(
                PageCommand::new("profile", tr!("My profile"), tr!("What friends see: your game now, name and bio"), Icon::AccountCircle)
                    .keywords(KEYWORDS),
            );
        }
        out
    }

    fn run_palette_command(&mut self, id: &str, _window: &mut Window, cx: &mut Context<Self>) {
        match id {
            "settings" => self.open_settings(cx),
            "friends" => self.tab = Tab::Friends,
            "chats" => self.tab = Tab::Chats,
            "profile" => self.tab = Tab::Profile,
            _ => {}
        }
        cx.notify();
    }
}
