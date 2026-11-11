//! Where the Nostr secret key lives (or, with a remote signer, this app's
//! client key for the session), never in the clear: on Windows sealed with
//! DPAPI for this user in `nostr.json` (`heronite_nostr::prefs`); elsewhere
//! in the desktop's keyring (the Secret Service, through GPUI's credentials
//! API), one item per identity, so Heronites with other data folders
//! (`HERONITE_DATA_DIR`) keep their own. Neither the key nor its sealed form
//! is logged.

use gpui_kit::*;
use heronite_nostr::prefs::Prefs;

/// The keyring item of versions that kept one for every identity (Linux):
/// read once, moved to the identity's own item when it is that identity's.
#[cfg_attr(windows, allow(dead_code))]
const LEGACY_KEY: &str = "heronite://nostr";

/// The keyring item of the identity with this public key (Linux).
#[cfg_attr(windows, allow(dead_code))]
fn key(public_key: &str) -> String {
    format!("{LEGACY_KEY}/{public_key}")
}

/// The kept secret bytes, if any.
pub fn load(prefs: &Prefs, cx: &App) -> Task<Result<Option<Vec<u8>>, String>> {
    #[cfg(windows)]
    {
        let _ = cx;
        let sealed = prefs.identity.as_ref().map(|i| i.sealed_secret().to_string()).unwrap_or_default();
        if sealed.is_empty() {
            return Task::ready(Ok(None));
        }
        Task::ready(heronite_nostr::prefs::unseal(&sealed).map(Some).ok_or_else(|| "the saved key cannot be opened".to_string()))
    }
    #[cfg(not(windows))]
    {
        let Some(user) = prefs.identity.as_ref().map(|i| i.public_key().to_string()) else { return Task::ready(Ok(None)) };
        let read = cx.read_credentials(&key(&user));
        cx.spawn(async move |cx| {
            if let Some((_, bytes)) = read.await.map_err(|e| format!("{e:#}"))? {
                return Ok(Some(bytes));
            }
            // Kept by an older version, in the one item for every identity.
            let legacy = cx.update(|cx| cx.read_credentials(LEGACY_KEY));
            let Some((owner, bytes)) = legacy.await.map_err(|e| format!("{e:#}"))? else { return Ok(None) };
            if owner != user {
                return Ok(None);
            }
            let write = cx.update(|cx| cx.write_credentials(&key(&user), &user, &bytes));
            match write.await {
                Ok(()) => {
                    let delete = cx.update(|cx| cx.delete_credentials(LEGACY_KEY));
                    if let Err(e) = delete.await {
                        log::warn!("removing the old Nostr keyring item: {e:#}");
                    }
                }
                Err(e) => log::warn!("moving the Nostr key to its own keyring item: {e:#}"),
            }
            Ok(Some(bytes))
        })
    }
}

/// Keeps `secret` for the identity in `prefs`. On Windows the sealed form
/// goes into `prefs` (the caller saves them); elsewhere into the keyring.
pub fn store(prefs: &mut Prefs, secret: &[u8], cx: &App) -> Task<Result<(), String>> {
    #[cfg(windows)]
    {
        let _ = cx;
        let sealed = match heronite_nostr::prefs::seal(secret) {
            Ok(s) => s,
            Err(e) => return Task::ready(Err(e.to_string())),
        };
        match prefs.identity.as_mut() {
            Some(identity) => {
                identity.set_sealed_secret(sealed);
                Task::ready(Ok(()))
            }
            None => Task::ready(Err("no identity to keep a key for".into())),
        }
    }
    #[cfg(not(windows))]
    {
        if let Some(identity) = prefs.identity.as_mut() {
            identity.set_sealed_secret(String::new());
        }
        let Some(user) = prefs.identity.as_ref().map(|i| i.public_key().to_string()) else {
            return Task::ready(Err("no identity to keep a key for".into()));
        };
        let write = cx.write_credentials(&key(&user), &user, secret);
        cx.background_spawn(async move { write.await.map_err(|e| format!("{e:#}")) })
    }
}

/// Forgets the key kept for the identity with this public key (the caller
/// clears the identity in the prefs).
pub fn forget(public_key: &str, cx: &App) -> Task<()> {
    #[cfg(windows)]
    {
        let _ = (public_key, cx);
        Task::ready(())
    }
    #[cfg(not(windows))]
    {
        let user = public_key.to_string();
        let delete = cx.delete_credentials(&key(&user));
        let legacy = cx.read_credentials(LEGACY_KEY);
        cx.spawn(async move |cx| {
            if let Err(e) = delete.await {
                log::warn!("forgetting the Nostr key in the keyring: {e:#}");
            }
            // Not moved yet (kept by an older version and never opened since).
            if let Ok(Some((owner, _))) = legacy.await {
                if owner == user {
                    let _ = cx.update(|cx| cx.delete_credentials(LEGACY_KEY)).await;
                }
            }
        })
    }
}
