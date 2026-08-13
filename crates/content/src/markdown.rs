//! Line-based Markdown parser producing a block tree.
//!
//! Supports the MVP block kinds: headings, paragraphs, blockquotes, fenced
//! code, horizontal rules, image-only blocks, and flat lists (unordered `-`,
//! `*`, `+` and ordered `N.`). Images accept an optional `=H` or `=WxH` size
//! suffix after the URL (`![alt](src =80)`) that renders as `width`/`height`
//! attributes. Parsing is deliberately simple and lossless enough for the
//! semantic-document model. Inline formatting (`**bold**`, `*italic*`,
//! `` `code` ``, `[links](url)`) is rendered at display time by
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

        if let Some((alt, src, size)) = parse_image_line(trimmed) {
            let mut content = json!({ "src": src, "alt": alt });
            if !size.is_empty() {
                content["size"] = json!(size);
            }
            blocks.push(ParsedBlock {
                kind: BlockKind::Image,
                content,
            });
            i += 1;
            continue;
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

fn parse_image_line(line: &str) -> Option<(String, String, String)> {
    let line = line.trim();
    let rest = line.strip_prefix('!')?;
    let open = rest.find('[')?;
    let close = rest[open + 1..].find(']')?;
    let alt = rest[open + 1..open + 1 + close].to_string();
    let after = rest[open + 1 + close + 1..].trim_start();
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
    Some((alt, src, size))
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
        (w.is_empty() || digits(w)) && (h.is_empty() || digits(h)) && !(w.is_empty() && h.is_empty())
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
                let size = content.get("size").and_then(|v| v.as_str()).unwrap_or("");
                out.push_str(&format!(
                    "<p><img src=\"{}\" alt=\"{}\"{} /></p>\n",
                    html_escape(src),
                    html_escape(alt),
                    image_size_attrs(size)
                ));
            }
            BlockKind::Divider => out.push_str("<hr />\n"),
            BlockKind::CallToAction => {
                let text = text_of(content);
                out.push_str(&format!(
                    "<p><strong>{}</strong></p>\n",
                    render_inline(&text)
                ));
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
            out.push_str(&format!("<a href=\"{url}\">"));
            out.push_str(&inline_spans(label));
            out.push_str("</a>");
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
            "<p><img src=\"https://fdroid.gitlab.io/artwork/badge/get-it-on.png\" alt=\"Get it on F-Droid\" height=\"80\" /></p>\n"
        );
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
            "<p><img src=\"https://e.com/x.png =8.5\" alt=\"a\" /></p>\n"
        );
    }

    #[test]
    fn forged_size_cannot_inject_attributes() {
        let html = render_html(vec![(
            BlockKind::Image,
            &json!({ "src": "/x.png", "alt": "x", "size": "\" onerror=\"alert(1)" }),
        )]);
        assert_eq!(html, "<p><img src=\"/x.png\" alt=\"x\" /></p>\n");
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
}
