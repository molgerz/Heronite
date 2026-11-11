//! "Discussions" of a game (its context menu): the threads about it on
//! Nostr (NIP-7D) from any store's copy, one thread with its comments
//! (NIP-22), and writing either. Who of the friends plays it now, and
//! whether playing it is shared, sit on top. The reading lasts while the
//! dialog is open (`services::nostr`).

use gpui_kit::component::input::InputState;
use gpui_kit::prelude::*;
use gpui_kit::*;
use heronite_i18n::{tr, trn};
use heronite_nostr::threads::{Comment, Reason, Target, Thread};
use heronite_nostr::EventId;

use super::DialogEvent;
use crate::icons::Icon;
use crate::pages::{empty_row, icon_action};
use crate::services::nostr::{self as service, Nostr};
use crate::theme;
use crate::ui::controls::{button, button_sm, caption, card, help_text, toggle_row, ButtonKind};
use crate::ui::dialog::{dialog, DialogProps};
use crate::ui::fields::{input_box, labeled, new_input};

pub struct GameCommunityDialog {
    svc: Entity<Nostr>,
    key: String,
    title: String,
    open: Option<EventId>,
    /// "New thread" is open.
    writing: bool,
    thread_title: Entity<InputState>,
    thread_body: Entity<InputState>,
    reply: Entity<InputState>,
    console_list: crate::console::list::ListNav,
    _sub: Subscription,
}

impl EventEmitter<DialogEvent> for GameCommunityDialog {}

