//! Settings → Community, kept in `AppPaths::nostr_prefs()` (`nostr.json`):
//! they belong to this computer, not to the library.
//!
//! - `enabled`: off until the user turns it on. While off, nothing connects.
//! - `identity`: `null`, or which key signs:
//!   - `{"kind": "local", "publicKey": <hex>, "secret": <base64>}`: the key
//!     is on this computer. `secret` is the key sealed with DPAPI for this
//!     Windows user; other systems keep it in the desktop's keyring and
//!     leave it empty.
//!   - `{"kind": "remote", "publicKey": <hex>, "signer": <hex>,
//!     "relays": [..], "secret": <base64>}`: a remote signer (NIP-46).
//!     `publicKey` is the user's, `signer` the remote signer's; `secret`
//!     is this app's own client key for that session, kept like a local key.
//! - `relays`: `[{"url", "read", "write"}]`, the user's relays.
//! - `sharePlaying`: publish what the user plays (off by default).
//! - `neverShare`: Collection keys of games whose play is never published.
//! - `mediaServers`: Blossom servers shared files go to, first one first.
//! - `notifyMessages`: a desktop notification for a new private message.
//! - `notifyWhilePlaying`: also while a game runs.
//! - `keepHistory`: keep private messages on disk (encrypted); otherwise they
//!   are read from the relays again at every start.

use anyhow::Result;
use base64::Engine;
use heronite_data::json_store;
use heronite_data::paths::AppPaths;
use serde_json::{json, Value};

/// One relay of the user's list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelayPref {
    /// `wss://…` (or `ws://` for one on this computer).
    pub url: String,
    /// Events of others are read from it.
    pub read: bool,
    /// The user's events are published to it.
    pub write: bool,
}

impl RelayPref {
    pub fn new(url: impl Into<String>) -> Self {
        Self { url: url.into(), read: true, write: true }
    }
}

/// Which key signs for the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityRef {
    /// The secret key is on this computer.
    Local {
        /// Hex.
        public_key: String,
        /// The DPAPI-sealed secret key, base64 (Windows); empty elsewhere.
        sealed_secret: String,
    },
    /// A remote signer holds the key (NIP-46).
    Remote {
        /// The user's public key, hex.
        public_key: String,
        /// The remote signer's public key, hex.
        signer: String,
        /// Where the remote signer listens.
        relays: Vec<String>,
        /// This app's client key for the session, sealed like a local key.
        sealed_secret: String,
    },
}

impl IdentityRef {
    pub fn public_key(&self) -> &str {
        match self {
            IdentityRef::Local { public_key, .. } | IdentityRef::Remote { public_key, .. } => public_key,
        }
    }

    pub fn sealed_secret(&self) -> &str {
        match self {
            IdentityRef::Local { sealed_secret, .. } | IdentityRef::Remote { sealed_secret, .. } => sealed_secret,
        }
    }

    pub fn set_sealed_secret(&mut self, sealed: String) {
        match self {
            IdentityRef::Local { sealed_secret, .. } | IdentityRef::Remote { sealed_secret, .. } => *sealed_secret = sealed,
        }
    }

    fn to_json(&self) -> Value {
        match self {
            IdentityRef::Local { public_key, sealed_secret } => {
                json!({"kind": "local", "publicKey": public_key, "secret": sealed_secret})
            }
            IdentityRef::Remote { public_key, signer, relays, sealed_secret } => json!({
                "kind": "remote",
                "publicKey": public_key,
                "signer": signer,
                "relays": relays,
                "secret": sealed_secret,
            }),
        }
    }

