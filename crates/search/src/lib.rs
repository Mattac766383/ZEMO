//! Local hybrid search: lexical relevance, optional local dense embeddings and
//! Reciprocal Rank Fusion.  No cloud LLM is required for retrieval.

mod ann_index;
mod chunking;
mod embedding;
mod interpretation;
mod local;
mod model_download;
mod model_manager;
mod onnx_provider;
mod ranking;

pub use ann_index::*;
pub use chunking::*;
pub use embedding::*;
pub use interpretation::*;
pub use local::*;
pub use model_download::*;
pub use model_manager::*;
pub use onnx_provider::*;
pub use ranking::*;

use domain::{SearchDocument, SearchHit, SearchResponse};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

/// Prepare a filesystem path for C runtimes (ORT, USearch) that reject `\\?\`.
/// Keeps the verbatim form for UNC and paths that would exceed a short DOS limit.
#[must_use]
pub(crate) fn native_filesystem_path_for_c_runtime(path: &Path) -> PathBuf {
    let Some(text) = path.to_str() else {
        return path.to_path_buf();
    };
    let Some(rest) = text.strip_prefix(r"\\?\") else {
        return path.to_path_buf();
    };
    if rest.starts_with("UNC\\") || rest.starts_with("UNC/") {
        return path.to_path_buf();
    }
    let bytes = rest.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
        && rest.len() < 240
    {
        return PathBuf::from(rest);
    }
    path.to_path_buf()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SearchConfig {
    pub limit: usize,
    pub rrf_k: f32,
    pub embedding_dimensions: usize,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            limit: 20,
            rrf_k: 60.0,
            embedding_dimensions: 256,
        }
    }
}

#[derive(Debug, Default)]
pub struct HybridSearchEngine {
    documents: Vec<SearchDocument>,
}

impl HybridSearchEngine {
    #[must_use]
    pub fn new(documents: Vec<SearchDocument>) -> Self {
        Self { documents }
    }

    #[must_use]
    pub fn search(&self, query: &str, config: SearchConfig) -> SearchResponse {
        let query_tokens = tokenize(query);
        if query_tokens.is_empty() {
            return SearchResponse {
                query: query.to_owned(),
                hits: Vec::new(),
            };
        }

        let lexical = lexical_ranking(&self.documents, &query_tokens);
        let query_vector = hashed_embedding(query, config.embedding_dimensions);
        let semantic =
            semantic_ranking(&self.documents, &query_vector, config.embedding_dimensions);
        let scores = reciprocal_rank_fusion(&lexical, &semantic, config.rrf_k);
        let lexical_positions = rank_positions(&lexical);
        let semantic_positions = rank_positions(&semantic);

        let mut fused = scores.into_iter().collect::<Vec<_>>();
        fused.sort_by(|left, right| {
            right
                .1
                .partial_cmp(&left.1)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.0.cmp(&right.0))
        });
        let hits = fused
            .into_iter()
            .take(config.limit)
            .filter_map(|(index, score)| {
                self.documents.get(index).map(|document| SearchHit {
                    file_id: document.file_id,
                    file_version_id: document.file_version_id,
                    display_label: document.display_label.clone(),
                    summary: best_excerpt(&document.body, &query_tokens, 240),
                    score,
                    lexical_rank: lexical_positions.get(&index).copied(),
                    semantic_rank: semantic_positions.get(&index).copied(),
                    evidence: document.evidence.clone(),
                })
            })
            .collect();

        SearchResponse {
            query: query.to_owned(),
            hits,
        }
    }
}

fn lexical_ranking(documents: &[SearchDocument], query_tokens: &[String]) -> Vec<(usize, f32)> {
    let document_count = documents.len().max(1) as f32;
    let mut document_frequency = HashMap::<&str, usize>::new();
    for token in query_tokens {
        let count = documents
            .iter()
            .filter(|document| {
                let tokens = document_tokens(document);
                tokens.contains(token.as_str())
            })
            .count();
        document_frequency.insert(token, count);
    }

    let mut scores = documents
        .iter()
        .enumerate()
        .map(|(index, document)| {
            let tokens = document_tokens(document);
            let mut score = 0.0_f32;
            for query_token in query_tokens {
                let frequency = tokens
                    .iter()
                    .filter(|token| **token == query_token.as_str())
                    .count() as f32;
                if frequency == 0.0 {
                    continue;
                }
                let df = *document_frequency.get(query_token.as_str()).unwrap_or(&0) as f32;
                let inverse_frequency = ((document_count + 1.0) / (df + 1.0)).ln() + 1.0;
                score += (1.0 + frequency.ln()) * inverse_frequency;
            }
            (index, score)
        })
        .filter(|(_, score)| *score > 0.0)
        .collect::<Vec<_>>();
    sort_scores(&mut scores);
    scores
}

