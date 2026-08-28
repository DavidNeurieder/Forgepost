//! Import existing Markdown posts (optionally bundled with their images in a
//! `.zip`) as new draft documents.
//!
//! Everything here is pure and side-effect free: the caller (the `/admin/import`
//! route) is responsible for persisting extracted images to the media store and
//! creating the document. Front matter is stripped *before* the content is
//! parsed by `forgepost_content`, because the Markdown parser has no front
//! matter support of its own (a leading `---` would otherwise render as
//! dividers and the keys as a paragraph).

use std::collections::HashSet;
use std::io::{Read, Seek};

use zip::ZipArchive;

/// Meta extracted from YAML-ish front matter. Deliberately minimal: only the
/// keys Forgepost understands are read; anything else is ignored and dropped.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct FrontMatter {
    pub title: Option<String>,
    pub tags: Vec<String>,
}

/// A markdown post plus the zip directory it lives in (used to resolve
/// relative image paths like `images/foo.png`).
#[derive(Debug)]
pub struct ExtractedPost {
    pub markdown: String,
    /// Zip path of the directory containing the markdown (`""` at the root).
    pub base_dir: String,
}

/// A local image reference discovered in the markdown that should be imported.
#[derive(Debug)]
pub struct LocalImage {
    /// The URL exactly as it appears in the markdown (map key for rewriting).
    pub url: String,
    pub alt: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("not a valid zip archive")]
    InvalidArchive,
    #[error("the zip contains no markdown (.md) file")]
    NoMarkdown,
    #[error(
        "the zip contains multiple markdown files ({}) — import one post at a time",
        .0.join(", ")
    )]
    MultipleMarkdown(Vec<String>),
    #[error("archive too large")]
    TooLarge,
    #[error("archive contains too many entries")]
    TooManyEntries,
}

/// Split optional YAML-ish front matter off the start of a markdown document.
///
/// The block is `---` delimited; only `title:` and `tags:` (comma separated)
/// are read. Returns `(Some(meta), body)` when a well-formed block is present,
/// `(None, source)` otherwise (source untouched).
pub fn parse_front_matter(source: &str) -> (Option<FrontMatter>, String) {
    let trimmed = source.trim_start();
    if !trimmed.starts_with("---\n") {
        return (None, source.to_string());
    }
    let rest = &trimmed[4..];
    let Some(end) = rest.find("\n---") else {
        return (None, source.to_string());
    };
    let mut meta = FrontMatter::default();
    for line in rest[..end].lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim().to_ascii_lowercase().as_str() {
            "title" => meta.title = Some(value.to_string()),
            "tags" => {
                meta.tags = value
                    .split(',')
                    .map(|s| s.trim().trim_matches('"').to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            _ => {}
        }
    }
    let body = rest[end + 4..].trim_start().to_string();
    (Some(meta), body)
}

/// Pull the single markdown file out of a zip archive.
pub fn extract_post<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    max_total_bytes: u64,
    max_entries: usize,
) -> Result<ExtractedPost, ImportError> {
    if archive.len() > max_entries {
        return Err(ImportError::TooManyEntries);
    }
    let mut md_paths = Vec::new();
    let mut total = 0u64;
    for i in 0..archive.len() {
        let entry = archive
            .by_index(i)
            .map_err(|_| ImportError::InvalidArchive)?;
        if entry.is_dir() {
            continue;
        }
        total = total.saturating_add(entry.size());
        if total > max_total_bytes {
            return Err(ImportError::TooLarge);
        }
        if entry.name().to_ascii_lowercase().ends_with(".md") {
            md_paths.push(entry.name().to_string());
        }
    }
    if md_paths.is_empty() {
        return Err(ImportError::NoMarkdown);
    }
    if md_paths.len() > 1 {
        return Err(ImportError::MultipleMarkdown(md_paths));
    }
    let md_name = &md_paths[0];
    let mut file = archive
        .by_name(md_name)
        .map_err(|_| ImportError::InvalidArchive)?;
    let mut content = Vec::new();
    file.read_to_end(&mut content)
        .map_err(|_| ImportError::InvalidArchive)?;
    let base_dir = md_name
        .rfind('/')
        .map(|idx| md_name[..idx].to_string())
        .unwrap_or_default();
    Ok(ExtractedPost {
        markdown: String::from_utf8_lossy(&content).into_owned(),
        base_dir,
    })
}

