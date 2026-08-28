//! Line-based Markdown parser producing a block tree.
//!
//! Supports the MVP block kinds: headings, paragraphs, blockquotes, fenced
//! code, horizontal rules, image-only blocks, and flat lists (unordered `-`,
//! `*`, `+` and ordered `N.`). Images accept an optional `=H` or `=WxH` size
//! suffix after the URL (`![alt](src =80)`) that renders as `width`/`height`
//! attributes, and may be wrapped in a link (`[![alt](src)](href)`). A
//! paragraph consisting entirely of raw HTML `<img>` tags (each optionally
//! wrapped in a `[...](url)` link, e.g. README badges) is converted to image
//! blocks, so `[<img src="…" height="80">](https://…)` renders as a linked
//! image. A paragraph consisting entirely of raw HTML `<iframe>` tags is
//! converted to a video block (whitelisted `src`/`title`/`width`/`height`
//! attributes; the `src` must be http(s)). A line that is *exactly* one
//! YouTube or Rumble URL is parsed as a video block too; URLs embedded in
//! prose stay paragraphs. Parsing is deliberately simple and lossless enough
//! for the semantic-document model. Inline formatting (`**bold**`,
//! `*italic*`, `` `code` ``, `[links](url)`) is rendered at display time by
//! [`render_inline`]; source text keeps the raw markers.

use serde_json::json;

use crate::{BlockContent, BlockKind, ParsedBlock};

/// Parse a Markdown document into an ordered list of blocks.
pub fn parse_markdown(source: &str) -> Vec<ParsedBlock> {
    let lines: Vec<&str> = source.lines().collect();
    let mut blocks = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        if let Some(video) = parse_video_line(trimmed) {
            blocks.push(video);
            i += 1;
            continue;
        }

        if trimmed.starts_with("```") {
            let language = trimmed.trim_start_matches('`').trim().to_string();
            i += 1;
            let mut code = String::new();
            while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                code.push_str(lines[i]);
                code.push('\n');
                i += 1;
            }
            i += 1; // skip closing fence
            if code.ends_with('\n') {
                code.pop();
            }
            blocks.push(ParsedBlock {
                kind: BlockKind::Code,
                content: json!({ "language": language, "code": code }),
            });
            continue;
        }

        if let Some((level, text)) = parse_heading(trimmed) {
            blocks.push(ParsedBlock {
                kind: BlockKind::Heading { level },
                content: json!({ "text": text }),
            });
            i += 1;
            continue;
        }

        if trimmed.starts_with('>') {
            let mut quote = String::new();
            while i < lines.len() && lines[i].trim_start().starts_with('>') {
                let q = lines[i].trim_start().trim_start_matches('>').trim();
                if !quote.is_empty() {
                    quote.push('\n');
                }
                quote.push_str(q);
                i += 1;
            }
            blocks.push(ParsedBlock {
                kind: BlockKind::Quote,
                content: json!({ "text": quote }),
            });
            continue;
        }

        if is_horizontal_rule(trimmed) {
            blocks.push(ParsedBlock {
                kind: BlockKind::Divider,
                content: json!({}),
            });
            i += 1;
            continue;
        }

        if trimmed.starts_with('!') || trimmed.starts_with("[![") {
            let mut rest = trimmed;
            let mut images = Vec::new();
            loop {
                let Some(img) = parse_image_at_start(rest) else {
                    break;
                };
                let consumed = img.consumed;
                images.push(img);
                rest = rest[consumed..].trim_start();
                if rest.is_empty() {
                    break;
                }
                if !(rest.starts_with('!') || rest.starts_with("[![")) {
                    break;
                }
            }
            if !images.is_empty() && rest.is_empty() {
                for img in images {
                    blocks.push(ParsedBlock {
                        kind: BlockKind::Image,
                        content: image_content(&img.src, &img.alt, &img.size, &img.href),
                    });
                }
                i += 1;
                continue;
            }
        }

        if let Some((ordered, item)) = parse_list_marker(trimmed) {
            let mut items = vec![item];
            i += 1;
            while i < lines.len() {
                let l = lines[i].trim();
                if l.is_empty() {
                    break;
                }
                match parse_list_marker(l) {
                    Some((o, item)) if o == ordered => {
                        items.push(item);
                        i += 1;
                    }
                    _ => break,
                }
            }
            blocks.push(ParsedBlock {
                kind: BlockKind::List { ordered },
                content: json!({ "items": items }),
            });
            continue;
        }

        let mut paragraph = String::new();
        while i < lines.len() {
            let l = lines[i];
            let t = l.trim();
            if t.is_empty()
                || t.starts_with('#')
                || t.starts_with('>')
                || t.starts_with("```")
                || is_horizontal_rule(t)
                || parse_list_marker(t).is_some()
            {
                break;
            }
            if !paragraph.is_empty() {
                paragraph.push('\n');
            }
            paragraph.push_str(l.trim_end());
            i += 1;
        }
        if let Some(images) = html_img_paragraph(&paragraph) {
            for img in images {
                blocks.push(ParsedBlock {
                    kind: BlockKind::Image,
                    content: image_content(&img.src, &img.alt, &img.size, &img.href),
                });
            }
            continue;
        }
        if let Some(video) = iframe_video_paragraph(&paragraph) {
            blocks.push(video);
            continue;
        }
        blocks.push(ParsedBlock {
            kind: BlockKind::Paragraph,
            content: json!({ "text": paragraph }),
        });
    }

    blocks
}

fn parse_heading(line: &str) -> Option<(u8, String)> {
    let level = line.chars().take_while(|&c| c == '#').count();
    if level == 0 {
        return None;
    }
    let text = line[level..].trim().to_string();
    Some((level.clamp(1, 6) as u8, text))
}

fn is_horizontal_rule(line: &str) -> bool {
    let chars: Vec<char> = line.chars().collect();
    chars.len() >= 3 && chars.iter().all(|&c| c == '-' || c == '_' || c == '*')
}

/// The JSON content of an image block.
fn image_content(src: &str, alt: &str, size: &str, href: &str) -> serde_json::Value {
    let mut content = json!({ "src": src, "alt": alt });
    if !size.is_empty() {
        content["size"] = json!(size);
    }
    if !href.is_empty() {
        content["href"] = json!(href);
    }
    content
}

/// One image parsed from Markdown or a raw HTML `<img>` tag.
struct ParsedImage {
    src: String,
    alt: String,
    size: String,
    href: String,
    /// Bytes consumed from the start of the input, including a link wrapper.
    consumed: usize,
}

/// Parse an image at the very start of `s`: either Markdown `![alt](src)`,
/// a linked Markdown image `[![alt](src)](href)`, or (not here) raw HTML —
/// handled by [`html_img_paragraph`]. Returns `None` when `s` does not start
/// with an image.
fn parse_image_at_start(s: &str) -> Option<ParsedImage> {
    if let Some(inner) = s.strip_prefix('[') {
        let plain = parse_plain_image_at_start(inner)?;
        let tail = inner[plain.consumed..].strip_prefix("](")?;
        let close = tail.find(')')?;
        let href = tail[..close].trim().to_string();
        if href.is_empty() {
            return None;
        }
        let consumed = 1 + plain.consumed + 1 + 1 + close + 1;
        return Some(ParsedImage {
            src: plain.src,
            alt: plain.alt,
            size: plain.size,
            href,
            consumed,
        });
    }
    let plain = parse_plain_image_at_start(s)?;
    Some(ParsedImage {
        src: plain.src,
        alt: plain.alt,
        size: plain.size,
        href: String::new(),
        consumed: plain.consumed,
    })
}

