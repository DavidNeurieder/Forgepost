//! Line-based Markdown parser producing a block tree.
//!
//! Supports the MVP block kinds: headings, paragraphs, blockquotes, fenced
//! code, horizontal rules, and image-only blocks. Parsing is deliberately
//! simple and lossless enough for the semantic-document model; inline
//! formatting is preserved as raw text and rendered later.

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

        if let Some((alt, src)) = parse_image_line(trimmed) {
            blocks.push(ParsedBlock {
                kind: BlockKind::Image,
                content: json!({ "src": src, "alt": alt }),
            });
            i += 1;
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

fn parse_image_line(line: &str) -> Option<(String, String)> {
    let line = line.trim();
    let rest = line.strip_prefix('!')?;
    let open = rest.find('[')?;
    let close = rest[open + 1..].find(']')?;
    let alt = rest[open + 1..open + 1 + close].to_string();
    let after = rest[open + 1 + close + 1..].trim_start();
    let paren_open = after.strip_prefix('(')?;
    let paren_close = paren_open.find(')')?;
    let src = paren_open[..paren_close].trim().to_string();
    if src.is_empty() {
        return None;
    }
    Some((alt, src))
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

/// Render blocks to HTML. Inline Markdown (links, emphasis) is not parsed yet;
/// text is escaped and line breaks are preserved as paragraphs/preformatted.
pub fn render_html<'a>(blocks: impl IntoIterator<Item = (BlockKind, &'a BlockContent)>) -> String {
    let mut out = String::new();
    for (kind, content) in blocks {
        match kind {
            BlockKind::Heading { level } => {
                let text = text_of(content);
                out.push_str(&format!(
                    "<h{}>{}</h{}>\n",
                    level,
                    html_escape(&text),
                    level
                ));
            }
            BlockKind::Paragraph => {
                let text = text_of(content);
                out.push_str(&format!("<p>{}</p>\n", html_escape(&text)));
            }
            BlockKind::Quote => {
                let text = text_of(content);
                out.push_str(&format!(
                    "<blockquote>{}</blockquote>\n",
                    html_escape(&text)
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
                out.push_str(&format!(
                    "<p><img src=\"{}\" alt=\"{}\" /></p>\n",
                    html_escape(src),
                    html_escape(alt)
                ));
            }
            BlockKind::Divider => out.push_str("<hr />\n"),
            BlockKind::CallToAction => {
                let text = text_of(content);
                out.push_str(&format!("<p><strong>{}</strong></p>\n", html_escape(&text)));
            }
        }
    }
    out
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
}
