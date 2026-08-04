//! Document / block model with immutable versions.
//!
//! Content is a semantic document: an ordered list of blocks. Each block has a
//! stable id and points at its *current* immutable version. Block versions are
//! append-only; experiments attach as overlays pointing into the same immutable
//! version pool (§5.1 of the plan), so block-level analytics stay meaningful
//! across edits.

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

pub type DocumentId = Uuid;
pub type VersionId = Uuid;
pub type BlockId = Uuid;

/// The JSON payload of a single immutable block version.
pub type BlockContent = serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BlockKind {
    Paragraph,
    Heading { level: u8 },
    Image,
    Quote,
    Code,
    CallToAction,
    Divider,
}

impl BlockKind {
    /// Blocks that make sense to A/B test (§4, item 3).
    pub fn is_experimentable(&self) -> bool {
        matches!(
            self,
            BlockKind::Paragraph
                | BlockKind::Heading { .. }
                | BlockKind::Image
                | BlockKind::CallToAction
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockVersion {
    pub id: VersionId,
    pub block_id: BlockId,
    pub content: BlockContent,
    /// Unix timestamp in milliseconds.
    pub created_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Block {
    pub id: BlockId,
    pub kind: BlockKind,
    /// Current immutable version; older versions are append-only.
    pub version_id: VersionId,
    /// When the block last changed (Unix ms).
    pub updated_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Document {
    pub id: DocumentId,
    pub title: String,
    /// Ordered blocks of the document.
    pub blocks: Vec<Block>,
    /// Append-only history of every version ever created for these blocks.
    pub versions: Vec<BlockVersion>,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
}

impl Document {
    pub fn empty(title: impl Into<String>) -> Self {
        let now = now_ms();
        Self {
            id: Uuid::new_v4(),
            title: title.into(),
            blocks: Vec::new(),
            versions: Vec::new(),
            created_at_ms: now,
            updated_at_ms: now,
        }
    }

    pub fn block(&self, id: BlockId) -> Option<&Block> {
        self.blocks.iter().find(|b| b.id == id)
    }

    pub fn version(&self, id: VersionId) -> Option<&BlockVersion> {
        self.versions.iter().find(|v| v.id == id)
    }

    /// The content currently rendered for a block, if any.
    pub fn current_content(&self, id: BlockId) -> Option<&BlockContent> {
        let block = self.block(id)?;
        self.version(block.version_id).map(|v| &v.content)
    }
}

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_content_resolves_latest_version() {
        let mut doc = Document::empty("Test");
        let block_id = BlockId::new_v4();
        let v1 = BlockVersion {
            id: VersionId::new_v4(),
            block_id,
            content: serde_json::json!({ "text": "old" }),
            created_at_ms: 1,
        };
        let v2 = BlockVersion {
            id: VersionId::new_v4(),
            block_id,
            content: serde_json::json!({ "text": "new" }),
            created_at_ms: 2,
        };
        doc.blocks.push(Block {
            id: block_id,
            kind: BlockKind::Paragraph,
            version_id: v2.id,
            updated_at_ms: 2,
        });
        doc.versions.extend([v1, v2]);

        assert_eq!(
            doc.current_content(block_id),
            Some(&serde_json::json!({ "text": "new" }))
        );
        assert!(doc.block(BlockId::new_v4()).is_none());
    }

    #[test]
    fn headings_and_ctas_are_experimentable() {
        assert!(BlockKind::Heading { level: 2 }.is_experimentable());
        assert!(BlockKind::CallToAction.is_experimentable());
        assert!(!BlockKind::Divider.is_experimentable());
    }
}
