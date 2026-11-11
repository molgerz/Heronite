//! Settings → Community: the optional community layer on Nostr
//! (`heronite_nostr`). Off until the user turns it on; while off, nothing
//! connects and nothing is published.
//!
//! The service holds the identity (a key on this computer or a remote
//! signer), keeps the user's relays open (`heronite_nostr::Pool`) and turns
//! what they send into what the pages show: profiles, the Friends list,
//! friends' games, private messages and the discussions of the games open on
//! screen. What it publishes is signed off the UI thread (a remote signer may
//! wait for an approval on the phone).
//!
//! * `identity.rs`: creating, importing, pairing and forgetting.
//! * `events.rs`: the relays' events and what they change.
//! * `social.rs`: the profile, following, muting and reporting.
//! * `play.rs`: "playing" while a game runs.
//! * `share.rs`: sharing a screenshot or a replay.
//! * `threads.rs`: the discussions of a game.
//! * `chat.rs`: private messages.
//! * `secret.rs`: where the secret key is kept.

mod chat;
mod events;
mod identity;
mod play;
mod secret;
mod share;
mod social;
mod threads;

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;

use futures::channel::mpsc::unbounded;
use futures::StreamExt;
use gpui_kit::*;
use heronite_data::{AppPaths, DataPaths, Library};
use heronite_nostr::cache::Cache;
use heronite_nostr::game::GameRef;
use heronite_nostr::pool::RelayStatus;
use heronite_nostr::prefs::{Prefs, RelayPref};
use heronite_nostr::social::Profile;
use heronite_nostr::status::Playing;
use heronite_nostr::threads::Mutes;
use heronite_nostr::{EventBuilder, Kind, Pool, PoolEvent, PublicKey, Signer};

pub use chat::ChatKey;
pub use identity::{ImportProblem, Pairing};
pub use play::{game_ended, game_started};
pub use share::{ShareKind, ShareState};
pub use threads::GameThreads;

/// Wait after a cache change before writing it (events come in bursts).
const CACHE_SETTLE: Duration = Duration::from_secs(5);

/// Where the identity stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Account {
    /// No identity yet (or it was forgotten).
    None,
    /// Reading the key, or waiting for the phone to pair.
    Opening,
    Ready,
    /// The key could not be read or the pairing failed: the text says why.
    Failed(String),
}

pub struct Nostr {
    app_paths: AppPaths,
    prefs: Prefs,
    account: Account,
    signer: Option<Signer>,
    pool: Option<Pool>,
    relay_status: BTreeMap<String, RelayStatus>,
    cache: Cache,
    cache_save: Option<Task<()>>,
    /// Derived from the cache: everyone's newest profile.
    profiles: HashMap<PublicKey, Profile>,
    /// The people the user follows (their kind-3 list), in its order.
    friends: Vec<PublicKey>,
    mutes: Mutes,
    /// Friends' live "playing" statuses.
    playing: HashMap<PublicKey, Playing>,
    /// The games of the last library, by Collection key, for naming them.
    games: HashMap<String, GameRef>,
    play: play::State,
    /// Drops friends' statuses as they expire, while connected.
    expiry: Option<Task<()>>,
    chats: chat::State,
    share: ShareState,
    threads: HashMap<String, GameThreads>,
    pairing: identity::Pairing,
    /// A short message about the last action, for the Settings card
    /// (`true` = an error).
    notice: Option<(String, bool)>,
}

struct NostrGlobal(Entity<Nostr>);
impl Global for NostrGlobal {}

/// The service (created on first use; it connects when it is on).
pub fn get(cx: &mut App) -> Entity<Nostr> {
    if let Some(g) = cx.try_global::<NostrGlobal>() {
        return g.0.clone();
    }
    let app_paths = AppPaths::for_data(&DataPaths::detect());
    let prefs = Prefs::load(&app_paths);
    let cache = Cache::load(&app_paths);
    let entity = cx.new(|_| Nostr {
        app_paths,
        prefs,
        account: Account::None,
        signer: None,
        pool: None,
        relay_status: BTreeMap::new(),
        cache,
        cache_save: None,
        profiles: HashMap::new(),
        friends: Vec::new(),
        mutes: Mutes::default(),
        playing: HashMap::new(),
        games: HashMap::new(),
        play: play::State::default(),
        expiry: None,
        chats: chat::State::default(),
        share: ShareState::Idle,
        threads: HashMap::new(),
        pairing: identity::Pairing::None,
        notice: None,
    });
    cx.set_global(NostrGlobal(entity.clone()));
    entity.update(cx, |s, cx| {
        s.derive();
        s.open(cx);
    });
    entity
}