/// Collect every local (non-remote) image reference in the markdown and load
/// its bytes from the zip archive, resolving paths relative to `base_dir`.
///
/// Remote (`http(s)://`, `data:`, absolute `/`, anchor `#`) references are
/// never touched. References whose file is missing, unreadable, or larger than
/// `max_bytes` are skipped so the import can continue; the rewrite pass leaves
/// them as-is.
pub fn scan_local_images<R: Read + Seek>(
    markdown: &str,
    base_dir: &str,
    archive: &mut ZipArchive<R>,
    max_bytes: u64,
) -> Vec<LocalImage> {
    let mut images = Vec::new();
    let mut seen = HashSet::new();
    for_each_image_ref(markdown, &mut |alt: &str, url: &str| {
        if looks_remote(url) || !seen.insert(url.to_string()) {
            return;
        }
        if let Some(bytes) = read_zip_image(archive, base_dir, url, max_bytes) {
            images.push(LocalImage {
                url: url.to_string(),
                alt: alt.to_string(),
                bytes,
            });
        }
    });
    images
}

/// Rewrite image references in the markdown, calling `resolve(alt, url)` for
/// every non-remote reference. Returns the rewritten markdown plus counts of
/// rewritten and unresolved references.
pub fn rewrite_image_refs(
    markdown: &str,
    resolve: &mut dyn FnMut(&str, &str) -> Option<String>,
) -> (String, usize, usize) {
    let mut out = String::with_capacity(markdown.len());
    let mut imported = 0usize;
    let mut unresolved = 0usize;
    let mut rest = markdown;
    while let Some(idx) = rest.find("![") {
        out.push_str(&rest[..idx]);
        let after = &rest[idx + 2..];
        let Some(close) = after.find(']') else {
            out.push_str("![");
            rest = after;
            continue;
        };
        let alt = &after[..close];
        let tail = &after[close + 1..];
        let Some(paren) = tail.strip_prefix('(') else {
            out.push_str("![");
            out.push_str(&after[..=close]);
            rest = tail;
            continue;
        };
        let Some(end) = paren.find(')') else {
            out.push_str("![");
            out.push_str(&after[..=close]);
            out.push('(');
            rest = paren;
            continue;
        };
        let url = &paren[..end];
        let rest_after = &paren[end + 1..];
        match resolve(alt, url) {
            Some(new_url) => {
                out.push_str(&format!("![{alt}]({new_url})"));
                imported += 1;
            }
            None => {
                out.push_str(&format!("![{alt}]({url})"));
                if !looks_remote(url) {
                    unresolved += 1;
                }
            }
        }
        rest = rest_after;
    }
    out.push_str(rest);
    (out, imported, unresolved)
}

/// Visit every `![alt](url)` reference in the markdown, in order.
fn for_each_image_ref(markdown: &str, mut f: impl FnMut(&str, &str)) {
    let mut rest = markdown;
    while let Some(idx) = rest.find("![") {
        let after = &rest[idx + 2..];
        let Some(close) = after.find(']') else {
            return;
        };
        let alt = &after[..close];
        let tail = &after[close + 1..];
        let Some(paren) = tail.strip_prefix('(') else {
            rest = tail;
            continue;
        };
        let Some(end) = paren.find(')') else {
            return;
        };
        f(alt, &paren[..end]);
        rest = &paren[end + 1..];
    }
}

fn looks_remote(url: &str) -> bool {
    url.starts_with("http://")
        || url.starts_with("https://")
        || url.starts_with("data:")
        || url.starts_with('/')
        || url.starts_with('#')
}