/// Parse an unlinked Markdown image `![alt](src =size)` at the start of `s`.
fn parse_plain_image_at_start(s: &str) -> Option<ParsedImage> {
    let rest = s.strip_prefix('!')?;
    let close = rest.find(']')?;
    if !rest.starts_with('[') {
        return None;
    }
    let alt = rest[1..close].to_string();
    let after = rest[close + 1..].trim_start();
    let paren_open = after.strip_prefix('(')?;
    let paren_close = paren_open.find(')')?;
    let src = paren_open[..paren_close].trim();
    if src.is_empty() {
        return None;
    }
    let (src, size) = split_image_size(src);
    if src.is_empty() {
        return None;
    }
    // consumed: `!` + `[alt]` + `(` + src + `)`
    let consumed = 1 + (close + 1) + 1 + paren_close + 1;
    Some(ParsedImage {
        src,
        alt,
        size,
        href: String::new(),
        consumed,
    })
}

/// A raw HTML `<img>` tag extracted from a paragraph.
struct HtmlImg {
    src: String,
    alt: String,
    size: String,
    href: String,
}

/// If `text` is a paragraph consisting entirely of raw HTML `<img>` tags —
/// each optionally wrapped in a `[<img …>](url)` link and separated by
/// whitespace — return the images in document order. Any other content returns
/// `None`, leaving the paragraph untouched so raw HTML stays escaped and can
/// never inject markup. Tags may span multiple lines (as README badges do).
fn html_img_paragraph(text: &str) -> Option<Vec<HtmlImg>> {
    let mut rest = text.trim();
    let mut out = Vec::new();
    while !rest.is_empty() {
        if let Some(after_bracket) = rest.strip_prefix('[') {
            let link_close = after_bracket.find("](")?;
            let inner = &after_bracket[..link_close];
            let href_part = &after_bracket[link_close + 2..];
            let close_paren = href_part.find(')')?;
            let href = href_part[..close_paren].trim().to_string();
            if href.is_empty() {
                return None;
            }
            let (img, consumed) = parse_html_img(inner)?;
            if consumed != inner.trim_start().len() {
                return None;
            }
            let mut img = img;
            img.href = href;
            out.push(img);
            rest = href_part[close_paren + 1..].trim_start();
            continue;
        }
        let (img, consumed) = parse_html_img(rest)?;
        out.push(img);
        rest = rest[consumed..].trim_start();
    }
    Some(out)
}

/// Parse one raw HTML `<img>` tag starting at `s`. Returns the (whitelisted)
/// attributes and the number of bytes the whole tag occupies. Unknown or
/// malformed attributes (e.g. `onerror`, non-digit `width`) are dropped.
fn parse_html_img(s: &str) -> Option<(HtmlImg, usize)> {
    let s = s.trim_start();
    let t = s.strip_prefix('<')?;
    let (name, rest) = read_ident(t)?;
    if !name.eq_ignore_ascii_case("img") {
        return None;
    }
    let mut src = String::new();
    let mut alt = String::new();
    let mut width = String::new();
    let mut height = String::new();
    let bytes = rest.as_bytes();
    let mut pos = 0usize;
    loop {
        while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r') {
            pos += 1;
        }
        if pos >= bytes.len() {
            return None;
        }
        if bytes[pos] == b'>' {
            pos += 1;
            break;
        }
        if bytes[pos] == b'/' {
            pos += 1;
            if pos < bytes.len() && bytes[pos] == b'>' {
                pos += 1;
                break;
            }
            continue;
        }
        let name_start = pos;
        while pos < bytes.len()
            && !matches!(
                bytes[pos],
                b'=' | b' ' | b'\t' | b'\n' | b'\r' | b'>' | b'/'
            )
        {
            pos += 1;
        }
        let attr = rest[name_start..pos].to_ascii_lowercase();
        while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r') {
            pos += 1;
        }
        let mut value = String::new();
        if pos < bytes.len() && bytes[pos] == b'=' {
            pos += 1;
            while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r') {
                pos += 1;
            }
            if pos < bytes.len() && (bytes[pos] == b'"' || bytes[pos] == b'\'') {
                let quote = bytes[pos];
                pos += 1;
                while pos < bytes.len() && bytes[pos] != quote {
                    value.push(bytes[pos] as char);
                    pos += 1;
                }
                if pos >= bytes.len() {
                    return None;
                }
                pos += 1;
            } else {
                while pos < bytes.len()
                    && !matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r' | b'>')
                {
                    value.push(bytes[pos] as char);
                    pos += 1;
                }
            }
        }
        let all_digits = |v: &str| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit());
        match attr.as_str() {
            "src" => src = value,
            "alt" => alt = value,
            "width" if all_digits(&value) => width = value,
            "height" if all_digits(&value) => height = value,
            _ => {}
        }
    }
    if src.is_empty() {
        return None;
    }
    let size = match (width.is_empty(), height.is_empty()) {
        (false, false) => format!("{width}x{height}"),
        (false, true) => format!("{width}x"),
        (true, false) => height,
        (true, true) => String::new(),
    };
    // consumed: `<` + tag name + attributes + `>`
    let consumed = 1 + name.len() + pos;
    Some((
        HtmlImg {
            src,
            alt,
            size,
            href: String::new(),
        },
        consumed,
    ))
}

/// Split a leading identifier (`[A-Za-z0-9_-]*`) off `s`.
fn read_ident(s: &str) -> Option<(&str, &str)> {
    let end = s.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    match end {
        Some(0) => None,
        Some(e) => Some((&s[..e], &s[e..])),
        None if !s.is_empty() => Some((s, "")),
        None => None,
    }
}

// ---------------------------------------------------------------------------
// Video blocks
// ---------------------------------------------------------------------------

/// A video block's JSON content (`provider`, `id`, `url`, plus optional
/// `title` / `thumbnail` / `width` / `height`).
fn video_block(provider: &'static str, id: String, url: &str) -> ParsedBlock {
    ParsedBlock {
        kind: BlockKind::Video,
        content: json!({ "provider": provider, "id": id, "url": url }),
    }
}

/// If `line` is *exactly* a single YouTube or Rumble video URL, return the
/// parsed video block. A URL embedded in prose never matches, so plain links
/// stay paragraphs.
fn parse_video_line(line: &str) -> Option<ParsedBlock> {
    let line = line.trim();
    let (provider, id) = video_from_url(line)?;
    Some(video_block(provider, id, line))
}