/// The service if something started it already (callers that must not
/// start it, such as menus drawn on every frame).
pub fn started(cx: &App) -> Option<Entity<Nostr>> {
    cx.try_global::<NostrGlobal>().map(|g| g.0.clone())
}

/// Whether this system offers the community layer at all.
pub fn available() -> bool {
    heronite_platform::supported(heronite_platform::Feature::Nostr)
}

/// The Collection loaded `lib`: remember how its games are named, and
/// start the service when it is on.
pub fn library_loaded(lib: &Library, cx: &mut App) {
    if !available() {
        return;
    }
    let games: HashMap<String, GameRef> = lib
        .games
        .iter()
        .map(|g| (g.key.clone(), GameRef { title: g.title.clone(), steam_app_id: g.steam_app_id.clone(), igdb_slug: None }))
        .collect();
    get(cx).update(cx, |s, cx| {
        s.games = games;
        cx.notify();
    });
}

impl Nostr {
    pub fn prefs(&self) -> &Prefs {
        &self.prefs
    }

    pub fn account(&self) -> &Account {
        &self.account
    }

    /// The user's public key, once there is an identity.
    pub fn me(&self) -> Option<PublicKey> {
        self.prefs.identity.as_ref().and_then(|i| PublicKey::from_hex(i.public_key()).ok())
    }

    pub fn is_remote(&self) -> bool {
        matches!(self.prefs.identity, Some(heronite_nostr::prefs::IdentityRef::Remote { .. }))
    }

    /// On, with an identity that signs.
    pub fn ready(&self) -> bool {
        self.prefs.enabled && self.account == Account::Ready
    }

    pub fn relay_status(&self) -> &BTreeMap<String, RelayStatus> {
        &self.relay_status
    }

    pub fn connected_relays(&self) -> usize {
        self.relay_status.values().filter(|s| **s == RelayStatus::Connected).count()
    }

    pub fn notice(&self) -> Option<&(String, bool)> {
        self.notice.as_ref()
    }

    /// What to call someone: their profile's name, else a short npub.
    pub fn label(&self, key: &PublicKey) -> String {
        self.profiles.get(key).cloned().unwrap_or_default().label(key)
    }

    pub fn friends(&self) -> &[PublicKey] {
        &self.friends
    }

    pub fn mutes(&self) -> &Mutes {
        &self.mutes
    }

    /// What a friend plays now, if they share it.
    pub fn playing(&self, key: &PublicKey) -> Option<&Playing> {
        self.playing.get(key)
    }

    /// Friends playing the game with this Collection key now.
    pub fn friends_playing(&self, game_key: &str) -> Vec<PublicKey> {
        let Some(ids) = self.games.get(game_key).map(GameRef::ids) else { return Vec::new() };
        self.friends
            .iter()
            .filter(|f| self.playing.get(f).is_some_and(|p| heronite_nostr::game::same_game(&p.ids, &ids)))
            .copied()
            .collect()
    }

    pub fn game(&self, key: &str) -> Option<&GameRef> {
        self.games.get(key)
    }

    /// Changes the settings and applies what changed: turning it on or off,
    /// the relays, sharing.
    pub fn set_prefs(&mut self, change: impl FnOnce(&mut Prefs), cx: &mut Context<Self>) {
        let before = self.prefs.clone();
        change(&mut self.prefs);
        self.save_prefs(cx);
        if before.enabled != self.prefs.enabled {
            if self.prefs.enabled {
                self.open(cx);
            } else {
                self.close(cx);
            }
        } else if before.relays != self.prefs.relays {
            self.apply_relays();
            self.publish_relay_lists(cx);
        }
        if before.share_playing && !self.prefs.share_playing {
            self.stop_playing(cx);
        }
        if before.keep_history && !self.prefs.keep_history {
            self.cache.clear_wraps();
            self.cache_changed(cx);
        }
        cx.notify();
    }

