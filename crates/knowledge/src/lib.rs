//! Local semantic assertions with explicit provenance and calibration state.

mod identity;
mod semantic;

pub use identity::*;
pub use semantic::*;

use domain::{CalibrationState, Confidence, FileVersionId};
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SemanticClass {
    Invoice,
    Quote,
    Contract,
    CustomerRecord,
    SupplierRecord,
    HumanResources,
    Accounting,
    Legal,
    Administrative,
    Photo,
    Video,
    Screenshot,
    Archive,
    Personal,
    Professional,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassificationAssertion {
    pub class: SemanticClass,
    pub confidence: Confidence,
    pub evidence: Vec<TextEvidence>,
    pub calibrator_version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextEvidence {
    pub start: usize,
    pub end: usize,
    pub excerpt: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EntityAssertion {
    pub entity_type: String,
    pub canonical_name: String,
    pub evidence: TextEvidence,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FactAssertion {
    pub predicate: String,
    pub value: FactValue,
    pub evidence: TextEvidence,
    pub confidence: Confidence,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum FactValue {
    Text(String),
    Money { amount_minor: i64, currency: String },
    Date(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RelationshipAssertion {
    pub source_entity: String,
    pub predicate: String,
    pub target_entity: String,
    pub confidence: Confidence,
    pub evidence: Vec<TextEvidence>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticDocument {
    pub file_version_id: FileVersionId,
    pub classifications: Vec<ClassificationAssertion>,
    pub entities: Vec<EntityAssertion>,
    pub facts: Vec<FactAssertion>,
    pub relationships: Vec<RelationshipAssertion>,
    pub pipeline_version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BinaryCalibrator {
    pub version: String,
    pub slope: f32,
    pub intercept: f32,
    pub trained_samples: usize,
    pub minimum_samples: usize,
}

impl BinaryCalibrator {
    pub fn calibrate(
        &self,
        raw_score: f32,
        out_of_distribution: bool,
    ) -> Result<Confidence, KnowledgeError> {
        if out_of_distribution {
            return Confidence::new(raw_score, None, CalibrationState::OutOfDistribution)
                .map_err(KnowledgeError::Confidence);
        }
        if self.trained_samples < self.minimum_samples {
            return Confidence::new(raw_score, None, CalibrationState::Uncalibrated)
                .map_err(KnowledgeError::Confidence);
        }
        let logit = self.slope.mul_add(raw_score, self.intercept);
        let probability = 1.0 / (1.0 + (-logit).exp());
        Confidence::new(raw_score, Some(probability), CalibrationState::Calibrated)
            .map_err(KnowledgeError::Confidence)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CalibrationMetrics {
    pub brier_score: f32,
    pub expected_calibration_error: f32,
}

impl CalibrationMetrics {
    #[must_use]
    pub fn from_predictions(predictions: &[(f32, bool)], bins: usize) -> Self {
        if predictions.is_empty() {
            return Self {
                brier_score: 0.0,
                expected_calibration_error: 0.0,
            };
        }
        let brier_score = predictions
            .iter()
            .map(|(probability, positive)| {
                let target = if *positive { 1.0 } else { 0.0 };
                (probability - target).powi(2)
            })
            .sum::<f32>()
            / predictions.len() as f32;
        let bins = bins.max(1);
        let mut expected_calibration_error = 0.0_f32;
        for bin in 0..bins {
            let lower = bin as f32 / bins as f32;
            let upper = (bin + 1) as f32 / bins as f32;
            let values = predictions
                .iter()
                .filter(|(probability, _)| {
                    *probability >= lower
                        && (*probability < upper || (bin + 1 == bins && *probability <= upper))
                })
                .collect::<Vec<_>>();
            if values.is_empty() {
                continue;
            }
            let confidence =
                values.iter().map(|(value, _)| *value).sum::<f32>() / values.len() as f32;
            let accuracy = values.iter().filter(|(_, positive)| *positive).count() as f32
                / values.len() as f32;
            expected_calibration_error +=
                (confidence - accuracy).abs() * (values.len() as f32 / predictions.len() as f32);
        }
        Self {
            brier_score,
            expected_calibration_error,
        }
    }
}

#[derive(Debug, Default)]
pub struct LocalSemanticAnalyzer;

impl LocalSemanticAnalyzer {
    pub fn analyze(
        &self,
        file_version_id: FileVersionId,
        display_label: &str,
        mime: Option<&str>,
        text: &str,
    ) -> Result<SemanticDocument, KnowledgeError> {
        let haystack = format!("{display_label}\n{text}");
        let lowered = haystack.to_lowercase();
        let mut classifications = classify(&haystack, &lowered, mime)?;
        if classifications.is_empty() {
            classifications.push(ClassificationAssertion {
                class: SemanticClass::Unknown,
                confidence: Confidence::new(0.0, None, CalibrationState::OutOfDistribution)?,
                evidence: Vec::new(),
                calibrator_version: None,
            });
        }
        let entities = extract_entities(&haystack)?;
        let facts = extract_facts(&haystack)?;
        let relationships = infer_relationships(&entities)?;

        Ok(SemanticDocument {
            file_version_id,
            classifications,
            entities,
            facts,
            relationships,
            pipeline_version: "local-rules-1".to_owned(),
        })
    }
}

fn classify(
    source: &str,
    lowered: &str,
    mime: Option<&str>,
) -> Result<Vec<ClassificationAssertion>, KnowledgeError> {
    let rules: &[(SemanticClass, &[&str])] = &[
        (
            SemanticClass::Invoice,
            &["facture", "invoice", "invoice number", "montant ttc"],
        ),
        (
            SemanticClass::Quote,
            &[
                "devis",
                "quotation",
                "quote number",
                "proposition commerciale",
            ],
        ),
        (
            SemanticClass::Contract,
            &["contrat", "contract", "agreement", "conditions générales"],
        ),
        (
            SemanticClass::HumanResources,
            &[
                "bulletin de paie",
                "payslip",
                "curriculum vitae",
                "contrat de travail",
            ],
        ),
        (
            SemanticClass::Accounting,
            &[
                "grand livre",
                "journal comptable",
                "balance comptable",
                "fiscal",
            ],
        ),
        (
            SemanticClass::Legal,
            &[
                "tribunal",
                "legal notice",
                "assignation",
                "statuts de la société",
            ],
        ),
        (
            SemanticClass::Administrative,
            &["administration", "cerfa", "attestation", "justificatif"],
        ),
    ];
    let mut assertions = Vec::new();
    for (class, terms) in rules {
        let mut evidence = Vec::new();
        for term in *terms {
            if let Some(start) = lowered.find(term) {
                let end = start + term.len();
                evidence.push(TextEvidence {
                    start,
                    end,
                    excerpt: source.get(start..end).unwrap_or(term).to_owned(),
                });
            }
        }
        if !evidence.is_empty() {
            let raw = (evidence.len() as f32 / terms.len() as f32).clamp(0.0, 1.0);
            assertions.push(ClassificationAssertion {
                class: *class,
                confidence: Confidence::new(raw, None, CalibrationState::Uncalibrated)?,
                evidence,
                calibrator_version: None,
            });
        }
    }

    if mime.is_some_and(|value| value.starts_with("image/")) {
        assertions.push(ClassificationAssertion {
            class: if lowered.contains("screenshot") || lowered.contains("capture") {
                SemanticClass::Screenshot
            } else {
                SemanticClass::Photo
            },
            confidence: Confidence::new(0.9, None, CalibrationState::Uncalibrated)?,
            evidence: Vec::new(),
            calibrator_version: None,
        });
    } else if mime.is_some_and(|value| value.starts_with("video/")) {
        assertions.push(ClassificationAssertion {
            class: SemanticClass::Video,
            confidence: Confidence::new(1.0, Some(1.0), CalibrationState::Calibrated)?,
            evidence: Vec::new(),
            calibrator_version: Some("mime-observation-1".to_owned()),
        });
    } else if mime.is_some_and(|value| {
        value == "application/zip"
            || value == "application/x-rar-compressed"
            || value == "application/x-7z-compressed"
    }) {
        assertions.push(ClassificationAssertion {
            class: SemanticClass::Archive,
            confidence: Confidence::new(1.0, Some(1.0), CalibrationState::Calibrated)?,
            evidence: Vec::new(),
            calibrator_version: Some("mime-observation-1".to_owned()),
        });
    }
    Ok(assertions)
}

fn extract_entities(source: &str) -> Result<Vec<EntityAssertion>, KnowledgeError> {
    let pattern = Regex::new(
        r"(?im)^(?:client|customer|fournisseur|supplier|société|company|projet|project)\s*[:#-]\s*([^\r\n]{2,100})",
    )?;
    let mut output = Vec::new();
    for capture in pattern.captures_iter(source) {
        let Some(full) = capture.get(0) else {
            continue;
        };
        let Some(name) = capture.get(1) else {
            continue;
        };
        let prefix = full
            .as_str()
            .split([':', '#', '-'])
            .next()
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        let entity_type = match prefix.as_str() {
            "client" | "customer" => "customer",
            "fournisseur" | "supplier" => "supplier",
            "projet" | "project" => "project",
            _ => "organization",
        };
        output.push(EntityAssertion {
            entity_type: entity_type.to_owned(),
            canonical_name: normalize_entity_name(name.as_str()),
            evidence: TextEvidence {
                start: full.start(),
                end: full.end(),
                excerpt: full.as_str().to_owned(),
            },
            confidence: Confidence::new(0.82, None, CalibrationState::Uncalibrated)?,
        });
    }
    Ok(output)
}

fn extract_facts(source: &str) -> Result<Vec<FactAssertion>, KnowledgeError> {
    let invoice_number = Regex::new(
        r"(?i)(?:facture|invoice)\s*(?:n[°o.]|number|#)?\s*[:#-]?\s*([A-Z0-9][A-Z0-9._/-]{2,40})",
    )?;
    let money = Regex::new(
        r"(?i)\b([0-9]{1,9}(?:[ ,.][0-9]{3})*(?:[,.][0-9]{2})?)\s*(EUR|USD|GBP|€|\$|£)\b",
    )?;
    let date = Regex::new(r"\b(20[0-9]{2}[-/](?:0[1-9]|1[0-2])[-/](?:0[1-9]|[12][0-9]|3[01]))\b")?;
    let mut facts = Vec::new();

    for capture in invoice_number.captures_iter(source).take(4) {
        if let (Some(full), Some(value)) = (capture.get(0), capture.get(1)) {
            facts.push(FactAssertion {
                predicate: "document_number".to_owned(),
                value: FactValue::Text(value.as_str().to_owned()),
                evidence: TextEvidence {
                    start: full.start(),
                    end: full.end(),
                    excerpt: full.as_str().to_owned(),
                },
                confidence: Confidence::new(0.86, None, CalibrationState::Uncalibrated)?,
            });
        }
    }
    for capture in money.captures_iter(source).take(8) {
        if let (Some(full), Some(amount), Some(currency)) =
            (capture.get(0), capture.get(1), capture.get(2))
            && let Some(amount_minor) = parse_money_minor(amount.as_str())
        {
            facts.push(FactAssertion {
                predicate: "amount".to_owned(),
                value: FactValue::Money {
                    amount_minor,
                    currency: normalize_currency(currency.as_str()).to_owned(),
                },
                evidence: TextEvidence {
                    start: full.start(),
                    end: full.end(),
                    excerpt: full.as_str().to_owned(),
                },
                confidence: Confidence::new(0.9, None, CalibrationState::Uncalibrated)?,
            });
        }
    }
    for capture in date.captures_iter(source).take(8) {
        if let Some(full) = capture.get(1) {
            facts.push(FactAssertion {
                predicate: "date".to_owned(),
                value: FactValue::Date(full.as_str().replace('/', "-")),
                evidence: TextEvidence {
                    start: full.start(),
                    end: full.end(),
                    excerpt: full.as_str().to_owned(),
                },
                confidence: Confidence::new(1.0, Some(1.0), CalibrationState::Calibrated)?,
            });
        }
    }
    Ok(facts)
}

fn infer_relationships(
    entities: &[EntityAssertion],
) -> Result<Vec<RelationshipAssertion>, KnowledgeError> {
    let customers = entities
        .iter()
        .filter(|entity| entity.entity_type == "customer");
    let projects = entities
        .iter()
        .filter(|entity| entity.entity_type == "project")
        .collect::<Vec<_>>();
    let mut output = Vec::new();
    for customer in customers {
        for project in &projects {
            output.push(RelationshipAssertion {
                source_entity: project.canonical_name.clone(),
                predicate: "belongs_to_customer".to_owned(),
                target_entity: customer.canonical_name.clone(),
                confidence: Confidence::new(0.6, None, CalibrationState::Uncalibrated)?,
                evidence: vec![customer.evidence.clone(), project.evidence.clone()],
            });
        }
    }
    Ok(output)
}

fn normalize_entity_name(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_matches(|character: char| character.is_ascii_punctuation())
        .to_owned()
}

fn normalize_currency(value: &str) -> &'static str {
    match value.to_ascii_uppercase().as_str() {
        "$" | "USD" => "USD",
        "£" | "GBP" => "GBP",
        _ => "EUR",
    }
}

fn parse_money_minor(value: &str) -> Option<i64> {
    semantic::parse_decimal_minor(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_invoice_customer_and_amount_with_evidence() {
        let output = LocalSemanticAnalyzer
            .analyze(
                FileVersionId::new(),
                "invoice.pdf",
                Some("application/pdf"),
                "Facture N° INV-2042\nClient: ACME SAS\nTotal 1 250,00 EUR\n2026-08-09",
            )
            .unwrap_or_else(|error| panic!("analysis should succeed: {error}"));

        assert!(
            output
                .classifications
                .iter()
                .any(|item| item.class == SemanticClass::Invoice)
        );
        assert!(
            output
                .entities
                .iter()
                .any(|entity| entity.entity_type == "customer")
        );
        assert!(output.facts.iter().any(|fact| fact.predicate == "amount"));
        assert!(
            output
                .facts
                .iter()
                .all(|fact| !fact.evidence.excerpt.is_empty())
        );
    }

    #[test]
    fn calibrator_refuses_to_claim_calibration_without_samples() {
        let calibrator = BinaryCalibrator {
            version: "test".to_owned(),
            slope: 1.0,
            intercept: 0.0,
            trained_samples: 4,
            minimum_samples: 100,
        };
        let confidence = calibrator
            .calibrate(0.9, false)
            .unwrap_or_else(|error| panic!("calibration should produce a safe state: {error}"));
        assert_eq!(confidence.calibration, CalibrationState::Uncalibrated);
        assert_eq!(confidence.probability, None);
    }
}
