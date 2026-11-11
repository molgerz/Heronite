//! The discussions of the games open on screen: a game's page subscribes
//! while it shows its Community tab, and a thread its comments while open.

use std::collections::HashMap;

use gpui_kit::*;
use heronite_nostr::threads::{self, Comment, Thread};
use heronite_nostr::{Event, EventId};

use super::Nostr;

/// Threads shown at most per game.
const MAX_THREADS: usize = 100;

#[derive(Debug, Clone, Default)]
pub struct GameThreads {
    /// Newest first.
    pub threads: Vec<Thread>,
    /// By thread, oldest first.
    pub comments: HashMap<EventId, Vec<Comment>>,
    /// The relays sent their stored threads (no more "Loading…").
    pub loaded: bool,
}

fn sub_threads(key: &str) -> String {
    format!("threads:{key}")
}

fn sub_comments(key: &str) -> String {
    format!("comments:{key}")
}

impl Nostr {
    pub fn game_threads(&self, key: &str) -> Option<&GameThreads> {
        self.threads.get(key)
    }

    /// Starts reading the discussions of the game with this Collection key.
    pub fn open_game(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(game) = self.games.get(key).cloned() else { return };
        if !self.ready() || self.threads.contains_key(key) {
            return;
        }
        if let Some(pool) = self.pool.as_ref() {
            pool.subscribe(&sub_threads(key), vec![threads::threads_filter(&game, MAX_THREADS)]);
        }
        self.threads.insert(key.to_string(), GameThreads::default());
        cx.notify();
    }

    pub fn close_game(&mut self, key: &str) {
        if self.threads.remove(key).is_some() {
            if let Some(pool) = self.pool.as_ref() {
                pool.unsubscribe(&sub_threads(key));
                pool.unsubscribe(&sub_comments(key));
            }
        }
    }

    /// Reads the comments of `thread` (one open thread per game).
    pub fn open_thread(&mut self, key: &str, thread: &EventId) {
        if let Some(pool) = self.pool.as_ref() {
            pool.subscribe(&sub_comments(key), vec![threads::comments_filter(thread)]);
        }
    }

    pub fn post_thread(&mut self, key: &str, title: &str, body: &str, cx: &mut Context<Self>) {
        let Some(game) = self.games.get(key).cloned() else { return };
        self.publish_discussion(key, threads::new_thread(&game, title, body), cx);
    }

    pub fn post_comment(&mut self, key: &str, thread: &Thread, parent: Option<&Comment>, body: &str, cx: &mut Context<Self>) {
        self.publish_discussion(key, threads::new_comment(thread, parent, body), cx);
    }

    /// Like `publish`, and shows the post at once (the relays echo it to
    /// the subscription later; it is not added twice).
    fn publish_discussion(&mut self, key: &str, builder: heronite_nostr::EventBuilder, cx: &mut Context<Self>) {
        let Some(signer) = self.signer.clone() else { return };
        let key = key.to_string();
        cx.spawn(async move |this, cx| {
            let signed = cx.background_spawn(async move { signer.sign(builder) }).await;
            this.update(cx, |s, cx| match signed {
                Ok(event) => {
                    s.add_discussion(&key, event.clone());
                    if let Some(pool) = s.pool.as_ref() {
                        pool.publish(&event);
                    }
                    cx.notify();
                }
                Err(e) => s.set_notice(heronite_i18n::tr!("Not signed: {error}", error = e.to_string()), true, cx),
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn on_discussion(&mut self, subscription: &str, event: Event) {
        let key = subscription.split_once(':').map(|(_, k)| k.to_string()).unwrap_or_default();
        self.add_discussion(&key, event);
    }

    fn add_discussion(&mut self, key: &str, event: Event) {
        let Some(game) = self.threads.get_mut(key) else { return };
        if let Some(t) = threads::read_thread(&event) {
            if !game.threads.iter().any(|x| x.id == t.id) {
                game.threads.push(t);
                game.threads.sort_by_key(|t| std::cmp::Reverse(t.at));
                game.threads.truncate(MAX_THREADS);
            }
        } else if let Some(c) = threads::read_comment(&event) {
            let list = game.comments.entry(c.root).or_default();
            if !list.iter().any(|x| x.id == c.id) {
                list.push(c);
                list.sort_by_key(|c| c.at);
            }
        }
    }

    pub(super) fn on_end_of_stored(&mut self, subscription: &str) {
        if let Some(key) = subscription.strip_prefix("threads:") {
            if let Some(g) = self.threads.get_mut(key) {
                g.loaded = true;
            }
        }
    }
}
