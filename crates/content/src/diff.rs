//! Stable block identity across edits.
//!
//! This is the plan's #2 implementation risk: block IDs must survive edits so
//! per-block analytics and block experiments stay meaningful. We match new
//! blocks against previous ones with a longest-common-subsequence on block
//! *kind*, so:
//!
//! - edited blocks keep their id and gain a new immutable version;
//! - inserts and deletes leave the surrounding blocks' ids untouched;
//! - content-identical blocks reuse their existing version pointer.
//!
//! Deleted blocks are dropped from the document but their versions remain in
//! the append-only store (and any experiment overlays still reference them).

use crate::{Block, BlockContent, BlockKind, BlockVersion, ParsedBlock, VersionId};

pub struct MergeResult {
    /// Final ordered blocks with resolved ids and version pointers.
    pub blocks: Vec<Block>,
    /// Only the *new* immutable versions that must be persisted.
    pub versions: Vec<BlockVersion>,
}

/// Merge a freshly parsed block list into the previous document state.
pub fn merge_blocks(
    previous: &[Block],
    previous_versions: &[BlockVersion],
    parsed: Vec<ParsedBlock>,
    now_ms: i64,
) -> MergeResult {
    let prev_kinds: Vec<BlockKind> = previous.iter().map(|b| b.kind).collect();
    let new_kinds: Vec<BlockKind> = parsed.iter().map(|b| b.kind).collect();
    let matches = lcs_matches(&prev_kinds, &new_kinds);
    // Map new index -> previous index (LCS already guarantees increasing order).
    let match_for: std::collections::HashMap<usize, usize> =
        matches.into_iter().map(|(p, n)| (n, p)).collect();

    let current_content = |block: &Block| -> Option<&BlockContent> {
        previous_versions
            .iter()
            .find(|v| v.id == block.version_id)
            .map(|v| &v.content)
    };

    let mut blocks = Vec::with_capacity(parsed.len());
    let mut versions = Vec::new();

    for (position, parsed_block) in parsed.into_iter().enumerate() {
        let block = match match_for.get(&position) {
            Some(&prev_idx) => {
                let prev = &previous[prev_idx];
                let content_changed = match parsed_block.kind {
                    // Video identity is the provider/id/url triple; oEmbed
                    // fields (title, thumbnail) are derived metadata, so a
                    // refreshed fetch must never mint a new version.
                    BlockKind::Video => {
                        current_content(prev).map(crate::video_identity)
                            != Some(crate::video_identity(&parsed_block.content))
                    }
                    _ => current_content(prev) != Some(&parsed_block.content),
                };
                if content_changed {
                    let version_id = VersionId::new_v4();
                    versions.push(BlockVersion {
                        id: version_id,
                        block_id: prev.id,
                        content: parsed_block.content,
                        created_at_ms: now_ms,
                    });
                    Block {
                        id: prev.id,
                        kind: parsed_block.kind,
                        version_id,
                        position: 0,
                        created_at_ms: prev.created_at_ms,
                        updated_at_ms: now_ms,
                    }
                } else {
                    Block {
                        id: prev.id,
                        kind: prev.kind,
                        version_id: prev.version_id,
                        position: 0,
                        created_at_ms: prev.created_at_ms,
                        updated_at_ms: prev.updated_at_ms,
                    }
                }
            }
            None => {
                let id = crate::BlockId::new_v4();
                let version_id = VersionId::new_v4();
                versions.push(BlockVersion {
                    id: version_id,
                    block_id: id,
                    content: parsed_block.content,
                    created_at_ms: now_ms,
                });
                Block {
                    id,
                    kind: parsed_block.kind,
                    version_id,
                    position: 0,
                    created_at_ms: now_ms,
                    updated_at_ms: now_ms,
                }
            }
        };
        blocks.push(block);
    }

    // Reassign contiguous positions in document order.
    for (position, block) in blocks.iter_mut().enumerate() {
        let _ = position;
        block.position = position as i64;
    }

    MergeResult { blocks, versions }
}