    fn from_json(v: &Value) -> Option<Self> {
        let text = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        let public_key = text("publicKey").filter(|k| k.len() == 64)?;
        let sealed_secret = text("secret").unwrap_or_default();
        match v.get("kind").and_then(Value::as_str)? {
            "local" => Some(IdentityRef::Local { public_key, sealed_secret }),
            "remote" => Some(IdentityRef::Remote {
                public_key,
                signer: text("signer").filter(|k| k.len() == 64)?,
                relays: strings(v.get("relays")),
                sealed_secret,
            }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prefs {
    pub enabled: bool,
    pub identity: Option<IdentityRef>,
    pub relays: Vec<RelayPref>,
    pub share_playing: bool,
    pub never_share: Vec<String>,
    pub media_servers: Vec<String>,
    pub notify_messages: bool,
    pub notify_while_playing: bool,
    pub keep_history: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            enabled: false,
            identity: None,
            relays: crate::DEFAULT_RELAYS.iter().map(|u| RelayPref::new(*u)).collect(),
            share_playing: false,
            never_share: Vec::new(),
            media_servers: crate::DEFAULT_MEDIA_SERVERS.iter().map(|s| s.to_string()).collect(),
            notify_messages: true,
            notify_while_playing: false,
            keep_history: false,
        }
    }
}

impl Prefs {
    /// A missing or invalid file (or key) gives the defaults.
    pub fn load(paths: &AppPaths) -> Self {
        Self::from_value(&json_store::read_value(&paths.nostr_prefs()).unwrap_or(Value::Null))
    }

    fn from_value(v: &Value) -> Self {
        let d = Self::default();
        let flag = |k: &str, default: bool| v.get(k).and_then(Value::as_bool).unwrap_or(default);
        let relays = match v.get("relays").and_then(Value::as_array) {
            Some(list) => list
                .iter()
                .filter_map(|r| {
                    let url = r.get("url").and_then(Value::as_str)?.trim();
                    (!url.is_empty()).then(|| RelayPref {
                        url: url.to_string(),
                        read: r.get("read").and_then(Value::as_bool).unwrap_or(true),
                        write: r.get("write").and_then(Value::as_bool).unwrap_or(true),
                    })
                })
                .collect(),
            None => d.relays,
        };
        Self {
            enabled: flag("enabled", d.enabled),
            identity: v.get("identity").and_then(IdentityRef::from_json),
            relays,
            share_playing: flag("sharePlaying", d.share_playing),
            never_share: strings(v.get("neverShare")),
            media_servers: v.get("mediaServers").map(|s| strings(Some(s))).unwrap_or(d.media_servers),
            notify_messages: flag("notifyMessages", d.notify_messages),
            notify_while_playing: flag("notifyWhilePlaying", d.notify_while_playing),
            keep_history: flag("keepHistory", d.keep_history),
        }
    }

    /// Writes atomically, keeping keys this struct does not model.
    pub fn save(&self, paths: &AppPaths) -> Result<()> {
        json_store::update_value(&paths.nostr_prefs(), |doc| {
            if !doc.is_object() {
                *doc = json!({});
            }
            doc["enabled"] = json!(self.enabled);
            doc["identity"] = self.identity.as_ref().map_or(Value::Null, IdentityRef::to_json);
            doc["relays"] = self.relays.iter().map(|r| json!({"url": r.url, "read": r.read, "write": r.write})).collect::<Vec<_>>().into();
            doc["sharePlaying"] = json!(self.share_playing);
            doc["neverShare"] = json!(self.never_share);
            doc["mediaServers"] = json!(self.media_servers);
            doc["notifyMessages"] = json!(self.notify_messages);
            doc["notifyWhilePlaying"] = json!(self.notify_while_playing);
            doc["keepHistory"] = json!(self.keep_history);
            Ok(())
        })
    }

    /// The URLs events are read from.
    pub fn read_relays(&self) -> Vec<String> {
        self.relays.iter().filter(|r| r.read).map(|r| r.url.clone()).collect()
    }

    /// The URLs the user's events go to.
    pub fn write_relays(&self) -> Vec<String> {
        self.relays.iter().filter(|r| r.write).map(|r| r.url.clone()).collect()
    }

    /// Whether playing the game with this Collection key may be published.
    pub fn may_share(&self, game_key: &str) -> bool {
        self.share_playing && !self.never_share.iter().any(|k| k == game_key)
    }
}

fn strings(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).filter(|s| !s.trim().is_empty()).collect())
        .unwrap_or_default()
}

/// `secret` sealed for this Windows user, as stored in `secret`. Fails on
/// systems without DPAPI: nothing secret is ever kept in the clear.
pub fn seal(secret: &[u8]) -> std::io::Result<String> {
    let sealed = heronite_platform::secret::protect(secret)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(sealed))
}

/// What [`seal`] made, when this user made it on this computer.
pub fn unseal(stored: &str) -> Option<Vec<u8>> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(stored.trim()).ok()?;
    heronite_platform::secret::unprotect(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(dir: &std::path::Path) -> AppPaths {
        AppPaths { config: dir.join("config"), cache: dir.join("cache") }
    }

    const PK: &str = "aa4fc8665f5696e33db7e1a572e3b0f5b3d615837b0f362dcb1c8068b098c7b4";

    #[test]
    fn defaults_are_off_with_public_relays() {
        let dir = tempfile::tempdir().unwrap();
        let p = Prefs::load(&paths(dir.path()));
        assert_eq!(p, Prefs::default());
        assert!(!p.enabled && !p.share_playing && p.identity.is_none());
        assert_eq!(p.read_relays().len(), crate::DEFAULT_RELAYS.len());
    }

    #[test]
    fn round_trip_keeps_unknown_keys() {
        let dir = tempfile::tempdir().unwrap();
        let p = paths(dir.path());
        std::fs::create_dir_all(&p.config).unwrap();
        std::fs::write(p.nostr_prefs(), r#"{"other": 1, "identity": {"kind": "nope"}}"#).unwrap();
        assert_eq!(Prefs::load(&p).identity, None);
        let prefs = Prefs {
            enabled: true,
            identity: Some(IdentityRef::Remote {
                public_key: PK.into(),
                signer: PK.into(),
                relays: vec!["wss://relay.example".into()],
                sealed_secret: "c2VhbGVk".into(),
            }),
            relays: vec![RelayPref { url: "wss://a.example".into(), read: true, write: false }],
            share_playing: true,
            never_share: vec!["steam_1".into()],
            media_servers: vec![],
            notify_messages: false,
            notify_while_playing: true,
            keep_history: true,
        };
        prefs.save(&p).unwrap();
        assert_eq!(Prefs::load(&p), prefs);
        assert_eq!(json_store::read_value(&p.nostr_prefs()).unwrap()["other"], 1);
        assert_eq!(prefs.write_relays(), Vec::<String>::new());
        assert!(!prefs.may_share("steam_1"));
        assert!(prefs.may_share("gog_2"));
    }

    #[test]
    fn a_local_identity_needs_a_whole_public_key() {
        let v = json!({"identity": {"kind": "local", "publicKey": "abc"}});
        assert_eq!(Prefs::from_value(&v).identity, None);
        let v = json!({"identity": {"kind": "local", "publicKey": PK}});
        assert_eq!(Prefs::from_value(&v).identity.unwrap().public_key(), PK);
    }

    #[cfg(not(windows))]
    #[test]
    fn sealing_fails_without_dpapi() {
        assert!(seal(b"x").is_err());
    }
}