    /// Adds a relay typed by the user; `false` when it is not a relay address.
    pub fn add_relay(&mut self, input: &str, cx: &mut Context<Self>) -> bool {
        let Some(url) = heronite_nostr::relay::normalize_url(input) else { return false };
        if !self.prefs.relays.iter().any(|r| r.url == url) {
            self.set_prefs(|p| p.relays.push(RelayPref::new(url)), cx);
        }
        true
    }

    pub fn remove_relay(&mut self, url: &str, cx: &mut Context<Self>) {
        let url = url.to_string();
        self.set_prefs(|p| p.relays.retain(|r| r.url != url), cx);
    }

    pub fn reset_relays(&mut self, cx: &mut Context<Self>) {
        self.set_prefs(|p| p.relays = Prefs::default().relays, cx);
    }

    fn save_prefs(&mut self, cx: &mut Context<Self>) {
        if let Err(e) = self.prefs.save(&self.app_paths) {
            log::error!("saving the community settings: {e:#}");
        }
        cx.notify();
    }

    fn set_notice(&mut self, text: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        let text = text.into();
        if error {
            log::warn!("community: {text}");
        }
        self.notice = Some((text, error));
        cx.notify();
    }

    /// Reads the key and connects, when on and an identity exists.
    fn open(&mut self, cx: &mut Context<Self>) {
        if !available() || !self.prefs.enabled || self.prefs.identity.is_none() || self.account == Account::Opening {
            return;
        }
        if let Some(signer) = self.signer.clone() {
            self.start_pool(signer, cx);
            return;
        }
        self.account = Account::Opening;
        cx.notify();
        let read = secret::load(&self.prefs, cx);
        cx.spawn(async move |this, cx| {
            let result = read.await;
            this.update(cx, |s, cx| match result {
                Ok(Some(bytes)) => match s.signer_from_secret(&bytes) {
                    Some(signer) => {
                        s.signer = Some(signer.clone());
                        s.account = Account::Ready;
                        s.start_pool(signer, cx);
                    }
                    None => s.account = Account::Failed("the saved key is not a Nostr key".into()),
                },
                Ok(None) => s.account = Account::Failed("no key is saved on this computer".into()),
                Err(e) => s.account = Account::Failed(e),
            })
            .ok();
        })
        .detach();
    }

    /// Disconnects and forgets what came from the relays (the cache stays).
    fn close(&mut self, cx: &mut Context<Self>) {
        self.stop_playing(cx);
        if let Some(mut pool) = self.pool.take() {
            pool.stop();
        }
        self.relay_status.clear();
        self.expiry = None;
        self.playing.clear();
        self.threads.clear();
        if self.account != Account::None && self.prefs.identity.is_some() {
            // The key stays known: turning on again needs no keyring read.
            self.account = if self.signer.is_some() { Account::Ready } else { Account::None };
        }
        cx.notify();
    }

