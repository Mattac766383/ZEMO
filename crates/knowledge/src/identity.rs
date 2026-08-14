//! Conservative, deterministic cross-file identity resolution.
//!
//! Scores in this module are policy scores, not calibrated probabilities.
//! Resolution is local-only and consumes bounded semantic values as untrusted
//! data. Name similarity can create review work, but can never auto-link an
//! identity without corroborating evidence.

use crate::ConfidenceScore;
use serde::{Deserialize, Serialize};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashSet},
};
use unicode_normalization::UnicodeNormalization;

pub const RESOLVER_ID: &str = "deterministic-cross-file-resolution";
pub const RESOLVER_VERSION: &str = "6.0.0";
pub const MAX_IDENTITY_VALUE_CHARS: usize = 512;
pub const MAX_SIGNALS_PER_OCCURRENCE: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityType {
    Organization,
    Person,
    Project,
}

impl IdentityType {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Organization => "organization",
            Self::Person => "person",
            Self::Project => "project",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityRole {
    Customer,
    Supplier,
}

impl IdentityRole {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Customer => "customer",
            Self::Supplier => "supplier",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    Name,
    CompanyIdentifier,
    VatIdentifier,
    Email,
    Domain,
    Phone,
    Address,
    AccountIdentifier,
    ProjectReference,
    CustomerIdentity,
    Date,
    PathContext,
}

impl SignalKind {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::CompanyIdentifier => "company_identifier",
            Self::VatIdentifier => "vat_identifier",
            Self::Email => "email",
            Self::Domain => "domain",
            Self::Phone => "phone",
            Self::Address => "address",
            Self::AccountIdentifier => "account_identifier",
            Self::ProjectReference => "project_reference",
            Self::CustomerIdentity => "customer_identity",
            Self::Date => "date",
            Self::PathContext => "path_context",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IdentitySignal {
    pub kind: SignalKind,
    pub original_value: String,
    pub normalized_value: String,
}

impl IdentitySignal {
    pub fn new(kind: SignalKind, value: &str) -> Result<Option<Self>, IdentityResolutionError> {
        validate_bounded(value)?;
        let normalized_value = normalize_signal(kind, value);
        Ok(normalized_value.map(|normalized_value| Self {
            kind,
            original_value: value.to_owned(),
            normalized_value,
        }))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentityOccurrence {
    pub occurrence_key: String,
    pub file_id: String,
    pub semantic_entity_id: Option<String>,
    pub semantic_field_id: Option<String>,
    pub identity_type: IdentityType,
    pub role: Option<IdentityRole>,
    pub original_value: String,
    pub normalized_name: NormalizedName,
    pub confidence: ConfidenceScore,
    pub signals: Vec<IdentitySignal>,
    pub analyzer_version: String,
}

impl IdentityOccurrence {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        occurrence_key: &str,
        file_id: &str,
        semantic_entity_id: Option<String>,
        semantic_field_id: Option<String>,
        identity_type: IdentityType,
        role: Option<IdentityRole>,
        original_value: &str,
        confidence: f32,
        analyzer_version: &str,
        extra_signals: impl IntoIterator<Item = (SignalKind, String)>,
    ) -> Result<Self, IdentityResolutionError> {
        validate_identifier(occurrence_key)?;
        validate_identifier(file_id)?;
        validate_bounded(original_value)?;
        validate_version(analyzer_version)?;
        let normalized_name =
            normalize_name(original_value).ok_or(IdentityResolutionError::EmptyValue)?;
        let confidence = ConfidenceScore::new(confidence)
            .map_err(|_| IdentityResolutionError::InvalidConfidence)?;
        let mut signals = vec![IdentitySignal {
            kind: SignalKind::Name,
            original_value: original_value.to_owned(),
            normalized_value: normalized_name.exact.clone(),
        }];
        for (kind, value) in extra_signals {
            if signals.len() >= MAX_SIGNALS_PER_OCCURRENCE {
                return Err(IdentityResolutionError::TooManySignals);
            }
            if let Some(signal) = IdentitySignal::new(kind, &value)?
                && !signals.iter().any(|existing| {
                    existing.kind == signal.kind
                        && existing.normalized_value == signal.normalized_value
                })
            {
                signals.push(signal);
            }
        }
        signals.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then_with(|| left.normalized_value.cmp(&right.normalized_value))
        });
        Ok(Self {
            occurrence_key: occurrence_key.to_owned(),
            file_id: file_id.to_owned(),
            semantic_entity_id,
            semantic_field_id,
            identity_type,
            role,
            original_value: original_value.to_owned(),
            normalized_name,
            confidence,
            signals,
            analyzer_version: analyzer_version.to_owned(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedName {
    /// Punctuation-normalized name retaining a canonical legal suffix.
    pub exact: String,
    /// Name used only as supporting evidence; legal suffixes are removed.
    pub core: String,
    /// Canonical legal suffix, retained as a separate signal.
    pub legal_suffix: Option<String>,
    pub tokens: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStrength {
    VeryStrong,
    Strong,
    Medium,
    Weak,
    Conflicting,
}

impl EvidenceStrength {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::VeryStrong => "very_strong",
            Self::Strong => "strong",
            Self::Medium => "medium",
            Self::Weak => "weak",
            Self::Conflicting => "conflicting",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidencePolarity {
    Supports,
    Conflicts,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentityEvidence {
    pub evidence_type: String,
    pub strength: EvidenceStrength,
    pub polarity: EvidencePolarity,
    pub left_value: String,
    pub right_value: String,
    pub weight: f32,
    pub explanation: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolutionDecision {
    AutoLink,
    Review,
    KeepSeparate,
    Unknown,
}

impl ResolutionDecision {
    #[must_use]
    pub const fn database_name(self) -> &'static str {
        match self {
            Self::AutoLink => "auto_link",
            Self::Review => "review",
            Self::KeepSeparate => "keep_separate",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MatchAssessment {
    pub score: f32,
    pub decision: ResolutionDecision,
    pub evidence: Vec<IdentityEvidence>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct IdentityResolutionPolicy {
    pub auto_link_threshold: f32,
    pub review_threshold: f32,
    pub fuzzy_name_threshold: f32,
    pub max_block_neighbors: usize,
    pub max_candidates_per_occurrence: usize,
    pub max_block_members: usize,
}

impl Default for IdentityResolutionPolicy {
    fn default() -> Self {
        Self {
            auto_link_threshold: 0.97,
            review_threshold: 0.50,
            fuzzy_name_threshold: 0.82,
            max_block_neighbors: 16,
            max_candidates_per_occurrence: 32,
            max_block_members: 512,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IdentityCandidate {
    pub left_index: usize,
    pub right_index: usize,
    pub blocking_keys: Vec<String>,
    pub assessment: MatchAssessment,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateGenerationStats {
    pub occurrences: usize,
    pub blocks: usize,
    pub blocking_memberships: usize,
    pub comparisons: usize,
    pub candidates: usize,
    pub truncated_blocks: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateGeneration {
    pub candidates: Vec<IdentityCandidate>,
    pub stats: CandidateGenerationStats,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockingKey {
    pub key: String,
    pub strong: bool,
}

#[must_use]
pub fn normalize_name(value: &str) -> Option<NormalizedName> {
    if value.chars().count() > MAX_IDENTITY_VALUE_CHARS {
        return None;
    }
    let folded = fold_and_separate(value);
    let mut tokens = folded
        .split_whitespace()
        .filter(|token| !token.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        return None;
    }
    let legal_suffix = take_legal_suffix(&mut tokens);
    let core = tokens.join(" ");
    if core.is_empty() {
        return None;
    }
    let mut exact_tokens = tokens.clone();
    if let Some(suffix) = &legal_suffix {
        exact_tokens.push(suffix.clone());
    }
    Some(NormalizedName {
        exact: exact_tokens.join(" "),
        core,
        legal_suffix,
        tokens,
    })
}

#[must_use]
pub fn normalize_email(value: &str) -> Option<String> {
    let folded = value.nfkc().collect::<String>().trim().to_lowercase();
    if folded.len() > 254 || folded.contains(char::is_whitespace) {
        return None;
    }
    let mut parts = folded.split('@');
    let local = parts.next()?;
    let domain = parts.next()?;
    if parts.next().is_some()
        || local.is_empty()
        || local.len() > 64
        || local.starts_with('.')
        || local.ends_with('.')
        || local.contains("..")
        || !local.chars().all(|character| {
            character.is_ascii_alphanumeric() || ".!#$%&'*+/=?^_`{|}~-".contains(character)
        })
    {
        return None;
    }
    let domain = normalize_domain(domain)?;
    Some(format!("{local}@{domain}"))
}

#[must_use]
pub fn normalize_domain(value: &str) -> Option<String> {
    let normalized = value
        .nfkc()
        .collect::<String>()
        .trim()
        .trim_matches('.')
        .to_ascii_lowercase();
    if normalized.len() > 253
        || normalized.is_empty()
        || !normalized.contains('.')
        || !normalized.is_ascii()
    {
        return None;
    }
    let labels = normalized.split('.').collect::<Vec<_>>();
    if labels.iter().any(|label| {
        label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
    }) {
        return None;
    }
    Some(normalized)
}

#[must_use]
pub fn normalize_phone(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed
            .chars()
            .any(|character| !character.is_ascii_digit() && !" +-.()".contains(character))
        || trimmed.matches('+').count() > 1
        || (trimmed.contains('+') && !trimmed.starts_with('+'))
    {
        return None;
    }
    let international = trimmed.starts_with('+');
    let digits = trimmed
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>();
    if !(8..=15).contains(&digits.len()) {
        return None;
    }
    Some(if international {
        format!("+{digits}")
    } else {
        digits
    })
}

#[must_use]
pub fn normalize_company_identifier(value: &str) -> Option<String> {
    let normalized = value
        .nfkc()
        .filter(|character| character.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_uppercase();
    if !(8..=32).contains(&normalized.len())
        || normalized
            .chars()
            .next()
            .is_some_and(|first| normalized.chars().all(|character| character == first))
    {
        return None;
    }
    if normalized
        .chars()
        .all(|character| character.is_ascii_digit())
        && matches!(normalized.len(), 9 | 14)
        && !luhn_valid(&normalized)
    {
        return None;
    }
    Some(normalized)
}

#[must_use]
pub fn blocking_keys(occurrence: &IdentityOccurrence) -> Vec<BlockingKey> {
    let mut output = BTreeMap::<String, bool>::new();
    output.insert(
        format!(
            "type:{}:name:{}",
            occurrence.identity_type.database_name(),
            occurrence.normalized_name.exact
        ),
        false,
    );
    if occurrence.normalized_name.core != occurrence.normalized_name.exact
        && useful_core_name(&occurrence.normalized_name)
    {
        output.insert(
            format!(
                "type:{}:core:{}",
                occurrence.identity_type.database_name(),
                occurrence.normalized_name.core
            ),
            false,
        );
    }
    for signal in &occurrence.signals {
        let strong = matches!(
            signal.kind,
            SignalKind::CompanyIdentifier
                | SignalKind::VatIdentifier
                | SignalKind::Email
                | SignalKind::Phone
                | SignalKind::AccountIdentifier
                | SignalKind::ProjectReference
        );
        if signal.kind == SignalKind::Domain && generic_email_domain(&signal.normalized_value) {
            continue;
        }
        if signal.kind != SignalKind::Name {
            output.insert(
                format!(
                    "type:{}:{}:{}",
                    occurrence.identity_type.database_name(),
                    signal.kind.database_name(),
                    signal.normalized_value
                ),
                strong,
            );
        }
    }
    output
        .into_iter()
        .map(|(key, strong)| BlockingKey { key, strong })
        .collect()
}

#[must_use]
pub fn assess_match(
    left: &IdentityOccurrence,
    right: &IdentityOccurrence,
    policy: IdentityResolutionPolicy,
) -> MatchAssessment {
    if left.identity_type != right.identity_type || left.occurrence_key == right.occurrence_key {
        return MatchAssessment {
            score: 0.0,
            decision: ResolutionDecision::Unknown,
            evidence: Vec::new(),
        };
    }

    let mut evidence = Vec::new();
    compare_exclusive_identifiers(left, right, &mut evidence);
    compare_names(left, right, policy, &mut evidence);
    compare_shared_signals(left, right, &mut evidence);

    evidence.sort_by(evidence_order);
    let has_conflict = evidence
        .iter()
        .any(|item| item.polarity == EvidencePolarity::Conflicts);
    let score = evidence
        .iter()
        .filter(|item| item.polarity == EvidencePolarity::Supports)
        .map(|item| item.weight)
        .sum::<f32>()
        .clamp(0.0, 1.0);
    let decision = if has_blocking_conflict(&evidence) {
        ResolutionDecision::KeepSeparate
    } else if score >= policy.auto_link_threshold && auto_link_requirements(left, right, &evidence)
    {
        ResolutionDecision::AutoLink
    } else if score >= policy.review_threshold || has_conflict {
        ResolutionDecision::Review
    } else {
        ResolutionDecision::Unknown
    };
    MatchAssessment {
        score,
        decision,
        evidence,
    }
}

#[must_use]
pub fn generate_candidates(
    occurrences: &[IdentityOccurrence],
    policy: IdentityResolutionPolicy,
) -> CandidateGeneration {
    let mut blocks = BTreeMap::<String, (bool, Vec<usize>)>::new();
    let mut stats = CandidateGenerationStats {
        occurrences: occurrences.len(),
        ..CandidateGenerationStats::default()
    };
    for (index, occurrence) in occurrences.iter().enumerate() {
        for key in blocking_keys(occurrence) {
            let entry = blocks.entry(key.key).or_insert((key.strong, Vec::new()));
            entry.0 |= key.strong;
            entry.1.push(index);
            stats.blocking_memberships = stats.blocking_memberships.saturating_add(1);
        }
    }
    stats.blocks = blocks.len();

    let mut pair_keys = BTreeMap::<(usize, usize), BTreeSet<String>>::new();
    for (key, (strong, mut members)) in blocks {
        members.sort_unstable();
        members.dedup();
        if members.len() < 2 {
            continue;
        }
        if members.len() > policy.max_block_members && !strong {
            stats.truncated_blocks = stats.truncated_blocks.saturating_add(1);
            continue;
        }
        let neighbor_limit = if strong {
            policy.max_block_neighbors.saturating_mul(2)
        } else {
            policy.max_block_neighbors
        }
        .max(1);
        for (position, left) in members.iter().enumerate() {
            for right in members.iter().skip(position + 1).take(neighbor_limit) {
                pair_keys
                    .entry((*left, *right))
                    .or_default()
                    .insert(key.clone());
            }
        }
    }

    let mut pairs = pair_keys.into_iter().collect::<Vec<_>>();
    pairs.sort_by(|left, right| {
        pair_block_priority(&left.1)
            .cmp(&pair_block_priority(&right.1))
            .then_with(|| {
                left.0
                    .1
                    .saturating_sub(left.0.0)
                    .cmp(&right.0.1.saturating_sub(right.0.0))
            })
            .then_with(|| left.0.cmp(&right.0))
    });
    let mut per_occurrence = vec![0_usize; occurrences.len()];
    let mut candidates = Vec::new();
    for ((left_index, right_index), keys) in pairs {
        if per_occurrence[left_index] >= policy.max_candidates_per_occurrence
            || per_occurrence[right_index] >= policy.max_candidates_per_occurrence
        {
            continue;
        }
        stats.comparisons = stats.comparisons.saturating_add(1);
        let assessment = assess_match(&occurrences[left_index], &occurrences[right_index], policy);
        if assessment.decision != ResolutionDecision::Unknown {
            per_occurrence[left_index] = per_occurrence[left_index].saturating_add(1);
            per_occurrence[right_index] = per_occurrence[right_index].saturating_add(1);
            candidates.push(IdentityCandidate {
                left_index,
                right_index,
                blocking_keys: keys.into_iter().collect(),
                assessment,
            });
        }
    }
    candidates.sort_by(|left, right| {
        right
            .assessment
            .score
            .partial_cmp(&left.assessment.score)
            .unwrap_or(Ordering::Equal)
            .then_with(|| left.left_index.cmp(&right.left_index))
            .then_with(|| left.right_index.cmp(&right.right_index))
    });
    stats.candidates = candidates.len();
    CandidateGeneration { candidates, stats }
}

fn pair_block_priority(keys: &BTreeSet<String>) -> u8 {
    keys.iter()
        .map(|key| {
            if [
                ":company_identifier:",
                ":vat_identifier:",
                ":email:",
                ":phone:",
                ":account_identifier:",
                ":project_reference:",
            ]
            .iter()
            .any(|marker| key.contains(marker))
            {
                0
            } else if key.contains(":name:") {
                1
            } else if key.contains(":core:") {
                2
            } else {
                3
            }
        })
        .min()
        .unwrap_or(4)
}

#[must_use]
pub fn canonical_display_name<'a>(
    occurrences: impl IntoIterator<Item = &'a IdentityOccurrence>,
) -> Option<String> {
    let mut values = occurrences.into_iter().collect::<Vec<_>>();
    values.sort_by(|left, right| {
        right
            .confidence
            .value()
            .partial_cmp(&left.confidence.value())
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                left.original_value
                    .chars()
                    .count()
                    .cmp(&right.original_value.chars().count())
            })
            .then_with(|| left.original_value.cmp(&right.original_value))
    });
    values
        .first()
        .map(|occurrence| smart_title_case(&occurrence.original_value))
}

fn normalize_signal(kind: SignalKind, value: &str) -> Option<String> {
    match kind {
        SignalKind::Name => normalize_name(value).map(|name| name.exact),
        SignalKind::CompanyIdentifier
        | SignalKind::VatIdentifier
        | SignalKind::AccountIdentifier
        | SignalKind::ProjectReference => normalize_company_identifier(value),
        SignalKind::Email => normalize_email(value),
        SignalKind::Domain => normalize_domain(value),
        SignalKind::Phone => normalize_phone(value),
        SignalKind::Address | SignalKind::CustomerIdentity | SignalKind::PathContext => {
            normalize_name(value).map(|name| name.exact)
        }
        SignalKind::Date => normalize_date(value),
    }
}

fn fold_and_separate(value: &str) -> String {
    let mut output = String::with_capacity(value.len());
    let mut last_separator = true;
    for character in value.nfkc().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            output.push(character);
            last_separator = false;
        } else if !last_separator {
            output.push(' ');
            last_separator = true;
        }
    }
    output.trim().to_owned()
}

fn take_legal_suffix(tokens: &mut Vec<String>) -> Option<String> {
    const SUFFIXES: &[&str] = &[
        "sas",
        "sasu",
        "sarl",
        "sa",
        "eurl",
        "scop",
        "sei",
        "llc",
        "ltd",
        "limited",
        "inc",
        "incorporated",
        "corp",
        "corporation",
        "gmbh",
        "ag",
        "bv",
        "nv",
        "plc",
    ];
    const SPACED_SUFFIXES: &[(&[&str], &str)] = &[
        (&["s", "a", "s"], "sas"),
        (&["s", "a", "r", "l"], "sarl"),
        (&["s", "a"], "sa"),
        (&["l", "l", "c"], "llc"),
    ];
    for (parts, canonical) in SPACED_SUFFIXES {
        if tokens.len() >= parts.len()
            && tokens[tokens.len() - parts.len()..]
                .iter()
                .map(String::as_str)
                .eq(parts.iter().copied())
        {
            tokens.truncate(tokens.len() - parts.len());
            return Some((*canonical).to_owned());
        }
    }
    if tokens
        .last()
        .is_some_and(|candidate| SUFFIXES.contains(&candidate.as_str()))
    {
        return tokens.pop();
    }
    None
}

fn luhn_valid(value: &str) -> bool {
    let mut sum = 0_u32;
    let mut double = false;
    for character in value.chars().rev() {
        let Some(mut digit) = character.to_digit(10) else {
            return false;
        };
        if double {
            digit *= 2;
            if digit > 9 {
                digit -= 9;
            }
        }
        sum += digit;
        double = !double;
    }
    sum.is_multiple_of(10)
}

fn normalize_date(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let bytes = trimmed.as_bytes();
    if bytes.len() == 10
        && bytes[4] == b'-'
        && bytes[7] == b'-'
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
    {
        Some(trimmed.to_owned())
    } else {
        None
    }
}

fn useful_core_name(name: &NormalizedName) -> bool {
    name.tokens.len() >= 2 || name.core.chars().count() >= 6
}

fn compare_exclusive_identifiers(
    left: &IdentityOccurrence,
    right: &IdentityOccurrence,
    evidence: &mut Vec<IdentityEvidence>,
) {
    for kind in [
        SignalKind::CompanyIdentifier,
        SignalKind::VatIdentifier,
        SignalKind::AccountIdentifier,
        SignalKind::ProjectReference,
    ] {
        let left_values = values_for(left, kind);
        let right_values = values_for(right, kind);
        if left_values.is_empty() || right_values.is_empty() {
            continue;
        }
        let shared = left_values
            .intersection(&right_values)
            .cloned()
            .collect::<Vec<_>>();
        if let Some(value) = shared.first() {
            let (strength, weight, explanation) = match kind {
                SignalKind::CompanyIdentifier | SignalKind::VatIdentifier => (
                    EvidenceStrength::VeryStrong,
                    0.72,
                    "same validated company identifier",
                ),
                SignalKind::ProjectReference => (
                    EvidenceStrength::VeryStrong,
                    0.62,
                    "same explicit project reference",
                ),
                _ => (
                    EvidenceStrength::Strong,
                    0.58,
                    "same explicit account identifier",
                ),
            };
            evidence.push(supporting(
                kind.database_name(),
                strength,
                value,
                value,
                weight,
                explanation,
            ));
        } else {
            evidence.push(conflicting(
                kind.database_name(),
                left_values.iter().next().map_or("", String::as_str),
                right_values.iter().next().map_or("", String::as_str),
                match kind {
                    SignalKind::ProjectReference => "different explicit project references",
                    _ => "different validated strong identifiers",
                },
            ));
        }
    }
}

fn compare_names(
    left: &IdentityOccurrence,
    right: &IdentityOccurrence,
    policy: IdentityResolutionPolicy,
    evidence: &mut Vec<IdentityEvidence>,
) {
    if left.normalized_name.exact == right.normalized_name.exact {
        evidence.push(supporting(
            "normalized_name",
            EvidenceStrength::Medium,
            &left.normalized_name.exact,
            &right.normalized_name.exact,
            0.62,
            "same normalized name",
        ));
        return;
    }
    if left.normalized_name.core == right.normalized_name.core {
        match (
            left.normalized_name.legal_suffix.as_deref(),
            right.normalized_name.legal_suffix.as_deref(),
        ) {
            (Some(left_suffix), Some(right_suffix)) if left_suffix != right_suffix => {
                evidence.push(IdentityEvidence {
                    evidence_type: "legal_suffix".to_owned(),
                    strength: EvidenceStrength::Conflicting,
                    polarity: EvidencePolarity::Conflicts,
                    left_value: left_suffix.to_owned(),
                    right_value: right_suffix.to_owned(),
                    weight: 0.0,
                    explanation: "different legal suffixes can identify different businesses"
                        .to_owned(),
                });
            }
            _ => evidence.push(supporting(
                "normalized_name_core",
                EvidenceStrength::Medium,
                &left.normalized_name.core,
                &right.normalized_name.core,
                0.54,
                "same normalized core name; legal suffix retained separately",
            )),
        }
        return;
    }
    let similarity = token_similarity(&left.normalized_name.tokens, &right.normalized_name.tokens);
    if similarity >= policy.fuzzy_name_threshold {
        evidence.push(supporting(
            "bounded_name_similarity",
            EvidenceStrength::Weak,
            &left.normalized_name.exact,
            &right.normalized_name.exact,
            0.34,
            &format!("bounded token similarity {:.0}%", similarity * 100.0),
        ));
    }
}

fn compare_shared_signals(
    left: &IdentityOccurrence,
    right: &IdentityOccurrence,
    evidence: &mut Vec<IdentityEvidence>,
) {
    for kind in [
        SignalKind::Email,
        SignalKind::Domain,
        SignalKind::Phone,
        SignalKind::Address,
        SignalKind::CustomerIdentity,
        SignalKind::Date,
        SignalKind::PathContext,
    ] {
        let left_values = values_for(left, kind);
        let right_values = values_for(right, kind);
        let Some(value) = left_values.intersection(&right_values).next() else {
            continue;
        };
        let (strength, weight, explanation) = match kind {
            SignalKind::Email => (
                EvidenceStrength::VeryStrong,
                0.70,
                "same exact normalized email",
            ),
            SignalKind::Phone => (
                EvidenceStrength::Strong,
                0.55,
                "same exact normalized phone",
            ),
            SignalKind::Domain if generic_email_domain(value) => (
                EvidenceStrength::Weak,
                0.03,
                "same generic email domain; weak context only",
            ),
            SignalKind::Domain => (
                EvidenceStrength::Strong,
                0.24,
                "same non-generic normalized email domain",
            ),
            SignalKind::Address => (EvidenceStrength::Weak, 0.14, "same normalized address"),
            SignalKind::CustomerIdentity => {
                (EvidenceStrength::Medium, 0.22, "same customer association")
            }
            SignalKind::Date => (EvidenceStrength::Weak, 0.04, "compatible exact date"),
            SignalKind::PathContext => (EvidenceStrength::Weak, 0.03, "same path context"),
            _ => continue,
        };
        evidence.push(supporting(
            kind.database_name(),
            strength,
            value,
            value,
            weight,
            explanation,
        ));
    }
}

fn values_for(occurrence: &IdentityOccurrence, kind: SignalKind) -> BTreeSet<String> {
    occurrence
        .signals
        .iter()
        .filter(|signal| signal.kind == kind)
        .map(|signal| signal.normalized_value.clone())
        .collect()
}

fn supporting(
    evidence_type: &str,
    strength: EvidenceStrength,
    left_value: &str,
    right_value: &str,
    weight: f32,
    explanation: &str,
) -> IdentityEvidence {
    IdentityEvidence {
        evidence_type: evidence_type.to_owned(),
        strength,
        polarity: EvidencePolarity::Supports,
        left_value: left_value.to_owned(),
        right_value: right_value.to_owned(),
        weight,
        explanation: explanation.to_owned(),
    }
}

fn conflicting(
    evidence_type: &str,
    left_value: &str,
    right_value: &str,
    explanation: &str,
) -> IdentityEvidence {
    IdentityEvidence {
        evidence_type: evidence_type.to_owned(),
        strength: EvidenceStrength::Conflicting,
        polarity: EvidencePolarity::Conflicts,
        left_value: left_value.to_owned(),
        right_value: right_value.to_owned(),
        weight: 0.0,
        explanation: explanation.to_owned(),
    }
}

fn has_blocking_conflict(evidence: &[IdentityEvidence]) -> bool {
    evidence.iter().any(|item| {
        item.polarity == EvidencePolarity::Conflicts
            && matches!(
                item.evidence_type.as_str(),
                "company_identifier"
                    | "vat_identifier"
                    | "account_identifier"
                    | "project_reference"
            )
    })
}

fn auto_link_requirements(
    left: &IdentityOccurrence,
    right: &IdentityOccurrence,
    evidence: &[IdentityEvidence],
) -> bool {
    if left.identity_type != right.identity_type {
        return false;
    }
    let has = |evidence_type: &str| {
        evidence.iter().any(|item| {
            item.evidence_type == evidence_type && item.polarity == EvidencePolarity::Supports
        })
    };
    let compatible_name = has("normalized_name") || has("normalized_name_core");
    match left.identity_type {
        IdentityType::Organization => {
            compatible_name && (has("company_identifier") || has("vat_identifier"))
        }
        IdentityType::Person => has("normalized_name") && has("email") && has("phone"),
        IdentityType::Project => {
            has("project_reference")
                && has("customer_identity")
                && (compatible_name || has("address"))
        }
    }
}

fn evidence_order(left: &IdentityEvidence, right: &IdentityEvidence) -> Ordering {
    let rank = |item: &IdentityEvidence| match item.strength {
        EvidenceStrength::Conflicting => 0_u8,
        EvidenceStrength::VeryStrong => 1,
        EvidenceStrength::Strong => 2,
        EvidenceStrength::Medium => 3,
        EvidenceStrength::Weak => 4,
    };
    rank(left)
        .cmp(&rank(right))
        .then_with(|| left.evidence_type.cmp(&right.evidence_type))
        .then_with(|| left.left_value.cmp(&right.left_value))
}

fn token_similarity(left: &[String], right: &[String]) -> f32 {
    if left.is_empty() || right.is_empty() {
        return 0.0;
    }
    let left = left.iter().map(String::as_str).collect::<HashSet<_>>();
    let right = right.iter().map(String::as_str).collect::<HashSet<_>>();
    let intersection = left.intersection(&right).count() as f32;
    let union = left.union(&right).count() as f32;
    if union == 0.0 {
        0.0
    } else {
        intersection / union
    }
}

fn generic_email_domain(value: &str) -> bool {
    matches!(
        value,
        "gmail.com"
            | "googlemail.com"
            | "outlook.com"
            | "hotmail.com"
            | "live.com"
            | "yahoo.com"
            | "yahoo.fr"
            | "icloud.com"
            | "proton.me"
            | "protonmail.com"
            | "orange.fr"
            | "free.fr"
            | "laposte.net"
    )
}

fn smart_title_case(value: &str) -> String {
    let trimmed = value.trim();
    let has_lowercase = trimmed.chars().any(char::is_lowercase);
    if has_lowercase {
        return trimmed.to_owned();
    }
    trimmed
        .split_whitespace()
        .map(|token| {
            let mut characters = token.chars();
            characters.next().map_or_else(String::new, |first| {
                first
                    .to_uppercase()
                    .chain(characters.flat_map(char::to_lowercase))
                    .collect()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn validate_bounded(value: &str) -> Result<(), IdentityResolutionError> {
    let count = value.chars().count();
    if count == 0 {
        return Err(IdentityResolutionError::EmptyValue);
    }
    if count > MAX_IDENTITY_VALUE_CHARS {
        return Err(IdentityResolutionError::ValueTooLong);
    }
    if value.chars().any(|character| character == '\0') {
        return Err(IdentityResolutionError::InvalidValue);
    }
    Ok(())
}

fn validate_identifier(value: &str) -> Result<(), IdentityResolutionError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_:.".contains(character))
    {
        return Err(IdentityResolutionError::InvalidIdentifier);
    }
    Ok(())
}

fn validate_version(value: &str) -> Result<(), IdentityResolutionError> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || "-_+.".contains(character))
    {
        return Err(IdentityResolutionError::InvalidVersion);
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum IdentityResolutionError {
    #[error("identity value is empty")]
    EmptyValue,
    #[error("identity value exceeds the bounded length")]
    ValueTooLong,
    #[error("identity value is invalid")]
    InvalidValue,
    #[error("identity identifier is invalid")]
    InvalidIdentifier,
    #[error("identity confidence is invalid")]
    InvalidConfidence,
    #[error("identity analyzer version is invalid")]
    InvalidVersion,
    #[error("identity occurrence has too many signals")]
    TooManySignals,
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_SIRET_A: &str = "73282932000074";
    const VALID_SIRET_B: &str = "55210055400013";

    fn occurrence(
        key: &str,
        identity_type: IdentityType,
        name: &str,
        signals: &[(&str, SignalKind)],
    ) -> IdentityOccurrence {
        IdentityOccurrence::new(
            key,
            &format!("file-{key}"),
            Some(format!("entity-{key}")),
            None,
            identity_type,
            None,
            name,
            0.9,
            "5.0.0",
            signals
                .iter()
                .map(|(value, kind)| (*kind, (*value).to_owned())),
        )
        .unwrap_or_else(|error| panic!("fixture occurrence should be valid: {error}"))
    }

    #[test]
    fn conservative_name_normalization_preserves_legal_suffix_signal() {
        let plain = normalize_name(" POINT.P ").unwrap_or_else(|| panic!("name should normalize"));
        let company =
            normalize_name("Point P S.A.S.").unwrap_or_else(|| panic!("name should normalize"));
        assert_eq!(plain.exact, "point p");
        assert_eq!(company.exact, "point p sas");
        assert_eq!(company.core, "point p");
        assert_eq!(company.legal_suffix.as_deref(), Some("sas"));
    }

    #[test]
    fn dataset_a_same_supplier_with_company_id_auto_links() {
        let left = occurrence(
            "a",
            IdentityType::Organization,
            "Point P",
            &[(VALID_SIRET_A, SignalKind::CompanyIdentifier)],
        );
        let right = occurrence(
            "b",
            IdentityType::Organization,
            "POINT.P",
            &[(VALID_SIRET_A, SignalKind::CompanyIdentifier)],
        );
        let result = assess_match(&left, &right, IdentityResolutionPolicy::default());
        assert_eq!(result.decision, ResolutionDecision::AutoLink);
        assert!(result.score >= 0.97);
    }

    #[test]
    fn dataset_b_conflicting_company_ids_stay_separate() {
        let left = occurrence(
            "a",
            IdentityType::Organization,
            "Martin SARL",
            &[(VALID_SIRET_A, SignalKind::CompanyIdentifier)],
        );
        let right = occurrence(
            "b",
            IdentityType::Organization,
            "Martin SARL",
            &[(VALID_SIRET_B, SignalKind::CompanyIdentifier)],
        );
        let result = assess_match(&left, &right, IdentityResolutionPolicy::default());
        assert_eq!(result.decision, ResolutionDecision::KeepSeparate);
        assert!(result.evidence.iter().any(|item| {
            item.strength == EvidenceStrength::Conflicting
                && item.evidence_type == "company_identifier"
        }));
    }

    #[test]
    fn datasets_c_and_e_similar_names_never_auto_link() {
        let construction = occurrence(
            "construction",
            IdentityType::Organization,
            "Dupont Construction",
            &[],
        );
        let electricity = occurrence(
            "electricity",
            IdentityType::Organization,
            "Dupont Électricité",
            &[],
        );
        let jean = occurrence("jean", IdentityType::Person, "Jean Martin", &[]);
        let jean_pierre = occurrence(
            "jean-pierre",
            IdentityType::Person,
            "Jean-Pierre Martin",
            &[],
        );
        assert_ne!(
            assess_match(
                &construction,
                &electricity,
                IdentityResolutionPolicy::default()
            )
            .decision,
            ResolutionDecision::AutoLink
        );
        assert_ne!(
            assess_match(&jean, &jean_pierre, IdentityResolutionPolicy::default()).decision,
            ResolutionDecision::AutoLink
        );
    }

    #[test]
    fn same_name_without_corroborration_requires_review() {
        let left = occurrence("a", IdentityType::Organization, "Point P", &[]);
        let right = occurrence("b", IdentityType::Organization, "POINT.P", &[]);
        let result = assess_match(&left, &right, IdentityResolutionPolicy::default());
        assert_eq!(result.decision, ResolutionDecision::Review);
        assert!(result.score < IdentityResolutionPolicy::default().auto_link_threshold);
    }

    #[test]
    fn person_auto_link_requires_name_email_and_phone() {
        let left = occurrence(
            "a",
            IdentityType::Person,
            "Alice Martin",
            &[
                ("alice@example.test", SignalKind::Email),
                ("+33 6 12 34 56 78", SignalKind::Phone),
            ],
        );
        let email_only = occurrence(
            "b",
            IdentityType::Person,
            "Alice Martin",
            &[("alice@example.test", SignalKind::Email)],
        );
        let complete = occurrence(
            "c",
            IdentityType::Person,
            "Alice Martin",
            &[
                ("alice@example.test", SignalKind::Email),
                ("+33 6 12 34 56 78", SignalKind::Phone),
            ],
        );
        assert_eq!(
            assess_match(&left, &email_only, IdentityResolutionPolicy::default()).decision,
            ResolutionDecision::Review
        );
        assert_eq!(
            assess_match(&left, &complete, IdentityResolutionPolicy::default()).decision,
            ResolutionDecision::AutoLink
        );
    }

    #[test]
    fn datasets_f_and_g_projects_need_reference_customer_and_context() {
        let shared = [
            ("MARTIN-BDX-26", SignalKind::ProjectReference),
            ("customer-martin", SignalKind::CustomerIdentity),
            ("10 rue Exemple Bordeaux", SignalKind::Address),
        ];
        let first = occurrence("first", IdentityType::Project, "Martin Bordeaux", &shared);
        let second = occurrence("second", IdentityType::Project, "Projet Martin", &shared);
        assert_eq!(
            assess_match(&first, &second, IdentityResolutionPolicy::default()).decision,
            ResolutionDecision::AutoLink
        );

        let other_project = occurrence(
            "other",
            IdentityType::Project,
            "Martin Lyon",
            &[
                ("MARTIN-LYO-26", SignalKind::ProjectReference),
                ("customer-martin", SignalKind::CustomerIdentity),
            ],
        );
        assert_eq!(
            assess_match(&first, &other_project, IdentityResolutionPolicy::default()).decision,
            ResolutionDecision::KeepSeparate
        );
    }

    #[test]
    fn weak_signals_do_not_create_high_confidence_links() {
        let same_city = occurrence(
            "a",
            IdentityType::Organization,
            "Alpha",
            &[("Bordeaux", SignalKind::Address)],
        );
        let other = occurrence(
            "b",
            IdentityType::Organization,
            "Beta",
            &[("Bordeaux", SignalKind::Address)],
        );
        let generic_domain = occurrence(
            "c",
            IdentityType::Organization,
            "Gamma",
            &[("gmail.com", SignalKind::Domain)],
        );
        let generic_other = occurrence(
            "d",
            IdentityType::Organization,
            "Delta",
            &[("gmail.com", SignalKind::Domain)],
        );
        for result in [
            assess_match(&same_city, &other, IdentityResolutionPolicy::default()),
            assess_match(
                &generic_domain,
                &generic_other,
                IdentityResolutionPolicy::default(),
            ),
        ] {
            assert_eq!(result.decision, ResolutionDecision::Unknown);
            assert!(result.score < IdentityResolutionPolicy::default().review_threshold);
        }
    }

    #[test]
    fn candidate_generation_is_bounded_and_not_all_pairs() {
        let occurrences = (0..10_000)
            .map(|index| {
                occurrence(
                    &format!("{index}"),
                    IdentityType::Organization,
                    &format!("Supplier {index}"),
                    &[],
                )
            })
            .collect::<Vec<_>>();
        let generation = generate_candidates(&occurrences, IdentityResolutionPolicy::default());
        assert_eq!(generation.stats.occurrences, 10_000);
        assert_eq!(generation.stats.comparisons, 0);
        assert_eq!(generation.stats.candidates, 0);
        assert!(generation.stats.blocking_memberships <= 20_000);
    }

    #[test]
    fn strong_identifier_pairs_are_prioritized_before_weak_candidate_caps() {
        let base_name = "alpha beta gamma delta epsilon zeta";
        let mut occurrences = vec![occurrence(
            "hub",
            IdentityType::Organization,
            base_name,
            &[
                ("one.example", SignalKind::Domain),
                ("two.example", SignalKind::Domain),
                ("three.example", SignalKind::Domain),
                (VALID_SIRET_A, SignalKind::CompanyIdentifier),
            ],
        )];
        for index in 0..48 {
            let domain = match index % 3 {
                0 => "one.example",
                1 => "two.example",
                _ => "three.example",
            };
            occurrences.push(occurrence(
                &format!("weak-{index}"),
                IdentityType::Organization,
                &format!("{base_name} branch{index}"),
                &[(domain, SignalKind::Domain)],
            ));
        }
        occurrences.push(occurrence(
            "strong",
            IdentityType::Organization,
            base_name,
            &[(VALID_SIRET_A, SignalKind::CompanyIdentifier)],
        ));
        let strong_index = occurrences.len() - 1;
        let generation = generate_candidates(&occurrences, IdentityResolutionPolicy::default());
        assert!(generation.candidates.iter().any(|candidate| {
            candidate.left_index == 0
                && candidate.right_index == strong_index
                && candidate.assessment.decision == ResolutionDecision::AutoLink
        }));
    }

    #[test]
    fn large_strong_blocks_keep_a_bounded_connected_candidate_chain() {
        let occurrences = (0..1_024)
            .map(|index| {
                occurrence(
                    &format!("strong-block-{index}"),
                    IdentityType::Organization,
                    "Shared Strong Supplier",
                    &[(VALID_SIRET_A, SignalKind::CompanyIdentifier)],
                )
            })
            .collect::<Vec<_>>();
        let generation = generate_candidates(&occurrences, IdentityResolutionPolicy::default());
        let mut adjacency = vec![Vec::new(); occurrences.len()];
        for candidate in &generation.candidates {
            assert_eq!(candidate.assessment.decision, ResolutionDecision::AutoLink);
            adjacency[candidate.left_index].push(candidate.right_index);
            adjacency[candidate.right_index].push(candidate.left_index);
        }
        let mut visited = vec![false; occurrences.len()];
        let mut pending = vec![0_usize];
        while let Some(index) = pending.pop() {
            if visited[index] {
                continue;
            }
            visited[index] = true;
            pending.extend(adjacency[index].iter().copied());
        }
        assert!(visited.into_iter().all(|value| value));
        assert!(generation.stats.comparisons < 40_000);
        assert!(generation.stats.truncated_blocks >= 1);
    }

    #[test]
    fn adversarial_values_are_bounded_and_never_interpreted() {
        let script = occurrence(
            "script",
            IdentityType::Organization,
            "<script>alert('x')</script>; DROP TABLE identities;--",
            &[],
        );
        assert!(
            script
                .normalized_name
                .exact
                .contains("drop table identities")
        );
        assert!(
            IdentityOccurrence::new(
                "long",
                "file-long",
                None,
                None,
                IdentityType::Organization,
                None,
                &"x".repeat(MAX_IDENTITY_VALUE_CHARS + 1),
                0.5,
                "5.0.0",
                [],
            )
            .is_err()
        );
        assert!(normalize_email("not an email").is_none());
        assert!(normalize_phone("123").is_none());
        assert!(normalize_phone("call me 06 12 34 56 78").is_none());
        assert!(normalize_company_identifier("00000000000000").is_none());
        assert!(normalize_name("").is_none());
        assert!(normalize_domain("-invalid.example").is_none());
    }

    #[test]
    fn unicode_compatibility_is_normalized_without_merging_confusables() {
        let compatibility = normalize_name("ＰＯＩＮＴ\u{00a0}Ｐ")
            .unwrap_or_else(|| panic!("name should normalize"));
        assert_eq!(compatibility.exact, "point p");
        let latin = normalize_name("Point P").unwrap_or_else(|| panic!("name should normalize"));
        let cyrillic = normalize_name("Рoint P").unwrap_or_else(|| panic!("name should normalize"));
        assert_ne!(latin.exact, cyrillic.exact);
    }
}
