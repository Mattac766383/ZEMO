//! Bounded, deterministic, local-only document understanding.
//!
//! Confidence scores are evidence-strength scores on a 0–1 scale. They are
//! not probability guarantees. Document content is always treated as
//! untrusted data and is never interpreted as an application instruction.

use regex::Regex;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    sync::LazyLock,
    time::{Duration, Instant},
};

const ANALYZER_ID: &str = "deterministic-document-understanding";
const ANALYZER_VERSION: &str = "5.0.0";
const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConfidenceScore(f32);

impl ConfidenceScore {
    pub fn new(value: f32) -> Result<Self, KnowledgeError> {
        if !value.is_finite() || !(0.0..=1.0).contains(&value) {
            return Err(KnowledgeError::InvalidConfidence);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub const fn value(self) -> f32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceBand {
    VeryHigh,
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ConfidencePolicy {
    pub very_high: f32,
    pub high: f32,
    pub medium: f32,
    pub conflict_margin: f32,
}

impl Default for ConfidencePolicy {
    fn default() -> Self {
        Self {
            very_high: 0.95,
            high: 0.85,
            medium: 0.65,
            conflict_margin: 0.12,
        }
    }
}

impl ConfidencePolicy {
    #[must_use]
    pub fn band(self, confidence: ConfidenceScore) -> ConfidenceBand {
        let value = confidence.value();
        if value >= self.very_high {
            ConfidenceBand::VeryHigh
        } else if value >= self.high {
            ConfidenceBand::High
        } else if value >= self.medium {
            ConfidenceBand::Medium
        } else {
            ConfidenceBand::Low
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticStatus {
    Pending,
    Running,
    Success,
    Partial,
    Unknown,
    Failed,
    Cancelled,
}

impl SemanticStatus {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Success => "success",
            Self::Partial => "partial",
            Self::Unknown => "unknown",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldStatus {
    Confirmed,
    Inferred,
    Ambiguous,
    Unknown,
    Conflicting,
}

impl FieldStatus {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Inferred => "inferred",
            Self::Ambiguous => "ambiguous",
            Self::Unknown => "unknown",
            Self::Conflicting => "conflicting",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentType {
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

impl DocumentType {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Invoice => "invoice",
            Self::Quote => "quote",
            Self::Contract => "contract",
            Self::PurchaseOrder => "purchase_order",
            Self::DeliveryNote => "delivery_note",
            Self::BankStatement => "bank_statement",
            Self::TaxDocument => "tax_document",
            Self::Payslip => "payslip",
            Self::EmploymentContract => "employment_contract",
            Self::InsuranceDocument => "insurance_document",
            Self::LegalDocument => "legal_document",
            Self::AdministrativeDocument => "administrative_document",
            Self::Receipt => "receipt",
            Self::Report => "report",
            Self::Letter => "letter",
            Self::Cv => "cv",
            Self::Photo => "photo",
            Self::Video => "video",
            Self::Spreadsheet => "spreadsheet",
            Self::Presentation => "presentation",
            Self::Archive => "archive",
            Self::Other => "other",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentContext {
    Personal,
    Business,
    Mixed,
    Unknown,
}

impl DocumentContext {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Personal => "personal",
            Self::Business => "business",
            Self::Mixed => "mixed",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticFieldType {
    DocumentType,
    Context,
    SupplierCandidate,
    CustomerCandidate,
    Issuer,
    InvoiceNumber,
    QuoteNumber,
    DocumentNumber,
    IssueDate,
    DueDate,
    ExpirationDate,
    DocumentDate,
    Subtotal,
    Tax,
    Total,
    Amount,
    Currency,
    PurchaseOrderReference,
    ProjectReferenceCandidate,
    ContractParties,
    ContractTitle,
    ContractType,
    CompanyIdentifier,
}

impl SemanticFieldType {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::DocumentType => "document_type",
            Self::Context => "context",
            Self::SupplierCandidate => "supplier_candidate",
            Self::CustomerCandidate => "customer_candidate",
            Self::Issuer => "issuer",
            Self::InvoiceNumber => "invoice_number",
            Self::QuoteNumber => "quote_number",
            Self::DocumentNumber => "document_number",
            Self::IssueDate => "issue_date",
            Self::DueDate => "due_date",
            Self::ExpirationDate => "expiration_date",
            Self::DocumentDate => "document_date",
            Self::Subtotal => "subtotal",
            Self::Tax => "tax",
            Self::Total => "total",
            Self::Amount => "amount",
            Self::Currency => "currency",
            Self::PurchaseOrderReference => "purchase_order_reference",
            Self::ProjectReferenceCandidate => "project_reference_candidate",
            Self::ContractParties => "contract_parties",
            Self::ContractTitle => "contract_title",
            Self::ContractType => "contract_type",
            Self::CompanyIdentifier => "company_identifier",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityType {
    Person,
    Organization,
    CustomerCandidate,
    SupplierCandidate,
    ProjectCandidate,
    Address,
    Email,
    Phone,
    Date,
    Amount,
    Currency,
    DocumentNumber,
    InvoiceNumber,
    SiretOrCompanyId,
    OtherIdentifier,
}

impl EntityType {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Organization => "organization",
            Self::CustomerCandidate => "customer_candidate",
            Self::SupplierCandidate => "supplier_candidate",
            Self::ProjectCandidate => "project_candidate",
            Self::Address => "address",
            Self::Email => "email",
            Self::Phone => "phone",
            Self::Date => "date",
            Self::Amount => "amount",
            Self::Currency => "currency",
            Self::DocumentNumber => "document_number",
            Self::InvoiceNumber => "invoice_number",
            Self::SiretOrCompanyId => "siret_or_company_id",
            Self::OtherIdentifier => "other_identifier",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceType {
    TextSpan,
    Filename,
    Metadata,
    StructuralIndicator,
    ParserMatch,
    OcrText,
}

impl EvidenceType {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::TextSpan => "text_span",
            Self::Filename => "filename",
            Self::Metadata => "metadata",
            Self::StructuralIndicator => "structural_indicator",
            Self::ParserMatch => "parser_match",
            Self::OcrText => "ocr_text",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceMethod {
    DeterministicRule,
    RegexParser,
    StructuredParser,
    FilenameHint,
    Metadata,
    LocalSemanticProvider,
}

impl SourceMethod {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::DeterministicRule => "deterministic_rule",
            Self::RegexParser => "regex_parser",
            Self::StructuredParser => "structured_parser",
            Self::FilenameHint => "filename_hint",
            Self::Metadata => "metadata",
            Self::LocalSemanticProvider => "local_semantic_provider",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SemanticValue {
    Text {
        value: String,
    },
    Date {
        iso_date: String,
    },
    Money {
        amount_minor: i64,
        scale: u8,
        currency: Option<String>,
    },
    DocumentType {
        value: DocumentType,
    },
    Context {
        value: DocumentContext,
    },
    TextList {
        values: Vec<String>,
    },
}

impl SemanticValue {
    #[must_use]
    pub const fn kind_name(&self) -> &'static str {
        match self {
            Self::Text { .. } => "text",
            Self::Date { .. } => "date",
            Self::Money { .. } => "money",
            Self::DocumentType { .. } => "document_type",
            Self::Context { .. } => "context",
            Self::TextList { .. } => "text_list",
        }
    }

    #[must_use]
    pub fn display_value(&self) -> String {
        match self {
            Self::Text { value } => value.clone(),
            Self::Date { iso_date } => iso_date.clone(),
            Self::Money {
                amount_minor,
                scale,
                currency,
            } => {
                let divisor = 10_i64.checked_pow(u32::from(*scale)).unwrap_or(100);
                let major = amount_minor / divisor;
                let fraction = amount_minor.unsigned_abs() % divisor.unsigned_abs();
                let amount = format!("{major}.{fraction:0width$}", width = usize::from(*scale));
                currency
                    .as_ref()
                    .map_or(amount.clone(), |code| format!("{amount} {code}"))
            }
            Self::DocumentType { value } => value.database_name().to_owned(),
            Self::Context { value } => value.database_name().to_owned(),
            Self::TextList { values } => values.join(" · "),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticEvidence {
    pub evidence_type: EvidenceType,
    pub exact_text: String,
    pub start_offset: Option<usize>,
    pub end_offset: Option<usize>,
    pub page_number: Option<u32>,
    pub sheet_name: Option<String>,
    pub slide_number: Option<u32>,
    pub source_label: String,
    pub explanation: String,
    pub extraction_method: String,
    pub analyzer_version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticCandidate {
    pub value: SemanticValue,
    pub original_value: String,
    pub confidence: ConfidenceScore,
    pub evidence: Vec<SemanticEvidence>,
    pub source_method: SourceMethod,
    pub ambiguous: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticField {
    pub field_type: SemanticFieldType,
    pub value: Option<SemanticValue>,
    pub original_value: Option<String>,
    pub confidence: ConfidenceScore,
    pub status: FieldStatus,
    pub evidence: Vec<SemanticEvidence>,
    pub candidates: Vec<SemanticCandidate>,
    pub source_method: SourceMethod,
    pub analyzer_version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticEntity {
    pub candidate_key: String,
    pub entity_type: EntityType,
    pub original_value: String,
    pub normalized_value: String,
    pub confidence: ConfidenceScore,
    pub status: FieldStatus,
    pub evidence: Vec<SemanticEvidence>,
    pub source_method: SourceMethod,
    pub analyzer_version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InputQuality {
    pub score: ConfidenceScore,
    pub status: InputQualityStatus,
    pub reasons: Vec<InputQualityReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputQualityStatus {
    Good,
    Degraded,
    Poor,
    Unusable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputQualityReason {
    PartialExtraction,
    FailedExtraction,
    EmptyText,
    TruncatedText,
    OcrUsed,
    LowOcrConfidence,
    MalformedText,
    SemanticInputLimit,
}

impl InputQualityReason {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::PartialExtraction => "partial_extraction",
            Self::FailedExtraction => "failed_extraction",
            Self::EmptyText => "empty_text",
            Self::TruncatedText => "truncated_text",
            Self::OcrUsed => "ocr_used",
            Self::LowOcrConfidence => "low_ocr_confidence",
            Self::MalformedText => "malformed_text",
            Self::SemanticInputLimit => "semantic_input_limit",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticReviewReason {
    SemanticAmbiguity,
    ConflictingFields,
    LowConfidenceDocumentType,
    LowConfidenceContext,
    MissingCriticalFields,
}

impl SemanticReviewReason {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::SemanticAmbiguity => "semantic_ambiguity",
            Self::ConflictingFields => "conflicting_fields",
            Self::LowConfidenceDocumentType => "low_confidence_document_type",
            Self::LowConfidenceContext => "low_confidence_context",
            Self::MissingCriticalFields => "missing_critical_fields",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SemanticAnalyzer {
    pub analyzer_id: String,
    pub analyzer_version: String,
    pub provider_id: String,
    pub provider_version: String,
    pub schema_version: u32,
    pub local_only: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticAnalysis {
    pub status: SemanticStatus,
    pub fields: Vec<SemanticField>,
    pub entities: Vec<SemanticEntity>,
    pub input_quality: InputQuality,
    pub language: Option<String>,
    pub analyzer: SemanticAnalyzer,
    pub review_reasons: Vec<SemanticReviewReason>,
    pub duration_ms: u64,
    pub input_character_count: usize,
    pub analyzed_character_count: usize,
}

impl SemanticAnalysis {
    pub fn validate(&self, limits: SemanticLimits) -> Result<(), KnowledgeError> {
        if !self.analyzer.local_only {
            return Err(KnowledgeError::RemoteProviderRejected);
        }
        if self.fields.len() > limits.max_fields || self.entities.len() > limits.max_entities {
            return Err(KnowledgeError::OutputLimitExceeded);
        }
        for field in &self.fields {
            validate_field(field, limits)?;
        }
        for entity in &self.entities {
            if entity.original_value.chars().count() > limits.max_value_chars
                || entity.normalized_value.chars().count() > limits.max_value_chars
                || entity.evidence.len() > limits.max_evidence_per_claim
            {
                return Err(KnowledgeError::OutputLimitExceeded);
            }
            for evidence in &entity.evidence {
                validate_evidence(evidence, limits)?;
            }
        }
        Ok(())
    }

    #[must_use]
    pub fn primary_field(&self, field_type: SemanticFieldType) -> Option<&SemanticField> {
        self.fields
            .iter()
            .find(|field| field.field_type == field_type)
    }
}

fn validate_field(field: &SemanticField, limits: SemanticLimits) -> Result<(), KnowledgeError> {
    if field.evidence.len() > limits.max_evidence_per_claim
        || field.candidates.len() > limits.max_candidates_per_field
    {
        return Err(KnowledgeError::OutputLimitExceeded);
    }
    if field.status == FieldStatus::Confirmed && field.value.is_none() {
        return Err(KnowledgeError::InvalidStructuredOutput(
            "confirmed field has no value".to_owned(),
        ));
    }
    if let Some(value) = &field.value {
        validate_value(value, limits)?;
    }
    for evidence in &field.evidence {
        validate_evidence(evidence, limits)?;
    }
    for candidate in &field.candidates {
        validate_value(&candidate.value, limits)?;
        if candidate.original_value.chars().count() > limits.max_value_chars
            || candidate.evidence.len() > limits.max_evidence_per_claim
        {
            return Err(KnowledgeError::OutputLimitExceeded);
        }
        for evidence in &candidate.evidence {
            validate_evidence(evidence, limits)?;
        }
    }
    Ok(())
}

fn validate_value(value: &SemanticValue, limits: SemanticLimits) -> Result<(), KnowledgeError> {
    match value {
        SemanticValue::Text { value } => {
            if value.chars().count() > limits.max_value_chars {
                return Err(KnowledgeError::OutputLimitExceeded);
            }
        }
        SemanticValue::Date { iso_date } => {
            if parse_iso_date(iso_date).is_none() {
                return Err(KnowledgeError::InvalidStructuredOutput(
                    "date is not a valid ISO calendar date".to_owned(),
                ));
            }
        }
        SemanticValue::Money {
            amount_minor,
            scale,
            currency,
        } => {
            if *amount_minor < 0 || *scale > 4 {
                return Err(KnowledgeError::InvalidStructuredOutput(
                    "money value is outside supported bounds".to_owned(),
                ));
            }
            if currency.as_ref().is_some_and(|code| {
                code.len() != 3 || !code.bytes().all(|byte| byte.is_ascii_uppercase())
            }) {
                return Err(KnowledgeError::InvalidStructuredOutput(
                    "currency must be an uppercase ISO-style code".to_owned(),
                ));
            }
        }
        SemanticValue::TextList { values } => {
            if values.len() > 16
                || values
                    .iter()
                    .any(|value| value.chars().count() > limits.max_value_chars)
            {
                return Err(KnowledgeError::OutputLimitExceeded);
            }
        }
        SemanticValue::DocumentType { .. } | SemanticValue::Context { .. } => {}
    }
    Ok(())
}

fn validate_evidence(
    evidence: &SemanticEvidence,
    limits: SemanticLimits,
) -> Result<(), KnowledgeError> {
    if evidence.exact_text.chars().count() > limits.max_evidence_chars
        || evidence.source_label.chars().count() > 128
        || evidence.explanation.chars().count() > 256
        || evidence.extraction_method.chars().count() > 80
    {
        return Err(KnowledgeError::OutputLimitExceeded);
    }
    match (evidence.start_offset, evidence.end_offset) {
        (Some(start), Some(end)) if start <= end => {}
        (None, None) => {}
        _ => {
            return Err(KnowledgeError::InvalidStructuredOutput(
                "evidence offsets are inconsistent".to_owned(),
            ));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticInput {
    pub file_version_id: String,
    pub filename: String,
    pub extension: Option<String>,
    pub detected_content_type: Option<String>,
    pub extraction_status: String,
    pub extracted_text: String,
    pub extractor_type: Option<String>,
    pub extractor_version: Option<String>,
    pub page_count: Option<u32>,
    pub sheet_count: Option<u32>,
    pub slide_count: Option<u32>,
    pub ocr_used: bool,
    pub ocr_confidence: Option<f32>,
    pub extraction_truncated: bool,
    pub language_hint: Option<String>,
    pub locale_hint: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticLimits {
    pub max_input_chars: usize,
    pub max_fields: usize,
    pub max_entities: usize,
    pub max_candidates_per_field: usize,
    pub max_evidence_per_claim: usize,
    pub max_evidence_chars: usize,
    pub max_value_chars: usize,
    pub max_duration: Duration,
    pub max_workers: usize,
    pub queue_capacity: usize,
}

impl Default for SemanticLimits {
    fn default() -> Self {
        Self {
            max_input_chars: 250_000,
            max_fields: 64,
            max_entities: 128,
            max_candidates_per_field: 8,
            max_evidence_per_claim: 8,
            max_evidence_chars: 500,
            max_value_chars: 256,
            max_duration: Duration::from_secs(2),
            max_workers: 2,
            queue_capacity: 16,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalProviderKind {
    DeterministicRules,
    LocalModel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticProviderDescriptor {
    pub provider_id: String,
    pub provider_version: String,
    pub kind: LocalProviderKind,
    pub max_input_chars: usize,
}

pub trait SemanticProvider: Send + Sync {
    fn descriptor(&self) -> SemanticProviderDescriptor;
    fn limits(&self) -> SemanticLimits;
    fn analyze(
        &self,
        input: &SemanticInput,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<SemanticAnalysis, KnowledgeError>;
}

#[derive(Debug, Clone, Default)]
pub struct DeterministicSemanticProvider {
    limits: SemanticLimits,
    confidence_policy: ConfidencePolicy,
}

impl DeterministicSemanticProvider {
    #[must_use]
    pub const fn with_limits(limits: SemanticLimits, confidence_policy: ConfidencePolicy) -> Self {
        Self {
            limits,
            confidence_policy,
        }
    }
}

impl SemanticProvider for DeterministicSemanticProvider {
    fn descriptor(&self) -> SemanticProviderDescriptor {
        SemanticProviderDescriptor {
            provider_id: "builtin-local-rules".to_owned(),
            provider_version: ANALYZER_VERSION.to_owned(),
            kind: LocalProviderKind::DeterministicRules,
            max_input_chars: self.limits.max_input_chars,
        }
    }

    fn limits(&self) -> SemanticLimits {
        self.limits
    }

    fn analyze(
        &self,
        input: &SemanticInput,
        is_cancelled: &(dyn Fn() -> bool + Sync),
    ) -> Result<SemanticAnalysis, KnowledgeError> {
        let started = Instant::now();
        if is_cancelled() {
            return Err(KnowledgeError::Cancelled);
        }

        let input_character_count = input.extracted_text.chars().count();
        let (text, semantic_limit_applied) =
            bounded_text(&input.extracted_text, self.limits.max_input_chars);
        let analyzed_character_count = text.chars().count();
        let quality = assess_input_quality(input, &text, semantic_limit_applied)?;
        let language = detect_language(input, &text);
        check_control(started, self.limits.max_duration, is_cancelled)?;

        let document_candidates = classify_document(input, &text, &quality)?;
        let document_field = resolve_field(
            SemanticFieldType::DocumentType,
            document_candidates,
            self.confidence_policy,
        )?;
        let document_type = selected_document_type(&document_field);
        check_control(started, self.limits.max_duration, is_cancelled)?;

        let context_candidates =
            classify_context(input, &text, document_type, &quality, &document_field)?;
        let context_field = resolve_field(
            SemanticFieldType::Context,
            context_candidates,
            self.confidence_policy,
        )?;
        check_control(started, self.limits.max_duration, is_cancelled)?;

        let mut fields = vec![document_field, context_field];
        let mut entities = extract_entities(input, &text, &quality)?;
        let mut business_fields =
            extract_business_fields(input, &text, document_type, &quality, &mut entities)?;
        fields.append(&mut business_fields);
        deduplicate_entities(&mut entities);
        fields.truncate(self.limits.max_fields);
        entities.truncate(self.limits.max_entities);

        let review_reasons = review_reasons(&fields, document_type);
        let status = analysis_status(&fields, &quality);
        let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let analysis = SemanticAnalysis {
            status,
            fields,
            entities,
            input_quality: quality,
            language,
            analyzer: SemanticAnalyzer {
                analyzer_id: ANALYZER_ID.to_owned(),
                analyzer_version: ANALYZER_VERSION.to_owned(),
                provider_id: "builtin-local-rules".to_owned(),
                provider_version: ANALYZER_VERSION.to_owned(),
                schema_version: SCHEMA_VERSION,
                local_only: true,
            },
            review_reasons,
            duration_ms,
            input_character_count,
            analyzed_character_count,
        };
        analysis.validate(self.limits)?;
        Ok(analysis)
    }
}

fn check_control(
    started: Instant,
    max_duration: Duration,
    is_cancelled: &(dyn Fn() -> bool + Sync),
) -> Result<(), KnowledgeError> {
    if is_cancelled() {
        Err(KnowledgeError::Cancelled)
    } else if started.elapsed() > max_duration {
        Err(KnowledgeError::TimedOut)
    } else {
        Ok(())
    }
}

fn bounded_text(source: &str, max_chars: usize) -> (String, bool) {
    if source.chars().count() <= max_chars {
        return (source.to_owned(), false);
    }
    (source.chars().take(max_chars).collect(), true)
}

fn assess_input_quality(
    input: &SemanticInput,
    text: &str,
    semantic_limit_applied: bool,
) -> Result<InputQuality, KnowledgeError> {
    let mut score = match input.extraction_status.as_str() {
        "success" => 1.0_f32,
        "partial" => 0.78,
        "unsupported" | "skipped" => 0.35,
        "failed" => 0.2,
        _ => 0.3,
    };
    let mut reasons = Vec::new();
    if input.extraction_status == "partial" {
        reasons.push(InputQualityReason::PartialExtraction);
    }
    if matches!(
        input.extraction_status.as_str(),
        "failed" | "unsupported" | "skipped"
    ) {
        reasons.push(InputQualityReason::FailedExtraction);
    }
    if text.trim().is_empty() {
        score = score.min(0.25);
        reasons.push(InputQualityReason::EmptyText);
    }
    if input.extraction_truncated {
        score *= 0.82;
        reasons.push(InputQualityReason::TruncatedText);
    }
    if semantic_limit_applied {
        score *= 0.86;
        reasons.push(InputQualityReason::SemanticInputLimit);
    }
    if input.ocr_used {
        reasons.push(InputQualityReason::OcrUsed);
        let ocr = input.ocr_confidence.unwrap_or(0.55).clamp(0.0, 1.0);
        score *= 0.55 + 0.45 * ocr;
        if ocr < 0.65 {
            reasons.push(InputQualityReason::LowOcrConfidence);
        }
    }
    if malformed_text_ratio(text) > 0.08 {
        score *= 0.65;
        reasons.push(InputQualityReason::MalformedText);
    }
    score = score.clamp(0.0, 1.0);
    let status = if text.trim().is_empty() && score <= 0.25 {
        InputQualityStatus::Unusable
    } else if score >= 0.85 {
        InputQualityStatus::Good
    } else if score >= 0.6 {
        InputQualityStatus::Degraded
    } else {
        InputQualityStatus::Poor
    };
    Ok(InputQuality {
        score: ConfidenceScore::new(score)?,
        status,
        reasons,
    })
}

fn malformed_text_ratio(text: &str) -> f32 {
    let mut total = 0_u32;
    let mut malformed = 0_u32;
    for character in text.chars().take(20_000) {
        total = total.saturating_add(1);
        if character == '\u{fffd}'
            || (character.is_control() && !matches!(character, '\n' | '\r' | '\t'))
        {
            malformed = malformed.saturating_add(1);
        }
    }
    if total == 0 {
        0.0
    } else {
        malformed as f32 / total as f32
    }
}

fn detect_language(input: &SemanticInput, text: &str) -> Option<String> {
    if let Some(language) = input
        .language_hint
        .as_deref()
        .filter(|value| value.len() <= 16)
    {
        let normalized = language.to_ascii_lowercase();
        if normalized.starts_with("fr") {
            return Some("fr".to_owned());
        }
        if normalized.starts_with("en") {
            return Some("en".to_owned());
        }
    }
    let lowered = text.chars().take(20_000).collect::<String>().to_lowercase();
    let french = [" le ", " la ", " de ", " facture", " montant", " client"]
        .iter()
        .filter(|term| lowered.contains(*term))
        .count();
    let english = [" the ", " of ", " invoice", " amount", " customer", " due "]
        .iter()
        .filter(|term| lowered.contains(*term))
        .count();
    if french >= 2 && french > english {
        Some("fr".to_owned())
    } else if english >= 2 && english > french {
        Some("en".to_owned())
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy)]
struct Indicator {
    terms: &'static [&'static str],
    weight: f32,
    header_only: bool,
    explanation: &'static str,
}

fn classify_document(
    input: &SemanticInput,
    text: &str,
    quality: &InputQuality,
) -> Result<Vec<SemanticCandidate>, KnowledgeError> {
    let mut output = Vec::new();
    add_metadata_document_candidates(input, &mut output)?;
    let rules: &[(DocumentType, &[Indicator])] = &[
        (
            DocumentType::Invoice,
            &[
                Indicator {
                    terms: &[
                        "facture n",
                        "facture no",
                        "invoice number",
                        "invoice no",
                        "invoice #",
                    ],
                    weight: 0.48,
                    header_only: false,
                    explanation: "explicit invoice-number label",
                },
                Indicator {
                    terms: &["facture", "invoice"],
                    weight: 0.28,
                    header_only: true,
                    explanation: "invoice header",
                },
                Indicator {
                    terms: &["montant ttc", "total ttc", "total due", "amount due"],
                    weight: 0.24,
                    header_only: false,
                    explanation: "invoice total indicator",
                },
                Indicator {
                    terms: &["fournisseur", "supplier", "vendor"],
                    weight: 0.12,
                    header_only: false,
                    explanation: "supplier label",
                },
            ],
        ),
        (
            DocumentType::Quote,
            &[
                Indicator {
                    terms: &[
                        "devis n",
                        "devis no",
                        "quote number",
                        "quotation no",
                        "estimate no",
                    ],
                    weight: 0.5,
                    header_only: false,
                    explanation: "explicit quote-number label",
                },
                Indicator {
                    terms: &["devis", "quotation", "quote", "estimate"],
                    weight: 0.3,
                    header_only: true,
                    explanation: "quote header",
                },
                Indicator {
                    terms: &["validité", "valid until", "expiration date"],
                    weight: 0.2,
                    header_only: false,
                    explanation: "quote validity indicator",
                },
            ],
        ),
        (
            DocumentType::Contract,
            &[
                Indicator {
                    terms: &["contrat", "contract", "agreement"],
                    weight: 0.48,
                    header_only: true,
                    explanation: "contract title",
                },
                Indicator {
                    terms: &[
                        "entre les soussignés",
                        "between the parties",
                        "party 1",
                        "party 2",
                    ],
                    weight: 0.28,
                    header_only: false,
                    explanation: "contract party indicator",
                },
                Indicator {
                    terms: &["signature", "signed by", "fait à"],
                    weight: 0.2,
                    header_only: false,
                    explanation: "signature indicator",
                },
            ],
        ),
        (
            DocumentType::PurchaseOrder,
            &[
                Indicator {
                    terms: &["bon de commande", "purchase order"],
                    weight: 0.65,
                    header_only: true,
                    explanation: "purchase-order header",
                },
                Indicator {
                    terms: &["po number", "commande n"],
                    weight: 0.3,
                    header_only: false,
                    explanation: "purchase-order number",
                },
            ],
        ),
        (
            DocumentType::DeliveryNote,
            &[
                Indicator {
                    terms: &["bon de livraison", "delivery note"],
                    weight: 0.7,
                    header_only: true,
                    explanation: "delivery-note header",
                },
                Indicator {
                    terms: &["livré le", "delivered on"],
                    weight: 0.22,
                    header_only: false,
                    explanation: "delivery indicator",
                },
            ],
        ),
        (
            DocumentType::BankStatement,
            &[
                Indicator {
                    terms: &["relevé de compte", "bank statement", "account statement"],
                    weight: 0.7,
                    header_only: true,
                    explanation: "bank-statement header",
                },
                Indicator {
                    terms: &["iban", "solde", "opening balance", "closing balance"],
                    weight: 0.22,
                    header_only: false,
                    explanation: "bank account indicator",
                },
            ],
        ),
        (
            DocumentType::TaxDocument,
            &[
                Indicator {
                    terms: &[
                        "avis d'impôt",
                        "avis d’imposition",
                        "tax notice",
                        "income tax",
                    ],
                    weight: 0.7,
                    header_only: true,
                    explanation: "tax-document header",
                },
                Indicator {
                    terms: &["revenu fiscal", "taxpayer", "numéro fiscal"],
                    weight: 0.22,
                    header_only: false,
                    explanation: "tax indicator",
                },
            ],
        ),
        (
            DocumentType::Payslip,
            &[
                Indicator {
                    terms: &[
                        "bulletin de paie",
                        "bulletin de salaire",
                        "payslip",
                        "pay stub",
                    ],
                    weight: 0.72,
                    header_only: true,
                    explanation: "payslip header",
                },
                Indicator {
                    terms: &["salaire brut", "net à payer", "gross pay", "net pay"],
                    weight: 0.22,
                    header_only: false,
                    explanation: "pay indicator",
                },
            ],
        ),
        (
            DocumentType::EmploymentContract,
            &[
                Indicator {
                    terms: &["contrat de travail", "employment contract"],
                    weight: 0.78,
                    header_only: true,
                    explanation: "employment-contract title",
                },
                Indicator {
                    terms: &["employeur", "employee", "salarié"],
                    weight: 0.18,
                    header_only: false,
                    explanation: "employment party indicator",
                },
            ],
        ),
        (
            DocumentType::InsuranceDocument,
            &[
                Indicator {
                    terms: &[
                        "attestation d'assurance",
                        "police d'assurance",
                        "insurance policy",
                    ],
                    weight: 0.72,
                    header_only: true,
                    explanation: "insurance-document header",
                },
                Indicator {
                    terms: &["assuré", "policyholder", "numéro de police"],
                    weight: 0.2,
                    header_only: false,
                    explanation: "insurance indicator",
                },
            ],
        ),
        (
            DocumentType::Receipt,
            &[
                Indicator {
                    terms: &["ticket de caisse", "receipt"],
                    weight: 0.62,
                    header_only: true,
                    explanation: "receipt header",
                },
                Indicator {
                    terms: &["merci de votre achat", "cash", "change due"],
                    weight: 0.2,
                    header_only: false,
                    explanation: "point-of-sale indicator",
                },
            ],
        ),
        (
            DocumentType::Cv,
            &[
                Indicator {
                    terms: &["curriculum vitae", "resume"],
                    weight: 0.7,
                    header_only: true,
                    explanation: "CV title",
                },
                Indicator {
                    terms: &["expérience professionnelle", "work experience", "education"],
                    weight: 0.2,
                    header_only: false,
                    explanation: "CV section",
                },
            ],
        ),
        (
            DocumentType::Report,
            &[Indicator {
                terms: &["rapport", "report"],
                weight: 0.68,
                header_only: true,
                explanation: "report title",
            }],
        ),
        (
            DocumentType::Letter,
            &[
                Indicator {
                    terms: &["objet :", "subject:"],
                    weight: 0.35,
                    header_only: true,
                    explanation: "letter subject",
                },
                Indicator {
                    terms: &["madame, monsieur", "dear sir", "dear madam"],
                    weight: 0.35,
                    header_only: false,
                    explanation: "letter salutation",
                },
            ],
        ),
    ];

    for (document_type, indicators) in rules {
        if let Some(candidate) =
            score_document_rule(*document_type, indicators, input, text, quality)?
        {
            output.push(candidate);
        }
    }

    output.sort_by(|left, right| {
        right
            .confidence
            .value()
            .partial_cmp(&left.confidence.value())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    output.truncate(8);
    Ok(output)
}

fn add_metadata_document_candidates(
    input: &SemanticInput,
    output: &mut Vec<SemanticCandidate>,
) -> Result<(), KnowledgeError> {
    let content_type = input
        .detected_content_type
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let extension = input
        .extension
        .as_deref()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let candidate = if content_type.starts_with("image/") {
        Some((DocumentType::Photo, 0.93, "detected image media type"))
    } else if content_type.starts_with("video/") {
        Some((DocumentType::Video, 0.97, "detected video media type"))
    } else if content_type.contains("spreadsheet")
        || content_type.contains("excel")
        || matches!(extension.as_str(), "xls" | "xlsx" | "ods" | "csv")
    {
        Some((
            DocumentType::Spreadsheet,
            0.96,
            "detected spreadsheet format",
        ))
    } else if content_type.contains("presentation")
        || content_type.contains("powerpoint")
        || matches!(extension.as_str(), "ppt" | "pptx" | "odp")
    {
        Some((
            DocumentType::Presentation,
            0.96,
            "detected presentation format",
        ))
    } else if content_type.contains("zip")
        || matches!(
            extension.as_str(),
            "zip" | "7z" | "rar" | "tar" | "gz" | "bz2" | "xz"
        )
    {
        Some((DocumentType::Archive, 0.97, "detected archive format"))
    } else {
        None
    };
    if let Some((document_type, confidence, explanation)) = candidate {
        let observed = if content_type.is_empty() {
            extension
        } else {
            content_type
        };
        output.push(SemanticCandidate {
            value: SemanticValue::DocumentType {
                value: document_type,
            },
            original_value: observed.clone(),
            confidence: ConfidenceScore::new(confidence)?,
            evidence: vec![metadata_evidence(&observed, explanation, input)],
            source_method: SourceMethod::Metadata,
            ambiguous: false,
        });
    }
    Ok(())
}

fn score_document_rule(
    document_type: DocumentType,
    indicators: &[Indicator],
    input: &SemanticInput,
    text: &str,
    quality: &InputQuality,
) -> Result<Option<SemanticCandidate>, KnowledgeError> {
    let mut score = 0.0_f32;
    let mut evidence = Vec::new();
    for indicator in indicators {
        if let Some(line_match) = find_terms(text, indicator.terms, indicator.header_only) {
            score += indicator.weight;
            evidence.push(text_evidence(
                input,
                &line_match,
                indicator.explanation,
                EvidenceType::StructuralIndicator,
            ));
        }
    }
    let filename = input.filename.to_lowercase();
    let filename_terms: &[&str] = match document_type {
        DocumentType::Invoice => &["facture", "invoice"],
        DocumentType::Quote => &["devis", "quote", "quotation"],
        DocumentType::Contract => &["contrat", "contract"],
        DocumentType::PurchaseOrder => &["commande", "purchase_order"],
        DocumentType::DeliveryNote => &["livraison", "delivery"],
        DocumentType::BankStatement => &["releve", "statement"],
        DocumentType::TaxDocument => &["impot", "tax"],
        DocumentType::Payslip => &["paie", "payslip"],
        DocumentType::EmploymentContract => &["contrat_travail", "employment"],
        DocumentType::InsuranceDocument => &["assurance", "insurance"],
        DocumentType::Receipt => &["ticket", "receipt"],
        DocumentType::Cv => &["cv", "resume"],
        DocumentType::Report => &["rapport", "report"],
        DocumentType::Letter => &["lettre", "letter"],
        _ => &[],
    };
    if filename_terms.iter().any(|term| filename.contains(term)) {
        score += 0.2;
        evidence.push(filename_evidence(
            input,
            "filename contains a weak document-type hint",
        ));
    }
    if score <= 0.0 {
        return Ok(None);
    }
    let content_evidence = evidence
        .iter()
        .any(|item| item.evidence_type != EvidenceType::Filename);
    let adjusted = if content_evidence {
        adjust_for_quality(score.min(0.99), quality)
    } else {
        score.min(0.4)
    };
    Ok(Some(SemanticCandidate {
        value: SemanticValue::DocumentType {
            value: document_type,
        },
        original_value: document_type.database_name().to_owned(),
        confidence: ConfidenceScore::new(adjusted)?,
        evidence,
        source_method: if content_evidence {
            SourceMethod::DeterministicRule
        } else {
            SourceMethod::FilenameHint
        },
        ambiguous: false,
    }))
}

#[derive(Debug, Clone)]
struct LineMatch {
    start: usize,
    end: usize,
    text: String,
}

fn find_terms(source: &str, terms: &[&str], header_only: bool) -> Option<LineMatch> {
    let mut offset = 0_usize;
    for (index, segment) in source.split_inclusive('\n').enumerate() {
        let line = segment.trim_end_matches(['\r', '\n']);
        let trimmed = line.trim();
        let lowered = trimmed.to_lowercase();
        let header_eligible = !header_only || index < 12;
        let matched = terms.iter().any(|term| {
            if !header_eligible || !lowered.contains(term) {
                return false;
            }
            !header_only
                || lowered.starts_with(term)
                || lowered == *term
                || (index < 4 && lowered.contains(term))
        });
        if matched && !trimmed.is_empty() {
            let leading = line.len().saturating_sub(line.trim_start().len());
            let start = offset.saturating_add(leading);
            return Some(LineMatch {
                start,
                end: start.saturating_add(trimmed.len()),
                text: truncate_chars(trimmed, 500),
            });
        }
        offset = offset.saturating_add(segment.len());
    }
    None
}

fn classify_context(
    input: &SemanticInput,
    text: &str,
    document_type: Option<DocumentType>,
    quality: &InputQuality,
    document_field: &SemanticField,
) -> Result<Vec<SemanticCandidate>, KnowledgeError> {
    let business_terms = [
        "siret",
        "tva",
        "vat number",
        "fournisseur",
        "supplier",
        "client:",
        "customer:",
        "purchase order",
        "bon de commande",
    ];
    let personal_terms = [
        "numéro fiscal",
        "taxpayer",
        "date de naissance",
        "date of birth",
        "domicile",
        "adresse personnelle",
    ];
    let business_matches = matched_term_evidence(input, text, &business_terms, 3);
    let personal_matches = matched_term_evidence(input, text, &personal_terms, 3);
    let employment_mixed = matches!(
        document_type,
        Some(DocumentType::Payslip | DocumentType::EmploymentContract)
    );
    let personal_type = matches!(
        document_type,
        Some(DocumentType::TaxDocument | DocumentType::Cv)
    );
    let business_type = matches!(
        document_type,
        Some(
            DocumentType::Invoice
                | DocumentType::Quote
                | DocumentType::Contract
                | DocumentType::PurchaseOrder
                | DocumentType::DeliveryNote
                | DocumentType::BankStatement
        )
    );

    let mut output = Vec::new();
    if employment_mixed || (!business_matches.is_empty() && !personal_matches.is_empty()) {
        let mut evidence = business_matches;
        evidence.extend(personal_matches);
        evidence.extend(document_field.evidence.iter().take(1).cloned());
        output.push(context_candidate(
            DocumentContext::Mixed,
            0.82,
            evidence,
            quality,
            false,
        )?);
    } else if business_type || business_matches.len() >= 2 {
        let mut evidence = business_matches;
        evidence.extend(document_field.evidence.iter().take(1).cloned());
        let base = if evidence.len() >= 2 { 0.92 } else { 0.74 };
        output.push(context_candidate(
            DocumentContext::Business,
            base,
            evidence,
            quality,
            false,
        )?);
    } else if personal_type || personal_matches.len() >= 2 {
        let mut evidence = personal_matches;
        evidence.extend(document_field.evidence.iter().take(1).cloned());
        let base = if evidence.len() >= 2 { 0.88 } else { 0.72 };
        output.push(context_candidate(
            DocumentContext::Personal,
            base,
            evidence,
            quality,
            false,
        )?);
    }

    if output.is_empty() {
        output.push(SemanticCandidate {
            value: SemanticValue::Context {
                value: DocumentContext::Unknown,
            },
            original_value: "unknown".to_owned(),
            confidence: ConfidenceScore::new(0.0)?,
            evidence: Vec::new(),
            source_method: SourceMethod::DeterministicRule,
            ambiguous: false,
        });
    }
    Ok(output)
}

fn context_candidate(
    context: DocumentContext,
    base: f32,
    evidence: Vec<SemanticEvidence>,
    quality: &InputQuality,
    ambiguous: bool,
) -> Result<SemanticCandidate, KnowledgeError> {
    Ok(SemanticCandidate {
        value: SemanticValue::Context { value: context },
        original_value: context.database_name().to_owned(),
        confidence: ConfidenceScore::new(adjust_for_quality(base, quality))?,
        evidence,
        source_method: SourceMethod::DeterministicRule,
        ambiguous,
    })
}

fn matched_term_evidence(
    input: &SemanticInput,
    text: &str,
    terms: &[&str],
    limit: usize,
) -> Vec<SemanticEvidence> {
    let mut output = Vec::new();
    for term in terms {
        if output.len() >= limit {
            break;
        }
        if let Some(found) = find_terms(text, &[*term], false) {
            output.push(text_evidence(
                input,
                &found,
                "context indicator",
                EvidenceType::TextSpan,
            ));
        }
    }
    output
}

fn resolve_field(
    field_type: SemanticFieldType,
    mut candidates: Vec<SemanticCandidate>,
    policy: ConfidencePolicy,
) -> Result<SemanticField, KnowledgeError> {
    merge_equivalent_candidates(&mut candidates)?;
    candidates.sort_by(|left, right| {
        right
            .confidence
            .value()
            .partial_cmp(&left.confidence.value())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.truncate(8);
    let Some(primary) = candidates.first().cloned() else {
        return unknown_field(field_type);
    };
    if matches!(
        &primary.value,
        SemanticValue::DocumentType {
            value: DocumentType::Unknown
        } | SemanticValue::Context {
            value: DocumentContext::Unknown
        }
    ) {
        return Ok(SemanticField {
            field_type,
            value: None,
            original_value: None,
            confidence: primary.confidence,
            status: FieldStatus::Unknown,
            evidence: primary.evidence.clone(),
            candidates,
            source_method: primary.source_method,
            analyzer_version: ANALYZER_VERSION.to_owned(),
        });
    }
    let conflicting = candidates.get(1).is_some_and(|second| {
        primary.confidence.value() >= policy.medium
            && second.confidence.value() >= policy.medium
            && primary.confidence.value() - second.confidence.value() < policy.conflict_margin
            && value_key(&primary.value) != value_key(&second.value)
    });
    if conflicting {
        let mut evidence = primary.evidence.clone();
        if let Some(second) = candidates.get(1) {
            append_unique_evidence(&mut evidence, &second.evidence);
        }
        return Ok(SemanticField {
            field_type,
            value: None,
            original_value: None,
            confidence: primary.confidence,
            status: FieldStatus::Conflicting,
            evidence,
            candidates,
            source_method: primary.source_method,
            analyzer_version: ANALYZER_VERSION.to_owned(),
        });
    }
    if primary.confidence.value() < policy.medium {
        return Ok(SemanticField {
            field_type,
            value: None,
            original_value: None,
            confidence: primary.confidence,
            status: if primary.ambiguous {
                FieldStatus::Ambiguous
            } else {
                FieldStatus::Unknown
            },
            evidence: primary.evidence.clone(),
            candidates,
            source_method: primary.source_method,
            analyzer_version: ANALYZER_VERSION.to_owned(),
        });
    }
    let status = if primary.ambiguous {
        FieldStatus::Ambiguous
    } else if primary.confidence.value() >= policy.high
        && primary.source_method != SourceMethod::FilenameHint
    {
        FieldStatus::Confirmed
    } else {
        FieldStatus::Inferred
    };
    Ok(SemanticField {
        field_type,
        value: Some(primary.value.clone()),
        original_value: Some(primary.original_value.clone()),
        confidence: primary.confidence,
        status,
        evidence: primary.evidence.clone(),
        candidates,
        source_method: primary.source_method,
        analyzer_version: ANALYZER_VERSION.to_owned(),
    })
}

fn merge_equivalent_candidates(
    candidates: &mut Vec<SemanticCandidate>,
) -> Result<(), KnowledgeError> {
    let mut merged: BTreeMap<String, SemanticCandidate> = BTreeMap::new();
    for candidate in candidates.drain(..) {
        let key = value_key(&candidate.value);
        if let Some(existing) = merged.get_mut(&key) {
            let combined =
                (existing.confidence.value() + candidate.confidence.value() * 0.18).min(0.99);
            existing.confidence = ConfidenceScore::new(combined)?;
            existing.ambiguous |= candidate.ambiguous;
            append_unique_evidence(&mut existing.evidence, &candidate.evidence);
        } else {
            merged.insert(key, candidate);
        }
    }
    *candidates = merged.into_values().collect();
    Ok(())
}

fn append_unique_evidence(destination: &mut Vec<SemanticEvidence>, source: &[SemanticEvidence]) {
    for evidence in source {
        if destination.len() >= 8 {
            break;
        }
        if !destination.iter().any(|existing| {
            existing.evidence_type == evidence.evidence_type
                && existing.start_offset == evidence.start_offset
                && existing.exact_text == evidence.exact_text
        }) {
            destination.push(evidence.clone());
        }
    }
}

fn value_key(value: &SemanticValue) -> String {
    match value {
        SemanticValue::Text { value } => format!("text:{}", value.to_lowercase()),
        SemanticValue::Date { iso_date } => format!("date:{iso_date}"),
        SemanticValue::Money {
            amount_minor,
            scale,
            currency,
        } => format!(
            "money:{amount_minor}:{scale}:{}",
            currency.as_deref().unwrap_or("")
        ),
        SemanticValue::DocumentType { value } => {
            format!("document_type:{}", value.database_name())
        }
        SemanticValue::Context { value } => format!("context:{}", value.database_name()),
        SemanticValue::TextList { values } => format!("list:{}", values.join("|").to_lowercase()),
    }
}

fn unknown_field(field_type: SemanticFieldType) -> Result<SemanticField, KnowledgeError> {
    Ok(SemanticField {
        field_type,
        value: None,
        original_value: None,
        confidence: ConfidenceScore::new(0.0)?,
        status: FieldStatus::Unknown,
        evidence: Vec::new(),
        candidates: Vec::new(),
        source_method: SourceMethod::DeterministicRule,
        analyzer_version: ANALYZER_VERSION.to_owned(),
    })
}

fn selected_document_type(field: &SemanticField) -> Option<DocumentType> {
    match field.value.as_ref() {
        Some(SemanticValue::DocumentType { value }) => Some(*value),
        _ => None,
    }
}

fn extract_entities(
    input: &SemanticInput,
    text: &str,
    quality: &InputQuality,
) -> Result<Vec<SemanticEntity>, KnowledgeError> {
    let mut entities = Vec::new();
    let label_groups: &[(EntityType, &[&str], f32)] = &[
        (
            EntityType::CustomerCandidate,
            &["client", "customer", "bill to", "facturé à"],
            0.92,
        ),
        (
            EntityType::SupplierCandidate,
            &["fournisseur", "supplier", "vendor"],
            0.92,
        ),
        (
            EntityType::ProjectCandidate,
            &["projet", "project", "référence projet", "project reference"],
            0.84,
        ),
        (
            EntityType::Organization,
            &[
                "société",
                "company",
                "entreprise",
                "organisation",
                "organization",
            ],
            0.88,
        ),
        (
            EntityType::Person,
            &["nom", "name", "contact", "signataire", "signatory"],
            0.82,
        ),
        (EntityType::Address, &["adresse", "address"], 0.9),
    ];
    for (entity_type, labels, base) in label_groups {
        for value in labeled_values(text, labels, 12) {
            if value.value.chars().count() < 2 {
                continue;
            }
            entities.push(make_entity(
                input,
                *entity_type,
                &value,
                adjust_for_quality(*base, quality),
                SourceMethod::StructuredParser,
                "explicit labeled entity",
            )?);
        }
    }

    for capture in EMAIL_REGEX.captures_iter(text).take(24) {
        if let Some(found) = capture.get(0) {
            let located = LocatedValue::from_match(found);
            entities.push(make_entity(
                input,
                EntityType::Email,
                &located,
                adjust_for_quality(0.99, quality),
                SourceMethod::RegexParser,
                "email parser match",
            )?);
        }
    }
    for capture in PHONE_REGEX.captures_iter(text).take(16) {
        if let Some(found) = capture.get(1) {
            let located = LocatedValue::from_match(found);
            entities.push(make_entity(
                input,
                EntityType::Phone,
                &located,
                adjust_for_quality(0.9, quality),
                SourceMethod::RegexParser,
                "labeled phone parser match",
            )?);
        }
    }
    for capture in COMPANY_ID_REGEX.captures_iter(text).take(16) {
        if let Some(found) = capture.get(1) {
            let located = LocatedValue::from_match(found);
            entities.push(make_entity(
                input,
                EntityType::SiretOrCompanyId,
                &located,
                adjust_for_quality(0.97, quality),
                SourceMethod::RegexParser,
                "explicit company identifier",
            )?);
        }
    }
    for parsed in parse_dates(text, input, quality).into_iter().take(24) {
        entities.push(SemanticEntity {
            candidate_key: candidate_key(EntityType::Date, &parsed.normalized),
            entity_type: EntityType::Date,
            original_value: parsed.located.value.clone(),
            normalized_value: parsed.normalized,
            confidence: ConfidenceScore::new(parsed.confidence)?,
            status: if parsed.alternate.is_some() {
                FieldStatus::Ambiguous
            } else if parsed.confidence >= 0.85 {
                FieldStatus::Confirmed
            } else {
                FieldStatus::Inferred
            },
            evidence: vec![located_evidence(
                input,
                &parsed.located,
                "calendar date parser match",
                EvidenceType::ParserMatch,
            )],
            source_method: SourceMethod::RegexParser,
            analyzer_version: ANALYZER_VERSION.to_owned(),
        });
    }
    for parsed in parse_money_values(text, true).into_iter().take(24) {
        let Some(currency) = parsed.currency.clone() else {
            continue;
        };
        let normalized = format!("{}:{}:{currency}", parsed.amount_minor, parsed.scale);
        entities.push(SemanticEntity {
            candidate_key: candidate_key(EntityType::Amount, &normalized),
            entity_type: EntityType::Amount,
            original_value: parsed.located.value.clone(),
            normalized_value: normalized,
            confidence: ConfidenceScore::new(adjust_for_quality(parsed.confidence, quality))?,
            status: FieldStatus::Confirmed,
            evidence: vec![located_evidence(
                input,
                &parsed.located,
                "currency amount parser match",
                EvidenceType::ParserMatch,
            )],
            source_method: SourceMethod::RegexParser,
            analyzer_version: ANALYZER_VERSION.to_owned(),
        });
        entities.push(SemanticEntity {
            candidate_key: candidate_key(EntityType::Currency, &currency),
            entity_type: EntityType::Currency,
            original_value: currency.clone(),
            normalized_value: currency,
            confidence: ConfidenceScore::new(adjust_for_quality(parsed.confidence, quality))?,
            status: FieldStatus::Confirmed,
            evidence: vec![located_evidence(
                input,
                &parsed.located,
                "currency symbol or code",
                EvidenceType::ParserMatch,
            )],
            source_method: SourceMethod::RegexParser,
            analyzer_version: ANALYZER_VERSION.to_owned(),
        });
    }
    Ok(entities)
}

#[derive(Debug, Clone)]
struct LocatedValue {
    value: String,
    start: usize,
    end: usize,
}

impl LocatedValue {
    fn from_match(found: regex::Match<'_>) -> Self {
        Self {
            value: truncate_chars(found.as_str().trim(), 256),
            start: found.start(),
            end: found.end(),
        }
    }
}

fn labeled_values(source: &str, labels: &[&str], limit: usize) -> Vec<LocatedValue> {
    let mut output = Vec::new();
    let mut offset = 0_usize;
    for segment in source.split_inclusive('\n') {
        if output.len() >= limit {
            break;
        }
        let line = segment.trim_end_matches(['\r', '\n']);
        let trimmed_start = line.trim_start();
        let leading = line.len().saturating_sub(trimmed_start.len());
        let lowered = trimmed_start.to_lowercase();
        for label in labels {
            if !lowered.starts_with(label) {
                continue;
            }
            let rest = &trimmed_start[label.len()..];
            let separator_length = rest
                .char_indices()
                .find(|(_, character)| !character.is_whitespace())
                .and_then(|(index, character)| {
                    matches!(character, ':' | '#' | '-' | '–')
                        .then_some(index + character.len_utf8())
                });
            let Some(separator_length) = separator_length else {
                continue;
            };
            let raw_value = rest[separator_length..].trim();
            if raw_value.is_empty() {
                continue;
            }
            let bounded = truncate_chars(raw_value, 256);
            let value_position = trimmed_start.find(raw_value).unwrap_or(label.len());
            let start = offset
                .saturating_add(leading)
                .saturating_add(value_position);
            output.push(LocatedValue {
                value: bounded.clone(),
                start,
                end: start.saturating_add(bounded.len()),
            });
            break;
        }
        offset = offset.saturating_add(segment.len());
    }
    output
}

fn make_entity(
    input: &SemanticInput,
    entity_type: EntityType,
    located: &LocatedValue,
    confidence: f32,
    source_method: SourceMethod,
    explanation: &str,
) -> Result<SemanticEntity, KnowledgeError> {
    let normalized = normalize_entity_value(&located.value);
    Ok(SemanticEntity {
        candidate_key: candidate_key(entity_type, &normalized),
        entity_type,
        original_value: located.value.clone(),
        normalized_value: normalized,
        confidence: ConfidenceScore::new(confidence)?,
        status: if confidence >= 0.85 {
            FieldStatus::Confirmed
        } else {
            FieldStatus::Inferred
        },
        evidence: vec![located_evidence(
            input,
            located,
            explanation,
            EvidenceType::ParserMatch,
        )],
        source_method,
        analyzer_version: ANALYZER_VERSION.to_owned(),
    })
}

fn normalize_entity_value(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|character: char| {
            character.is_ascii_punctuation() && !matches!(character, '@' | '+' | '-')
        })
        .to_owned()
}

fn candidate_key(entity_type: EntityType, normalized: &str) -> String {
    truncate_chars(
        &format!(
            "{}:{}",
            entity_type.database_name(),
            normalized.to_lowercase()
        ),
        320,
    )
}

fn deduplicate_entities(entities: &mut Vec<SemanticEntity>) {
    let mut seen = HashSet::new();
    entities.retain(|entity| seen.insert(entity.candidate_key.clone()));
    entities.sort_by(|left, right| {
        right
            .confidence
            .value()
            .partial_cmp(&left.confidence.value())
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

fn extract_business_fields(
    input: &SemanticInput,
    text: &str,
    document_type: Option<DocumentType>,
    quality: &InputQuality,
    entities: &mut Vec<SemanticEntity>,
) -> Result<Vec<SemanticField>, KnowledgeError> {
    let mut fields = Vec::new();
    let policy = ConfidencePolicy::default();

    let customer_candidates = candidates_from_labels(
        input,
        text,
        &["client", "customer", "bill to", "facturé à"],
        quality,
        0.94,
    )?;
    if !customer_candidates.is_empty() {
        fields.push(resolve_field(
            SemanticFieldType::CustomerCandidate,
            customer_candidates,
            policy,
        )?);
    }
    let mut supplier_candidates = candidates_from_labels(
        input,
        text,
        &["fournisseur", "supplier", "vendor", "issuer", "émetteur"],
        quality,
        0.94,
    )?;
    if supplier_candidates.is_empty()
        && matches!(
            document_type,
            Some(DocumentType::Invoice | DocumentType::Quote)
        )
        && let Some(header) = supplier_header_candidate(input, text, quality)?
    {
        supplier_candidates.push(header);
    }
    if !supplier_candidates.is_empty() {
        if let Some(primary) = supplier_candidates.first() {
            let located = primary
                .evidence
                .first()
                .and_then(evidence_as_located)
                .unwrap_or(LocatedValue {
                    value: primary.original_value.clone(),
                    start: 0,
                    end: 0,
                });
            entities.push(make_entity(
                input,
                EntityType::SupplierCandidate,
                &located,
                primary.confidence.value(),
                primary.source_method,
                "supplier candidate used by business field extraction",
            )?);
        }
        fields.push(resolve_field(
            if document_type == Some(DocumentType::Quote) {
                SemanticFieldType::Issuer
            } else {
                SemanticFieldType::SupplierCandidate
            },
            supplier_candidates,
            policy,
        )?);
    }

    match document_type {
        Some(DocumentType::Invoice) => {
            let identifiers = identifier_candidates(
                input,
                text,
                &["facture", "invoice"],
                quality,
                EntityType::InvoiceNumber,
                entities,
            )?;
            if !identifiers.is_empty() {
                fields.push(resolve_field(
                    SemanticFieldType::InvoiceNumber,
                    identifiers,
                    policy,
                )?);
            }
            append_invoice_fields(input, text, quality, &mut fields)?;
        }
        Some(DocumentType::Quote) => {
            let identifiers = identifier_candidates(
                input,
                text,
                &["devis", "quote", "quotation", "estimate"],
                quality,
                EntityType::DocumentNumber,
                entities,
            )?;
            if !identifiers.is_empty() {
                fields.push(resolve_field(
                    SemanticFieldType::QuoteNumber,
                    identifiers,
                    policy,
                )?);
            }
            append_quote_fields(input, text, quality, &mut fields)?;
        }
        Some(DocumentType::Contract | DocumentType::EmploymentContract) => {
            append_contract_fields(input, text, quality, &mut fields)?;
        }
        _ => {}
    }

    let company_ids = candidates_from_labels(
        input,
        text,
        &[
            "siret",
            "siren",
            "company id",
            "registration number",
            "vat number",
        ],
        quality,
        0.97,
    )?;
    if !company_ids.is_empty() {
        fields.push(resolve_field(
            SemanticFieldType::CompanyIdentifier,
            company_ids,
            policy,
        )?);
    }
    let references = candidates_from_labels(
        input,
        text,
        &[
            "commande",
            "bon de commande",
            "purchase order",
            "po reference",
            "order reference",
        ],
        quality,
        0.9,
    )?;
    if !references.is_empty() {
        fields.push(resolve_field(
            SemanticFieldType::PurchaseOrderReference,
            references,
            policy,
        )?);
    }
    let projects = candidates_from_labels(
        input,
        text,
        &["projet", "project", "référence projet", "project reference"],
        quality,
        0.84,
    )?;
    if !projects.is_empty() {
        fields.push(resolve_field(
            SemanticFieldType::ProjectReferenceCandidate,
            projects,
            policy,
        )?);
    }
    Ok(fields)
}

fn candidates_from_labels(
    input: &SemanticInput,
    text: &str,
    labels: &[&str],
    quality: &InputQuality,
    base_confidence: f32,
) -> Result<Vec<SemanticCandidate>, KnowledgeError> {
    labeled_values(text, labels, 8)
        .into_iter()
        .map(|located| {
            Ok(SemanticCandidate {
                value: SemanticValue::Text {
                    value: normalize_entity_value(&located.value),
                },
                original_value: located.value.clone(),
                confidence: ConfidenceScore::new(adjust_for_quality(base_confidence, quality))?,
                evidence: vec![located_evidence(
                    input,
                    &located,
                    "explicit labeled field",
                    EvidenceType::ParserMatch,
                )],
                source_method: SourceMethod::StructuredParser,
                ambiguous: false,
            })
        })
        .collect()
}

fn supplier_header_candidate(
    input: &SemanticInput,
    text: &str,
    quality: &InputQuality,
) -> Result<Option<SemanticCandidate>, KnowledgeError> {
    let mut offset = 0_usize;
    for (index, segment) in text.split_inclusive('\n').enumerate().take(6) {
        let line = segment.trim_end_matches(['\r', '\n']);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            offset = offset.saturating_add(segment.len());
            continue;
        }
        let lowered = trimmed.to_lowercase();
        let letters = trimmed
            .chars()
            .filter(|character| character.is_alphabetic())
            .count();
        let uppercase = trimmed
            .chars()
            .filter(|character| character.is_alphabetic() && character.is_uppercase())
            .count();
        let plausible = index < 4
            && letters >= 2
            && trimmed.chars().count() <= 80
            && uppercase.saturating_mul(100) / letters.max(1) >= 70
            && !["facture", "invoice", "devis", "quote"]
                .iter()
                .any(|term| lowered.contains(term));
        if plausible {
            let leading = line.len().saturating_sub(line.trim_start().len());
            let located = LocatedValue {
                value: truncate_chars(trimmed, 256),
                start: offset.saturating_add(leading),
                end: offset.saturating_add(leading).saturating_add(trimmed.len()),
            };
            return Ok(Some(SemanticCandidate {
                value: SemanticValue::Text {
                    value: normalize_entity_value(trimmed),
                },
                original_value: trimmed.to_owned(),
                confidence: ConfidenceScore::new(adjust_for_quality(0.82, quality))?,
                evidence: vec![located_evidence(
                    input,
                    &located,
                    "organization-like invoice header; candidate, not global identity",
                    EvidenceType::StructuralIndicator,
                )],
                source_method: SourceMethod::DeterministicRule,
                ambiguous: false,
            }));
        }
        offset = offset.saturating_add(segment.len());
    }
    Ok(None)
}

fn identifier_candidates(
    input: &SemanticInput,
    text: &str,
    labels: &[&str],
    quality: &InputQuality,
    entity_type: EntityType,
    entities: &mut Vec<SemanticEntity>,
) -> Result<Vec<SemanticCandidate>, KnowledgeError> {
    let mut output = Vec::new();
    for capture in DOCUMENT_NUMBER_REGEX.captures_iter(text).take(8) {
        let Some(label) = capture.get(1) else {
            continue;
        };
        if !labels
            .iter()
            .any(|expected| label.as_str().to_lowercase().contains(expected))
        {
            continue;
        }
        let Some(number) = capture.get(2) else {
            continue;
        };
        let normalized = number
            .as_str()
            .trim_matches(|character: char| character.is_ascii_punctuation())
            .to_ascii_uppercase();
        if normalized.len() < 3
            || normalized.len() > 48
            || !normalized.bytes().any(|byte| byte.is_ascii_digit())
        {
            continue;
        }
        let full = capture.get(0).unwrap_or(number);
        let located = LocatedValue::from_match(full);
        let confidence = adjust_for_quality(0.98, quality);
        output.push(SemanticCandidate {
            value: SemanticValue::Text {
                value: normalized.clone(),
            },
            original_value: number.as_str().to_owned(),
            confidence: ConfidenceScore::new(confidence)?,
            evidence: vec![located_evidence(
                input,
                &located,
                "explicit document-number label",
                EvidenceType::ParserMatch,
            )],
            source_method: SourceMethod::RegexParser,
            ambiguous: false,
        });
        entities.push(SemanticEntity {
            candidate_key: candidate_key(entity_type, &normalized),
            entity_type,
            original_value: number.as_str().to_owned(),
            normalized_value: normalized,
            confidence: ConfidenceScore::new(confidence)?,
            status: FieldStatus::Confirmed,
            evidence: vec![located_evidence(
                input,
                &located,
                "explicit document-number label",
                EvidenceType::ParserMatch,
            )],
            source_method: SourceMethod::RegexParser,
            analyzer_version: ANALYZER_VERSION.to_owned(),
        });
    }
    Ok(output)
}

fn append_invoice_fields(
    input: &SemanticInput,
    text: &str,
    quality: &InputQuality,
    fields: &mut Vec<SemanticField>,
) -> Result<(), KnowledgeError> {
    append_date_field(
        input,
        text,
        quality,
        &[
            "date de facture",
            "invoice date",
            "date d'émission",
            "date d’émission",
            "issue date",
        ],
        SemanticFieldType::IssueDate,
        fields,
    )?;
    append_first_document_date(input, text, quality, fields)?;
    append_date_field(
        input,
        text,
        quality,
        &[
            "date d'échéance",
            "date d’échéance",
            "échéance",
            "due date",
            "payment due",
        ],
        SemanticFieldType::DueDate,
        fields,
    )?;
    append_money_field(
        input,
        text,
        quality,
        &["sous-total", "subtotal", "total ht"],
        SemanticFieldType::Subtotal,
        fields,
    )?;
    append_money_field(
        input,
        text,
        quality,
        &["tva", "tax", "vat"],
        SemanticFieldType::Tax,
        fields,
    )?;
    append_money_field(
        input,
        text,
        quality,
        &[
            "montant ttc",
            "total ttc",
            "total due",
            "amount due",
            "net à payer",
            "total",
        ],
        SemanticFieldType::Total,
        fields,
    )
}

fn append_quote_fields(
    input: &SemanticInput,
    text: &str,
    quality: &InputQuality,
    fields: &mut Vec<SemanticField>,
) -> Result<(), KnowledgeError> {
    append_date_field(
        input,
        text,
        quality,
        &[
            "date du devis",
            "quote date",
            "issue date",
            "date d'émission",
            "date d’émission",
        ],
        SemanticFieldType::IssueDate,
        fields,
    )?;
    append_first_document_date(input, text, quality, fields)?;
    append_date_field(
        input,
        text,
        quality,
        &[
            "date d'expiration",
            "date d’expiration",
            "validité",
            "expiration date",
            "valid until",
        ],
        SemanticFieldType::ExpirationDate,
        fields,
    )?;
    append_money_field(
        input,
        text,
        quality,
        &["montant", "amount", "total ttc", "total"],
        SemanticFieldType::Amount,
        fields,
    )
}

fn append_contract_fields(
    input: &SemanticInput,
    text: &str,
    quality: &InputQuality,
    fields: &mut Vec<SemanticField>,
) -> Result<(), KnowledgeError> {
    append_date_field(
        input,
        text,
        quality,
        &[
            "date de signature",
            "signature date",
            "date du contrat",
            "contract date",
            "issue date",
        ],
        SemanticFieldType::IssueDate,
        fields,
    )?;
    append_first_document_date(input, text, quality, fields)?;
    let parties = candidates_from_labels(
        input,
        text,
        &["parties", "contracting parties", "entre les soussignés"],
        quality,
        0.9,
    )?;
    if !parties.is_empty() {
        fields.push(resolve_field(
            SemanticFieldType::ContractParties,
            parties,
            ConfidencePolicy::default(),
        )?);
    }
    if let Some(title) = first_meaningful_line(text, &["contrat", "contract", "agreement"]) {
        fields.push(resolve_field(
            SemanticFieldType::ContractTitle,
            vec![SemanticCandidate {
                value: SemanticValue::Text {
                    value: title.text.clone(),
                },
                original_value: title.text.clone(),
                confidence: ConfidenceScore::new(adjust_for_quality(0.88, quality))?,
                evidence: vec![text_evidence(
                    input,
                    &title,
                    "contract title line",
                    EvidenceType::StructuralIndicator,
                )],
                source_method: SourceMethod::DeterministicRule,
                ambiguous: false,
            }],
            ConfidencePolicy::default(),
        )?);
    }
    Ok(())
}

fn append_first_document_date(
    input: &SemanticInput,
    text: &str,
    quality: &InputQuality,
    fields: &mut Vec<SemanticField>,
) -> Result<(), KnowledgeError> {
    if fields
        .iter()
        .any(|field| field.field_type == SemanticFieldType::IssueDate)
    {
        return Ok(());
    }
    let Some(parsed) = parse_dates(text, input, quality).into_iter().next() else {
        return Ok(());
    };
    let mut candidates = vec![SemanticCandidate {
        value: SemanticValue::Date {
            iso_date: parsed.normalized,
        },
        original_value: parsed.located.value.clone(),
        confidence: ConfidenceScore::new((parsed.confidence * 0.88).clamp(0.0, 1.0))?,
        evidence: vec![located_evidence(
            input,
            &parsed.located,
            "first document date near a recognized business-document header",
            EvidenceType::ParserMatch,
        )],
        source_method: SourceMethod::DeterministicRule,
        ambiguous: parsed.alternate.is_some(),
    }];
    if let Some(alternate) = parsed.alternate {
        candidates.push(SemanticCandidate {
            value: SemanticValue::Date {
                iso_date: alternate,
            },
            original_value: parsed.located.value.clone(),
            confidence: ConfidenceScore::new((parsed.confidence * 0.86).clamp(0.0, 1.0))?,
            evidence: vec![located_evidence(
                input,
                &parsed.located,
                "alternate locale interpretation of a document date",
                EvidenceType::ParserMatch,
            )],
            source_method: SourceMethod::DeterministicRule,
            ambiguous: true,
        });
    }
    fields.push(resolve_field(
        SemanticFieldType::IssueDate,
        candidates,
        ConfidencePolicy::default(),
    )?);
    Ok(())
}

fn first_meaningful_line(source: &str, terms: &[&str]) -> Option<LineMatch> {
    find_terms(source, terms, true)
}

fn append_date_field(
    input: &SemanticInput,
    text: &str,
    quality: &InputQuality,
    labels: &[&str],
    field_type: SemanticFieldType,
    fields: &mut Vec<SemanticField>,
) -> Result<(), KnowledgeError> {
    let mut candidates = Vec::new();
    for labeled in labeled_values(text, labels, 8) {
        for parsed in parse_dates(&labeled.value, input, quality) {
            let shift = labeled.start;
            let located = LocatedValue {
                value: parsed.located.value,
                start: shift.saturating_add(parsed.located.start),
                end: shift.saturating_add(parsed.located.end),
            };
            candidates.push(SemanticCandidate {
                value: SemanticValue::Date {
                    iso_date: parsed.normalized,
                },
                original_value: located.value.clone(),
                confidence: ConfidenceScore::new(parsed.confidence)?,
                evidence: vec![located_evidence(
                    input,
                    &located,
                    "date found under an explicit semantic label",
                    EvidenceType::ParserMatch,
                )],
                source_method: SourceMethod::StructuredParser,
                ambiguous: parsed.alternate.is_some(),
            });
            if let Some(alternate) = parsed.alternate {
                candidates.push(SemanticCandidate {
                    value: SemanticValue::Date {
                        iso_date: alternate,
                    },
                    original_value: located.value.clone(),
                    confidence: ConfidenceScore::new((parsed.confidence - 0.02).max(0.0))?,
                    evidence: vec![located_evidence(
                        input,
                        &located,
                        "alternate locale interpretation of an ambiguous numeric date",
                        EvidenceType::ParserMatch,
                    )],
                    source_method: SourceMethod::StructuredParser,
                    ambiguous: true,
                });
            }
        }
    }
    if !candidates.is_empty() {
        fields.push(resolve_field(
            field_type,
            candidates,
            ConfidencePolicy::default(),
        )?);
    }
    Ok(())
}

fn append_money_field(
    input: &SemanticInput,
    text: &str,
    quality: &InputQuality,
    labels: &[&str],
    field_type: SemanticFieldType,
    fields: &mut Vec<SemanticField>,
) -> Result<(), KnowledgeError> {
    let mut candidates = Vec::new();
    for labeled in labeled_values(text, labels, 8) {
        for parsed in parse_money_values(&labeled.value, false)
            .into_iter()
            .take(2)
        {
            let located = LocatedValue {
                value: parsed.located.value,
                start: labeled.start.saturating_add(parsed.located.start),
                end: labeled.start.saturating_add(parsed.located.end),
            };
            let has_currency = parsed.currency.is_some();
            candidates.push(SemanticCandidate {
                value: SemanticValue::Money {
                    amount_minor: parsed.amount_minor,
                    scale: parsed.scale,
                    currency: parsed.currency,
                },
                original_value: located.value.clone(),
                confidence: ConfidenceScore::new(adjust_for_quality(
                    if has_currency {
                        parsed.confidence
                    } else {
                        0.68
                    },
                    quality,
                ))?,
                evidence: vec![located_evidence(
                    input,
                    &located,
                    if has_currency {
                        "money parser match under an explicit amount label"
                    } else {
                        "amount has no evidenced currency"
                    },
                    EvidenceType::ParserMatch,
                )],
                source_method: SourceMethod::StructuredParser,
                ambiguous: !has_currency,
            });
        }
    }
    if !candidates.is_empty() {
        fields.push(resolve_field(
            field_type,
            candidates,
            ConfidencePolicy::default(),
        )?);
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct ParsedDate {
    located: LocatedValue,
    normalized: String,
    alternate: Option<String>,
    confidence: f32,
}

fn parse_dates(source: &str, input: &SemanticInput, quality: &InputQuality) -> Vec<ParsedDate> {
    let mut output = Vec::new();
    let french_locale = input
        .locale_hint
        .as_deref()
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("fr"))
        || input
            .language_hint
            .as_deref()
            .is_some_and(|value| value.to_ascii_lowercase().starts_with("fr"));
    for capture in NUMERIC_DATE_REGEX.captures_iter(source).take(32) {
        let (Some(full), Some(first), Some(second), Some(third)) = (
            capture.get(0),
            capture.get(1),
            capture.get(2),
            capture.get(3),
        ) else {
            continue;
        };
        let Ok(a) = first.as_str().parse::<u32>() else {
            continue;
        };
        let Ok(b) = second.as_str().parse::<u32>() else {
            continue;
        };
        let Ok(c) = third.as_str().parse::<u32>() else {
            continue;
        };
        let (year, month, day, alternate, base) = if first.as_str().len() == 4 {
            (a, b, c, None, 0.99)
        } else if third.as_str().len() == 4 {
            if a > 12 {
                (c, b, a, None, 0.98)
            } else if b > 12 {
                (c, a, b, None, 0.95)
            } else {
                let primary = if french_locale { (c, b, a) } else { (c, a, b) };
                let alternate = if french_locale { (c, a, b) } else { (c, b, a) };
                (
                    primary.0,
                    primary.1,
                    primary.2,
                    valid_date(alternate.0, alternate.1, alternate.2)
                        .then(|| iso_date(alternate.0, alternate.1, alternate.2)),
                    0.72,
                )
            }
        } else {
            continue;
        };
        if !valid_date(year, month, day) {
            continue;
        }
        output.push(ParsedDate {
            located: LocatedValue::from_match(full),
            normalized: iso_date(year, month, day),
            alternate,
            confidence: adjust_for_quality(base, quality),
        });
    }
    for capture in TEXT_DATE_REGEX.captures_iter(source).take(24) {
        let (Some(full), Some(day), Some(month), Some(year)) = (
            capture.get(0),
            capture.get(1),
            capture.get(2),
            capture.get(3),
        ) else {
            continue;
        };
        let (Ok(day), Some(month), Ok(year)) = (
            day.as_str().parse::<u32>(),
            month_number(month.as_str()),
            year.as_str().parse::<u32>(),
        ) else {
            continue;
        };
        if valid_date(year, month, day) {
            output.push(ParsedDate {
                located: LocatedValue::from_match(full),
                normalized: iso_date(year, month, day),
                alternate: None,
                confidence: adjust_for_quality(0.98, quality),
            });
        }
    }
    for capture in ENGLISH_TEXT_DATE_REGEX.captures_iter(source).take(24) {
        let (Some(full), Some(month), Some(day), Some(year)) = (
            capture.get(0),
            capture.get(1),
            capture.get(2),
            capture.get(3),
        ) else {
            continue;
        };
        let (Some(month), Ok(day), Ok(year)) = (
            month_number(month.as_str()),
            day.as_str().parse::<u32>(),
            year.as_str().parse::<u32>(),
        ) else {
            continue;
        };
        if valid_date(year, month, day) {
            output.push(ParsedDate {
                located: LocatedValue::from_match(full),
                normalized: iso_date(year, month, day),
                alternate: None,
                confidence: adjust_for_quality(0.98, quality),
            });
        }
    }
    output
}

fn parse_iso_date(value: &str) -> Option<(u32, u32, u32)> {
    let mut parts = value.split('-');
    let year = parts.next()?.parse().ok()?;
    let month = parts.next()?.parse().ok()?;
    let day = parts.next()?.parse().ok()?;
    if parts.next().is_some() || !valid_date(year, month, day) {
        None
    } else {
        Some((year, month, day))
    }
}

fn valid_date(year: u32, month: u32, day: u32) -> bool {
    if !(1900..=2200).contains(&year) || !(1..=12).contains(&month) {
        return false;
    }
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let maximum = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=maximum).contains(&day)
}

fn iso_date(year: u32, month: u32, day: u32) -> String {
    format!("{year:04}-{month:02}-{day:02}")
}

fn month_number(value: &str) -> Option<u32> {
    let lowered = value.to_lowercase();
    match lowered.trim_end_matches('.') {
        "janvier" | "january" | "jan" => Some(1),
        "février" | "fevrier" | "february" | "feb" => Some(2),
        "mars" | "march" | "mar" => Some(3),
        "avril" | "april" | "apr" => Some(4),
        "mai" | "may" => Some(5),
        "juin" | "june" | "jun" => Some(6),
        "juillet" | "july" | "jul" => Some(7),
        "août" | "aout" | "august" | "aug" => Some(8),
        "septembre" | "september" | "sep" | "sept" => Some(9),
        "octobre" | "october" | "oct" => Some(10),
        "novembre" | "november" | "nov" => Some(11),
        "décembre" | "decembre" | "december" | "dec" => Some(12),
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct ParsedMoney {
    located: LocatedValue,
    amount_minor: i64,
    scale: u8,
    currency: Option<String>,
    confidence: f32,
}

fn parse_money_values(source: &str, require_currency: bool) -> Vec<ParsedMoney> {
    let mut output = Vec::new();
    for capture in MONEY_REGEX.captures_iter(source).take(32) {
        let Some(full) = capture.get(0) else {
            continue;
        };
        let Some(amount) = capture.name("amount") else {
            continue;
        };
        let currency_surface = capture
            .name("prefix")
            .or_else(|| capture.name("suffix"))
            .map(|value| value.as_str());
        if require_currency && currency_surface.is_none() {
            continue;
        }
        let Some(amount_minor) = parse_decimal_minor(amount.as_str()) else {
            continue;
        };
        let (currency, confidence) = normalize_currency(currency_surface);
        output.push(ParsedMoney {
            located: LocatedValue::from_match(full),
            amount_minor,
            scale: 2,
            currency,
            confidence,
        });
    }
    output
}

pub(crate) fn parse_decimal_minor(value: &str) -> Option<i64> {
    let compact: String = value
        .chars()
        .filter(|character| !matches!(character, ' ' | '\u{00a0}' | '\u{202f}' | '\''))
        .collect();
    if compact.is_empty() || compact.starts_with('-') {
        return None;
    }
    let comma_positions = compact.match_indices(',').map(|(index, _)| index);
    let dot_positions = compact.match_indices('.').map(|(index, _)| index);
    let separators = comma_positions.chain(dot_positions).collect::<Vec<_>>();
    let decimal_index = separators.last().copied().filter(|index| {
        compact
            .len()
            .checked_sub(index.saturating_add(1))
            .is_some_and(|digits| digits == 2)
    });
    let mut digits = String::new();
    let mut fraction_digits = 0_u8;
    for (index, character) in compact.char_indices() {
        if character.is_ascii_digit() {
            digits.push(character);
            if decimal_index.is_some_and(|decimal| index > decimal) {
                fraction_digits = fraction_digits.saturating_add(1);
            }
        } else if !matches!(character, ',' | '.') {
            return None;
        }
    }
    if digits.is_empty() || fraction_digits > 2 {
        return None;
    }
    let major_minor = digits.parse::<i128>().ok()?;
    let scaled = match fraction_digits {
        0 => major_minor.checked_mul(100)?,
        1 => major_minor.checked_mul(10)?,
        2 => major_minor,
        _ => return None,
    };
    i64::try_from(scaled).ok()
}

fn normalize_currency(surface: Option<&str>) -> (Option<String>, f32) {
    match surface
        .map(str::trim)
        .map(str::to_ascii_uppercase)
        .as_deref()
    {
        Some("EUR" | "€") => (Some("EUR".to_owned()), 0.98),
        Some("USD") => (Some("USD".to_owned()), 0.98),
        Some("$") => (Some("USD".to_owned()), 0.86),
        Some("GBP" | "£") => (Some("GBP".to_owned()), 0.98),
        _ => (None, 0.68),
    }
}

pub fn normalize_user_correction(
    field_type: SemanticFieldType,
    value: &str,
    locale_hint: Option<&str>,
) -> Result<SemanticValue, KnowledgeError> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().count() > 256 {
        return Err(KnowledgeError::InvalidStructuredOutput(
            "user correction must contain between 1 and 256 characters".to_owned(),
        ));
    }
    let normalized_label = trimmed.to_ascii_lowercase().replace([' ', '-'], "_");
    match field_type {
        SemanticFieldType::DocumentType => {
            let document_type = match normalized_label.as_str() {
                "invoice" | "facture" => DocumentType::Invoice,
                "quote" | "quotation" | "devis" => DocumentType::Quote,
                "contract" | "contrat" => DocumentType::Contract,
                "purchase_order" | "bon_de_commande" => DocumentType::PurchaseOrder,
                "delivery_note" | "bon_de_livraison" => DocumentType::DeliveryNote,
                "bank_statement" | "relevé_bancaire" | "releve_bancaire" => {
                    DocumentType::BankStatement
                }
                "tax_document" | "document_fiscal" => DocumentType::TaxDocument,
                "payslip" | "bulletin_de_paie" => DocumentType::Payslip,
                "employment_contract" | "contrat_de_travail" => DocumentType::EmploymentContract,
                "insurance_document" | "document_assurance" => DocumentType::InsuranceDocument,
                "legal_document" | "document_juridique" => DocumentType::LegalDocument,
                "administrative_document" | "document_administratif" => {
                    DocumentType::AdministrativeDocument
                }
                "receipt" | "reçu" | "recu" => DocumentType::Receipt,
                "report" | "rapport" => DocumentType::Report,
                "letter" | "lettre" => DocumentType::Letter,
                "cv" | "resume" => DocumentType::Cv,
                "photo" => DocumentType::Photo,
                "video" | "vidéo" => DocumentType::Video,
                "spreadsheet" | "tableur" => DocumentType::Spreadsheet,
                "presentation" | "présentation" => DocumentType::Presentation,
                "archive" => DocumentType::Archive,
                "other" | "autre" => DocumentType::Other,
                "unknown" | "inconnu" => DocumentType::Unknown,
                _ => {
                    return Err(KnowledgeError::InvalidStructuredOutput(
                        "unsupported document type correction".to_owned(),
                    ));
                }
            };
            Ok(SemanticValue::DocumentType {
                value: document_type,
            })
        }
        SemanticFieldType::Context => {
            let context = match normalized_label.as_str() {
                "personal" | "personnel" => DocumentContext::Personal,
                "business" | "professional" | "professionnel" => DocumentContext::Business,
                "mixed" | "mixte" => DocumentContext::Mixed,
                "unknown" | "inconnu" => DocumentContext::Unknown,
                _ => {
                    return Err(KnowledgeError::InvalidStructuredOutput(
                        "unsupported context correction".to_owned(),
                    ));
                }
            };
            Ok(SemanticValue::Context { value: context })
        }
        SemanticFieldType::IssueDate
        | SemanticFieldType::DueDate
        | SemanticFieldType::ExpirationDate
        | SemanticFieldType::DocumentDate => {
            let correction_input = SemanticInput {
                file_version_id: String::new(),
                filename: "user-correction".to_owned(),
                extension: None,
                detected_content_type: None,
                extraction_status: "success".to_owned(),
                extracted_text: trimmed.to_owned(),
                extractor_type: None,
                extractor_version: None,
                page_count: None,
                sheet_count: None,
                slide_count: None,
                ocr_used: false,
                ocr_confidence: None,
                extraction_truncated: false,
                language_hint: locale_hint.map(str::to_owned),
                locale_hint: locale_hint.map(str::to_owned),
            };
            let quality = InputQuality {
                score: ConfidenceScore::new(1.0)?,
                status: InputQualityStatus::Good,
                reasons: Vec::new(),
            };
            let parsed = parse_dates(trimmed, &correction_input, &quality);
            let date = parsed
                .first()
                .filter(|date| date.alternate.is_none())
                .ok_or_else(|| {
                    KnowledgeError::InvalidStructuredOutput(
                        "date correction is invalid or locale-ambiguous".to_owned(),
                    )
                })?;
            Ok(SemanticValue::Date {
                iso_date: date.normalized.clone(),
            })
        }
        SemanticFieldType::Subtotal
        | SemanticFieldType::Tax
        | SemanticFieldType::Total
        | SemanticFieldType::Amount => {
            let parsed = parse_money_values(trimmed, false)
                .into_iter()
                .next()
                .ok_or_else(|| {
                    KnowledgeError::InvalidStructuredOutput(
                        "money correction is invalid".to_owned(),
                    )
                })?;
            Ok(SemanticValue::Money {
                amount_minor: parsed.amount_minor,
                scale: parsed.scale,
                currency: parsed.currency,
            })
        }
        SemanticFieldType::ContractParties => {
            let values = trimmed
                .split([';', '|'])
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(normalize_entity_value)
                .take(16)
                .collect::<Vec<_>>();
            if values.is_empty() {
                return Err(KnowledgeError::InvalidStructuredOutput(
                    "contract parties correction is empty".to_owned(),
                ));
            }
            Ok(SemanticValue::TextList { values })
        }
        _ => Ok(SemanticValue::Text {
            value: normalize_entity_value(trimmed),
        }),
    }
}

fn analysis_status(fields: &[SemanticField], quality: &InputQuality) -> SemanticStatus {
    let meaningful = fields.iter().any(|field| {
        field.value.is_some()
            && !matches!(
                field.value,
                Some(
                    SemanticValue::DocumentType {
                        value: DocumentType::Unknown
                    } | SemanticValue::Context {
                        value: DocumentContext::Unknown
                    }
                )
            )
    });
    if !meaningful {
        SemanticStatus::Unknown
    } else if quality.status != InputQualityStatus::Good
        || fields.iter().any(|field| {
            matches!(
                field.status,
                FieldStatus::Ambiguous | FieldStatus::Conflicting
            )
        })
    {
        SemanticStatus::Partial
    } else {
        SemanticStatus::Success
    }
}

fn review_reasons(
    fields: &[SemanticField],
    document_type: Option<DocumentType>,
) -> Vec<SemanticReviewReason> {
    let mut output = Vec::new();
    let document = fields
        .iter()
        .find(|field| field.field_type == SemanticFieldType::DocumentType);
    let context = fields
        .iter()
        .find(|field| field.field_type == SemanticFieldType::Context);
    if fields
        .iter()
        .any(|field| field.status == FieldStatus::Conflicting)
    {
        output.push(SemanticReviewReason::ConflictingFields);
    }
    if document.is_some_and(|field| {
        field.value.is_none() && field.confidence.value() >= 0.3 && !field.candidates.is_empty()
    }) {
        output.push(SemanticReviewReason::LowConfidenceDocumentType);
    }
    if context.is_some_and(|field| {
        document_type.is_some()
            && matches!(field.status, FieldStatus::Unknown | FieldStatus::Ambiguous)
            && (field.confidence.value() >= 0.35 || field.status == FieldStatus::Ambiguous)
    }) {
        output.push(SemanticReviewReason::LowConfidenceContext);
    }
    if fields
        .iter()
        .any(|field| field.status == FieldStatus::Ambiguous)
        && !output.contains(&SemanticReviewReason::LowConfidenceContext)
    {
        output.push(SemanticReviewReason::SemanticAmbiguity);
    }
    let high_document_confidence = document.is_some_and(|field| field.confidence.value() >= 0.85);
    let missing_critical = match document_type {
        Some(DocumentType::Invoice) => {
            !has_field_value(fields, SemanticFieldType::InvoiceNumber)
                && !has_field_value(fields, SemanticFieldType::Total)
        }
        Some(DocumentType::Quote) => {
            !has_field_value(fields, SemanticFieldType::QuoteNumber)
                && !has_field_value(fields, SemanticFieldType::Amount)
        }
        Some(DocumentType::Contract | DocumentType::EmploymentContract) => {
            !has_field_value(fields, SemanticFieldType::ContractParties)
                && !has_field_value(fields, SemanticFieldType::IssueDate)
        }
        _ => false,
    };
    if high_document_confidence && missing_critical {
        output.push(SemanticReviewReason::MissingCriticalFields);
    }
    output
}

fn has_field_value(fields: &[SemanticField], field_type: SemanticFieldType) -> bool {
    fields
        .iter()
        .any(|field| field.field_type == field_type && field.value.is_some())
}

fn adjust_for_quality(base: f32, quality: &InputQuality) -> f32 {
    (base * (0.5 + 0.5 * quality.score.value())).clamp(0.0, 1.0)
}

fn text_evidence(
    input: &SemanticInput,
    found: &LineMatch,
    explanation: &str,
    evidence_type: EvidenceType,
) -> SemanticEvidence {
    SemanticEvidence {
        evidence_type: if input.ocr_used {
            EvidenceType::OcrText
        } else {
            evidence_type
        },
        exact_text: truncate_chars(&found.text, 500),
        start_offset: Some(found.start),
        end_offset: Some(found.end),
        page_number: (input.page_count == Some(1)).then_some(1),
        sheet_name: None,
        slide_number: None,
        source_label: input.filename.clone(),
        explanation: explanation.to_owned(),
        extraction_method: input
            .extractor_type
            .clone()
            .unwrap_or_else(|| "existing_metadata".to_owned()),
        analyzer_version: ANALYZER_VERSION.to_owned(),
    }
}

fn located_evidence(
    input: &SemanticInput,
    located: &LocatedValue,
    explanation: &str,
    evidence_type: EvidenceType,
) -> SemanticEvidence {
    text_evidence(
        input,
        &LineMatch {
            start: located.start,
            end: located.end,
            text: located.value.clone(),
        },
        explanation,
        evidence_type,
    )
}

fn filename_evidence(input: &SemanticInput, explanation: &str) -> SemanticEvidence {
    SemanticEvidence {
        evidence_type: EvidenceType::Filename,
        exact_text: truncate_chars(&input.filename, 500),
        start_offset: None,
        end_offset: None,
        page_number: None,
        sheet_name: None,
        slide_number: None,
        source_label: "filename".to_owned(),
        explanation: explanation.to_owned(),
        extraction_method: "catalog_metadata".to_owned(),
        analyzer_version: ANALYZER_VERSION.to_owned(),
    }
}

fn metadata_evidence(observed: &str, explanation: &str, input: &SemanticInput) -> SemanticEvidence {
    SemanticEvidence {
        evidence_type: EvidenceType::Metadata,
        exact_text: truncate_chars(observed, 500),
        start_offset: None,
        end_offset: None,
        page_number: None,
        sheet_name: None,
        slide_number: None,
        source_label: input.filename.clone(),
        explanation: explanation.to_owned(),
        extraction_method: "safe_type_detection".to_owned(),
        analyzer_version: ANALYZER_VERSION.to_owned(),
    }
}

fn evidence_as_located(evidence: &SemanticEvidence) -> Option<LocatedValue> {
    Some(LocatedValue {
        value: evidence.exact_text.clone(),
        start: evidence.start_offset?,
        end: evidence.end_offset?,
    })
}

fn truncate_chars(value: &str, maximum: usize) -> String {
    value.chars().take(maximum).collect()
}

fn compile_regex(pattern: &str) -> Regex {
    match Regex::new(pattern) {
        Ok(regex) => regex,
        Err(error) => panic!("invalid built-in semantic regex: {error}"),
    }
}

static EMAIL_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(r"(?i)\b[A-Z0-9._%+\-]{1,64}@[A-Z0-9.\-]{1,190}\.[A-Z]{2,24}\b")
});
static PHONE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(
        r"(?im)^(?:t[eé]l(?:[ée]phone)?|phone|mobile)\s*[:.\-]\s*(\+?[0-9][0-9 .()\-]{6,24})\s*$",
    )
});
static COMPANY_ID_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(
        r"(?im)^(?:siret|siren|company\s+id|registration\s+number|vat\s+number|n[°o]\s*tva)\s*[:#.\-]?\s*([A-Z0-9][A-Z0-9 .\-]{5,40})\s*$",
    )
});
static DOCUMENT_NUMBER_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(
        r"(?im)\b(facture|invoice|devis|quote|quotation|estimate)\s*(?:number|no\.?|n[°o.]?|#)?\s*[:#.\-]?\s*([A-Z0-9][A-Z0-9._/\-]{2,47})\b",
    )
});
static NUMERIC_DATE_REGEX: LazyLock<Regex> =
    LazyLock::new(|| compile_regex(r"\b([0-9]{1,4})[/-]([0-9]{1,2})[/-]([0-9]{1,4})\b"));
static TEXT_DATE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(
        r"(?i)\b([0-9]{1,2})\s+(janvier|january|jan\.?|février|fevrier|february|feb\.?|mars|march|mar\.?|avril|april|apr\.?|mai|may|juin|june|jun\.?|juillet|july|jul\.?|août|aout|august|aug\.?|septembre|september|sept?\.?|octobre|october|oct\.?|novembre|november|nov\.?|décembre|decembre|december|dec\.?)\s+([0-9]{4})\b",
    )
});
static ENGLISH_TEXT_DATE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(
        r"(?i)\b(january|jan\.?|february|feb\.?|march|mar\.?|april|apr\.?|may|june|jun\.?|july|jul\.?|august|aug\.?|september|sept?\.?|october|oct\.?|november|nov\.?|december|dec\.?)\s+([0-9]{1,2})(?:st|nd|rd|th)?[,]?\s+([0-9]{4})\b",
    )
});
static MONEY_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    compile_regex(
        r"(?ix)
        (?P<prefix>EUR|USD|GBP|€|\$|£)?
        \s*
        (?P<amount>
            [0-9]{1,3}(?:[\x20\u{00A0}\u{202F}.,'][0-9]{3})+(?:[.,][0-9]{2})?
            |
            [0-9]{1,12}(?:[.,][0-9]{2})?
        )
        \s*
        (?P<suffix>EUR|USD|GBP|€|\$|£)?
        ",
    )
});

#[derive(Debug, thiserror::Error)]
pub enum KnowledgeError {
    #[error("semantic pattern is invalid: {0}")]
    InvalidPattern(#[from] regex::Error),
    #[error("confidence invariant failed: {0}")]
    Confidence(#[from] domain::ConfidenceError),
    #[error("confidence score must be finite and between zero and one")]
    InvalidConfidence,
    #[error("semantic provider output is invalid: {0}")]
    InvalidStructuredOutput(String),
    #[error("semantic provider output exceeds a configured bound")]
    OutputLimitExceeded,
    #[error("semantic provider must be local for this pipeline")]
    RemoteProviderRejected,
    #[error("semantic analysis was cancelled")]
    Cancelled,
    #[error("semantic analysis exceeded its per-file time limit")]
    TimedOut,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(filename: &str, text: &str) -> SemanticInput {
        SemanticInput {
            file_version_id: "file-version-test".to_owned(),
            filename: filename.to_owned(),
            extension: filename
                .rsplit_once('.')
                .map(|(_, extension)| extension.to_owned()),
            detected_content_type: Some("application/pdf".to_owned()),
            extraction_status: "success".to_owned(),
            extracted_text: text.to_owned(),
            extractor_type: Some("pdf_text".to_owned()),
            extractor_version: Some("test".to_owned()),
            page_count: Some(1),
            sheet_count: None,
            slide_count: None,
            ocr_used: false,
            ocr_confidence: None,
            extraction_truncated: false,
            language_hint: Some("fr".to_owned()),
            locale_hint: Some("fr-FR".to_owned()),
        }
    }

    fn analyze(filename: &str, text: &str) -> SemanticAnalysis {
        DeterministicSemanticProvider::default()
            .analyze(&input(filename, text), &|| false)
            .unwrap_or_else(|error| panic!("semantic analysis should succeed: {error}"))
    }

    fn field(analysis: &SemanticAnalysis, field_type: SemanticFieldType) -> &SemanticField {
        analysis
            .primary_field(field_type)
            .unwrap_or_else(|| panic!("field {field_type:?} should exist"))
    }

    #[test]
    fn french_invoice_produces_normalized_explainable_fields() {
        let analysis = analyze(
            "scan_38492.pdf",
            "POINT P\nFacture n° FP-39482\n17/06/2026\nMontant TTC: 1 437,82 €\nClient: Dupont SARL",
        );
        assert_eq!(
            field(&analysis, SemanticFieldType::DocumentType).value,
            Some(SemanticValue::DocumentType {
                value: DocumentType::Invoice
            })
        );
        assert_eq!(
            field(&analysis, SemanticFieldType::InvoiceNumber).value,
            Some(SemanticValue::Text {
                value: "FP-39482".to_owned()
            })
        );
        assert_eq!(
            field(&analysis, SemanticFieldType::Total).value,
            Some(SemanticValue::Money {
                amount_minor: 143_782,
                scale: 2,
                currency: Some("EUR".to_owned())
            })
        );
        assert!(
            field(&analysis, SemanticFieldType::SupplierCandidate)
                .evidence
                .iter()
                .any(|evidence| evidence.exact_text == "POINT P" && evidence.page_number == Some(1))
        );
        assert!(analysis.analyzer.local_only);
    }

    #[test]
    fn random_invoice_mention_and_filename_are_not_confident_classification() {
        let mention = analyze(
            "notes.txt",
            "Remember to ask whether the word invoice is translated.",
        );
        let mention_type = field(&mention, SemanticFieldType::DocumentType);
        assert!(mention_type.value.is_none());
        assert!(mention_type.confidence.value() < ConfidencePolicy::default().medium);

        let filename = analyze("invoice.jpg", "A family picnic near the lake.");
        let filename_type = field(&filename, SemanticFieldType::DocumentType);
        assert_ne!(
            filename_type.value,
            Some(SemanticValue::DocumentType {
                value: DocumentType::Invoice
            })
        );
    }

    #[test]
    fn dates_handle_iso_english_french_and_locale_ambiguity() {
        let quality = assess_input_quality(&input("x", "x"), "x", false)
            .unwrap_or_else(|error| panic!("quality should be valid: {error}"));
        let base = input("x", "x");
        let french = parse_dates("17/06/2026", &base, &quality);
        assert_eq!(french[0].normalized, "2026-06-17");
        let iso = parse_dates("2026-06-17", &base, &quality);
        assert_eq!(iso[0].normalized, "2026-06-17");
        let english = parse_dates("June 17, 2026", &base, &quality);
        assert_eq!(english[0].normalized, "2026-06-17");
        let ambiguous = parse_dates("03/04/2026", &base, &quality);
        assert_eq!(ambiguous[0].normalized, "2026-04-03");
        assert_eq!(ambiguous[0].alternate.as_deref(), Some("2026-03-04"));
    }

    #[test]
    fn money_parser_is_exact_and_does_not_guess_missing_currency() {
        assert_eq!(parse_decimal_minor("1 437,82"), Some(143_782));
        assert_eq!(parse_decimal_minor("1,437.82"), Some(143_782));
        assert_eq!(parse_decimal_minor("1437.82"), Some(143_782));
        assert_eq!(parse_decimal_minor("-10.00"), None);

        let analysis = analyze("quote.pdf", "DEVIS N° Q-9\nMontant: 1437.82");
        let amount = field(&analysis, SemanticFieldType::Amount);
        assert!(matches!(
            amount.candidates.first().map(|candidate| &candidate.value),
            Some(SemanticValue::Money { currency: None, .. })
        ));
        assert_ne!(amount.status, FieldStatus::Confirmed);
    }

    #[test]
    fn conflicting_labeled_totals_are_retained_as_candidates() {
        let analysis = analyze(
            "invoice.pdf",
            "FACTURE N° INV-1\nTotal: 100,00 EUR\nTotal: 120,00 EUR\nClient: Example SAS",
        );
        let total = field(&analysis, SemanticFieldType::Total);
        assert_eq!(total.status, FieldStatus::Conflicting);
        assert!(total.value.is_none());
        assert_eq!(total.candidates.len(), 2);
        assert!(
            analysis
                .review_reasons
                .contains(&SemanticReviewReason::ConflictingFields)
        );
    }

    #[test]
    fn poor_ocr_reduces_confidence_and_analysis_is_partial() {
        let mut poor = input(
            "scan.pdf",
            "FACTURE N° INV-77\nT0ta1 TTC: 99,00 EUR\nClient: Example",
        );
        poor.extraction_status = "partial".to_owned();
        poor.ocr_used = true;
        poor.ocr_confidence = Some(0.35);
        let analysis = DeterministicSemanticProvider::default()
            .analyze(&poor, &|| false)
            .unwrap_or_else(|error| panic!("analysis should remain safe: {error}"));
        assert_eq!(analysis.status, SemanticStatus::Partial);
        assert!(
            field(&analysis, SemanticFieldType::DocumentType)
                .confidence
                .value()
                < 0.85
        );
    }

    #[test]
    fn empty_text_returns_unknown_without_review_spam() {
        let analysis = analyze("38492.bin", "");
        assert_eq!(analysis.status, SemanticStatus::Unknown);
        assert!(analysis.review_reasons.is_empty());
    }

    #[test]
    fn quote_and_contract_fields_are_structured_without_cross_file_identity() {
        let quote = analyze(
            "quote.pdf",
            "QUOTE NUMBER Q-204\nQuote date: June 17, 2026\nValid until: July 17, 2026\nAmount: USD 1,250.00\nCustomer: Contoso Ltd",
        );
        assert_eq!(
            field(&quote, SemanticFieldType::DocumentType).value,
            Some(SemanticValue::DocumentType {
                value: DocumentType::Quote
            })
        );
        assert_eq!(
            field(&quote, SemanticFieldType::QuoteNumber).value,
            Some(SemanticValue::Text {
                value: "Q-204".to_owned()
            })
        );
        assert_eq!(
            field(&quote, SemanticFieldType::ExpirationDate).value,
            Some(SemanticValue::Date {
                iso_date: "2026-07-17".to_owned()
            })
        );
        assert_eq!(
            field(&quote, SemanticFieldType::Amount).value,
            Some(SemanticValue::Money {
                amount_minor: 125_000,
                scale: 2,
                currency: Some("USD".to_owned())
            })
        );

        let contract = analyze(
            "agreement.pdf",
            "CONTRAT DE PRESTATION\nParties: Alpha SAS | Beta SARL\nDate de signature: 2026-06-01\nSignature",
        );
        assert_eq!(
            field(&contract, SemanticFieldType::DocumentType).value,
            Some(SemanticValue::DocumentType {
                value: DocumentType::Contract
            })
        );
        assert!(
            field(&contract, SemanticFieldType::ContractParties)
                .value
                .is_some()
        );
        assert!(
            field(&contract, SemanticFieldType::ContractTitle)
                .value
                .is_some()
        );
    }

    #[test]
    fn personal_business_mixed_and_unknown_contexts_are_conservative() {
        let business = analyze(
            "invoice.pdf",
            "FACTURE N° INV-8\nSIRET: 123 456 789 00012\nMontant TTC: 10,00 EUR",
        );
        assert_eq!(
            field(&business, SemanticFieldType::Context).value,
            Some(SemanticValue::Context {
                value: DocumentContext::Business
            })
        );

        let personal = analyze(
            "tax.pdf",
            "AVIS D’IMPOSITION\nNuméro fiscal: 123456\nRevenu fiscal de référence",
        );
        assert_eq!(
            field(&personal, SemanticFieldType::Context).value,
            Some(SemanticValue::Context {
                value: DocumentContext::Personal
            })
        );

        let mixed = analyze(
            "payslip.pdf",
            "BULLETIN DE PAIE\nEmployeur: Example SAS\nSalarié: Jeanne Dupont\nNet à payer: 2 100,00 EUR",
        );
        assert_eq!(
            field(&mixed, SemanticFieldType::Context).value,
            Some(SemanticValue::Context {
                value: DocumentContext::Mixed
            })
        );

        let unknown = analyze("notes.txt", "Meet at 14:00 near the station.");
        assert!(field(&unknown, SemanticFieldType::Context).value.is_none());
    }

    #[test]
    fn labeled_entities_are_typed_and_unrelated_mentions_are_not_promoted() {
        let analysis = analyze(
            "invoice.pdf",
            "FACTURE N° INV-42\nSupplier: Point P\nCustomer: Dupont SARL\nCompany: ACME SAS\nContact: Jeanne Martin\nAddress: 12 rue des Lilas, Paris\nEmail: test@example.com\nPhone: +33 1 23 45 67 89\nSIRET: 123 456 789 00012\nTotal: 42,00 EUR",
        );
        for entity_type in [
            EntityType::SupplierCandidate,
            EntityType::CustomerCandidate,
            EntityType::Organization,
            EntityType::Person,
            EntityType::Address,
            EntityType::Email,
            EntityType::Phone,
            EntityType::SiretOrCompanyId,
            EntityType::Amount,
            EntityType::Currency,
            EntityType::InvoiceNumber,
        ] {
            assert!(
                analysis
                    .entities
                    .iter()
                    .any(|entity| entity.entity_type == entity_type),
                "missing entity type {entity_type:?}"
            );
        }

        let unrelated = analyze(
            "correspondence.txt",
            "Point P was mentioned while discussing trends in invoice software.",
        );
        assert!(!unrelated.entities.iter().any(|entity| {
            entity.entity_type == EntityType::SupplierCandidate
                && entity.normalized_value == "Point P"
        }));
    }

    #[test]
    fn filenames_and_missing_identifiers_remain_weak_or_unknown() {
        let filename_only = analyze("invoice.bin", "");
        let document_type = field(&filename_only, SemanticFieldType::DocumentType);
        assert!(document_type.value.is_none());
        assert!(document_type.confidence.value() < 0.65);

        let no_identifier = analyze(
            "document.pdf",
            "FACTURE\nMontant TTC: 10,00 EUR\nClient: Example SAS",
        );
        assert!(
            no_identifier
                .primary_field(SemanticFieldType::InvoiceNumber)
                .is_none()
        );
    }

    #[test]
    fn cancellation_and_input_limits_are_enforced() {
        let provider = DeterministicSemanticProvider::default();
        assert!(matches!(
            provider.analyze(&input("x.txt", "invoice"), &|| true),
            Err(KnowledgeError::Cancelled)
        ));
        let oversized = "Facture ".repeat(provider.limits().max_input_chars);
        let analysis = provider
            .analyze(&input("x.txt", &oversized), &|| false)
            .unwrap_or_else(|error| panic!("bounded analysis should succeed: {error}"));
        assert!(
            analysis
                .input_quality
                .reasons
                .contains(&InputQualityReason::SemanticInputLimit)
        );
        assert!(analysis.analyzed_character_count <= provider.limits().max_input_chars);
    }
}