    fn start_pool(&mut self, signer: Signer, cx: &mut Context<Self>) {
        if !self.prefs.enabled {
            return;
        }
        let (pool, rx) = Pool::new();
        pool.set_signer(Some(signer));
        self.pool = Some(pool);
        self.apply_relays();
        self.subscribe_me();
        self.subscribe_friends();
        self.subscribe_inbox();
        self.watch_expiry(cx);
        self.open_wraps(Vec::new(), false, cx);
        // The pool's channel is a blocking one: a thread hands its events to
        // the UI thread, in bursts.
        let (tx, mut events) = unbounded::<PoolEvent>();
        std::thread::Builder::new()
            .name("nostr-events".into())
            .spawn(move || {
                for ev in rx {
                    if tx.unbounded_send(ev).is_err() {
                        break;
                    }
                }
            })
            .ok();
        cx.spawn(async move |this, cx| {
            while let Some(first) = events.next().await {
                let mut batch = vec![first];
                while let Ok(more) = events.try_recv() {
                    batch.push(more);
                }
                if this.update(cx, |s, cx| s.on_events(batch, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.notify();
    }

    fn apply_relays(&mut self) {
        let relays: Vec<(String, bool, bool)> = self.prefs.relays.iter().map(|r| (r.url.clone(), r.read, r.write)).collect();
        if let Some(pool) = self.pool.as_mut() {
            pool.set_relays(&relays);
        }
        let urls: HashSet<&String> = self.prefs.relays.iter().map(|r| &r.url).collect();
        self.relay_status.retain(|u, _| urls.contains(u));
    }

    /// Publishes the relay lists (NIP-65 and NIP-17's inbox), so others
    /// find where to read the user and where to send private messages.
    fn publish_relay_lists(&mut self, cx: &mut Context<Self>) {
        if !self.ready() {
            return;
        }
        self.publish(heronite_nostr::relays::relay_list(&self.prefs.relays), cx);
        let inbox = self.prefs.read_relays();
        if !inbox.is_empty() {
            self.publish(heronite_nostr::relays::inbox_list(&inbox), cx);
        }
    }

    /// Signs `builder` (off the UI thread) and sends it to the write relays;
    /// a replaceable event also goes into the cache at once.
    fn publish(&mut self, builder: EventBuilder, cx: &mut Context<Self>) {
        let Some(signer) = self.signer.clone() else { return };
        cx.spawn(async move |this, cx| {
            let signed = cx.background_spawn(async move { signer.sign(builder) }).await;
            this.update(cx, |s, cx| match signed {
                Ok(event) => {
                    if s.cache.put(event.clone()) {
                        s.derive();
                        s.cache_changed(cx);
                    }
                    if let Some(pool) = s.pool.as_ref() {
                        if pool.publish(&event) == 0 {
                            s.set_notice(heronite_i18n::tr!("No relay to publish to: add one in Settings → Community."), true, cx);
                        }
                    }
                    cx.notify();
                }
                Err(e) => s.set_notice(heronite_i18n::tr!("Not signed: {error}", error = e.to_string()), true, cx),
            })
            .ok();
        })
        .detach();
    }

    /// Profiles, friends and mutes from the cache.
    fn derive(&mut self) {
        self.profiles.clear();
        let me = self.me();
        let mut authors: Vec<PublicKey> = self.friends.clone();
        authors.extend(me);
        let mut friends = Vec::new();
        let mut mutes = Mutes::default();
        if let Some(me) = me {
            if let Some(list) = self.cache.latest(Kind::ContactList, &me) {
                friends = heronite_nostr::social::follows(list);
            }
            if let Some(list) = self.cache.latest(Kind::MuteList, &me) {
                mutes = Mutes::from_event(list);
            }
        }
        authors.extend(friends.iter().copied());
        for a in authors {
            if let Some(p) = self.cache.latest(Kind::Metadata, &a).and_then(heronite_nostr::social::Profile::from_event) {
                self.profiles.insert(a, p);
            }
        }
        let changed = friends != self.friends;
        self.friends = friends;
        self.mutes = mutes;
        self.playing.retain(|k, _| self.friends.contains(k));
        if changed {
            self.subscribe_friends();
        }
    }

    /// Writes the cache a few seconds after the last change.
    fn cache_changed(&mut self, cx: &mut Context<Self>) {
        self.cache_save = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CACHE_SETTLE).await;
            let Ok((cache, paths, keep)) = this.update(cx, |s, _| (s.cache.clone(), s.app_paths.clone(), s.prefs.keep_history)) else {
                return;
            };
            cx.background_spawn(async move {
                let mut cache = cache;
                if !keep {
                    cache.clear_wraps();
                }
                if let Err(e) = cache.save(&paths) {
                    log::warn!("saving the community cache: {e:#}");
                }
            })
            .await;
        }));
    }

    fn subscribe_me(&self) {
        let (Some(pool), Some(me)) = (self.pool.as_ref(), self.me()) else { return };
        let kinds = [Kind::Metadata, Kind::ContactList, Kind::MuteList, Kind::RelayList, Kind::InboxRelays];
        pool.subscribe("me", vec![heronite_nostr::Filter::new().author(me).kinds(kinds)]);
    }

    fn subscribe_friends(&self) {
        let Some(pool) = self.pool.as_ref() else { return };
        if self.friends.is_empty() {
            pool.unsubscribe("friends");
            return;
        }
        let authors = self.friends.iter().copied();
        let profiles = heronite_nostr::Filter::new().authors(authors.clone()).kind(Kind::Metadata);
        let statuses = heronite_nostr::Filter::new().authors(authors).kind(Kind::UserStatus).identifier(heronite_nostr::status::TYPE);
        pool.subscribe("friends", vec![profiles, statuses]);
    }
}