/// Longest-common-subsequence of block kinds, returning matched index pairs
/// `(prev_index, new_index)` in document order.
fn lcs_matches(prev: &[BlockKind], new: &[BlockKind]) -> Vec<(usize, usize)> {
    let (n, m) = (prev.len(), new.len());
    let mut table = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if prev[i] == new[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let mut pairs = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if prev[i] == new[j] {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn block(id: crate::BlockId, kind: BlockKind, v: VersionId) -> Block {
        Block {
            id,
            kind,
            version_id: v,
            position: 0,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    fn version(v: VersionId, block_id: crate::BlockId, content: BlockContent) -> BlockVersion {
        BlockVersion {
            id: v,
            block_id,
            content,
            created_at_ms: 0,
        }
    }

    fn p(kind: BlockKind, text: &str) -> ParsedBlock {
        ParsedBlock {
            kind,
            content: json!({ "text": text }),
        }
    }

    #[test]
    fn unchanged_document_reuses_ids_and_versions() {
        let b1 = crate::BlockId::new_v4();
        let b2 = crate::BlockId::new_v4();
        let v1 = VersionId::new_v4();
        let v2 = VersionId::new_v4();
        let previous = vec![
            block(b1, BlockKind::Paragraph, v1),
            block(b2, BlockKind::Paragraph, v2),
        ];
        let versions = vec![
            version(v1, b1, json!({ "text": "a" })),
            version(v2, b2, json!({ "text": "b" })),
        ];
        let parsed = vec![p(BlockKind::Paragraph, "a"), p(BlockKind::Paragraph, "b")];

        let result = merge_blocks(&previous, &versions, parsed, 1000);
        assert_eq!(result.blocks.len(), 2);
        assert_eq!(result.blocks[0].id, b1);
        assert_eq!(result.blocks[0].version_id, v1);
        assert_eq!(result.blocks[1].id, b2);
        assert_eq!(result.blocks[1].version_id, v2);
        assert!(result.versions.is_empty());
    }

    #[test]
    fn edited_paragraph_keeps_id_and_adds_version() {
        let b1 = crate::BlockId::new_v4();
        let v1 = VersionId::new_v4();
        let previous = vec![block(b1, BlockKind::Paragraph, v1)];
        let versions = vec![version(v1, b1, json!({ "text": "old" }))];
        let parsed = vec![p(BlockKind::Paragraph, "new")];

        let result = merge_blocks(&previous, &versions, parsed, 1000);
        assert_eq!(result.blocks.len(), 1);
        assert_eq!(result.blocks[0].id, b1, "id survives the edit");
        assert_ne!(result.blocks[0].version_id, v1, "new version");
        assert_eq!(result.versions.len(), 1);
        assert_eq!(result.versions[0].content, json!({ "text": "new" }));
        assert_eq!(result.versions[0].block_id, b1);
    }

    #[test]
    fn insert_keeps_surrounding_ids() {
        let b1 = crate::BlockId::new_v4();
        let b2 = crate::BlockId::new_v4();
        let v1 = VersionId::new_v4();
        let v2 = VersionId::new_v4();
        let previous = vec![
            block(b1, BlockKind::Paragraph, v1),
            block(b2, BlockKind::Paragraph, v2),
        ];
        let versions = vec![
            version(v1, b1, json!({ "text": "a" })),
            version(v2, b2, json!({ "text": "b" })),
        ];
        let parsed = vec![
            p(BlockKind::Paragraph, "a"),
            p(BlockKind::Heading { level: 2 }, "inserted"),
            p(BlockKind::Paragraph, "b"),
        ];

        let result = merge_blocks(&previous, &versions, parsed, 1000);
        assert_eq!(result.blocks.len(), 3);
        assert_eq!(result.blocks[0].id, b1);
        assert_eq!(result.blocks[2].id, b2, "block after insert keeps id");
        assert_eq!(result.blocks[1].kind, BlockKind::Heading { level: 2 });
        assert!(result.blocks[1].id != b1 && result.blocks[1].id != b2);
        // One new version (the inserted block).
        assert_eq!(result.versions.len(), 1);
    }

    #[test]
    fn replaced_kind_creates_new_block() {
        let b1 = crate::BlockId::new_v4();
        let v1 = VersionId::new_v4();
        let previous = vec![block(b1, BlockKind::Image, v1)];
        let versions = vec![version(v1, b1, json!({ "src": "x", "alt": "" }))];
        let parsed = vec![p(BlockKind::Paragraph, "replaced image with text")];

        let result = merge_blocks(&previous, &versions, parsed, 1000);
        assert_eq!(result.blocks.len(), 1);
        assert_ne!(result.blocks[0].id, b1, "image block is replaced");
        assert_eq!(result.blocks[0].kind, BlockKind::Paragraph);
    }

    #[test]
    fn video_metadata_change_does_not_mint_a_new_version() {
        let b1 = crate::BlockId::new_v4();
        let v1 = VersionId::new_v4();
        let previous = vec![block(b1, BlockKind::Video, v1)];
        let versions = vec![version(
            v1,
            b1,
            json!({ "provider": "rumble", "id": "1abc2", "url": "https://rumble.com/v1abc2-x.html", "title": "Old", "thumbnail": "https://old/x.jpg" }),
        )];
        let parsed = vec![ParsedBlock {
            kind: BlockKind::Video,
            content: json!({ "provider": "rumble", "id": "1abc2", "url": "https://rumble.com/v1abc2-x.html", "title": "New", "thumbnail": "https://new/x.jpg" }),
        }];

        let result = merge_blocks(&previous, &versions, parsed, 1000);
        assert_eq!(result.blocks[0].id, b1);
        assert_eq!(
            result.blocks[0].version_id, v1,
            "metadata-only change keeps version"
        );
        assert!(result.versions.is_empty());
    }

    #[test]
    fn video_identity_change_does_mint_a_new_version() {
        let b1 = crate::BlockId::new_v4();
        let v1 = VersionId::new_v4();
        let previous = vec![block(b1, BlockKind::Video, v1)];
        let versions = vec![version(
            v1,
            b1,
            json!({ "provider": "youtube", "id": "aaa", "url": "https://youtu.be/aaa" }),
        )];
        let parsed = vec![ParsedBlock {
            kind: BlockKind::Video,
            content: json!({ "provider": "youtube", "id": "bbb", "url": "https://youtu.be/bbb" }),
        }];

        let result = merge_blocks(&previous, &versions, parsed, 1000);
        assert_ne!(
            result.blocks[0].version_id, v1,
            "different video = new version"
        );
        assert_eq!(result.versions.len(), 1);
    }

    #[test]
    fn positions_are_contiguous() {
        let parsed = vec![
            p(BlockKind::Paragraph, "a"),
            p(BlockKind::Divider, ""),
            p(BlockKind::Paragraph, "c"),
        ];
        let result = merge_blocks(&[], &[], parsed, 1000);
        let positions: Vec<i64> = result.blocks.iter().map(|b| b.position).collect();
        assert_eq!(positions, vec![0, 1, 2]);
    }
}
