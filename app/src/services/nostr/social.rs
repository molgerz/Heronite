//! The user's profile, following, muting and reporting.

use gpui_kit::*;
use heronite_i18n::tr;
use heronite_nostr::social::{self, Profile};
use heronite_nostr::threads::{self, Reason, Target};
use heronite_nostr::{Kind, PublicKey};

use super::Nostr;

impl Nostr {
    /// The user's profile as last seen (empty before the first one).
    pub fn my_profile(&self) -> Profile {
        self.me().and_then(|me| self.profiles.get(&me).cloned()).unwrap_or_default()
    }

    /// Publishes the edited profile.
    pub fn save_profile(&mut self, profile: Profile, cx: &mut Context<Self>) {
        if let Some(me) = self.me() {
            self.profiles.insert(me, profile.clone());
        }
        self.publish(profile.to_builder(), cx);
        self.set_notice(tr!("Profile saved."), false, cx);
    }

    /// Follows someone given as `npub1…`, `nprofile1…`, hex or a NIP-05
    /// address (`name@domain`, looked up first). `done` gets an error text
    /// when it did not work. It always runs later, never inside this call:
    /// the caller is usually a view in the middle of its own update, which
    /// `done` updates again (GPUI aborts on a nested update of an entity).
    pub fn follow(&mut self, input: String, cx: &mut Context<Self>, done: impl FnOnce(Option<String>, &mut App) + 'static) {
        let input = input.trim().to_string();
        if let Some(key) = heronite_nostr::identity::parse_public_key(&input) {
            let problem = self.change_follow(key, true, cx);
            cx.defer(move |cx| done(problem, cx));
            return;
        }
        if social::parse_nip05(&input).is_none() {
            let problem = Some(tr!("Paste an npub1… key or a name@domain address.").into());
            cx.defer(move |cx| done(problem, cx));
            return;
        }
        cx.spawn(async move |this, cx| {
            let address = input.clone();
            let found = cx.background_spawn(async move { social::resolve_nip05(&address) }).await;
            let problem = match found {
                Ok(key) => this.update(cx, |s, cx| s.change_follow(key, true, cx)).ok().flatten(),
                Err(e) => Some(tr!("{address} was not found: {error}", address = input.as_str(), error = e.to_string())),
            };
            cx.update(|cx| done(problem, cx));
        })
        .detach();
    }

    pub fn unfollow(&mut self, key: PublicKey, cx: &mut Context<Self>) {
        self.change_follow(key, false, cx);
    }

    /// `None` when the change went out.
    fn change_follow(&mut self, key: PublicKey, follow: bool, cx: &mut Context<Self>) -> Option<String> {
        let me = self.me()?;
        if key == me {
            return Some(tr!("That is your own key.").into());
        }
        if follow == self.friends.contains(&key) {
            return None;
        }
        // Built on the newest list, so another app's follows are kept.
        let builder = social::change_follow(self.cache.latest(Kind::ContactList, &me), &key, follow);
        if follow {
            self.friends.push(key);
        } else {
            self.friends.retain(|f| *f != key);
            self.playing.remove(&key);
        }
        self.subscribe_friends();
        self.publish(builder, cx);
        cx.notify();
        None
    }

    pub fn set_muted(&mut self, target: Target, mute: bool, cx: &mut Context<Self>) {
        let Some(me) = self.me() else { return };
        let builder = threads::change_mute(self.cache.latest(Kind::MuteList, &me), target, mute);
        match target {
            Target::Person(pk) if mute => self.mutes.people.push(pk),
            Target::Person(pk) => self.mutes.people.retain(|p| *p != pk),
            Target::Post(id, _) if mute => self.mutes.events.push(id),
            Target::Post(id, _) => self.mutes.events.retain(|e| *e != id),
        }
        self.publish(builder, cx);
        cx.notify();
    }

    pub fn report(&mut self, target: Target, reason: Reason, note: String, cx: &mut Context<Self>) {
        self.publish(threads::report(target, reason, &note), cx);
        self.set_notice(tr!("Reported. Thank you."), false, cx);
    }
}