/// Extract `(provider, video id)` from a YouTube or Rumble URL.
fn video_from_url(url: &str) -> Option<(&'static str, String)> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = match rest.split_once('/') {
        Some((h, p)) => (h.to_ascii_lowercase(), p),
        None => (rest.to_ascii_lowercase(), ""),
    };
    let path = path.trim_end_matches('/');

    let is_yt = host == "youtu.be" || host == "youtube.com" || host.ends_with(".youtube.com");
    if is_yt {
        let id = if host == "youtu.be" {
            path.split('?').next().map(str::to_string)
        } else if let Some(rest) = path.strip_prefix("watch") {
            let query = rest.split_once('?').map(|(_, q)| q).unwrap_or("");
            extract_query_param(query, "v").map(str::to_string)
        } else {
            ["shorts/", "embed/", "v/", "live/"]
                .iter()
                .find_map(|prefix| path.strip_prefix(prefix))
                .map(|rest| rest.split('/').next().unwrap_or(rest).to_string())
        };
        if let Some(id) = id.filter(|id| valid_video_id(id)) {
            return Some(("youtube", id));
        }
        return None;
    }

    if host == "rumble.com" || host.ends_with(".rumble.com") {
        let id = if let Some(rest) = path.strip_prefix("v") {
            // Watch URLs look like `v<id>-<slug>.html`; the id is base36 and
            // starts with a digit, which keeps listing pages like `/videos` out.
            if rest.starts_with(|c: char| c.is_ascii_digit()) {
                Some(rest.split('-').next().unwrap_or(rest).to_string())
            } else {
                None
            }
        } else {
            path.strip_prefix("embed/")
                .map(|rest| rest.split('/').next().unwrap_or(rest).to_string())
        };
        if let Some(id) =
            id.filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()))
        {
            return Some(("rumble", id));
        }
        return None;
    }

    None
}

/// Extract the `key=value` query parameter from a URL query string.
fn extract_query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let mut parts = pair.splitn(2, '=');
        let k = parts.next().unwrap_or_default();
        let v = parts.next().unwrap_or_default();
        if k == key && !v.is_empty() {
            Some(v)
        } else {
            None
        }
    })
}

fn valid_video_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 32
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// If `text` is a paragraph consisting of a single raw HTML `<iframe>` element
/// (optionally with a closing `</iframe>` tag), return it as a video block.
/// Only `src`, `title`, `width`, and `height` are whitelisted; the `src` must
/// be http(s), so a hand-crafted tag can never smuggle `javascript:` in.
fn iframe_video_paragraph(text: &str) -> Option<ParsedBlock> {
    let text = text.trim();
    let (attrs, consumed) = parse_iframe_tag(text)?;
    if consumed != text.len() {
        return None;
    }
    let src = attrs
        .iter()
        .find(|(k, _)| k == "src")
        .map(|(_, v)| v.clone())?;
    if !(src.starts_with("https://") || src.starts_with("http://")) {
        return None;
    }
    let mut content = json!({ "provider": "iframe", "url": src });
    for (k, v) in attrs {
        match k.as_str() {
            "title" => content["title"] = json!(v),
            "width" => content["width"] = json!(v),
            "height" => content["height"] = json!(v),
            _ => {}
        }
    }
    Some(ParsedBlock {
        kind: BlockKind::Video,
        content,
    })
}

/// Parse one raw HTML `<iframe>` tag starting at `s`. Returns the whitelisted
/// attributes and the number of bytes the whole element occupies (including an
/// optional `</iframe>` closer). Unknown or malformed attributes are dropped.
fn parse_iframe_tag(s: &str) -> Option<(Vec<(String, String)>, usize)> {
    let t = s.strip_prefix('<')?;
    let (name, rest) = read_ident(t)?;
    if !name.eq_ignore_ascii_case("iframe") {
        return None;
    }
    let bytes = rest.as_bytes();
    let mut pos = 0usize;
    let mut attrs: Vec<(String, String)> = Vec::new();
    loop {
        while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r') {
            pos += 1;
        }
        if pos >= bytes.len() {
            return None;
        }
        if bytes[pos] == b'>' {
            pos += 1;
            break;
        }
        if bytes[pos] == b'/' {
            pos += 1;
            if pos < bytes.len() && bytes[pos] == b'>' {
                pos += 1;
                break;
            }
            continue;
        }
        let name_start = pos;
        while pos < bytes.len()
            && !matches!(
                bytes[pos],
                b'=' | b' ' | b'\t' | b'\n' | b'\r' | b'>' | b'/'
            )
        {
            pos += 1;
        }
        let attr = rest[name_start..pos].to_ascii_lowercase();
        while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r') {
            pos += 1;
        }
        let mut value = String::new();
        if pos < bytes.len() && bytes[pos] == b'=' {
            pos += 1;
            while pos < bytes.len() && matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r') {
                pos += 1;
            }
            if pos < bytes.len() && (bytes[pos] == b'"' || bytes[pos] == b'\'') {
                let quote = bytes[pos];
                pos += 1;
                while pos < bytes.len() && bytes[pos] != quote {
                    value.push(bytes[pos] as char);
                    pos += 1;
                }
                if pos >= bytes.len() {
                    return None;
                }
                pos += 1;
            } else {
                while pos < bytes.len()
                    && !matches!(bytes[pos], b' ' | b'\t' | b'\n' | b'\r' | b'>')
                {
                    value.push(bytes[pos] as char);
                    pos += 1;
                }
            }
        }
        let all_digits = |v: &str| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit());
        let keep = match attr.as_str() {
            "src" | "title" => Some(value),
            "width" if all_digits(&value) => Some(value),
            "height" if all_digits(&value) => Some(value),
            _ => None,
        };
        if let Some(v) = keep {
            attrs.push((attr, v));
        }
    }
    // `rest` was consumed up to `pos` (just past the opening `>`).
    let opening_end = 1 + name.len() + pos;
    let after_open = &s[opening_end..];
    let after_open_trimmed = after_open.trim_start();
    let skipped_ws = after_open.len() - after_open_trimmed.len();
    let closing_len = if let Some(rest) = after_open_trimmed.strip_prefix("</iframe") {
        let rest = rest.trim_start();
        let rest = rest.strip_prefix('>')?;
        if !rest.trim().is_empty() {
            return None;
        }
        after_open_trimmed.len()
    } else {
        if !after_open_trimmed.is_empty() {
            return None;
        }
        0
    };
    let total = opening_end + skipped_ws + closing_len;
    Some((attrs, total))
}

/// The iframe `src` that plays a video block (the embed URL). YouTube and
/// Rumble embeds are built from the video id (privacy-friendly hosts); generic
/// iframe blocks use the stored URL.
pub fn video_embed_url(content: &BlockContent) -> String {
    let provider = content
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let id = content.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let url = content.get("url").and_then(|v| v.as_str()).unwrap_or("");
    match provider {
        "youtube" if !id.is_empty() => {
            format!("https://www.youtube-nocookie.com/embed/{id}")
        }
        "rumble" if !id.is_empty() => format!("https://rumble.com/embed/{id}"),
        _ => url.to_string(),
    }
}

