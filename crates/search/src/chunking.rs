//! Deterministic, tokenizer-aware, structure-aware semantic chunking (M9.1 Step 2).

use crate::{EmbeddingDocument, EmbeddingInput, MAX_EMBEDDING_INPUT_CHARS};
use serde::{Deserialize, Serialize};

/// Centralized chunking policy versioned for index compatibility.
pub const CHUNKING_POLICY_VERSION: &str = "chunking-v1-granite-384";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChunkingPolicy {
    /// Soft target tokens per text chunk.
    pub target_tokens: usize,
    /// Hard max tokens per embedding unit (below model max 512).
    pub max_tokens: usize,
    /// Overlap tokens between consecutive text chunks.
    pub overlap_tokens: usize,
    /// Metadata summary always counts as one chunk when present.
    pub max_text_chunks: usize,
    /// Approximate chars/token used when a real tokenizer is unavailable.
    pub approx_chars_per_token: usize,
}

impl Default for ChunkingPolicy {
    fn default() -> Self {
        Self {
            target_tokens: 256,
            max_tokens: 480,
            overlap_tokens: 32,
            max_text_chunks: 16,
            approx_chars_per_token: 3,
        }
    }
}

impl ChunkingPolicy {
    #[must_use]
    pub fn version() -> &'static str {
        CHUNKING_POLICY_VERSION
    }

    #[must_use]
    pub fn max_chunks_total(&self) -> usize {
        self.max_text_chunks.saturating_add(1)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticChunk {
    pub source_id: String,
    pub source_kind: String,
    pub sequence_index: u32,
    pub text: String,
    pub text_hash: [u8; 32],
    pub start_offset: Option<usize>,
    pub end_offset: Option<usize>,
    pub page_number: Option<u32>,
    pub sheet_or_slide: Option<String>,
    pub truncated_file: bool,
}

/// Counts tokens for chunking. Production may use a real tokenizer; tests use approximation.
pub trait TokenCounter: Send + Sync {
    fn count_tokens(&self, text: &str) -> usize;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ApproximateTokenCounter {
    pub chars_per_token: usize,
}

impl ApproximateTokenCounter {
    #[must_use]
    pub fn from_policy(policy: &ChunkingPolicy) -> Self {
        Self {
            chars_per_token: policy.approx_chars_per_token.max(1),
        }
    }
}

impl TokenCounter for ApproximateTokenCounter {
    fn count_tokens(&self, text: &str) -> usize {
        let chars = text.chars().count();
        chars
            .div_ceil(self.chars_per_token)
            .max(usize::from(!text.is_empty()))
    }
}

/// Optional HuggingFace tokenizer counter for production-accurate bounds.
pub struct HfTokenCounter {
    tokenizer: tokenizers::Tokenizer,
}

impl std::fmt::Debug for HfTokenCounter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HfTokenCounter").finish_non_exhaustive()
    }
}

impl HfTokenCounter {
    pub fn from_file(path: &std::path::Path) -> Result<Self, String> {
        let tokenizer = tokenizers::Tokenizer::from_file(path).map_err(|e| e.to_string())?;
        Ok(Self { tokenizer })
    }
}

impl TokenCounter for HfTokenCounter {
    fn count_tokens(&self, text: &str) -> usize {
        self.tokenizer
            .encode(text, false)
            .map(|encoding| encoding.len())
            .unwrap_or_else(|_| ApproximateTokenCounter { chars_per_token: 3 }.count_tokens(text))
    }
}

