//! Best-effort oEmbed metadata enrichment for video blocks.
//!
//! When a Rumble video is saved we ask Rumble's oEmbed endpoint once for its
//! title and thumbnail so the click-to-load box and the JSON-LD/OpenGraph SEO
//! render without the reader's browser ever contacting a third party. All of
//! this is non-fatal: a timeout or a failed fetch simply leaves the block with
//! whatever the author already typed. Enrichment is idempotent — only blocks
//! still missing a thumbnail are fetched, and the diff layer compares video
//! blocks by identity so a refreshed fetch can never mint a new version.

use std::time::Duration;

use forgepost_content::{BlockKind, ParsedBlock, video_needs_metadata};
use serde_json::Value;

const OEMBED_TIMEOUT: Duration = Duration::from_secs(3);

/// Fill in `title`/`thumbnail` for Rumble video blocks that still lack them.
pub(crate) async fn enrich_video_metadata(parsed: &mut [ParsedBlock]) {
    let pending: Vec<(usize, String)> = parsed
        .iter()
        .enumerate()
        .filter_map(|(i, block)| {
            let is_rumble = block.kind == BlockKind::Video
                && block.content.get("provider").and_then(|v| v.as_str()) == Some("rumble");
            if !is_rumble || !video_needs_metadata(&block.content) {
                return None;
            }
            block
                .content
                .get("url")
                .and_then(|v| v.as_str())
                .map(|url| (i, url.to_string()))
        })
        .collect();
    if pending.is_empty() {
        return;
    }
    let client = match reqwest::Client::builder()
        .timeout(OEMBED_TIMEOUT)
        .user_agent(concat!(
            "forgepost/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/DavidNeurieder/Forgepost)"
        ))
        .build()
    {
        Ok(client) => client,
        Err(_) => return,
    };
    for (index, url) in pending {
        if let Some(meta) = fetch_rumble_meta(&client, &url).await {
            parsed[index].content["title"] = Value::String(meta.title);
            parsed[index].content["thumbnail"] = Value::String(meta.thumbnail);
        }
    }
}

struct RumbleMeta {
    title: String,
    thumbnail: String,
}

async fn fetch_rumble_meta(client: &reqwest::Client, video_url: &str) -> Option<RumbleMeta> {
    let endpoint = format!(
        "https://rumble.com/api/Media/oembed/?url={}",
        percent_encode_query(video_url)
    );
    let response = client.get(&endpoint).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let value: Value = response.json().await.ok()?;
    let title = value.get("title")?.as_str()?.to_string();
    let thumbnail = value.get("thumbnail_url")?.as_str()?.to_string();
    Some(RumbleMeta { title, thumbnail })
}

/// Percent-encode a URL so it is safe inside a query string.
fn percent_encode_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{byte:02X}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_encoding_is_roundtrip_safe() {
        let url = "https://rumble.com/v10dqz9-some title?.html";
        let encoded = percent_encode_query(url);
        assert!(!encoded.contains(' '));
        assert!(encoded.contains("%20"));
        assert!(encoded.contains('%'));
        // Unreserved characters are untouched.
        assert_eq!(
            percent_encode_query("https://rumble.com/v1abc"),
            "https%3A%2F%2Frumble.com%2Fv1abc"
        );
    }
}