/// Absolute thumbnail URL for a video block (empty when unknown). YouTube's is
/// deterministic from the id; other providers store one fetched via oEmbed.
pub fn video_thumbnail(content: &BlockContent) -> String {
    let provider = content
        .get("provider")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let id = content.get("id").and_then(|v| v.as_str()).unwrap_or("");
    if provider == "youtube" && !id.is_empty() {
        return format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg");
    }
    content
        .get("thumbnail")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

/// Whether a video block still needs oEmbed metadata (thumbnail/title).
pub fn video_needs_metadata(content: &BlockContent) -> bool {
    video_thumbnail(content).is_empty()
}

/// The `(provider, id, url)` triple that uniquely identifies a video embed.
/// oEmbed-derived fields (`title`, `thumbnail`) are metadata, so this is the
/// identity used when diffing content for block versions.
pub fn video_identity(content: &BlockContent) -> (String, String, String) {
    (
        content
            .get("provider")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        content
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        content
            .get("url")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
    )
}

/// Split a trailing ` =SIZE` suffix off an image URL, where SIZE is `H`,
/// `WxH`, `Wx`, or `xH` (digits only). Returns `(url, size)`; anything that
/// doesn't match is left in the URL, preserving the previous behavior.
fn split_image_size(src: &str) -> (String, String) {
    if let Some(eq) = src.rfind('=') {
        let before = &src[..eq];
        let after = &src[eq + 1..];
        if (before.ends_with(' ') || before.ends_with('\t')) && is_valid_size(after) {
            return (before.trim_end().to_string(), after.to_string());
        }
    }
    (src.to_string(), String::new())
}

/// Whether `s` is a valid size: `H`, `WxH`, `Wx`, or `xH` with digits only.
fn is_valid_size(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    if let Some((w, h)) = s.split_once('x') {
        let digits = |part: &str| part.chars().all(|c| c.is_ascii_digit());
        (w.is_empty() || digits(w))
            && (h.is_empty() || digits(h))
            && !(w.is_empty() && h.is_empty())
    } else {
        s.chars().all(|c| c.is_ascii_digit())
    }
}

/// The `width`/`height` attributes (including the leading space) for an image
/// size suffix, or an empty string when there is none. The value is
/// re-validated so a hand-crafted block can never inject extra attributes.
fn image_size_attrs(size: &str) -> String {
    let digits = |part: &str| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit());
    if let Some((w, h)) = size.split_once('x') {
        let mut attrs = String::new();
        if digits(w) {
            attrs.push_str(&format!(" width=\"{w}\""));
        }
        if digits(h) {
            attrs.push_str(&format!(" height=\"{h}\""));
        }
        attrs
    } else if digits(size) {
        format!(" height=\"{size}\"")
    } else {
        String::new()
    }
}

/// Parse a list item line. Returns `(ordered, item_text)` for unordered
/// markers (`- `, `* `, `+ `) and ordered markers (`1. `, `42. `, …). The
/// marker must be followed by whitespace, matching CommonMark.
fn parse_list_marker(line: &str) -> Option<(bool, String)> {
    let line = line.trim();
    let (ordered, after_marker) = if let Some(rest) = line.strip_prefix('-') {
        (false, rest)
    } else if let Some(rest) = line.strip_prefix('*') {
        (false, rest)
    } else if let Some(rest) = line.strip_prefix('+') {
        (false, rest)
    } else {
        // Ordered: `N.` where N is one or more ASCII digits.
        let digits = line.chars().take_while(|c| c.is_ascii_digit()).count();
        if digits == 0 {
            return None;
        }
        let rest = line.get(digits..)?;
        let rest = rest.strip_prefix('.')?;
        (true, rest)
    };
    let item = after_marker.trim();
    if item.is_empty() {
        return None;
    }
    Some((ordered, item.to_string()))
}

/// Escape text for safe HTML output.
pub fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Whether a URL is allowed into an `href`/`src` attribute. Executable schemes
/// (`javascript:`, `vbscript:`, `file:`, `data:` carrying HTML/script) are
/// rejected so attacker-controlled Markdown can never plant a click-to-run
/// payload; http(s), protocol-relative, relative, anchor, and inert
/// `data:image/...` references remain allowed.
///
/// Browsers strip ASCII tab/newline/carriage-return from URLs before scheme
/// detection, so those are removed up front: `java\nscript:` must not slip
/// past a naive string comparison.
pub fn is_safe_url(url: &str) -> bool {
    let url = url.trim().trim_start_matches('\u{200B}');
    let normalized: String = url
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let lower = normalized.to_ascii_lowercase();
    !(lower.starts_with("javascript:")
        || lower.starts_with("vbscript:")
        || lower.starts_with("file:")
        || lower.starts_with("data:text/html")
        || lower.starts_with("data:text/javascript")
        || lower.starts_with("data:application/xhtml")
        || lower.starts_with("data:image/svg"))
}

/// Render blocks to HTML. Inline Markdown (`**bold**`, `*italic*`,
/// `` `code` ``, `[links](url)`) is rendered by [`render_inline`]; text is
/// escaped first so user input can never inject markup.
pub fn render_html<'a>(blocks: impl IntoIterator<Item = (BlockKind, &'a BlockContent)>) -> String {
    let mut out = String::new();
    for (kind, content) in blocks {
        match kind {
            BlockKind::Heading { level } => {
                let text = text_of(content);
                out.push_str(&format!(
                    "<h{}>{}</h{}>\n",
                    level,
                    render_inline(&text),
                    level
                ));
            }
            BlockKind::Paragraph => {
                let text = text_of(content);
                out.push_str(&format!("<p>{}</p>\n", render_inline(&text)));
            }
            BlockKind::Quote => {
                let text = text_of(content);
                out.push_str(&format!(
                    "<blockquote>{}</blockquote>\n",
                    render_inline(&text)
                ));
            }
            BlockKind::Code => {
                let language = content
                    .get("language")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let code = content.get("code").and_then(|v| v.as_str()).unwrap_or("");
                let lang_attr = if language.is_empty() {
                    String::new()
                } else {
                    format!(" class=\"language-{}\"", html_escape(language))
                };
                out.push_str(&format!(
                    "<pre><code{}>{}</code></pre>\n",
                    lang_attr,
                    html_escape(code)
                ));
            }
            BlockKind::Image => {
                let src = content.get("src").and_then(|v| v.as_str()).unwrap_or("");
                let alt = content.get("alt").and_then(|v| v.as_str()).unwrap_or("");
                if !is_safe_url(src) {
                    out.push_str(&format!(
                        "<p><strong>Blocked link:</strong> {}</p>\n",
                        html_escape(alt)
                    ));
                    continue;
                }
                let size = content.get("size").and_then(|v| v.as_str()).unwrap_or("");
                let href = content.get("href").and_then(|v| v.as_str()).unwrap_or("");
                let img = format!(
                    "<img src=\"{}\" alt=\"{}\" loading=\"lazy\" decoding=\"async\"{} />",
                    html_escape(src),
                    html_escape(alt),
                    image_size_attrs(size)
                );
                if href.is_empty() || !is_safe_url(href) {
                    out.push_str(&format!("<p>{img}</p>\n"));
                } else {
                    out.push_str(&format!(
                        "<p><a href=\"{}\">{img}</a></p>\n",
                        html_escape(href)
                    ));
                }
            }
            BlockKind::Divider => out.push_str("<hr />\n"),
            BlockKind::CallToAction => {
                let text = text_of(content);
                out.push_str(&format!(
                    "<p><strong>{}</strong></p>\n",
                    render_inline(&text)
                ));
            }
            BlockKind::Video => {
                let url = video_embed_url(content);
                let title = content.get("title").and_then(|v| v.as_str()).unwrap_or("");
                let thumb = video_thumbnail(content);
                let label = if title.is_empty() {
                    "Play video".to_string()
                } else {
                    format!("Play video: {title}")
                };
                let mut figure = format!(
                    "<figure class=\"video\">\n<button type=\"button\" class=\"video-box\" data-video data-src=\"{}\" aria-label=\"{}\">\n",
                    html_escape(&url),
                    html_escape(&label)
                );
                if !thumb.is_empty() {
                    figure.push_str(&format!(
                        "<img class=\"video-thumb\" src=\"{}\" alt=\"\" loading=\"lazy\" decoding=\"async\">\n",
                        html_escape(&thumb)
                    ));
                }
                figure.push_str(
                    "<span class=\"video-play\" aria-hidden=\"true\">&#9654;</span>\n</button>\n",
                );
                if !title.is_empty() {
                    figure.push_str(&format!(
                        "<figcaption>{}</figcaption>\n",
                        html_escape(title)
                    ));
                }
                figure.push_str("</figure>\n");
                out.push_str(&figure);
            }
            BlockKind::List { ordered } => {
                let tag = if ordered { "ol" } else { "ul" };
                out.push_str(&format!("<{tag}>\n"));
                if let Some(items) = content.get("items").and_then(|v| v.as_array()) {
                    for item in items {
                        out.push_str(&format!(
                            "<li>{}</li>\n",
                            render_inline(item.as_str().unwrap_or_default())
                        ));
                    }
                }
                out.push_str(&format!("</{tag}>\n"));
            }
        }
    }
    out
}