#[must_use]
pub fn chunk_embedding_document(
    document: &EmbeddingDocument,
    policy: &ChunkingPolicy,
    counter: &dyn TokenCounter,
) -> Vec<SemanticChunk> {
    let mut chunks = Vec::new();
    let mut truncated = false;

    let metadata = build_metadata_summary(document);
    if !metadata.trim().is_empty() {
        let text = truncate_to_tokens(&metadata, policy.max_tokens, counter);
        chunks.push(make_chunk(
            "metadata",
            "semantic_summary",
            0,
            text,
            None,
            None,
            None,
            None,
            false,
        ));
    }

    let segments = structure_aware_segments(&document.extracted_text);
    let mut sequence = 1_u32;
    let mut text_chunks = 0_usize;
    let mut char_cursor = 0_usize;

    for segment in segments {
        if text_chunks >= policy.max_text_chunks {
            truncated = true;
            break;
        }
        let segment_start = char_cursor;
        let segment_chars = segment.text.chars().count();
        char_cursor = char_cursor.saturating_add(segment_chars);

        let windows = tokenize_windows(&segment.text, policy, counter);
        for window in windows {
            if text_chunks >= policy.max_text_chunks {
                truncated = true;
                break;
            }
            let absolute_start = segment_start.saturating_add(window.start_char);
            let absolute_end = segment_start.saturating_add(window.end_char);
            chunks.push(make_chunk(
                &format!("text:{}", sequence.saturating_sub(1)),
                "text_chunk",
                sequence,
                window.text,
                Some(absolute_start),
                Some(absolute_end),
                segment.page_number,
                segment.sheet_or_slide.clone(),
                false,
            ));
            sequence = sequence.saturating_add(1);
            text_chunks = text_chunks.saturating_add(1);
        }
    }

    if truncated {
        for chunk in &mut chunks {
            chunk.truncated_file = true;
        }
    }
    chunks
}

#[must_use]
pub fn semantic_chunks_to_embedding_inputs(chunks: &[SemanticChunk]) -> Vec<EmbeddingInput> {
    chunks
        .iter()
        .map(|chunk| EmbeddingInput {
            source_id: chunk.source_id.clone(),
            source_kind: chunk.source_kind.clone(),
            text: chunk.text.chars().take(MAX_EMBEDDING_INPUT_CHARS).collect(),
            start_offset: chunk.start_offset,
            end_offset: chunk.end_offset,
        })
        .collect()
}

/// Backward-compatible wrapper used by existing call sites.
#[must_use]
pub fn bounded_embedding_inputs_v2(document: &EmbeddingDocument) -> Vec<EmbeddingInput> {
    let policy = ChunkingPolicy::default();
    let counter = ApproximateTokenCounter::from_policy(&policy);
    let chunks = chunk_embedding_document(document, &policy, &counter);
    semantic_chunks_to_embedding_inputs(&chunks)
}

#[derive(Debug, Clone)]
struct TextSegment {
    text: String,
    page_number: Option<u32>,
    sheet_or_slide: Option<String>,
}

#[derive(Debug, Clone)]
struct TextWindow {
    text: String,
    start_char: usize,
    end_char: usize,
}

fn build_metadata_summary(document: &EmbeddingDocument) -> String {
    let mut metadata = format!("filename: {}", bounded_chars(&document.filename, 256));
    for (kind, value) in document.semantic_fields.iter().take(24) {
        metadata.push_str(&format!(
            "\n{}: {}",
            bounded_chars(kind, 64),
            bounded_chars(value, 256)
        ));
    }
    for (kind, value) in document.identities.iter().take(24) {
        metadata.push_str(&format!(
            "\n{}: {}",
            bounded_chars(kind, 64),
            bounded_chars(value, 256)
        ));
    }
    metadata
}

fn structure_aware_segments(text: &str) -> Vec<TextSegment> {
    if text.trim().is_empty() {
        return Vec::new();
    }

    // Form-feed / explicit page breaks (PDF flatteners).
    if text.contains('\u{000C}') {
        return text
            .split('\u{000C}')
            .enumerate()
            .filter_map(|(index, part)| {
                let trimmed = part.trim();
                if trimmed.is_empty() {
                    None
                } else {
                    Some(TextSegment {
                        text: trimmed.to_owned(),
                        page_number: Some(
                            u32::try_from(index.saturating_add(1)).unwrap_or(u32::MAX),
                        ),
                        sheet_or_slide: None,
                    })
                }
            })
            .collect();
    }

    // Sheet/slide markers commonly produced by extractors.
    let sheet_re =
        regex::Regex::new(r"(?im)^(?:sheet|feuille|slide|diapositive)\s*[:#-]?\s*(.+)$").ok();
    if let Some(re) = sheet_re.as_ref()
        && re.is_match(text)
    {
        let mut segments = Vec::new();
        let mut current_name: Option<String> = None;
        let mut buffer = String::new();
        for line in text.lines() {
            if let Some(captures) = re.captures(line) {
                if !buffer.trim().is_empty() {
                    segments.push(TextSegment {
                        text: buffer.trim().to_owned(),
                        page_number: None,
                        sheet_or_slide: current_name.clone(),
                    });
                    buffer.clear();
                }
                current_name = captures
                    .get(1)
                    .map(|m| bounded_chars(m.as_str().trim(), 80));
            } else {
                buffer.push_str(line);
                buffer.push('\n');
            }
        }
        if !buffer.trim().is_empty() {
            segments.push(TextSegment {
                text: buffer.trim().to_owned(),
                page_number: None,
                sheet_or_slide: current_name,
            });
        }
        if !segments.is_empty() {
            return segments;
        }
    }

    // Paragraph-aware fallback.
    let paragraphs = text
        .split("\n\n")
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| TextSegment {
            text: part.to_owned(),
            page_number: None,
            sheet_or_slide: None,
        })
        .collect::<Vec<_>>();
    if paragraphs.len() > 1 {
        return paragraphs;
    }

    vec![TextSegment {
        text: text.to_owned(),
        page_number: None,
        sheet_or_slide: None,
    }]
}

