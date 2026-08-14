use crate::{ArtifactId, DisplayLabel, FileId, FileVersionId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvidenceLocator {
    Text {
        start: usize,
        end: usize,
        line_start: Option<u32>,
        line_end: Option<u32>,
    },
    Page {
        page: u32,
        normalized_box: Option<[f32; 4]>,
    },
    Media {
        start_ms: u64,
        end_ms: u64,
        frame: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EvidenceRef {
    pub artifact_id: ArtifactId,
    pub file_version_id: FileVersionId,
    pub display_label: DisplayLabel,
    pub locator: EvidenceLocator,
    pub excerpt: String,
    pub excerpt_digest: [u8; 32],
    pub explanation: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchDocument {
    pub file_id: FileId,
    pub file_version_id: FileVersionId,
    pub display_label: DisplayLabel,
    pub title: String,
    pub body: String,
    pub detected_mime: Option<String>,
    pub language: Option<String>,
    pub lexical_tokens: Vec<String>,
    pub embedding: Option<Vec<f32>>,
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub file_id: FileId,
    pub file_version_id: FileVersionId,
    pub display_label: DisplayLabel,
    pub summary: String,
    pub score: f32,
    pub lexical_rank: Option<usize>,
    pub semantic_rank: Option<usize>,
    pub evidence: Vec<EvidenceRef>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResponse {
    pub query: String,
    pub hits: Vec<SearchHit>,
}
