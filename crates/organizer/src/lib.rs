//! Deterministic organization compiler.

mod consumer;
mod organization;
mod path_safety;
mod rules;

pub use consumer::*;
pub use organization::*;
pub use path_safety::*;
pub use rules::*;

use domain::{
    ArtifactId, CalibrationState, Confidence, DestinationIntent, DisplayLabel, EvidenceLocator,
    EvidenceRef, FileId, FileVersionId, ProposalAction, ProposalId, ProposalItem, ProposalItemId,
    ProposalRevision, ReviewReason, ReviewState, RootId, ScanId, TaxonomyId, WorkspaceId,
};
use knowledge::{FactValue, SemanticClass, SemanticDocument};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaxonomyNode {
    pub stable_code: String,
    pub label: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaxonomyEdge {
    pub parent_code: String,
    pub child_code: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaxonomyVersion {
    pub id: TaxonomyId,
    pub version: u32,
    pub nodes: Vec<TaxonomyNode>,
    pub edges: Vec<TaxonomyEdge>,
    pub content_digest: [u8; 32],
}

#[derive(Debug, thiserror::Error)]
pub enum TaxonomyError {
    #[error("taxonomy contains a duplicate node")]
    DuplicateNode,
    #[error("taxonomy edge references an unknown node")]
    UnknownNode,
    #[error("taxonomy contains a cycle")]
    Cycle,
    #[error("taxonomy serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
}

impl TaxonomyVersion {
    pub fn new(
        version: u32,
        nodes: Vec<TaxonomyNode>,
        edges: Vec<TaxonomyEdge>,
    ) -> Result<Self, TaxonomyError> {
        let node_codes = nodes
            .iter()
            .map(|node| node.stable_code.as_str())
            .collect::<HashSet<_>>();
        if node_codes.len() != nodes.len() {
            return Err(TaxonomyError::DuplicateNode);
        }
        if edges.iter().any(|edge| {
            !node_codes.contains(edge.parent_code.as_str())
                || !node_codes.contains(edge.child_code.as_str())
        }) {
            return Err(TaxonomyError::UnknownNode);
        }
        ensure_acyclic(&node_codes, &edges)?;
        let encoded = serde_json::to_vec(&(version, &nodes, &edges))?;
        Ok(Self {
            id: TaxonomyId::new(),
            version,
            nodes,
            edges,
            content_digest: *blake3::hash(&encoded).as_bytes(),
        })
    }

    pub fn seeded() -> Result<Self, TaxonomyError> {
        let labels = [
            ("professional", "Professionnel"),
            ("personal", "Personnel"),
            ("accounting", "Comptabilité"),
            ("legal", "Juridique"),
            ("hr", "Ressources humaines"),
            ("customers", "Clients"),
            ("suppliers", "Fournisseurs"),
            ("projects", "Projets"),
            ("invoices", "Factures"),
            ("quotes", "Devis"),
            ("contracts", "Contrats"),
            ("photos", "Photos"),
            ("videos", "Vidéos"),
            ("screenshots", "Captures"),
            ("archives", "Archives"),
            ("to_review", "TO_REVIEW"),
        ];
        let nodes = labels
            .into_iter()
            .map(|(stable_code, label)| TaxonomyNode {
                stable_code: stable_code.to_owned(),
                label: label.to_owned(),
                description: format!("Catégorie locale {label}"),
            })
            .collect();
        let edges = [
            ("professional", "accounting"),
            ("professional", "legal"),
            ("professional", "hr"),
            ("professional", "customers"),
            ("professional", "suppliers"),
            ("professional", "projects"),
            ("accounting", "invoices"),
            ("professional", "quotes"),
            ("legal", "contracts"),
            ("personal", "photos"),
            ("personal", "videos"),
            ("personal", "screenshots"),
        ]
        .into_iter()
        .map(|(parent_code, child_code)| TaxonomyEdge {
            parent_code: parent_code.to_owned(),
            child_code: child_code.to_owned(),
        })
        .collect();
        Self::new(1, nodes, edges)
    }
}

fn ensure_acyclic(node_codes: &HashSet<&str>, edges: &[TaxonomyEdge]) -> Result<(), TaxonomyError> {
    let mut adjacency: HashMap<&str, Vec<&str>> = HashMap::new();
    for edge in edges {
        adjacency
            .entry(edge.parent_code.as_str())
            .or_default()
            .push(edge.child_code.as_str());
    }
    fn visit<'a>(
        node: &'a str,
        adjacency: &HashMap<&'a str, Vec<&'a str>>,
        visiting: &mut HashSet<&'a str>,
        visited: &mut HashSet<&'a str>,
    ) -> Result<(), TaxonomyError> {
        if visiting.contains(node) {
            return Err(TaxonomyError::Cycle);
        }
        if !visited.insert(node) {
            return Ok(());
        }
        visiting.insert(node);
        for child in adjacency.get(node).into_iter().flatten() {
            visit(child, adjacency, visiting, visited)?;
        }
        visiting.remove(node);
        Ok(())
    }
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    for node in node_codes {
        visit(node, &adjacency, &mut visiting, &mut visited)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrganizationInput {
    pub file_id: FileId,
    pub file_version_id: FileVersionId,
    pub display_label: DisplayLabel,
    pub artifact_id: ArtifactId,
    pub semantics: SemanticDocument,
    pub anomalies: Vec<ReviewReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OrganizationPolicy {
    pub calibrated_ready_threshold: f32,
}

impl Default for OrganizationPolicy {
    fn default() -> Self {
        Self {
            calibrated_ready_threshold: 0.9,
        }
    }
}

#[derive(Debug, Default)]
pub struct OrganizationEngine;

impl OrganizationEngine {
    pub fn propose(
        &self,
        workspace_id: WorkspaceId,
        root_id: RootId,
        scan_id: ScanId,
        inputs: &[OrganizationInput],
        policy: OrganizationPolicy,
        now_unix_ms: i64,
    ) -> ProposalRevision {
        let policy_digest = *blake3::hash(
            format!("organizer-v1:{:.4}", policy.calibrated_ready_threshold).as_bytes(),
        )
        .as_bytes();
        let items = inputs
            .iter()
            .map(|input| compile_item(root_id, input, policy))
            .collect();

        ProposalRevision {
            id: ProposalId::new(),
            workspace_id,
            base_scan_id: scan_id,
            revision: 1,
            policy_digest,
            items,
            created_at_unix_ms: now_unix_ms,
        }
    }
}

fn compile_item(
    root_id: RootId,
    input: &OrganizationInput,
    policy: OrganizationPolicy,
) -> ProposalItem {
    let primary = input
        .semantics
        .classifications
        .iter()
        .max_by(|left, right| {
            left.confidence
                .raw_score
                .partial_cmp(&right.confidence.raw_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    let confidence = primary.map_or_else(
        || {
            Confidence::new(0.0, None, CalibrationState::OutOfDistribution).unwrap_or(Confidence {
                raw_score: 0.0,
                probability: None,
                calibration: CalibrationState::OutOfDistribution,
            })
        },
        |classification| classification.confidence,
    );
    let semantic_class = primary.map_or(SemanticClass::Unknown, |value| value.class);
    let destination = destination_for(root_id, input, semantic_class);
    let mut uncertainty = input.anomalies.clone();
    if confidence.calibration == CalibrationState::Uncalibrated {
        uncertainty.push(ReviewReason::Uncalibrated);
    } else if confidence.calibration == CalibrationState::OutOfDistribution {
        uncertainty.push(ReviewReason::OutOfDistribution);
    } else if !confidence.is_eligible(policy.calibrated_ready_threshold) {
        uncertainty.push(ReviewReason::LowConfidence);
    }

    let action = if semantic_class == SemanticClass::Unknown {
        ProposalAction::PlaceInReview { destination }
    } else {
        ProposalAction::Move { destination }
    };
    let review_state = if !input.anomalies.is_empty() {
        ReviewState::Blocked
    } else if uncertainty.is_empty() {
        ReviewState::Ready
    } else {
        ReviewState::ToReview
    };
    let evidence = primary
        .into_iter()
        .flat_map(|classification| classification.evidence.iter())
        .map(|source| EvidenceRef {
            artifact_id: input.artifact_id,
            file_version_id: input.file_version_id,
            display_label: input.display_label.clone(),
            locator: EvidenceLocator::Text {
                start: source.start,
                end: source.end,
                line_start: None,
                line_end: None,
            },
            excerpt: source.excerpt.clone(),
            excerpt_digest: *blake3::hash(source.excerpt.as_bytes()).as_bytes(),
            explanation: Some("indice utilisé par le classifieur local".to_owned()),
        })
        .collect();

    ProposalItem {
        id: ProposalItemId::new(),
        file_id: input.file_id,
        expected_file_version_id: input.file_version_id,
        action,
        review_state,
        confidence,
        rationale: rationale(semantic_class, review_state),
        evidence,
        counter_evidence: Vec::new(),
        uncertainty_reasons: uncertainty,
        alternatives: vec![ProposalAction::PlaceInReview {
            destination: DestinationIntent {
                root_id,
                folder_components: vec!["TO_REVIEW".to_owned()],
                file_name: input.display_label.as_str().to_owned(),
            },
        }],
    }
}

fn destination_for(
    root_id: RootId,
    input: &OrganizationInput,
    semantic_class: SemanticClass,
) -> DestinationIntent {
    let year = input
        .semantics
        .facts
        .iter()
        .find_map(|fact| match &fact.value {
            FactValue::Date(value) => value.get(..4).map(str::to_owned),
            _ => None,
        })
        .unwrap_or_else(|| "Sans date".to_owned());
    let customer = input
        .semantics
        .entities
        .iter()
        .find(|entity| matches!(entity.entity_type.as_str(), "customer" | "organization"))
        .map(|entity| entity.canonical_name.clone());
    let project = input
        .semantics
        .entities
        .iter()
        .find(|entity| entity.entity_type == "project")
        .map(|entity| entity.canonical_name.clone());

    let mut folder_components = match semantic_class {
        SemanticClass::Invoice => vec![
            "Professionnel".to_owned(),
            "Comptabilité".to_owned(),
            "Factures".to_owned(),
            year,
        ],
        SemanticClass::Quote => vec![
            "Professionnel".to_owned(),
            "Commercial".to_owned(),
            "Devis".to_owned(),
            year,
        ],
        SemanticClass::Contract => vec![
            "Professionnel".to_owned(),
            "Juridique".to_owned(),
            "Contrats".to_owned(),
        ],
        SemanticClass::HumanResources => {
            vec!["Professionnel".to_owned(), "Ressources humaines".to_owned()]
        }
        SemanticClass::Accounting => {
            vec!["Professionnel".to_owned(), "Comptabilité".to_owned()]
        }
        SemanticClass::Legal => vec!["Professionnel".to_owned(), "Juridique".to_owned()],
        SemanticClass::Administrative => vec!["Administratif".to_owned()],
        SemanticClass::Photo => vec!["Personnel".to_owned(), "Photos".to_owned(), year],
        SemanticClass::Video => vec!["Personnel".to_owned(), "Vidéos".to_owned(), year],
        SemanticClass::Screenshot => vec!["Personnel".to_owned(), "Captures".to_owned(), year],
        SemanticClass::Archive => vec!["Archives".to_owned(), year],
        SemanticClass::CustomerRecord => vec!["Professionnel".to_owned(), "Clients".to_owned()],
        SemanticClass::SupplierRecord => {
            vec!["Professionnel".to_owned(), "Fournisseurs".to_owned()]
        }
        SemanticClass::Personal => vec!["Personnel".to_owned()],
        SemanticClass::Professional => vec!["Professionnel".to_owned()],
        SemanticClass::Unknown => vec!["TO_REVIEW".to_owned()],
    };
    if let Some(customer) = customer {
        folder_components.push(customer);
    }
    if let Some(project) = project {
        folder_components.push(project);
    }
    DestinationIntent {
        root_id,
        folder_components,
        file_name: input.display_label.as_str().to_owned(),
    }
}

fn rationale(class: SemanticClass, review_state: ReviewState) -> String {
    let label = match class {
        SemanticClass::Invoice => "facture",
        SemanticClass::Quote => "devis",
        SemanticClass::Contract => "contrat",
        SemanticClass::CustomerRecord => "dossier client",
        SemanticClass::SupplierRecord => "dossier fournisseur",
        SemanticClass::HumanResources => "ressources humaines",
        SemanticClass::Accounting => "comptabilité",
        SemanticClass::Legal => "juridique",
        SemanticClass::Administrative => "administratif",
        SemanticClass::Photo => "photo",
        SemanticClass::Video => "vidéo",
        SemanticClass::Screenshot => "capture d’écran",
        SemanticClass::Archive => "archive",
        SemanticClass::Personal => "personnel",
        SemanticClass::Professional => "professionnel",
        SemanticClass::Unknown => "type inconnu",
    };
    if review_state == ReviewState::Ready {
        format!("Classé comme {label} par une preuve calibrée.")
    } else {
        format!("Classé comme {label}, avec validation humaine requise.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use knowledge::{ClassificationAssertion, SemanticDocument};

    #[test]
    fn uncalibrated_semantics_always_go_to_review() {
        let file_version_id = FileVersionId::new();
        let input = OrganizationInput {
            file_id: FileId::new(),
            file_version_id,
            display_label: DisplayLabel::new("invoice.pdf")
                .unwrap_or_else(|error| panic!("label should be valid: {error}")),
            artifact_id: ArtifactId::new(),
            semantics: SemanticDocument {
                file_version_id,
                classifications: vec![ClassificationAssertion {
                    class: SemanticClass::Invoice,
                    confidence: Confidence::new(0.99, None, CalibrationState::Uncalibrated)
                        .unwrap_or_else(|error| panic!("confidence should be valid: {error}")),
                    evidence: Vec::new(),
                    calibrator_version: None,
                }],
                entities: Vec::new(),
                facts: Vec::new(),
                relationships: Vec::new(),
                pipeline_version: "test".to_owned(),
            },
            anomalies: Vec::new(),
        };
        let proposal = OrganizationEngine.propose(
            WorkspaceId::new(),
            RootId::new(),
            ScanId::new(),
            &[input],
            OrganizationPolicy::default(),
            1,
        );
        assert_eq!(proposal.items[0].review_state, ReviewState::ToReview);
    }

    #[test]
    fn taxonomy_versions_reject_cycles() {
        let nodes = ["a", "b"]
            .into_iter()
            .map(|code| TaxonomyNode {
                stable_code: code.to_owned(),
                label: code.to_owned(),
                description: code.to_owned(),
            })
            .collect();
        let edges = vec![
            TaxonomyEdge {
                parent_code: "a".to_owned(),
                child_code: "b".to_owned(),
            },
            TaxonomyEdge {
                parent_code: "b".to_owned(),
                child_code: "a".to_owned(),
            },
        ];
        assert!(matches!(
            TaxonomyVersion::new(1, nodes, edges),
            Err(TaxonomyError::Cycle)
        ));
    }
}