fn tokenize_windows(
    text: &str,
    policy: &ChunkingPolicy,
    counter: &dyn TokenCounter,
) -> Vec<TextWindow> {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.is_empty() {
        return Vec::new();
    }
    if counter.count_tokens(text) <= policy.target_tokens {
        return vec![TextWindow {
            text: text.trim().to_owned(),
            start_char: 0,
            end_char: chars.len(),
        }];
    }

    let mut windows = Vec::new();
    let mut start = 0_usize;
    let max_chars = policy
        .max_tokens
        .saturating_mul(policy.approx_chars_per_token.max(1));
    let target_chars = policy
        .target_tokens
        .saturating_mul(policy.approx_chars_per_token.max(1));
    let overlap_chars = policy
        .overlap_tokens
        .saturating_mul(policy.approx_chars_per_token.max(1));

    while start < chars.len() {
        let mut end = (start + target_chars).min(chars.len());
        // Prefer paragraph/newline/whitespace boundaries near the target so
        // tokens are not split across chunks when modest overlap is used.
        if end < chars.len() {
            let search_from = start.saturating_add(target_chars / 2);
            if let Some(rel) = chars[search_from..end].iter().rposition(|c| *c == '\n') {
                end = search_from.saturating_add(rel).saturating_add(1);
            } else if let Some(rel) = chars[search_from..end]
                .iter()
                .rposition(|c| c.is_whitespace())
            {
                end = search_from.saturating_add(rel).saturating_add(1);
            }
        }
        // Grow/shrink to respect max tokens via counter.
        let mut candidate: String = chars[start..end].iter().collect();
        while counter.count_tokens(&candidate) > policy.max_tokens && end > start + 16 {
            end = end.saturating_sub(16).max(start + 1);
            candidate = chars[start..end].iter().collect();
        }
        while end < chars.len()
            && counter.count_tokens(&candidate) < policy.target_tokens
            && end - start < max_chars
        {
            end = (end + 16).min(chars.len());
            candidate = chars[start..end].iter().collect();
            if counter.count_tokens(&candidate) > policy.max_tokens {
                end = end.saturating_sub(16).max(start + 1);
                candidate = chars[start..end].iter().collect();
                break;
            }
        }
        let trimmed = candidate.trim();
        if !trimmed.is_empty() {
            windows.push(TextWindow {
                text: trimmed.to_owned(),
                start_char: start,
                end_char: end,
            });
        }
        if end >= chars.len() {
            break;
        }
        let next = end.saturating_sub(overlap_chars);
        start = next.max(start + 1);
    }
    windows
}

