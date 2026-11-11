//! What the relays send: relay status, the user's and friends' replaceable
//! events, friends' games, gift wraps and the discussions of open games.

use gpui_kit::*;
use heronite_nostr::pool::RelayStatus;
use heronite_nostr::{Event, Kind, PoolEvent, Timestamp};

use super::Nostr;

/// How often friends' statuses are checked for expiry.
const EXPIRY_CHECK: std::time::Duration = std::time::Duration::from_secs(30);

impl Nostr {
    pub(super) fn on_events(&mut self, batch: Vec<PoolEvent>, cx: &mut Context<Self>) {
        let mut derive = false;
        let mut wraps = Vec::new();
        for ev in batch {
            match ev {
                PoolEvent::Status { relay, status } => {
                    if let RelayStatus::Failed(e) = &status {
                        log::debug!("relay {relay}: {e}");
                    }
                    self.relay_status.insert(relay, status);
                }
                PoolEvent::Event { subscription, event, .. } => match event.kind {
                    Kind::GiftWrap => wraps.push(*event),
                    Kind::UserStatus => self.on_status(&event),
                    k if k == heronite_nostr::threads::THREAD || k == Kind::Comment => self.on_discussion(&subscription, *event),
                    _ => {
                        if self.cache.put(*event) {
                            derive = true;
                        }
                    }
                },
                PoolEvent::EndOfStored { subscription, .. } => self.on_end_of_stored(&subscription),
                PoolEvent::Published { relay, accepted, message, .. } => {
                    if !accepted {
                        log::info!("relay {relay} refused an event: {message}");
                    }
                }
            }
        }
        if derive {
            self.derive();
            self.cache_changed(cx);
        }
        if !wraps.is_empty() {
            self.open_wraps(wraps, true, cx);
        }
        cx.notify();
    }

    fn on_status(&mut self, event: &Event) {
        if !self.friends.contains(&event.pubkey) {
            return;
        }
        // A newer status replaces the older one; an empty or expired one
        // clears it.
        if self.playing.get(&event.pubkey).is_some_and(|p| p.since > event.created_at) {
            return;
        }
        match heronite_nostr::status::read(event, Timestamp::now()) {
            Some(p) => {
                self.playing.insert(event.pubkey, p);
            }
            None => {
                self.playing.remove(&event.pubkey);
            }
        }
    }

    /// Drops statuses that expired since they came (a friend's computer
    /// went off without clearing); `true` when one went.
    pub(super) fn expire_statuses(&mut self) -> bool {
        let now = Timestamp::now();
        let before = self.playing.len();
        self.playing.retain(|_, p| p.live_at(now));
        self.playing.len() != before
    }

    /// Checks friends' statuses for expiry every [`EXPIRY_CHECK`] while
    /// connected (relays do not say when a status ends on its own).
    pub(super) fn watch_expiry(&mut self, cx: &mut Context<Self>) {
        self.expiry = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(EXPIRY_CHECK).await;
            let alive = this
                .update(cx, |s, cx| {
                    if s.expire_statuses() {
                        cx.notify();
                    }
                })
                .is_ok();
            if !alive {
                break;
            }
        }));
    }
}