/// Find and read a single image from the archive for `url` (relative to
/// `base_dir`). Handles percent-encoded and `\`-separated paths.
fn read_zip_image<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    base_dir: &str,
    url: &str,
    max_bytes: u64,
) -> Option<Vec<u8>> {
    let decoded = percent_decode(url);
    let normalized = decoded
        .replace('\\', "/")
        .trim_start_matches('/')
        .to_string();
    if normalized.is_empty() {
        return None;
    }
    let joined = if base_dir.is_empty() {
        normalized
    } else {
        format!("{base_dir}/{normalized}")
    };
    let candidates = [joined.clone(), joined.replace('/', "\\")];
    for name in candidates {
        let Ok(mut entry) = archive.by_name(&name) else {
            continue;
        };
        if entry.size() > max_bytes {
            return None;
        }
        let mut bytes = Vec::new();
        if entry.read_to_end(&mut bytes).is_ok() && bytes.len() as u64 <= max_bytes {
            return Some(bytes);
        }
        return None;
    }
    None
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(hi), Some(lo)) = (hex_val(bytes[i + 1]), hex_val(bytes[i + 2]))
        {
            out.push(hi * 16 + lo);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn zip_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, data) in entries {
                writer.start_file(*name, opts).unwrap();
                writer.write_all(data).unwrap();
            }
            writer.finish().unwrap();
        }
        buf.into_inner()
    }

    #[test]
    fn front_matter_is_stripped_and_parsed() {
        let source = "---\ntitle: Hello Post\ntags: tech, blog\ndraft: true\n---\n\n# Body\n\ntext";
        let (meta, body) = parse_front_matter(source);
        let meta = meta.expect("front matter present");
        assert_eq!(meta.title.as_deref(), Some("Hello Post"));
        assert_eq!(meta.tags, vec!["tech", "blog"]);
        assert_eq!(body, "# Body\n\ntext");
    }

    #[test]
    fn front_matter_absent_returns_source_untouched() {
        let source = "# No front matter\n\ntext";
        let (meta, body) = parse_front_matter(source);
        assert!(meta.is_none());
        assert_eq!(body, source);
    }

    #[test]
    fn rewrite_leaves_remote_refs_and_rewrites_local() {
        let md = "![a](https://x/y.png)\n\n![b](images/foo.png)\n\n![c](data:image/png;base64,x)\n\n![d](/abs.png)";
        let mut resolver = |_alt: &str, url: &str| {
            if url == "images/foo.png" {
                Some("/media/abc.png".to_string())
            } else {
                None
            }
        };
        let (out, imported, unresolved) = rewrite_image_refs(md, &mut resolver);
        assert_eq!(imported, 1);
        assert_eq!(unresolved, 0);
        assert!(out.contains("![b](/media/abc.png)"));
        assert!(out.contains("![a](https://x/y.png)"));
        assert!(out.contains("![c](data:image/png;base64,x)"));
        assert!(out.contains("![d](/abs.png)"));
    }

    #[test]
    fn unresolved_local_ref_is_kept_and_counted() {
        let md = "![x](missing.png)";
        let mut resolver = |_alt: &str, _url: &str| None::<String>;
        let (out, imported, unresolved) = rewrite_image_refs(md, &mut resolver);
        assert_eq!(imported, 0);
        assert_eq!(unresolved, 1);
        assert_eq!(out, "![x](missing.png)");
    }

    #[test]
    fn extract_post_requires_exactly_one_markdown() {
        let bytes = zip_with(&[("post.md", b"# Hi"), ("images/a.png", b"png")]);
        let mut archive = ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let post = extract_post(&mut archive, 10_000, 100).unwrap();
        assert_eq!(post.markdown, "# Hi");
        assert_eq!(post.base_dir, "");

        let bytes = zip_with(&[("a.md", b"a"), ("b.md", b"b")]);
        let mut archive = ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert!(matches!(
            extract_post(&mut archive, 10_000, 100),
            Err(ImportError::MultipleMarkdown(_))
        ));

        let bytes = zip_with(&[("a.txt", b"a")]);
        let mut archive = ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert!(matches!(
            extract_post(&mut archive, 10_000, 100),
            Err(ImportError::NoMarkdown)
        ));
    }

    #[test]
    fn scan_loads_images_from_nested_dirs() {
        let png = b"\x89PNG\r\n\x1a\n0123456789";
        let bytes = zip_with(&[
            ("posts/x.md", b"# Post\n\n![p](images/p%20ic.png)"),
            ("posts/images/p ic.png", png),
        ]);
        let mut archive = ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let post = extract_post(&mut archive, 10_000, 100).unwrap();
        assert_eq!(post.base_dir, "posts");
        let images = scan_local_images(&post.markdown, &post.base_dir, &mut archive, 100);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].url, "images/p%20ic.png");
        assert_eq!(images[0].bytes, png);
    }

    #[test]
    fn scan_skips_missing_and_remote() {
        let bytes = zip_with(&[("post.md", b"# Hi"), ("images/a.png", b"x")]);
        let mut archive = ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let post = extract_post(&mut archive, 10_000, 100).unwrap();
        let md = "![remote](https://x/a.png)\n\n![missing](nope.png)\n\n![ok](images/a.png)";
        let images = scan_local_images(md, &post.base_dir, &mut archive, 100);
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].url, "images/a.png");
    }

    // -------------------------------------------------------------------------
    // No-panic / round-trip properties over generated input spaces (proptest).
    // -------------------------------------------------------------------------
    use proptest::{prop_assert, prop_assert_eq};

    proptest::proptest! {
        #![proptest_config(proptest::test_runner::Config::with_cases(64))]
        #[test]
        fn front_matter_parse_never_panics(
            source in proptest::collection::vec(proptest::char::any(), 0..4096),
        ) {
            let source: String = source.into_iter().collect();
            let (meta, body) = parse_front_matter(&source);
            match meta {
                // No well-formed block: source must be returned untouched.
                None => prop_assert_eq!(body, source),
                // Otherwise the body is a strict suffix-derived remainder and
                // the keys are unescaped strings bounded by the input length.
                Some(_) => {
                    prop_assert!(body.len() <= source.len());
                    prop_assert!(source.contains(&body) || body.len() == source.len());
                }
            }
        }

        #[test]
        fn rewrite_with_no_successful_resolution_is_lossless(
            source in proptest::collection::vec(proptest::char::any(), 0..4096),
        ) {
            let source: String = source.into_iter().collect();
            let mut resolver = |_alt: &str, _url: &str| None::<String>;
            let (out, imported, unresolved) = rewrite_image_refs(&source, &mut resolver);
            prop_assert_eq!(imported, 0);
            prop_assert_eq!(&out, &source);
            // Unresolved refs cannot exceed the number of image references.
            prop_assert!(unresolved <= source.matches("![").count());
        }

        #[test]
        fn extract_post_never_panics_and_base_dir_is_safe(
            name in "[a-zA-Z0-9._/ -]{1,80}",
            extra in proptest::collection::vec(proptest::char::any(), 0..300),
            data in proptest::collection::vec(0u8..255, 0..256),
        ) {
            let extra: String = extra.into_iter().collect();
            let mut owned = vec![(name.clone(), data)];
            if !extra.is_empty() {
                owned.push((format!("extra-{extra}.md"), b"# extra".to_vec()));
            }
            let refs: Vec<(&str, &[u8])> = owned
                .iter()
                .map(|(n, d)| (n.as_str(), d.as_slice()))
                .collect();
            let bytes = zip_with(&refs);
            let mut archive = ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
            match extract_post(&mut archive, 1024 * 64, 512) {
                Ok(post) => {
                    // A base dir derived from an archive entry name is never a
                    // drive root; the import layer only ever uses it as a
                    // prefix looked up inside the archive itself.
                    prop_assert!(!post.base_dir.starts_with('/'));
                }
                Err(e) => {
                    prop_assert!(matches!(
                        e,
                        ImportError::NoMarkdown
                            | ImportError::MultipleMarkdown(_)
                            | ImportError::TooLarge
                            | ImportError::TooManyEntries
                    ));
                }
            }
        }
    }

    #[test]
    fn extract_post_rejects_oversized_and_overpopulated_archives() {
        let big = vec![0u8; 1024];
        let bytes = zip_with(&[("big.md", big.as_slice())]);
        let mut archive = ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert!(matches!(
            extract_post(&mut archive, 512, 10),
            Err(ImportError::TooLarge)
        ));
        let many: Vec<(String, Vec<u8>)> = (0..100)
            .map(|i| (format!("f{i:03}.md"), b"x".to_vec()))
            .collect();
        let refs: Vec<(&str, &[u8])> = many
            .iter()
            .map(|(n, d)| (n.as_str(), d.as_slice()))
            .collect();
        let bytes = zip_with(&refs);
        let mut archive = ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert!(matches!(
            extract_post(&mut archive, 1024 * 1024, 50),
            Err(ImportError::TooManyEntries)
        ));
    }
}