fn truncate_to_tokens(text: &str, max_tokens: usize, counter: &dyn TokenCounter) -> String {
    if counter.count_tokens(text) <= max_tokens {
        return text.chars().take(MAX_EMBEDDING_INPUT_CHARS).collect();
    }
    let chars = text.chars().collect::<Vec<_>>();
    let mut lo = 0_usize;
    let mut hi = chars.len().min(MAX_EMBEDDING_INPUT_CHARS);
    while lo + 1 < hi {
        let mid = (lo + hi) / 2;
        let candidate: String = chars[..mid].iter().collect();
        if counter.count_tokens(&candidate) <= max_tokens {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    chars[..lo].iter().collect()
}

#[allow(clippy::too_many_arguments)]
fn make_chunk(
    source_id: &str,
    source_kind: &str,
    sequence_index: u32,
    text: String,
    start_offset: Option<usize>,
    end_offset: Option<usize>,
    page_number: Option<u32>,
    sheet_or_slide: Option<String>,
    truncated_file: bool,
) -> SemanticChunk {
    let text_hash = *blake3::hash(text.as_bytes()).as_bytes();
    SemanticChunk {
        source_id: source_id.to_owned(),
        source_kind: source_kind.to_owned(),
        sequence_index,
        text,
        text_hash,
        start_offset,
        end_offset,
        page_number,
        sheet_or_slide,
        truncated_file,
    }
}

fn bounded_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_documents_stay_single_text_chunk() {
        let policy = ChunkingPolicy::default();
        let counter = ApproximateTokenCounter::from_policy(&policy);
        let chunks = chunk_embedding_document(
            &EmbeddingDocument {
                filename: "note.txt".to_owned(),
                semantic_fields: vec![("document_type".to_owned(), "invoice".to_owned())],
                identities: vec![],
                extracted_text: "Facture Point P 1400 euros".to_owned(),
            },
            &policy,
            &counter,
        );
        assert_eq!(chunks[0].source_kind, "semantic_summary");
        assert_eq!(
            chunks
                .iter()
                .filter(|c| c.source_kind == "text_chunk")
                .count(),
            1
        );
    }

    #[test]
    fn respects_max_text_chunks_and_marks_partial() {
        let policy = ChunkingPolicy {
            max_text_chunks: 2,
            target_tokens: 8,
            max_tokens: 12,
            approx_chars_per_token: 1,
            ..ChunkingPolicy::default()
        };
        let counter = ApproximateTokenCounter::from_policy(&policy);
        let text = "alpha\n\n".repeat(200);
        let chunks = chunk_embedding_document(
            &EmbeddingDocument {
                filename: "long.txt".to_owned(),
                semantic_fields: vec![],
                identities: vec![],
                extracted_text: text,
            },
            &policy,
            &counter,
        );
        let text_chunks = chunks
            .iter()
            .filter(|c| c.source_kind == "text_chunk")
            .count();
        assert_eq!(text_chunks, 2);
        assert!(chunks.iter().any(|c| c.truncated_file));
    }

    #[test]
    fn page_breaks_create_structure_aware_chunks() {
        let policy = ChunkingPolicy::default();
        let counter = ApproximateTokenCounter::from_policy(&policy);
        let chunks = chunk_embedding_document(
            &EmbeddingDocument {
                filename: "scan.pdf".to_owned(),
                semantic_fields: vec![],
                identities: vec![],
                extracted_text: "Page one content about toiture\u{000C}Page two materials Point P"
                    .to_owned(),
            },
            &policy,
            &counter,
        );
        let pages = chunks
            .iter()
            .filter_map(|c| c.page_number)
            .collect::<Vec<_>>();
        assert!(pages.contains(&1));
        assert!(pages.contains(&2));
    }

    #[test]
    fn overlap_preserves_boundary_terms() {
        let policy = ChunkingPolicy {
            target_tokens: 40,
            max_tokens: 48,
            overlap_tokens: 12,
            approx_chars_per_token: 1,
            ..ChunkingPolicy::default()
        };
        let counter = ApproximateTokenCounter::from_policy(&policy);
        let text = format!(
            "{} BOUNDARY_MARK {}",
            "word ".repeat(40),
            "tail ".repeat(40)
        );
        let chunks = chunk_embedding_document(
            &EmbeddingDocument {
                filename: "boundary.txt".to_owned(),
                semantic_fields: vec![],
                identities: vec![],
                extracted_text: text,
            },
            &policy,
            &counter,
        );
        let text_chunks = chunks
            .iter()
            .filter(|c| c.source_kind == "text_chunk")
            .collect::<Vec<_>>();
        assert!(text_chunks.len() >= 2);
        let containing = text_chunks
            .iter()
            .filter(|c| c.text.contains("BOUNDARY_MARK"))
            .count();
        assert!(
            containing >= 1,
            "structure/overlap policy must keep the boundary term in at least one chunk"
        );
    }
}