/// Escape the text, then render inline Markdown markers to HTML. The input is
/// treated as plain text (never trusted), and any marker found is applied
/// after escaping, so `<script>` etc. can never reach the output.
fn render_inline(text: &str) -> String {
    inline_spans(&html_escape(text))
}

/// Scan an already-escaped string for inline markers. Runs recursively so
/// formatting can nest (e.g. bold containing italic or a link).
fn inline_spans(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while !rest.is_empty() {
        // Inline code spans win over everything else; content stays literal.
        if let Some(end) = rest
            .strip_prefix('`')
            .and_then(|after_open| after_open.find('`'))
        {
            out.push_str("<code>");
            out.push_str(&rest[1..end + 1]);
            out.push_str("</code>");
            rest = &rest[end + 2..];
            continue;
        }
        // Bold first so `**` isn't misread as two italics.
        if let Some((inner, after)) = between(rest, "**") {
            out.push_str("<strong>");
            out.push_str(&inline_spans(inner));
            out.push_str("</strong>");
            rest = after;
            continue;
        }
        // Links before italics so `[*x*](y)` still works.
        if let Some((label, url, after)) = link(rest) {
            if is_safe_url(url) {
                out.push_str(&format!("<a href=\"{url}\">"));
                out.push_str(&inline_spans(label));
                out.push_str("</a>");
            } else {
                // Executable scheme: keep the label as plain text so nothing
                // click-to-run ever reaches the document.
                out.push_str(&inline_spans(label));
            }
            rest = after;
            continue;
        }
        if let Some((inner, after)) = between(rest, "*") {
            out.push_str("<em>");
            out.push_str(&inline_spans(inner));
            out.push_str("</em>");
            rest = after;
            continue;
        }
        let c = rest.chars().next().expect("non-empty rest");
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// If `s` starts and (later) closes with `marker`, return the inner text and
/// the remainder after the closing marker.
fn between<'a>(s: &'a str, marker: &str) -> Option<(&'a str, &'a str)> {
    let rest = s.strip_prefix(marker)?;
    let end = rest.find(marker)?;
    Some((&rest[..end], &rest[end + marker.len()..]))
}

/// Parse `[label](url)` at the start of `s`.
fn link(s: &str) -> Option<(&str, &str, &str)> {
    let rest = s.strip_prefix('[')?;
    let close = rest.find(']')?;
    let label = &rest[..close];
    let after = rest[close + 1..].strip_prefix('(')?;
    let paren_close = after.find(')')?;
    let url = after[..paren_close].trim();
    if url.is_empty() {
        return None;
    }
    Some((label, url, &after[paren_close + 1..]))
}

/// The plain text that a block contributes to search indexing. Mirrors
/// `render_html` so the searchable text always matches what is displayed.
pub fn block_search_text(kind: &BlockKind, content: &BlockContent) -> String {
    match kind {
        BlockKind::Heading { .. }
        | BlockKind::Paragraph
        | BlockKind::Quote
        | BlockKind::CallToAction => text_of(content),
        BlockKind::Code => content
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        BlockKind::Image => content
            .get("alt")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        BlockKind::Video => content
            .get("title")
            .and_then(|v| v.as_str())
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                content
                    .get("url")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string()
            }),
        BlockKind::List { .. } => content
            .get("items")
            .and_then(|v| v.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|it| it.as_str())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default(),
        BlockKind::Divider => String::new(),
    }
}

