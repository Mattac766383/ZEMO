use crate::{EmbeddingAvailability, QueryChip};
use serde::{Deserialize, Serialize};

pub const DEFAULT_PAGE_SIZE: usize = 50;
pub const MAX_PAGE_SIZE: usize = 100;
pub const MAX_QUERY_CHARS: usize = 512;
const MAX_QUERY_TERMS: usize = 32;
const MAX_TERM_CHARS: usize = 64;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileTypeFilter {
    #[default]
    All,
    Pdf,
    Documents,
    Spreadsheets,
    Presentations,
    Images,
    Archives,
    Other,
}

impl FileTypeFilter {
    #[must_use]
    pub const fn database_name(self) -> Option<&'static str> {
        match self {
            Self::All => None,
            Self::Pdf => Some("pdf"),
            Self::Documents => Some("documents"),
            Self::Spreadsheets => Some("spreadsheets"),
            Self::Presentations => Some("presentations"),
            Self::Images => Some("images"),
            Self::Archives => Some("archives"),
            Self::Other => Some("other"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModifiedFilter {
    #[default]
    Any,
    Today,
    LastSevenDays,
    LastThirtyDays,
    ThisYear,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractionFilter {
    #[default]
    Any,
    Success,
    Partial,
    Failed,
    Unsupported,
}

impl ExtractionFilter {
    #[must_use]
    pub const fn database_name(self) -> Option<&'static str> {
        match self {
            Self::Any => None,
            Self::Success => Some("success"),
            Self::Partial => Some("partial"),
            Self::Failed => Some("failed"),
            Self::Unsupported => Some("unsupported"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OcrFilter {
    #[default]
    Any,
    Used,
    NotUsed,
    Unavailable,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentTypeFilter {
    #[default]
    Any,
    Invoice,
    Quote,
    Contract,
    PurchaseOrder,
    DeliveryNote,
    BankStatement,
    TaxDocument,
    Payslip,
    EmploymentContract,
    InsuranceDocument,
    LegalDocument,
    AdministrativeDocument,
    Receipt,
    Report,
    Letter,
    Cv,
    Photo,
    Video,
    Spreadsheet,
    Presentation,
    Archive,
    Other,
    Unknown,
}

impl DocumentTypeFilter {
    #[must_use]
    pub const fn database_name(self) -> Option<&'static str> {
        match self {
            Self::Any => None,
            Self::Invoice => Some("invoice"),
            Self::Quote => Some("quote"),
            Self::Contract => Some("contract"),
            Self::PurchaseOrder => Some("purchase_order"),
            Self::DeliveryNote => Some("delivery_note"),
            Self::BankStatement => Some("bank_statement"),
            Self::TaxDocument => Some("tax_document"),
            Self::Payslip => Some("payslip"),
            Self::EmploymentContract => Some("employment_contract"),
            Self::InsuranceDocument => Some("insurance_document"),
            Self::LegalDocument => Some("legal_document"),
            Self::AdministrativeDocument => Some("administrative_document"),
            Self::Receipt => Some("receipt"),
            Self::Report => Some("report"),
            Self::Letter => Some("letter"),
            Self::Cv => Some("cv"),
            Self::Photo => Some("photo"),
            Self::Video => Some("video"),
            Self::Spreadsheet => Some("spreadsheet"),
            Self::Presentation => Some("presentation"),
            Self::Archive => Some("archive"),
            Self::Other => Some("other"),
            Self::Unknown => Some("unknown"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextFilter {
    #[default]
    Any,
    Personal,
    Business,
    Mixed,
    Unknown,
}

impl ContextFilter {
    #[must_use]
    pub const fn database_name(self) -> Option<&'static str> {
        match self {
            Self::Any => None,
            Self::Personal => Some("personal"),
            Self::Business => Some("business"),
            Self::Mixed => Some("mixed"),
            Self::Unknown => Some("unknown"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticStatusFilter {
    #[default]
    Any,
    Success,
    Partial,
    Unknown,
    Failed,
    Pending,
}

impl SemanticStatusFilter {
    #[must_use]
    pub const fn database_name(self) -> Option<&'static str> {
        match self {
            Self::Any => None,
            Self::Success => Some("success"),
            Self::Partial => Some("partial"),
            Self::Unknown => Some("unknown"),
            Self::Failed => Some("failed"),
            Self::Pending => Some("pending"),
        }
    }
}

impl OcrFilter {
    #[must_use]
    pub const fn database_name(self) -> Option<&'static str> {
        match self {
            Self::Any => None,
            Self::Used => Some("used"),
            Self::NotUsed => Some("not_used"),
            Self::Unavailable => Some("unavailable"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchSort {
    #[default]
    Relevance,
    Newest,
    Oldest,
    Filename,
    Size,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchFilters {
    pub file_type: FileTypeFilter,
    pub modified: ModifiedFilter,
    pub extraction: ExtractionFilter,
    pub ocr: OcrFilter,
    pub minimum_size: Option<u64>,
    pub maximum_size: Option<u64>,
    pub document_type: DocumentTypeFilter,
    pub context: ContextFilter,
    pub customer: Option<String>,
    pub supplier: Option<String>,
    pub project: Option<String>,
    pub year: Option<i32>,
    pub amount_minimum_minor: Option<i64>,
    pub amount_maximum_minor: Option<i64>,
    pub currency: Option<String>,
    pub semantic_status: SemanticStatusFilter,
    pub minimum_confidence_percent: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SearchQuery {
    pub text: String,
    pub filters: SearchFilters,
    pub sort: SearchSort,
    pub page: usize,
    pub page_size: usize,
    pub semantic_search: bool,
    pub disabled_intents: Vec<String>,
}

impl Default for SearchQuery {
    fn default() -> Self {
        Self {
            text: String::new(),
            filters: SearchFilters::default(),
            sort: SearchSort::default(),
            page: 0,
            page_size: DEFAULT_PAGE_SIZE,
            semantic_search: true,
            disabled_intents: Vec::new(),
        }
    }
}

impl SearchQuery {
    #[must_use]
    pub fn bounded(mut self) -> Self {
        self.text = self.text.chars().take(MAX_QUERY_CHARS).collect();
        self.page = self.page.min(2_000_000);
        self.page_size = if self.page_size == 0 {
            DEFAULT_PAGE_SIZE
        } else {
            self.page_size.min(MAX_PAGE_SIZE)
        };
        if let (Some(minimum), Some(maximum)) =
            (self.filters.minimum_size, self.filters.maximum_size)
            && minimum > maximum
        {
            self.filters.minimum_size = Some(maximum);
            self.filters.maximum_size = Some(minimum);
        }
        if let (Some(minimum), Some(maximum)) = (
            self.filters.amount_minimum_minor,
            self.filters.amount_maximum_minor,
        ) && minimum > maximum
        {
            self.filters.amount_minimum_minor = Some(maximum);
            self.filters.amount_maximum_minor = Some(minimum);
        }
        self.filters.customer = bounded_optional(self.filters.customer, 128);
        self.filters.supplier = bounded_optional(self.filters.supplier, 128);
        self.filters.project = bounded_optional(self.filters.project, 128);
        self.filters.currency = self.filters.currency.and_then(|currency| {
            let currency = currency.trim().to_ascii_uppercase();
            (currency.len() == 3 && currency.bytes().all(|byte| byte.is_ascii_uppercase()))
                .then_some(currency)
        });
        self.filters.minimum_confidence_percent = self
            .filters
            .minimum_confidence_percent
            .map(|value| value.min(100));
        self.filters.year = self
            .filters
            .year
            .filter(|year| (1900..=2100).contains(year));
        self.disabled_intents = self
            .disabled_intents
            .into_iter()
            .map(|value| value.chars().take(32).collect::<String>())
            .filter(|value| {
                matches!(
                    value.as_str(),
                    "document_type"
                        | "context"
                        | "supplier"
                        | "customer"
                        | "project"
                        | "party"
                        | "amount"
                        | "date"
                )
            })
            .take(8)
            .collect();
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchSource {
    Filename,
    Path,
    Content,
    Metadata,
    Structured,
    Relationship,
    Semantic,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub file_id: String,
    pub filename: String,
    pub relative_path: String,
    pub detected_type: Option<String>,
    pub extension: Option<String>,
    pub byte_size: u64,
    pub modified_at: Option<String>,
    pub extraction_status: Option<String>,
    pub ocr_status: Option<String>,
    pub duplicate: bool,
    pub match_source: MatchSource,
    pub relevance: f64,
    pub snippet: String,
    pub why_matched: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingSearchStatus {
    pub availability: EmbeddingAvailability,
    pub provider_id: String,
    pub version: String,
    pub production_ready: bool,
    pub indexed_files: u64,
    /// Optional ANN lifecycle: not_available|building|ready|degraded|rebuild_required|failed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ann_index_status: Option<String>,
}

impl Default for EmbeddingSearchStatus {
    fn default() -> Self {
        Self {
            availability: EmbeddingAvailability::Unavailable,
            provider_id: "unavailable-local-embedding".to_owned(),
            version: "none".to_owned(),
            production_ready: false,
            indexed_files: 0,
            ann_index_status: None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchTimings {
    pub total_ms: u64,
    pub lexical_and_structured_ms: u64,
    pub query_embed_ms: u64,
    pub ann_ms: u64,
    pub vector_ms: u64,
    pub fusion_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchPage {
    pub query: String,
    pub page: usize,
    pub page_size: usize,
    pub total: u64,
    pub has_more: bool,
    pub results: Vec<SearchResult>,
    pub interpreted_query: Vec<QueryChip>,
    pub embeddings: EmbeddingSearchStatus,
    pub timings: SearchTimings,
}

#[must_use]
pub fn query_terms(value: &str) -> Vec<String> {
    value
        .chars()
        .take(MAX_QUERY_CHARS)
        .map(|character| {
            if character.is_alphanumeric() {
                character
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .take(MAX_QUERY_TERMS)
        .map(|term| term.chars().take(MAX_TERM_CHARS).collect::<String>())
        .filter(|term| !term.is_empty())
        .collect()
}

/// Converts normal user text into a bounded literal-prefix FTS expression.
///
/// Only alphanumeric terms survive, so quotes, operators and punctuation can
/// never become FTS syntax. The returned expression is still bound as a SQL
/// parameter by the persistence layer.
#[must_use]
pub fn safe_fts_query(value: &str) -> Option<String> {
    let terms = query_terms(value);
    (!terms.is_empty()).then(|| {
        terms
            .iter()
            .map(|term| {
                let literal = if term.chars().count() >= 2 {
                    format!("\"{term}\"*")
                } else {
                    format!("\"{term}\"")
                };
                if let Some(singular) = common_search_singular(term) {
                    format!("({literal} OR \"{singular}\"*)")
                } else {
                    literal
                }
            })
            .collect::<Vec<_>>()
            .join(" AND ")
    })
}

fn common_search_singular(term: &str) -> Option<&'static str> {
    match term.to_lowercase().as_str() {
        "administratifs" => Some("administratif"),
        "annees" => Some("annee"),
        "contrats" => Some("contrat"),
        "documents" => Some("document"),
        "euros" => Some("euro"),
        "factures" => Some("facture"),
        "personnels" => Some("personnel"),
        "photos" => Some("photo"),
        _ => None,
    }
}

fn bounded_optional(value: Option<String>, limit: usize) -> Option<String> {
    value.and_then(|value| {
        let bounded = value
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .chars()
            .take(limit)
            .collect::<String>();
        (!bounded.is_empty()).then_some(bounded)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_input_cannot_become_fts_syntax() {
        assert_eq!(
            safe_fts_query(r#"facture "Point P" OR (secret*)"#).as_deref(),
            Some(r#""facture"* AND "Point"* AND "P" AND "OR"* AND "secret"*"#)
        );
        assert_eq!(safe_fts_query("  🧾 ( ) \" * -  "), None);
    }

    #[test]
    fn unicode_and_apostrophes_are_normalized_as_terms() {
        assert_eq!(
            query_terms("l’école — été à Bordeaux"),
            ["l", "école", "été", "à", "Bordeaux"]
        );
    }

    #[test]
    fn common_french_plurals_keep_safe_singular_alternatives() {
        assert_eq!(
            safe_fts_query("documents administratifs personnels"),
            Some(
                "(\"documents\"* OR \"document\"*) AND (\"administratifs\"* OR \"administratif\"*) AND (\"personnels\"* OR \"personnel\"*)"
                    .to_owned()
            )
        );
        assert_eq!(safe_fts_query("Paris"), Some("\"Paris\"*".to_owned()));
    }

    #[test]
    fn query_and_page_bounds_are_enforced() {
        let query = SearchQuery {
            text: "x".repeat(MAX_QUERY_CHARS + 20),
            page_size: usize::MAX,
            filters: SearchFilters {
                minimum_size: Some(200),
                maximum_size: Some(100),
                amount_minimum_minor: Some(20_000),
                amount_maximum_minor: Some(10_000),
                ..SearchFilters::default()
            },
            ..SearchQuery::default()
        }
        .bounded();
        assert_eq!(query.text.chars().count(), MAX_QUERY_CHARS);
        assert_eq!(query.page_size, MAX_PAGE_SIZE);
        assert_eq!(query.filters.minimum_size, Some(100));
        assert_eq!(query.filters.maximum_size, Some(200));
        assert_eq!(query.filters.amount_minimum_minor, Some(10_000));
        assert_eq!(query.filters.amount_maximum_minor, Some(20_000));
    }
}