fn when(at: heronite_nostr::Timestamp) -> String {
    chrono::DateTime::from_timestamp(at.as_secs() as i64, 0)
        .map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

impl GameCommunityDialog {
    pub fn new(key: String, title: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let svc = service::get(cx);
        let sub = cx.observe(&svc, |_, _, cx| cx.notify());
        svc.update(cx, |s, cx| s.open_game(&key, cx));
        Self {
            svc,
            key,
            title,
            open: None,
            writing: false,
            thread_title: new_input(window, cx, "", tr!("Title")),
            thread_body: new_input(window, cx, "", tr!("What do you want to say?")),
            reply: new_input(window, cx, "", tr!("Write a reply")),
            console_list: Default::default(),
            _sub: sub,
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        let key = self.key.clone();
        self.svc.update(cx, |s, _| s.close_game(&key));
        cx.emit(DialogEvent::Close);
    }

    fn open_thread(&mut self, id: EventId, cx: &mut Context<Self>) {
        let key = self.key.clone();
        self.svc.update(cx, |s, _| s.open_thread(&key, &id));
        self.open = Some(id);
        cx.notify();
    }

    fn post_thread(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let title = self.thread_title.read(cx).value().trim().to_string();
        let body = self.thread_body.read(cx).value().trim().to_string();
        if title.is_empty() {
            return;
        }
        let key = self.key.clone();
        self.svc.update(cx, |s, cx| s.post_thread(&key, &title, &body, cx));
        for input in [self.thread_title.clone(), self.thread_body.clone()] {
            input.update(cx, |i, cx| i.set_value("", window, cx));
        }
        self.writing = false;
        cx.notify();
    }

    fn post_reply(&mut self, thread: Thread, window: &mut Window, cx: &mut Context<Self>) {
        let body = self.reply.read(cx).value().trim().to_string();
        if body.is_empty() {
            return;
        }
        let key = self.key.clone();
        self.svc.update(cx, |s, cx| s.post_comment(&key, &thread, None, &body, cx));
        self.reply.update(cx, |i, cx| i.set_value("", window, cx));
    }

    /// Mute and report buttons for a post by someone else.
    fn moderation(&self, id: &str, target: Target, cx: &mut Context<Self>) -> Div {
        let person = match target {
            Target::Post(_, author) | Target::Person(author) => author,
        };
        div()
            .flex()
            .gap(px(6.))
            .child(
                button_sm(SharedString::from(format!("{id}-mute")), ButtonKind::Ghost, tr!("Mute")).on_click(
                    cx.listener(move |this, _, _, cx| this.svc.update(cx, |s, cx| s.set_muted(Target::Person(person), true, cx))),
                ),
            )
            .child(button_sm(SharedString::from(format!("{id}-report")), ButtonKind::Ghost, tr!("Report spam")).on_click(
                cx.listener(move |this, _, _, cx| this.svc.update(cx, |s, cx| s.report(target, Reason::Spam, String::new(), cx))),
            ))
    }

    fn render_top(&self, cx: &mut Context<Self>) -> Div {
        let svc = self.svc.read(cx);
        let playing: Vec<String> = svc.friends_playing(&self.key).iter().map(|f| svc.label(f)).collect();
        let share = svc.prefs().share_playing;
        let shares_this = svc.prefs().may_share(&self.key);
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .when(!playing.is_empty(), |d| {
                d.child(
                    div().text_size(px(13.)).text_color(theme::text_muted()).child(tr!("Playing now: {names}", names = playing.join(", "))),
                )
            })
            .when(share, |d| {
                d.child(
                    toggle_row(
                        "gc-share",
                        tr!("Share when I play this game"),
                        Some(tr!("Friends see it in their list while it runs.")),
                        shares_this,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let key = this.key.clone();
                        this.svc.update(cx, |s, cx| s.set_never_share(&key, shares_this, cx));
                    })),
                )
            })
    }

    fn render_list(&mut self, cx: &mut Context<Self>) -> Div {
        let (threads, loaded) = {
            let svc = self.svc.read(cx);
            let g = svc.game_threads(&self.key);
            let threads: Vec<(Thread, usize, String)> = g
                .map(|g| {
                    g.threads
                        .iter()
                        .filter(|t| !svc.mutes().hides(&t.author, &t.id))
                        .map(|t| (t.clone(), g.comments.get(&t.id).map_or(0, Vec::len), svc.label(&t.author)))
                        .collect()
                })
                .unwrap_or_default();
            (threads, g.is_some_and(|g| g.loaded))
        };
        let rows: Vec<AnyElement> = if threads.is_empty() {
            vec![empty_row(if loaded { tr!("No discussion yet. Start the first one!") } else { tr!("Loading…") }).into_any_element()]
        } else {
            threads
                .into_iter()
                .enumerate()
                .map(|(i, (t, n, author))| {
                    let id = t.id;
                    div()
                        .id(("gc-thread", i))
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .py(px(10.))
                        .cursor_pointer()
                        .child(div().text_size(px(14.)).text_color(theme::text_primary()).child(if t.title.is_empty() {
                            t.body.clone()
                        } else {
                            t.title.clone()
                        }))
                        .child(div().text_size(px(12.)).text_color(theme::text_dim()).child(format!(
                            "{author} \u{b7} {} \u{b7} {}",
                            when(t.at),
                            trn!(n, "{n} reply", "{n} replies")
                        )))
                        .on_click(cx.listener(move |this, _, _, cx| this.open_thread(id, cx)))
                        .into_any_element()
                })
                .collect()
        };
        let new_thread = if self.writing {
            div()
                .flex()
                .flex_col()
                .gap(px(10.))
                .py(px(12.))
                .child(labeled(tr!("Title"), input_box(&self.thread_title, false)))
                .child(labeled(tr!("Text"), input_box(&self.thread_body, false)))
                .child(
                    div()
                        .flex()
                        .gap(px(10.))
                        .child(
                            button_sm("gc-post", ButtonKind::Primary, tr!("Post"))
                                .on_click(cx.listener(|this, _, window, cx| this.post_thread(window, cx))),
                        )
                        .child(button_sm("gc-post-cancel", ButtonKind::Ghost, tr!("Cancel")).on_click(cx.listener(|this, _, _, cx| {
                            this.writing = false;
                            cx.notify();
                        }))),
                )
                .child(help_text(tr!("Public: every Nostr app that shows discussions about this game can read it.")))
        } else {
            div().py(px(12.)).child(button_sm("gc-new", ButtonKind::Glass, tr!("New discussion")).on_click(cx.listener(
                |this, _, _, cx| {
                    this.writing = true;
                    cx.notify();
                },
            )))
        };
        div().flex().flex_col().child(card([new_thread.into_any_element()])).child(caption(tr!("Discussions"))).child(card(rows))
    }

    fn render_thread(&mut self, id: EventId, cx: &mut Context<Self>) -> Div {
        let (thread, comments, me) = {
            let svc = self.svc.read(cx);
            let g = svc.game_threads(&self.key);
            let thread = g.and_then(|g| g.threads.iter().find(|t| t.id == id).cloned());
            let comments: Vec<(Comment, String)> = g
                .and_then(|g| g.comments.get(&id))
                .map(|c| c.iter().filter(|c| !svc.mutes().hides(&c.author, &c.id)).map(|c| (c.clone(), svc.label(&c.author))).collect())
                .unwrap_or_default();
            (thread.map(|t| (t.clone(), svc.label(&t.author))), comments, svc.me())
        };
        let back = icon_action("gc-back", Icon::ChevronLeft, true).on_click(cx.listener(|this, _, _, cx| {
            this.open = None;
            cx.notify();
        }));
        let Some((thread, author)) = thread else {
            return div().child(back);
        };
        let mut rows: Vec<AnyElement> = vec![div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .py(px(12.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(back)
                    .child(div().text_size(px(16.)).text_color(theme::text_primary()).child(thread.title.clone())),
            )
            .child(div().text_size(px(13.5)).text_color(theme::text_primary()).child(thread.body.clone()))
            .child(div().text_size(px(12.)).text_color(theme::text_dim()).child(format!("{author} \u{b7} {}", when(thread.at))))
            .when(Some(thread.author) != me, |d| d.child(self.moderation("gc-t", Target::Post(thread.id, thread.author), cx)))
            .into_any_element()];
        for (i, (c, name)) in comments.into_iter().enumerate() {
            let mine = Some(c.author) == me;
            rows.push(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .py(px(10.))
                    .child(div().text_size(px(13.5)).text_color(theme::text_primary()).child(c.body.clone()))
                    .child(div().text_size(px(12.)).text_color(theme::text_dim()).child(format!("{name} \u{b7} {}", when(c.at))))
                    .when(!mine, |d| d.child(self.moderation(&format!("gc-c{i}"), Target::Post(c.id, c.author), cx)))
                    .into_any_element(),
            );
        }
        let reply_thread = thread.clone();
        rows.push(
            div()
                .flex()
                .gap(px(10.))
                .py(px(12.))
                .child(div().flex_1().min_w_0().child(input_box(&self.reply, false)))
                .child(
                    button_sm("gc-reply", ButtonKind::Primary, tr!("Reply"))
                        .on_click(cx.listener(move |this, _, window, cx| this.post_reply(reply_thread.clone(), window, cx))),
                )
                .into_any_element(),
        );
        div().flex().flex_col().child(card(rows))
    }

    fn console_rows(&self, cx: &App) -> Vec<crate::console::list::Row> {
        use crate::console::list::Row;
        let svc = self.svc.read(cx);
        let mut rows = vec![Row::header(tr!("Discussions"))];
        if let Some(g) = svc.game_threads(&self.key) {
            for (i, t) in g.threads.iter().take(30).enumerate() {
                rows.push(Row::info(SharedString::from(format!("t{i}")), t.title.clone(), svc.label(&t.author)));
            }
        }
        rows.push(Row::action("close", tr!("Close")));
        rows
    }
}

impl Render for GameCommunityDialog {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let heading = tr!("Discussions: {game}", game = self.title.as_str());
        if crate::console::active() {
            let rows = self.console_rows(cx);
            return crate::console::list::simple_dialog(&mut self.console_list, "console-game-community", &heading, "", None, &rows);
        }
        let ready = self.svc.read(cx).ready();
        let body = if !ready {
            div().child(help_text(tr!("Turn on the community layer in Settings → Community to read and write discussions.")))
        } else {
            let main = match self.open {
                Some(id) => self.render_thread(id, cx),
                None => self.render_list(cx),
            };
            div().flex().flex_col().gap(px(14.)).child(self.render_top(cx)).child(main)
        };
        let footer =
            div().flex().child(button("gc-close", ButtonKind::Ghost, tr!("Close")).on_click(cx.listener(|this, _, _, cx| this.close(cx))));
        let entity = cx.entity().downgrade();
        dialog(
            DialogProps { id: "game-community".into(), title: heading.into(), width: 640., height: Some(560.) },
            body.into_any_element(),
            Some(footer.into_any_element()),
            move |_, cx| {
                entity.update(cx, |this, cx| this.close(cx)).ok();
            },
        )
        .into_any_element()
    }
}

impl crate::console::PadTarget for GameCommunityDialog {
    fn on_pad(&mut self, pad: crate::console::Pad, _window: &mut Window, cx: &mut Context<Self>) -> bool {
        use crate::console::list::Outcome;
        let rows = self.console_rows(cx);
        match self.console_list.on_pad(pad, &rows) {
            Outcome::Activate(id) if id.as_ref() == "close" => self.close(cx),
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
