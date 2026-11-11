//! Sharing a screenshot or a replay: the file goes to a Blossom server
//! (blobs addressed by their SHA-256, BUD-01/02), then a picture event
//! (NIP-68, kind 20) or a video event (NIP-71, kind 21) points at it, with
//! the game's names ([`crate::game`]).
//!
//! Before upload, [`strip_metadata`] removes what should not leave the
//! computer: text chunks (which can hold file paths or the user name),
//! EXIF and XMP. A replay's MP4 is sent as it is.

use std::time::Duration;

use base64::Engine;
use nostr::prelude::{Event, EventBuilder, Kind, Tag, Timestamp};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::game::GameRef;
use crate::signer::Signer;

/// Uploads bigger than this are refused before they start: most free
/// servers take less, and a replay longer than that is not a clip.
pub const MAX_UPLOAD: usize = 100 << 20;
/// NIP-68 picture event.
pub const PICTURE: Kind = Kind::Custom(20);
/// NIP-71 video event (normal, landscape).
pub const VIDEO: Kind = Kind::Custom(21);

/// What a Blossom server says it stored (BUD-02 blob descriptor).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blob {
    pub url: String,
    /// Hex.
    pub sha256: String,
    pub size: u64,
    pub mime: String,
}

/// The MIME type of a file Heronite shares, from its extension.
pub fn mime_for(path: &std::path::Path) -> Option<&'static str> {
    match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "webp" => Some("image/webp"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "mp4" => Some("video/mp4"),
        "webm" => Some("video/webm"),
        "mkv" => Some("video/x-matroska"),
        _ => None,
    }
}

/// `bytes` without text, EXIF and XMP metadata (PNG, WebP, JPEG); other
/// types and unreadable files come back unchanged.
pub fn strip_metadata(bytes: Vec<u8>, mime: &str) -> Vec<u8> {
    let stripped = match mime {
        "image/png" => strip_png(&bytes),
        "image/webp" => strip_webp(&bytes),
        "image/jpeg" => strip_jpeg(&bytes),
        _ => None,
    };
    stripped.unwrap_or(bytes)
}

fn strip_png(b: &[u8]) -> Option<Vec<u8>> {
    const SIG: &[u8] = b"\x89PNG\r\n\x1a\n";
    if !b.starts_with(SIG) {
        return None;
    }
    let mut out = SIG.to_vec();
    let mut i = SIG.len();
    while i + 12 <= b.len() {
        let len = u32::from_be_bytes(b[i..i + 4].try_into().ok()?) as usize;
        let end = i.checked_add(12)?.checked_add(len)?;
        if end > b.len() {
            return None;
        }
        let kind = &b[i + 4..i + 8];
        if !matches!(kind, b"tEXt" | b"zTXt" | b"iTXt" | b"eXIf" | b"tIME") {
            out.extend_from_slice(&b[i..end]);
        }
        i = end;
        if kind == b"IEND" {
            return Some(out);
        }
    }
    None
}

fn strip_webp(b: &[u8]) -> Option<Vec<u8>> {
    if b.len() < 12 || &b[0..4] != b"RIFF" || &b[8..12] != b"WEBP" {
        return None;
    }
    let mut body = Vec::with_capacity(b.len());
    let mut i = 12;
    while i + 8 <= b.len() {
        let len = u32::from_le_bytes(b[i + 4..i + 8].try_into().ok()?) as usize;
        let padded = len + (len & 1);
        let end = i.checked_add(8)?.checked_add(padded)?.min(b.len());
        if i + 8 + len > b.len() {
            return None;
        }
        let kind = &b[i..i + 4];
        if kind == b"VP8X" && len >= 1 {
            let mut chunk = b[i..end].to_vec();
            // The flags byte: clear "has EXIF" (0x08) and "has XMP" (0x04).
            chunk[8] &= !0x0C;
            body.extend_from_slice(&chunk);
        } else if kind != b"EXIF" && kind != b"XMP " {
            body.extend_from_slice(&b[i..end]);
        }
        i = end;
    }
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&u32::try_from(body.len() + 4).ok()?.to_le_bytes());
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&body);
    Some(out)
}