fn semantic_ranking(
    documents: &[SearchDocument],
    query: &[f32],
    dimensions: usize,
) -> Vec<(usize, f32)> {
    let mut scores = documents
        .iter()
        .enumerate()
        .filter_map(|(index, document)| {
            let generated;
            let vector = if let Some(vector) = document.embedding.as_deref() {
                vector
            } else {
                generated =
                    hashed_embedding(&format!("{} {}", document.title, document.body), dimensions);
                &generated
            };
            let score = cosine_similarity(query, vector);
            (score > 0.0).then_some((index, score))
        })
        .collect::<Vec<_>>();
    sort_scores(&mut scores);
    scores
}

fn reciprocal_rank_fusion(
    lexical: &[(usize, f32)],
    semantic: &[(usize, f32)],
    k: f32,
) -> HashMap<usize, f32> {
    let mut scores = HashMap::new();
    for ranking in [lexical, semantic] {
        for (position, (index, _)) in ranking.iter().enumerate() {
            *scores.entry(*index).or_insert(0.0) += 1.0 / (k + position as f32 + 1.0);
        }
    }
    scores
}

fn rank_positions(ranking: &[(usize, f32)]) -> HashMap<usize, usize> {
    ranking
        .iter()
        .enumerate()
        .map(|(position, (index, _))| (*index, position + 1))
        .collect()
}

fn sort_scores(scores: &mut [(usize, f32)]) {
    scores.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.0.cmp(&right.0))
    });
}

fn document_tokens(document: &SearchDocument) -> HashSet<&str> {
    document.lexical_tokens.iter().map(String::as_str).collect()
}

#[must_use]
pub fn tokenize(value: &str) -> Vec<String> {
    value
        .split(|character: char| !character.is_alphanumeric())
        .filter(|token| token.chars().count() >= 2)
        .map(str::to_lowercase)
        .collect()
}

#[must_use]
pub fn hashed_embedding(value: &str, dimensions: usize) -> Vec<f32> {
    let dimensions = dimensions.max(32);
    let mut output = vec![0.0_f32; dimensions];
    for token in tokenize(value) {
        let digest = blake3::hash(token.as_bytes());
        let bytes = digest.as_bytes();
        let index = usize::from(u16::from_le_bytes([bytes[0], bytes[1]])) % dimensions;
        output[index] += if bytes[2] & 1 == 0 { 1.0 } else { -1.0 };
    }
    normalize(&mut output);
    output
}

#[must_use]
pub fn cosine_similarity(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return 0.0;
    }
    left.iter()
        .zip(right)
        .map(|(a, b)| a * b)
        .sum::<f32>()
        .clamp(-1.0, 1.0)
}

fn normalize(vector: &mut [f32]) {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in vector {
            *value /= norm;
        }
    }
}

fn best_excerpt(body: &str, query_tokens: &[String], max_chars: usize) -> String {
    let lowercase = body.to_lowercase();
    let start = query_tokens
        .iter()
        .filter_map(|token| lowercase.find(token))
        .min()
        .unwrap_or(0);
    let character_start = body[..start.min(body.len())]
        .chars()
        .count()
        .saturating_sub(40);
    body.chars()
        .skip(character_start)
        .take(max_chars)
        .collect::<String>()
        .trim()
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{DisplayLabel, FileId, FileVersionId};

    fn document(label: &str, body: &str) -> SearchDocument {
        SearchDocument {
            file_id: FileId::new(),
            file_version_id: FileVersionId::new(),
            display_label: DisplayLabel::new(label)
                .unwrap_or_else(|error| panic!("label should be valid: {error}")),
            title: label.to_owned(),
            body: body.to_owned(),
            detected_mime: Some("text/plain".to_owned()),
            language: Some("fr".to_owned()),
            lexical_tokens: tokenize(body),
            embedding: None,
            evidence: Vec::new(),
        }
    }

    #[test]
    fn strips_verbatim_dos_prefix_for_short_c_runtime_paths() {
        assert_eq!(
            native_filesystem_path_for_c_runtime(Path::new(r"\\?\D:\models\onnx\model.onnx")),
            PathBuf::from(r"D:\models\onnx\model.onnx")
        );
        assert_eq!(
            native_filesystem_path_for_c_runtime(Path::new(r"D:\models\onnx\model.onnx")),
            PathBuf::from(r"D:\models\onnx\model.onnx")
        );
        assert_eq!(
            native_filesystem_path_for_c_runtime(Path::new(r"\\?\UNC\server\share\model.onnx")),
            PathBuf::from(r"\\?\UNC\server\share\model.onnx")
        );
    }

    #[test]
    fn lexical_and_semantic_rankings_are_fused() {
        let engine = HybridSearchEngine::new(vec![
            document("contrat.txt", "contrat client ACME renouvellement"),
            document("photo.txt", "vacances montagne"),
        ]);

        let response = engine.search("contrat ACME", SearchConfig::default());
        assert_eq!(response.hits.len(), 1);
        assert_eq!(response.hits[0].display_label.as_str(), "contrat.txt");
        assert_eq!(response.hits[0].lexical_rank, Some(1));
    }
}
