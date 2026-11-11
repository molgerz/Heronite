//! Sharing a screenshot or a replay: always the user's explicit action,
//! with a caption. The file (without its metadata) goes to the first Blossom
//! server that takes it, then a picture or video event points at it, or a
//! private message to one friend does.

use std::path::PathBuf;

use gpui_kit::*;
use heronite_i18n::tr;
use heronite_nostr::game::GameRef;
use heronite_nostr::media::{self, Blob};
use heronite_nostr::PublicKey;

use super::Nostr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareKind {
    Screenshot,
    Replay,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareState {
    Idle,
    Uploading,
    /// Shared; the URL of the file.
    Done(String),
    Failed(String),
}

impl Nostr {
    pub fn share_state(&self) -> &ShareState {
        &self.share
    }

    pub fn reset_share(&mut self, cx: &mut Context<Self>) {
        if self.share != ShareState::Uploading {
            self.share = ShareState::Idle;
            cx.notify();
        }
    }

    /// Uploads `path` and publishes it about `game`, or sends it to `to`
    /// alone. A failed upload publishes nothing.
    pub fn share(&mut self, path: PathBuf, kind: ShareKind, game: GameRef, caption: String, to: Option<PublicKey>, cx: &mut Context<Self>) {
        let Some(signer) = self.signer.clone().filter(|_| self.ready()) else { return };
        if self.share == ShareState::Uploading {
            return;
        }
        let servers = self.prefs.media_servers.clone();
        if servers.is_empty() {
            self.share = ShareState::Failed(tr!("Add a media server in Settings → Community first.").into());
            cx.notify();
            return;
        }
        self.share = ShareState::Uploading;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let up_signer = signer.clone();
            let uploaded = cx.background_spawn(async move { upload(&path, &servers, &up_signer) }).await;
            let blob = match uploaded {
                Ok(blob) => blob,
                Err(e) => {
                    this.update(cx, |s, cx| {
                        s.share = ShareState::Failed(e);
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            this.update(cx, |s, cx| {
                match to {
                    Some(friend) => {
                        let text = if caption.trim().is_empty() { blob.url.clone() } else { format!("{}\n{}", caption.trim(), blob.url) };
                        s.send_message(vec![friend], text, None, cx);
                    }
                    None => {
                        let builder = match kind {
                            ShareKind::Screenshot => media::picture(&blob, &caption, &game, None),
                            ShareKind::Replay => media::video(&blob, &caption, &game, None),
                        };
                        s.publish(builder, cx);
                    }
                }
                s.share = ShareState::Done(blob.url);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

/// Reads, strips and uploads the file to the first server that takes it.
fn upload(path: &std::path::Path, servers: &[String], signer: &heronite_nostr::Signer) -> Result<Blob, String> {
    let mime = media::mime_for(path).ok_or_else(|| tr!("This kind of file cannot be shared.").to_string())?;
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let bytes = media::strip_metadata(bytes, mime);
    let mut last = String::new();
    for server in servers {
        match media::upload(server, &bytes, mime, signer) {
            Ok(blob) => return Ok(blob),
            Err(e) => {
                log::info!("sharing to {server}: {e:#}");
                last = format!("{e:#}");
            }
        }
    }
    Err(tr!("No media server took the file: {error}", error = last.as_str()))
}