fn strip_jpeg(b: &[u8]) -> Option<Vec<u8>> {
    if !b.starts_with(&[0xFF, 0xD8]) {
        return None;
    }
    let mut out = vec![0xFF, 0xD8];
    let mut i = 2;
    while i + 4 <= b.len() {
        if b[i] != 0xFF {
            return None;
        }
        let marker = b[i + 1];
        // Start of scan: the image data runs to the end.
        if marker == 0xDA {
            out.extend_from_slice(&b[i..]);
            return Some(out);
        }
        let len = u16::from_be_bytes([b[i + 2], b[i + 3]]) as usize;
        let end = i + 2 + len;
        if end > b.len() {
            return None;
        }
        // APP1 holds EXIF and XMP; COM is a free-text comment.
        if marker != 0xE1 && marker != 0xFE {
            out.extend_from_slice(&b[i..end]);
        }
        i = end;
    }
    None
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// The BUD-01 authorization for uploading the blob with `hash`: a signed
/// kind-24242 event, valid for a few minutes.
pub fn upload_auth(signer: &Signer, hash: &str, what: &str) -> Result<Event, crate::signer::Error> {
    let expires = Timestamp::from_secs(Timestamp::now().as_secs() + 5 * 60);
    signer.sign(
        EventBuilder::new(Kind::BlossomAuth, format!("Upload {what}"))
            .tag(Tag::custom("t", ["upload"]))
            .tag(Tag::custom("x", [hash.to_string()]))
            .tag(Tag::expiration(expires)),
    )
}

/// Uploads `bytes` to the Blossom server at `server` (`https://…`).
pub fn upload(server: &str, bytes: &[u8], mime: &str, signer: &Signer) -> anyhow::Result<Blob> {
    anyhow::ensure!(bytes.len() <= MAX_UPLOAD, "the file is bigger than {} MB", MAX_UPLOAD >> 20);
    let hash = sha256_hex(bytes);
    let auth = upload_auth(signer, &hash, mime)?;
    let header = format!("Nostr {}", base64::engine::general_purpose::STANDARD.encode(auth.as_json()));
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_global(Some(Duration::from_secs(10 * 60)))
        .http_status_as_error(false)
        .user_agent(crate::USER_AGENT)
        .build()
        .new_agent();
    let url = format!("{}/upload", server.trim().trim_end_matches('/'));
    let mut resp = agent.put(&url).header("Authorization", &header).header("Content-Type", mime).header("X-SHA-256", &hash).send(bytes)?;
    let status = resp.status();
    if !status.is_success() {
        // BUD-01: the reason is in `X-Reason`.
        let reason = resp.headers().get("X-Reason").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
        anyhow::bail!("{server} refused the upload ({status}){}", if reason.is_empty() { String::new() } else { format!(": {reason}") });
    }
    let doc: Value = serde_json::from_str(&resp.body_mut().with_config().limit(1 << 20).read_to_string()?)?;
    let blob = parse_blob(&doc, mime).ok_or_else(|| anyhow::anyhow!("{server} answered without a URL"))?;
    anyhow::ensure!(blob.sha256 == hash, "{server} stored something else than what was sent");
    Ok(blob)
}

fn parse_blob(doc: &Value, mime: &str) -> Option<Blob> {
    let url = doc.get("url")?.as_str()?.to_string();
    if !url.starts_with("https://") && !url.starts_with("http://") {
        return None;
    }
    Some(Blob {
        url,
        sha256: doc.get("sha256").and_then(Value::as_str).unwrap_or_default().to_ascii_lowercase(),
        size: doc.get("size").and_then(Value::as_u64).unwrap_or(0),
        mime: doc.get("type").and_then(Value::as_str).filter(|t| !t.is_empty()).unwrap_or(mime).to_string(),
    })
}

/// NIP-92 `imeta` tag for an uploaded blob.
fn imeta(blob: &Blob, dim: Option<(u32, u32)>) -> Tag {
    let mut parts = vec![format!("url {}", blob.url), format!("m {}", blob.mime), format!("x {}", blob.sha256)];
    if let Some((w, h)) = dim {
        parts.push(format!("dim {w}x{h}"));
    }
    Tag::custom("imeta", parts)
}

/// A NIP-68 picture event for a shared screenshot.
pub fn picture(blob: &Blob, caption: &str, game: &GameRef, dim: Option<(u32, u32)>) -> EventBuilder {
    EventBuilder::new(PICTURE, caption.trim())
        .tag(Tag::custom("title", [game.title.clone()]))
        .tag(imeta(blob, dim))
        .tag(Tag::custom("m", [blob.mime.clone()]))
        .tag(Tag::custom("x", [blob.sha256.clone()]))
        .tags(game.tags())
}

/// A NIP-71 video event for a shared replay. `seconds`: its length.
pub fn video(blob: &Blob, caption: &str, game: &GameRef, seconds: Option<u32>) -> EventBuilder {
    EventBuilder::new(VIDEO, caption.trim())
        .tag(Tag::custom("title", [game.title.clone()]))
        .tag(imeta(blob, None))
        .tag_maybe(seconds.map(|s| Tag::custom("duration", [s.to_string()])))
        .tag(Tag::custom("published_at", [Timestamp::now().as_secs().to_string()]))
        .tags(game.tags())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nostr::prelude::{FinalizeEvent, Keys};

    fn png_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut c = (data.len() as u32).to_be_bytes().to_vec();
        c.extend_from_slice(kind);
        c.extend_from_slice(data);
        c.extend_from_slice(&[0, 0, 0, 0]);
        c
    }

    #[test]
    fn png_text_chunks_go_and_the_image_stays() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend(png_chunk(b"IHDR", &[0; 13]));
        png.extend(png_chunk(b"tEXt", b"Comment\0/home/someone/Pictures"));
        png.extend(png_chunk(b"IDAT", b"pixels"));
        png.extend(png_chunk(b"iTXt", b"XML:com.adobe.xmp\0"));
        png.extend(png_chunk(b"IEND", b""));
        let out = strip_metadata(png.clone(), "image/png");
        assert!(!out.windows(8).any(|w| w == b"/home/so"));
        assert!(out.windows(4).any(|w| w == b"IDAT"));
        assert!(out.ends_with(&png_chunk(b"IEND", b"")));
        assert_eq!(
            out.len(),
            png.len() - png_chunk(b"tEXt", b"Comment\0/home/someone/Pictures").len() - png_chunk(b"iTXt", b"XML:com.adobe.xmp\0").len()
        );
        // A broken file is sent as it is rather than half.
        assert_eq!(strip_metadata(png[..20].to_vec(), "image/png"), png[..20].to_vec());
    }

    fn webp_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut c = kind.to_vec();
        c.extend_from_slice(&(data.len() as u32).to_le_bytes());
        c.extend_from_slice(data);
        if data.len() % 2 == 1 {
            c.push(0);
        }
        c
    }

    #[test]
    fn webp_exif_and_xmp_go_and_the_header_follows() {
        let mut body = b"WEBP".to_vec();
        body.extend(webp_chunk(b"VP8X", &[0x0C, 0, 0, 0, 1, 0, 0, 1, 0, 0]));
        body.extend(webp_chunk(b"VP8L", b"lossless"));
        body.extend(webp_chunk(b"EXIF", b"Exif\0\0secret"));
        body.extend(webp_chunk(b"XMP ", b"<x:xmpmeta/>"));
        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&(body.len() as u32).to_le_bytes());
        webp.extend(body);
        let out = strip_metadata(webp, "image/webp");
        assert!(!out.windows(6).any(|w| w == b"secret"));
        assert!(!out.windows(4).any(|w| w == b"XMP "));
        assert_eq!(u32::from_le_bytes(out[4..8].try_into().unwrap()) as usize, out.len() - 8);
        assert_eq!(out[20] & 0x0C, 0, "the VP8X flags no longer promise EXIF or XMP");
        assert!(out.windows(8).any(|w| w == b"lossless"));
    }

    #[test]
    fn jpeg_app1_and_comments_go() {
        let mut jpg = vec![0xFF, 0xD8];
        jpg.extend([0xFF, 0xE0, 0, 4, b'J', b'F']);
        jpg.extend([0xFF, 0xE1, 0, 8, b'E', b'x', b'i', b'f', 0, 0]);
        jpg.extend([0xFF, 0xFE, 0, 5, b'h', b'i', b'!']);
        jpg.extend([0xFF, 0xDA, 0, 2, 1, 2, 3, 0xFF, 0xD9]);
        let out = strip_metadata(jpg, "image/jpeg");
        assert_eq!(out, vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 4, b'J', b'F', 0xFF, 0xDA, 0, 2, 1, 2, 3, 0xFF, 0xD9]);
    }

    #[test]
    fn video_files_are_sent_as_they_are() {
        assert_eq!(strip_metadata(b"mp4 data".to_vec(), "video/mp4"), b"mp4 data");
        assert_eq!(mime_for(std::path::Path::new("/a/clip.MP4")), Some("video/mp4"));
        assert_eq!(mime_for(std::path::Path::new("/a/shot.webp")), Some("image/webp"));
        assert_eq!(mime_for(std::path::Path::new("/a/notes.txt")), None);
    }

    #[test]
    fn events_point_at_the_blob_and_name_the_game() {
        let blob = Blob { url: "https://cdn.example/ab.png".into(), sha256: "ab".into(), size: 3, mime: "image/png".into() };
        let game = GameRef::new("Celeste");
        let keys = Keys::generate();
        let pic = picture(&blob, " Summit! ", &game, Some((1920, 1080))).finalize(&keys).unwrap();
        assert_eq!(pic.kind, PICTURE);
        assert_eq!(pic.content, "Summit!");
        let imeta = pic.tags.iter().find(|t| t.kind() == "imeta").unwrap();
        assert_eq!(imeta.as_slice()[1..], ["url https://cdn.example/ab.png", "m image/png", "x ab", "dim 1920x1080"]);
        assert!(pic.tags.iter().any(|t| t.as_slice() == ["i", "#celeste"]));
        let vid = video(&blob, "", &game, Some(30)).finalize(&keys).unwrap();
        assert_eq!(vid.kind, VIDEO);
        assert!(vid.tags.iter().any(|t| t.as_slice() == ["duration", "30"]));
        assert!(vid.tags.iter().any(|t| t.as_slice() == ["title", "Celeste"]));
    }

    #[test]
    fn upload_auth_names_the_hash_and_expires() {
        let signer = Signer::Local(Keys::generate());
        let hash = sha256_hex(b"abc");
        assert_eq!(hash, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        let auth = upload_auth(&signer, &hash, "image/png").unwrap();
        assert_eq!(auth.kind, Kind::BlossomAuth);
        assert!(auth.tags.iter().any(|t| t.as_slice() == ["x", hash.as_str()]));
        assert!(auth.tags.iter().any(|t| t.as_slice() == ["t", "upload"]));
        assert!(!auth.is_expired());
    }

    #[test]
    fn blob_descriptors() {
        let doc = serde_json::json!({"url": "https://cdn.example/x.webp", "sha256": "AB", "size": 5, "type": "image/webp"});
        assert_eq!(
            parse_blob(&doc, "image/png"),
            Some(Blob { url: "https://cdn.example/x.webp".into(), sha256: "ab".into(), size: 5, mime: "image/webp".into() })
        );
        assert_eq!(parse_blob(&serde_json::json!({"url": "file:///etc/passwd"}), "image/png"), None);
        assert_eq!(parse_blob(&serde_json::json!({}), "image/png"), None);
    }
}