fn text_of(content: &BlockContent) -> String {
    content
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(blocks: &[ParsedBlock]) -> Vec<BlockKind> {
        blocks.iter().map(|b| b.kind).collect()
    }

    fn html_of(blocks: &[ParsedBlock]) -> String {
        render_html(blocks.iter().map(|b| (b.kind, &b.content)))
    }

    #[test]
    fn parses_headings_paragraphs_and_rules() {
        let src = "# Title\n\nSome paragraph here.\n\n---\n\n## Sub\n\ntext\n";
        let blocks = parse_markdown(src);
        assert_eq!(
            kinds(&blocks),
            vec![
                BlockKind::Heading { level: 1 },
                BlockKind::Paragraph,
                BlockKind::Divider,
                BlockKind::Heading { level: 2 },
                BlockKind::Paragraph,
            ]
        );
    }

    #[test]
    fn groups_paragraph_lines() {
        let src = "line one\nline two\n\nnew para";
        let blocks = parse_markdown(src);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].content.get("text").unwrap(), "line one\nline two");
    }

    #[test]
    fn parses_fenced_code_with_language() {
        let src = "```rust\nfn main() {}\n```\n";
        let blocks = parse_markdown(src);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::Code);
        assert_eq!(blocks[0].content.get("code").unwrap(), "fn main() {}");
        assert_eq!(blocks[0].content.get("language").unwrap(), "rust");
    }

    #[test]
    fn parses_quotes_and_images() {
        let src = "> quoted text\n\n![alt text](https://example.com/x.png)";
        let blocks = parse_markdown(src);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].kind, BlockKind::Quote);
        assert_eq!(blocks[1].kind, BlockKind::Image);
        assert_eq!(
            blocks[1].content.get("src").unwrap(),
            "https://example.com/x.png"
        );
    }

    #[test]
    fn parses_image_size_suffix() {
        let blocks = parse_markdown(
            "![Get it on F-Droid](https://fdroid.gitlab.io/artwork/badge/get-it-on.png =80)",
        );
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::Image);
        assert_eq!(
            blocks[0].content.get("src").unwrap(),
            "https://fdroid.gitlab.io/artwork/badge/get-it-on.png"
        );
        assert_eq!(blocks[0].content.get("size").unwrap(), "80");
    }

    #[test]
    fn renders_image_with_size_attributes() {
        for (suffix, attrs) in [
            ("=80", "height=\"80\""),
            ("=640x360", "width=\"640\" height=\"360\""),
            ("=640x", "width=\"640\""),
            ("=x360", "height=\"360\""),
        ] {
            let md = format!("![a](https://e.com/x.png {suffix})");
            let blocks = parse_markdown(&md);
            let html = render_html(vec![(blocks[0].kind, &blocks[0].content)]);
            assert!(html.contains(attrs), "for {suffix}: {html}");
        }

        // The exact badge case from the feature request.
        let blocks = parse_markdown(
            "![Get it on F-Droid](https://fdroid.gitlab.io/artwork/badge/get-it-on.png =80)",
        );
        let html = render_html(vec![(blocks[0].kind, &blocks[0].content)]);
        assert_eq!(
            html,
            "<p><img src=\"https://fdroid.gitlab.io/artwork/badge/get-it-on.png\" alt=\"Get it on F-Droid\" loading=\"lazy\" decoding=\"async\" height=\"80\" /></p>\n"
        );
    }

    #[test]
    fn parses_raw_html_img_link_wrapped_badge() {
        let src = "[<img src=\"https://fdroid.gitlab.io/artwork/badge/get-it-on.png\"\n     alt=\"Get it on F-Droid\"\n     height=\"80\">](https://f-droid.org/packages/com.offlinecurrencyconverter.app/)";
        let blocks = parse_markdown(src);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::Image);
        assert_eq!(
            blocks[0].content.get("src").unwrap(),
            "https://fdroid.gitlab.io/artwork/badge/get-it-on.png"
        );
        assert_eq!(blocks[0].content.get("alt").unwrap(), "Get it on F-Droid");
        assert_eq!(blocks[0].content.get("size").unwrap(), "80");
        assert_eq!(
            blocks[0].content.get("href").unwrap(),
            "https://f-droid.org/packages/com.offlinecurrencyconverter.app/"
        );
        let html = render_html(vec![(blocks[0].kind, &blocks[0].content)]);
        assert_eq!(
            html,
            "<p><a href=\"https://f-droid.org/packages/com.offlinecurrencyconverter.app/\"><img src=\"https://fdroid.gitlab.io/artwork/badge/get-it-on.png\" alt=\"Get it on F-Droid\" loading=\"lazy\" decoding=\"async\" height=\"80\" /></a></p>\n"
        );
    }

    #[test]
    fn parses_multiple_raw_html_imgs_on_one_line() {
        let src = concat!(
            "<img src=\"fastlane/metadata/android/en-US/images/phoneScreenshots/1.png\" width=\"180\" alt=\"Screenshot 1\"> ",
            "<img src=\"fastlane/metadata/android/en-US/images/phoneScreenshots/2.png\" width=\"180\" alt=\"Screenshot 2\"> ",
            "<img src=\"fastlane/metadata/android/en-US/images/phoneScreenshots/3.png\" width=\"180\" alt=\"Screenshot 3\">"
        );
        let blocks = parse_markdown(src);
        assert_eq!(blocks.len(), 3);
        for (i, b) in blocks.iter().enumerate() {
            assert_eq!(b.kind, BlockKind::Image);
            assert_eq!(b.content.get("size").unwrap(), "180x");
            let expected = format!(
                "fastlane/metadata/android/en-US/images/phoneScreenshots/{}.png",
                i + 1
            );
            assert_eq!(b.content.get("src").unwrap(), expected.as_str());
        }
    }

    #[test]
    fn parses_linked_markdown_image() {
        let blocks = parse_markdown("[![a](img.png)](https://example.com/page)");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::Image);
        assert_eq!(blocks[0].content.get("src").unwrap(), "img.png");
        assert_eq!(blocks[0].content.get("alt").unwrap(), "a");
        assert_eq!(
            blocks[0].content.get("href").unwrap(),
            "https://example.com/page"
        );
        let html = render_html(vec![(blocks[0].kind, &blocks[0].content)]);
        assert_eq!(
            html,
            "<p><a href=\"https://example.com/page\"><img src=\"img.png\" alt=\"a\" loading=\"lazy\" decoding=\"async\" /></a></p>\n"
        );
    }

    #[test]
    fn parses_multiple_markdown_images_per_line() {
        let blocks = parse_markdown("![a](1.png) ![b](2.png =40)");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].content.get("src").unwrap(), "1.png");
        assert_eq!(blocks[1].content.get("src").unwrap(), "2.png");
        assert_eq!(blocks[1].content.get("size").unwrap(), "40");
    }

    #[test]
    fn image_line_with_trailing_text_is_kept_as_paragraph() {
        let blocks = parse_markdown("![a](1.png) and text");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::Paragraph);
        assert_eq!(
            blocks[0].content.get("text").unwrap(),
            "![a](1.png) and text"
        );
    }

    #[test]
    fn raw_html_img_drops_dangerous_attributes() {
        let blocks =
            parse_markdown("<img src=\"x.png\" onerror=\"alert(1)\" width=\"180\" style=\"x\">");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::Image);
        let html = render_html(vec![(blocks[0].kind, &blocks[0].content)]);
        assert_eq!(
            html,
            "<p><img src=\"x.png\" alt=\"\" loading=\"lazy\" decoding=\"async\" width=\"180\" /></p>\n"
        );
        assert!(!html.contains("onerror"));
        assert!(!html.contains("style"));
    }

    #[test]
    fn raw_html_img_ignores_invalid_dimensions() {
        let blocks = parse_markdown("<img src=\"x.png\" width=\"12px\" height=\"80\">");
        assert_eq!(blocks[0].content.get("size").unwrap(), "80");
        let html = render_html(vec![(blocks[0].kind, &blocks[0].content)]);
        assert_eq!(
            html,
            "<p><img src=\"x.png\" alt=\"\" loading=\"lazy\" decoding=\"async\" height=\"80\" /></p>\n"
        );
    }

    #[test]
    fn raw_html_img_self_closing_and_unquoted_attrs() {
        let blocks = parse_markdown("<img src=/x.png alt=\"y\" width=120 />");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].content.get("src").unwrap(), "/x.png");
        assert_eq!(blocks[0].content.get("alt").unwrap(), "y");
        assert_eq!(blocks[0].content.get("size").unwrap(), "120x");
    }

    #[test]
    fn raw_html_img_mixed_with_text_stays_escaped_paragraph() {
        let blocks = parse_markdown("See <img src=\"x.png\" alt=\"x\"> now");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::Paragraph);
        let html = render_html(vec![(blocks[0].kind, &blocks[0].content)]);
        assert!(!html.contains("<img"));
        assert!(html.contains("&lt;img"));
    }

    #[test]
    fn malformed_size_stays_in_src() {
        let blocks = parse_markdown("![a](https://e.com/x.png =8.5)");
        assert_eq!(
            blocks[0].content.get("src").unwrap(),
            "https://e.com/x.png =8.5"
        );
        assert!(blocks[0].content.get("size").is_none());
        let html = render_html(vec![(blocks[0].kind, &blocks[0].content)]);
        assert_eq!(
            html,
            "<p><img src=\"https://e.com/x.png =8.5\" alt=\"a\" loading=\"lazy\" decoding=\"async\" /></p>\n"
        );
    }

    #[test]
    fn forged_size_cannot_inject_attributes() {
        let html = render_html(vec![(
            BlockKind::Image,
            &json!({ "src": "/x.png", "alt": "x", "size": "\" onerror=\"alert(1)" }),
        )]);
        assert_eq!(
            html,
            "<p><img src=\"/x.png\" alt=\"x\" loading=\"lazy\" decoding=\"async\" /></p>\n"
        );
    }

    #[test]
    fn renders_html_with_escaping() {
        let html = render_html(vec![
            (
                BlockKind::Heading { level: 1 },
                &json!({ "text": "Hi & <bye>" }),
            ),
            (BlockKind::Paragraph, &json!({ "text": "para" })),
            (
                BlockKind::Code,
                &json!({ "code": "x < y", "language": "rust" }),
            ),
        ]);
        assert_eq!(
            html,
            "<h1>Hi &amp; &lt;bye&gt;</h1>\n<p>para</p>\n<pre><code class=\"language-rust\">x &lt; y</code></pre>\n"
        );
    }

    #[test]
    fn parses_unordered_list_into_one_block() {
        let blocks =
            parse_markdown("- **Language:** Kotlin\n- **UI:** Jetpack Compose\n- **DI:** Hilt\n");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::List { ordered: false });
        let items = blocks[0].content.get("items").unwrap().as_array().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0], "**Language:** Kotlin");
    }

    #[test]
    fn parses_ordered_list_and_all_markers() {
        // `-`, `*`, and `+` are interchangeable bullet markers in one list
        // (CommonMark), so `* a\n+ b` is a single unordered list block.
        let blocks = parse_markdown("* a\n+ b\n1. c\n2. d\n3. e\n");
        assert_eq!(
            kinds(&blocks),
            vec![
                BlockKind::List { ordered: false },
                BlockKind::List { ordered: true },
            ]
        );
        assert_eq!(
            blocks[0]
                .content
                .get("items")
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            blocks[1]
                .content
                .get("items")
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            3
        );
    }

    #[test]
    fn list_breaks_on_blank_line_or_paragraph() {
        let src = "- one\n- two\n\nA paragraph.\n- three\n";
        let blocks = parse_markdown(src);
        assert_eq!(
            kinds(&blocks),
            vec![
                BlockKind::List { ordered: false },
                BlockKind::Paragraph,
                BlockKind::List { ordered: false },
            ]
        );
        // `- three` starts a fresh list block, not a paragraph continuation.
        let first = &blocks[0].content.get("items").unwrap().as_array().unwrap();
        assert_eq!(first.len(), 2);
    }

    #[test]
    fn renders_lists_and_inline_formatting() {
        let html = render_html(vec![
            (
                BlockKind::List { ordered: false },
                &json!({ "items": ["**bold** item", "*italic*", "plain & <safe>"] }),
            ),
            (
                BlockKind::List { ordered: true },
                &json!({ "items": ["step `code`", "[link](https://e.com/?a=1&b=2)"] }),
            ),
        ]);
        assert_eq!(
            html,
            "<ul>\n\
             <li><strong>bold</strong> item</li>\n\
             <li><em>italic</em></li>\n\
             <li>plain &amp; &lt;safe&gt;</li>\n\
             </ul>\n\
             <ol>\n\
             <li>step <code>code</code></li>\n\
             <li><a href=\"https://e.com/?a=1&amp;b=2\">link</a></li>\n\
             </ol>\n"
        );
    }

    #[test]
    fn inline_formatting_applies_in_paragraphs_with_escaping() {
        let html = render_html(vec![
            (BlockKind::Paragraph, &json!({ "text": "a **b** c" })),
            (BlockKind::Paragraph, &json!({ "text": "x <script> **y**" })),
        ]);
        assert_eq!(
            html,
            "<p>a <strong>b</strong> c</p>\n<p>x &lt;script&gt; <strong>y</strong></p>\n"
        );
    }

    #[test]
    fn list_text_is_searchable() {
        let mut doc = crate::Document::empty("Listy");
        let id = crate::BlockId::new_v4();
        let version = crate::BlockVersion {
            id: crate::VersionId::new_v4(),
            block_id: id,
            content: json!({ "items": ["Retrofit networking", "Room database"] }),
            created_at_ms: 0,
        };
        doc.blocks.push(crate::Block {
            id,
            kind: BlockKind::List { ordered: false },
            version_id: version.id,
            position: 0,
            created_at_ms: 0,
            updated_at_ms: 0,
        });
        doc.versions.push(version);
        let text = doc.searchable_text();
        assert!(text.contains("Retrofit networking"));
        assert!(text.contains("Room database"));
    }

    #[test]
    fn parses_standalone_youtube_urls_as_video_blocks() {
        let cases = [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ?t=42",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ",
            "https://www.youtube.com/embed/dQw4w9WgXcQ",
            "https://youtube.com/live/dQw4w9WgXcQ",
            "https://m.youtube.com/watch?v=dQw4w9WgXcQ&t=1",
        ];
        for url in cases {
            let blocks = parse_markdown(url);
            assert_eq!(blocks.len(), 1, "{url}");
            assert_eq!(blocks[0].kind, BlockKind::Video, "{url}");
            assert_eq!(blocks[0].content["provider"], "youtube", "{url}");
            assert_eq!(blocks[0].content["id"], "dQw4w9WgXcQ", "{url}");
        }
    }

    #[test]
    fn parses_standalone_rumble_urls_as_video_blocks() {
        let blocks = parse_markdown("https://rumble.com/v10dqz9-cool-video.html");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::Video);
        assert_eq!(blocks[0].content["provider"], "rumble");
        assert_eq!(blocks[0].content["id"], "10dqz9");

        let blocks = parse_markdown("https://www.rumble.com/embed/10dqz9");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].content["provider"], "rumble");
        assert_eq!(blocks[0].content["id"], "10dqz9");
    }

    #[test]
    fn url_in_prose_stays_a_paragraph() {
        for line in [
            "watch https://www.youtube.com/watch?v=dQw4w9WgXcQ now",
            "https://youtu.be/dQw4w9WgXcQ and more text",
            "> https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ.md",
            "https://vimeo.com/123456",
            "https://rumble.com/videos",
            "https://rumble.com/videos/animals",
        ] {
            let blocks = parse_markdown(line);
            assert_ne!(blocks[0].kind, BlockKind::Video, "{line}");
        }
    }

    #[test]
    fn raw_iframe_paragraph_becomes_a_video_block() {
        let blocks = parse_markdown(
            "<iframe src=\"https://example.com/v/123\" title=\"Demo\" width=\"560\" height=\"315\" allowfullscreen></iframe>",
        );
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].kind, BlockKind::Video);
        assert_eq!(blocks[0].content["provider"], "iframe");
        assert_eq!(blocks[0].content["url"], "https://example.com/v/123");
        assert_eq!(blocks[0].content["title"], "Demo");
        assert_eq!(blocks[0].content["width"], "560");
    }

    #[test]
    fn raw_iframe_rejects_non_http_and_trailing_text() {
        let blocks = parse_markdown("<iframe src=\"javascript:alert(1)\"></iframe>");
        assert_eq!(blocks[0].kind, BlockKind::Paragraph);
        let html = html_of(&blocks);
        assert!(!html.contains("<iframe"), "no live iframe ever rendered");
        assert!(html.contains("&lt;iframe"), "tag stays inert escaped text");

        let blocks = parse_markdown("<iframe src=\"https://a.com\"></iframe> trailing text");
        assert_eq!(blocks[0].kind, BlockKind::Paragraph);
        assert!(html_of(&blocks).contains("&lt;iframe"));
    }

    #[test]
    fn renders_video_with_click_to_load_box() {
        let html = html_of(&parse_markdown("https://youtu.be/dQw4w9WgXcQ"));
        assert!(html.contains("class=\"video-box\""));
        assert!(html.contains("data-src=\"https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ\""));
        assert!(html.contains("i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg"));
        assert!(html.contains("data-video"));
        assert!(!html.contains("<iframe"), "no iframe on page load");
    }

    #[test]
    fn renders_video_escaping_title_and_attr_values() {
        let html = html_of(&parse_markdown(
            "<iframe src=\"https://a.com/x\" title=\"&quot;evil&quot;\"></iframe>",
        ));
        assert!(
            html.contains("&amp;quot;evil&amp;quot;"),
            "title is escaped twice"
        );
        assert!(html.contains("data-src=\"https://a.com/x\""));
    }

    #[test]
    fn video_embed_url_and_thumbnail_helpers() {
        assert_eq!(
            video_embed_url(&json!({ "provider": "youtube", "id": "abc", "url": "x" })),
            "https://www.youtube-nocookie.com/embed/abc"
        );
        assert_eq!(
            video_embed_url(&json!({ "provider": "rumble", "id": "1a", "url": "x" })),
            "https://rumble.com/embed/1a"
        );
        assert_eq!(
            video_embed_url(&json!({ "provider": "iframe", "url": "https://e.com/x" })),
            "https://e.com/x"
        );
        assert_eq!(
            video_thumbnail(&json!({ "provider": "youtube", "id": "abc", "url": "x" })),
            "https://i.ytimg.com/vi/abc/hqdefault.jpg"
        );
        assert_eq!(
            video_thumbnail(
                &json!({ "provider": "iframe", "url": "x", "thumbnail": "https://t/x.jpg" })
            ),
            "https://t/x.jpg"
        );
        assert!(video_needs_metadata(
            &json!({ "provider": "iframe", "url": "x" })
        ));
    }

    // -------------------------------------------------------------------------
    // Safety properties over generated input spaces (proptest).
    // -------------------------------------------------------------------------
    use crate::{merge_blocks, parse_markdown, render_html};
    use proptest::prop_assert;

    #[test]
    fn executable_url_schemes_are_blocked_at_the_renderer() {
        for url in [
            "javascript:alert(1)",
            "  javascript:alert(1)",
            "JAVASCRIPT:alert(1)",
            "java\nscript:alert(1)",
            "java\tscript:alert(1)",
            "vbscript:msgbox(1)",
            "file:///etc/passwd",
            "data:text/html,<script>alert(1)</script>",
            "data:text/html;base64,PHNjcmlwdD4=",
            "data:image/svg+xml,<svg onload=alert(1)>",
        ] {
            let md = format!("[click me]({url})");
            let html = html_of(&parse_markdown(&md));
            assert!(
                !html.contains(&format!("href=\"{url}\"")),
                "scheme must not reach an href: {url}"
            );
            assert!(
                !html.contains("<a href="),
                "no anchor at all for an executable scheme: {url}"
            );
            let img_md = format!("![]({url})");
            let img_html = html_of(&parse_markdown(&img_md));
            assert!(
                !img_html.contains(&format!("src=\"{url}\"")),
                "scheme must not reach an img src: {url}"
            );
        }
        // Encoded-colon tricks: the whole URL is entity-escaped before the
        // scheme check, so a raw `javascript:` (colon intact) must never
        // appear anywhere — any leftover is a harmless relative URL.
        let encoded = html_of(&parse_markdown("[x](javascript&#58;alert(1))"));
        assert!(
            !encoded.contains("javascript:"),
            "entity-obfuscated scheme must not form a live attribute"
        );
        // The safe surface keeps working.
        let ok = html_of(&parse_markdown(
            "[a](https://example.com) [b](/relative/path) ![](data:image/png;base64,AAAA)",
        ));
        assert!(ok.contains("href=\"https://example.com\""));
        assert!(ok.contains("href=\"/relative/path\""));
        // Standalone image line lifts into an Image block with an inert
        // data:image src.
        let img = html_of(&parse_markdown("![c](data:image/png;base64,AAAA)"));
        assert!(
            img.contains("src=\"data:image/png;base64,AAAA\""),
            "inert data:image src survives"
        );
    }

    // The rendered HTML for *arbitrary* Markdown never contains live markup:
    // every `<` is emitted by the renderer (user input is always escaped), and
    // no executable-scheme attribute can appear. A raw `href="javascript:...`
    // or `onload="...` substring can only be authored by our own attribute
    // writers, so testing for their absence pins the sanitizer policy exactly.
    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(128))]
        #[test]
        fn render_of_arbitrary_markdown_never_injects_live_markup(
            source in proptest::collection::vec(proptest::char::any(), 0..2048),
        ) {
            let source: String = source.into_iter().collect();
            let blocks = parse_markdown(&source);
            let html = html_of(&blocks);

            // No unescaped HTML element that could run scripts.
            for forbidden in ["<script", "<iframe", "<object", "<embed", "<svg", "<form", "<meta"] {
                prop_assert!(!html.contains(forbidden), "output contains {forbidden}");
            }
            // No event-handler attribute written with a raw `"`.
            for forbidden in ["onload=", "onerror=", "onclick=", "onmouseover=", "onfocus="] {
                prop_assert!(!html.contains(forbidden), "output contains {forbidden}");
            }
            // No executable scheme can surface as a real attribute value.
            prop_assert!(!html.contains("href=\"javascript:"), "javascript: reached an href");
            prop_assert!(!html.contains("src=\"javascript:"), "javascript: reached an src");
            prop_assert!(!html.contains("href=\"data:text/html"), "data:text/html reached an href");
            // The escape pipeline is total: every literal `<` from user input
            // became `&lt;`, so the raw characters survive only from writers.
            prop_assert!(!html.contains("</script>"), "closing script tag leaked");
        }
    }

    // Parsing and the diff/merge machinery must never panic, no matter how
    // adversarial the source text. Merge is exercised round-trip: create,
    // re-parse, merge again.
    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(64))]
        #[test]
        fn parse_and_merge_never_panic_on_arbitrary_source(
            a in proptest::collection::vec(proptest::char::any(), 0..2048),
            b in proptest::collection::vec(proptest::char::any(), 0..2048),
            now in 0i64..1_000_000,
        ) {
            let a: String = a.into_iter().collect();
            let b: String = b.into_iter().collect();
            let parsed_a = parse_markdown(&a);
            let parsed_b = parse_markdown(&b);
            // Fresh document from the first parse, then merge the second, then
            // merge an empty edit on top.
            let first = merge_blocks(&[], &[], parsed_a, now);
            let second = merge_blocks(&first.blocks, &first.versions, parsed_b, now);
            let third = merge_blocks(&second.blocks, &second.versions, Vec::new(), now + 1);
            prop_assert!(third.blocks.len() <= second.blocks.len());
        }
    }
}
