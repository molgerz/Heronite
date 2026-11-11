//! Heronite's optional community layer on Nostr, an open protocol: the
//! identity is a key pair the user holds, messages travel through relays
//! anyone can run, and other Nostr apps read the same events. Off until the
//! user turns it on in Settings → Community.
//!
//! * [`prefs`]: Settings → Community (`nostr.json` in the app state).
//! * [`identity`]: creating, importing and exporting the key pair (NIP-19,
//!   NIP-49), sealed for this Windows user like the other secrets.
//! * [`relay`]: one relay over a blocking WebSocket; [`pool`]: every relay,
//!   each on its own thread, with reconnects.
//! * [`signer`]: what signs and encrypts: the key on this computer or a
//!   remote signer ([`remote`], NIP-46, a phone app reached by QR code).
//! * [`social`]: the profile (NIP-01), the follow list (NIP-02) and NIP-05
//!   addresses.
//! * [`relays`]: the user's relay lists (NIP-65, NIP-17's inbox relays).
//! * [`game`]: how a game is named on Nostr, the same from any store.
//! * [`status`]: what the user plays, as a user status (NIP-38).
//! * [`media`]: sharing screenshots and replays (Blossom, NIP-68, NIP-71).
//! * [`threads`]: discussions per game (NIP-7D, NIP-22), the mute list
//!   (NIP-51) and reports (NIP-56).
//! * [`chat`]: private messages (NIP-17, sealed and gift-wrapped by NIP-59).
//! * [`cache`]: what was read last, for the next start.
//!
//! Every function that touches the network blocks: call them off the UI
//! thread. Nothing here logs a secret key, a decrypted message or a
//! remote signer's connection secret.

pub mod cache;
pub mod chat;
pub mod game;
pub mod identity;
pub mod media;
pub mod pool;
pub mod prefs;
pub mod relay;
pub mod relays;
pub mod remote;
pub mod signer;
pub mod social;
pub mod status;
pub mod threads;

#[cfg(test)]
mod testing;

pub use nostr::prelude::{Event, EventBuilder, EventId, Filter, Keys, Kind, PublicKey, Tag, Timestamp};
pub use pool::{Pool, PoolEvent};
pub use signer::Signer;

/// What Heronite calls itself to relays, media servers and NIP-05 hosts.
pub const USER_AGENT: &str = concat!("Heronite/", env!("CARGO_PKG_VERSION"));

/// The relays a new identity starts with: large, free, general-purpose
/// ones. The user can change the list in Settings → Community.
pub const DEFAULT_RELAYS: [&str; 3] = ["wss://relay.damus.io", "wss://nos.lol", "wss://relay.primal.net"];

/// Where shared screenshots and replays are uploaded unless the user picks
/// other Blossom servers.
pub const DEFAULT_MEDIA_SERVERS: [&str; 1] = ["https://blossom.primal.net"];
