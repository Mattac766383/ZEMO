use crate::{MatchSource, QueryInterpretation, SearchResult, normalize_search_text};
use std::{cmp::Ordering, collections::HashSet};

#[derive(Debug, Clone, PartialEq)]
pub struct RankedSemanticFact {
    pub value: String,
    pub confidence: f32,
    pub user_confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedAmountFact {
    pub amount_minor: i64,
    pub currency: Option<String>,
    pub user_confirmed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RankedDateFact {
    pub iso_date: String,
    pub user_confirmed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RankedRelationshipFact {
    pub relationship_type: String,
    pub display_name: String,
    pub confidence: f32,
    pub user_confirmed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HybridCandidate {
    pub result: SearchResult,
    pub lexical_score: Option<f64>,
    pub document_type: Option<RankedSemanticFact>,
    pub context: Option<RankedSemanticFact>,
    pub semantic_status: Option<String>,
    pub semantic_confidence: Option<f32>,
    pub amounts: Vec<RankedAmountFact>,
    pub dates: Vec<RankedDateFact>,
    pub relationships: Vec<RankedRelationshipFact>,
    pub vector_similarity: Option<f32>,
    pub explicit_rule_boost: f64,
    pub explicit_rule_reasons: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HybridRankingPolicy {
    pub lexical_weight: f64,
    pub document_type_weight: f64,
    pub context_weight: f64,
    pub relationship_weight: f64,
    pub amount_weight: f64,
    pub date_weight: f64,
    pub vector_weight: f64,
    pub confirmed_bonus: f64,
    pub confirmed_mismatch_penalty: f64,
    pub minimum_vector_similarity: f32,
    pub minimum_result_score: f64,
}

impl Default for HybridRankingPolicy {
    fn default() -> Self {
        Self {
            lexical_weight: 0.34,
            document_type_weight: 0.20,
            context_weight: 0.08,
            relationship_weight: 0.24,
            amount_weight: 0.18,
            date_weight: 0.12,
            vector_weight: 0.10,
            confirmed_bonus: 0.08,
            confirmed_mismatch_penalty: 0.80,
            minimum_vector_similarity: 0.18,
            minimum_result_score: 0.035,
        }
    }
}

#[must_use]
pub fn rank_hybrid_candidates(
    candidates: Vec<HybridCandidate>,
    interpretation: &QueryInterpretation,
    policy: HybridRankingPolicy,
) -> Vec<SearchResult> {
    let has_query = !interpretation.lexical_text.is_empty()
        || interpretation.document_type.is_some()
        || interpretation.context.is_some()
        || interpretation.supplier.is_some()
        || interpretation.customer.is_some()
        || interpretation.project.is_some()
        || interpretation.party.is_some()
        || interpretation.amount.is_some()
        || interpretation.date.is_some();
    let mut ranked = candidates
        .into_iter()
        .filter_map(|candidate| score_candidate(candidate, interpretation, policy, has_query))
        .collect::<Vec<_>>();
    ranked.sort_by(|left, right| {
        right
            .relevance
            .partial_cmp(&left.relevance)
            .unwrap_or(Ordering::Equal)
            .then_with(|| {
                left.filename
                    .to_lowercase()
                    .cmp(&right.filename.to_lowercase())
            })
            .then_with(|| left.file_id.cmp(&right.file_id))
    });
    ranked
}

fn score_candidate(
    mut candidate: HybridCandidate,
    interpretation: &QueryInterpretation,
    policy: HybridRankingPolicy,
    has_query: bool,
) -> Option<SearchResult> {
    let mut score = candidate.lexical_score.unwrap_or(0.0).clamp(0.0, 1.0) * policy.lexical_weight;
    let mut explanations = Vec::new();
    let mut matched = candidate.lexical_score.is_some_and(|value| value > 0.0);
    let mut confirmed_mismatch = false;

    if matched {
        explanations.push(match candidate.result.match_source {
            MatchSource::Filename => "nom de fichier correspondant".to_owned(),
            MatchSource::Path => "emplacement correspondant".to_owned(),
            MatchSource::Metadata => "métadonnées correspondantes".to_owned(),
            _ => "texte du document correspondant".to_owned(),
        });
    }

    if let Some(expected) = interpretation.document_type.as_deref()
        && let Some(actual) = candidate.document_type.as_ref()
    {
        if actual.value == expected {
            score += policy.document_type_weight * f64::from(actual.confidence.clamp(0.0, 1.0))
                + if actual.user_confirmed {
                    policy.confirmed_bonus
                } else {
                    0.0
                };
            matched = true;
            explanations.push(format!(
                "type de document correspondant : {}{}",
                document_type_label(expected),
                confirmation_suffix(actual.user_confirmed)
            ));
            if candidate.lexical_score.is_none() {
                candidate.result.match_source = MatchSource::Structured;
            }
        } else if actual.user_confirmed {
            confirmed_mismatch = true;
        }
    }

    if let Some(expected) = interpretation.context.as_deref()
        && let Some(actual) = candidate.context.as_ref()
    {
        if actual.value == expected {
            score += policy.context_weight * f64::from(actual.confidence.clamp(0.0, 1.0))
                + if actual.user_confirmed {
                    policy.confirmed_bonus
                } else {
                    0.0
                };
            matched = true;
            explanations.push(format!(
                "contexte {}{}",
                context_label(expected),
                confirmation_suffix(actual.user_confirmed)
            ));
        } else if actual.user_confirmed {
            confirmed_mismatch = true;
        }
    }

    for (kind, expected) in [
        ("file_supplier", interpretation.supplier.as_deref()),
        ("file_customer", interpretation.customer.as_deref()),
        ("file_project", interpretation.project.as_deref()),
    ] {
        let Some(expected) = expected else {
            continue;
        };
        let (relation_score, relation_explanation, mismatch) =
            relationship_match(&candidate.relationships, kind, expected, policy);
        score += relation_score;
        matched |= relation_score > 0.0;
        confirmed_mismatch |= mismatch;
        if let Some(explanation) = relation_explanation {
            explanations.push(explanation);
            if candidate.lexical_score.is_none() {
                candidate.result.match_source = MatchSource::Relationship;
            }
        }
    }

    if let Some(expected) = interpretation.party.as_deref() {
        let (relation_score, relation_explanation, mismatch) =
            generic_party_match(&candidate.relationships, expected, policy);
        score += relation_score;
        matched |= relation_score > 0.0;
        // Generic party intent is inferred heuristically from free text. It
        // must not erase an exact lexical filename/path/content match merely
        // because that file has a different confirmed relationship.
        confirmed_mismatch |= mismatch && candidate.lexical_score.is_none();
        if let Some(explanation) = relation_explanation {
            explanations.push(explanation);
            if candidate.lexical_score.is_none() {
                candidate.result.match_source = MatchSource::Relationship;
            }
        }
    }

    if let Some(amount) = interpretation.amount.as_ref() {
        let mut best = None::<(&RankedAmountFact, f64)>;
        let has_confirmed = candidate.amounts.iter().any(|fact| fact.user_confirmed);
        for fact in candidate
            .amounts
            .iter()
            .filter(|fact| !has_confirmed || fact.user_confirmed)
        {
            if amount.currency.as_ref().is_some_and(|expected| {
                fact.currency
                    .as_ref()
                    .is_some_and(|actual| actual != expected)
            }) {
                continue;
            }
            let similarity = amount_similarity(fact.amount_minor, amount);
            if best.is_none_or(|(_, current)| similarity > current) {
                best = Some((fact, similarity));
            }
        }
        if let Some((fact, similarity)) = best
            && similarity > 0.0
        {
            score += policy.amount_weight * similarity
                + if fact.user_confirmed {
                    policy.confirmed_bonus
                } else {
                    0.0
                };
            matched = true;
            explanations.push(format!(
                "montant {}{}",
                amount_explanation(fact.amount_minor, amount.target_minor),
                confirmation_suffix(fact.user_confirmed)
            ));
            if candidate.lexical_score.is_none() {
                candidate.result.match_source = MatchSource::Structured;
            }
        } else if has_confirmed {
            confirmed_mismatch = true;
        }
    }

    if let Some(date) = interpretation.date.as_ref() {
        let has_confirmed = candidate.dates.iter().any(|fact| fact.user_confirmed);
        if let Some(fact) = candidate
            .dates
            .iter()
            .filter(|fact| !has_confirmed || fact.user_confirmed)
            .find(|fact| fact.iso_date >= date.from && fact.iso_date <= date.to)
        {
            score += policy.date_weight
                + if fact.user_confirmed {
                    policy.confirmed_bonus
                } else {
                    0.0
                };
            matched = true;
            explanations.push(format!(
                "date correspondant à {}{}",
                if date.month.is_some() {
                    &date.from[..7]
                } else {
                    &date.from[..4]
                },
                confirmation_suffix(fact.user_confirmed)
            ));
            if candidate.lexical_score.is_none() {
                candidate.result.match_source = MatchSource::Structured;
            }
        } else if has_confirmed {
            confirmed_mismatch = true;
        }
    }

    if let Some(similarity) = candidate.vector_similarity
        && similarity >= policy.minimum_vector_similarity
    {
        let normalized = f64::from(
            ((similarity - policy.minimum_vector_similarity)
                / (1.0 - policy.minimum_vector_similarity))
                .clamp(0.0, 1.0),
        );
        score += policy.vector_weight * normalized;
        matched = true;
        if candidate.result.snippet.trim().is_empty() {
            explanations.push("similarité sémantique locale".to_owned());
        } else {
            let preview = candidate
                .result
                .snippet
                .chars()
                .take(120)
                .collect::<String>();
            explanations.push(format!(
                "correspondance sémantique dans cette section : {preview}"
            ));
        }
        if candidate.lexical_score.is_none() {
            candidate.result.match_source = MatchSource::Semantic;
        }
    }

    if (!has_query || matched) && candidate.explicit_rule_boost > 0.0 {
        score += candidate.explicit_rule_boost.clamp(0.0, 0.25);
        explanations.extend(
            candidate
                .explicit_rule_reasons
                .into_iter()
                .map(|value| value.chars().take(512).collect::<String>()),
        );
    }
    if confirmed_mismatch {
        score -= policy.confirmed_mismatch_penalty;
    }
    if has_query && (!matched || score < policy.minimum_result_score || score <= 0.0) {
        return None;
    }

    let mut seen = HashSet::new();
    explanations.retain(|explanation| seen.insert(explanation.clone()));
    explanations.truncate(5);
    candidate.result.relevance = score.clamp(0.0, 1.0);
    candidate.result.why_matched = explanations;
    Some(candidate.result)
}

fn relationship_match(
    relationships: &[RankedRelationshipFact],
    expected_kind: &str,
    expected_name: &str,
    policy: HybridRankingPolicy,
) -> (f64, Option<String>, bool) {
    let expected = normalize_search_text(expected_name);
    let same_kind = relationships
        .iter()
        .filter(|relationship| relationship.relationship_type == expected_kind)
        .collect::<Vec<_>>();
    let has_confirmed = same_kind
        .iter()
        .any(|relationship| relationship.user_confirmed);
    let matched = same_kind.iter().find(|relationship| {
        (!has_confirmed || relationship.user_confirmed)
            && relationship_name_matches(&relationship.display_name, &expected)
    });
    if let Some(relationship) = matched {
        let base = policy.relationship_weight * f64::from(relationship.confidence.max(0.7));
        let score = base
            + if relationship.user_confirmed {
                policy.confirmed_bonus
            } else {
                0.0
            };
        let label = match expected_kind {
            "file_supplier" => "fournisseur",
            "file_customer" => "client",
            "file_project" => "projet",
            _ => "relation",
        };
        return (
            score,
            Some(format!(
                "{label} correspondant : {}{}",
                relationship.display_name,
                confirmation_suffix(relationship.user_confirmed)
            )),
            false,
        );
    }
    (0.0, None, has_confirmed)
}

fn generic_party_match(
    relationships: &[RankedRelationshipFact],
    expected_name: &str,
    policy: HybridRankingPolicy,
) -> (f64, Option<String>, bool) {
    let expected = normalize_search_text(expected_name);
    let relevant = relationships
        .iter()
        .filter(|relationship| {
            matches!(
                relationship.relationship_type.as_str(),
                "file_supplier" | "file_customer" | "file_project" | "semantic_party"
            )
        })
        .collect::<Vec<_>>();
    let has_confirmed = relevant
        .iter()
        .any(|relationship| relationship.user_confirmed);
    relevant
        .into_iter()
        .filter(|relationship| !has_confirmed || relationship.user_confirmed)
        .find_map(|relationship| {
            relationship_name_matches(&relationship.display_name, &expected).then(|| {
                (
                    policy.relationship_weight * f64::from(relationship.confidence.max(0.65))
                        + if relationship.user_confirmed {
                            policy.confirmed_bonus
                        } else {
                            0.0
                        },
                    Some(format!(
                        "identité correspondante : {}{}",
                        relationship.display_name,
                        confirmation_suffix(relationship.user_confirmed)
                    )),
                    false,
                )
            })
        })
        .unwrap_or((0.0, None, has_confirmed))
}

fn relationship_name_matches(actual: &str, normalized_expected: &str) -> bool {
    let actual = normalize_search_text(actual);
    actual == normalized_expected
        || actual.contains(normalized_expected)
        || normalized_expected.contains(&actual)
}

fn amount_similarity(actual: i64, expected: &crate::AmountIntent) -> f64 {
    if expected
        .minimum_minor
        .is_some_and(|minimum| actual < minimum)
        || expected
            .maximum_minor
            .is_some_and(|maximum| actual > maximum)
    {
        return 0.0;
    }
    expected.target_minor.map_or(1.0, |target| {
        let distance = actual.abs_diff(target) as f64;
        let scale = target.unsigned_abs().max(10_000) as f64;
        (1.0 - distance / scale).clamp(0.0, 1.0)
    })
}

fn amount_explanation(actual_minor: i64, target_minor: Option<i64>) -> String {
    let amount = format_currency(actual_minor);
    if target_minor.is_some_and(|target| target != actual_minor) {
        format!("proche de la recherche ({amount})")
    } else {
        format!("correspondant ({amount})")
    }
}

fn format_currency(minor: i64) -> String {
    let major = minor / 100;
    let fraction = minor.unsigned_abs() % 100;
    if fraction == 0 {
        format!("{major} €")
    } else {
        format!("{major},{fraction:02} €")
    }
}

fn confirmation_suffix(confirmed: bool) -> &'static str {
    if confirmed { " (confirmé)" } else { "" }
}

fn document_type_label(value: &str) -> &str {
    match value {
        "invoice" => "Facture",
        "quote" => "Devis",
        "contract" => "Contrat",
        "photo" => "Photo",
        "administrative_document" => "Document administratif",
        _ => value,
    }
}

fn context_label(value: &str) -> &str {
    match value {
        "personal" => "personnel",
        "business" => "professionnel",
        "mixed" => "mixte",
        _ => value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{EmbeddingSearchStatus, SearchPage, SearchTimings};

    fn candidate(name: &str) -> HybridCandidate {
        HybridCandidate {
            result: SearchResult {
                file_id: name.to_owned(),
                filename: name.to_owned(),
                relative_path: name.to_owned(),
                detected_type: Some("application/pdf".to_owned()),
                extension: Some("pdf".to_owned()),
                byte_size: 10,
                modified_at: None,
                extraction_status: Some("success".to_owned()),
                ocr_status: Some("not_used".to_owned()),
                duplicate: false,
                match_source: MatchSource::Content,
                relevance: 0.0,
                snippet: String::new(),
                why_matched: Vec::new(),
            },
            lexical_score: None,
            document_type: None,
            context: None,
            semantic_status: Some("success".to_owned()),
            semantic_confidence: Some(0.9),
            amounts: Vec::new(),
            dates: Vec::new(),
            relationships: Vec::new(),
            vector_similarity: None,
            explicit_rule_boost: 0.0,
            explicit_rule_reasons: Vec::new(),
        }
    }

    #[test]
    fn confirmed_structured_facts_outrank_vector_neighbors() {
        let mut correct = candidate("poor-name.pdf");
        correct.document_type = Some(RankedSemanticFact {
            value: "invoice".to_owned(),
            confidence: 0.98,
            user_confirmed: true,
        });
        correct.relationships.push(RankedRelationshipFact {
            relationship_type: "file_supplier".to_owned(),
            display_name: "Point P".to_owned(),
            confidence: 1.0,
            user_confirmed: true,
        });
        correct.amounts.push(RankedAmountFact {
            amount_minor: 140_000,
            currency: Some("EUR".to_owned()),
            user_confirmed: true,
        });
        correct.vector_similarity = Some(0.45);

        let mut neighbor = candidate("semantic-neighbor.pdf");
        neighbor.document_type = Some(RankedSemanticFact {
            value: "invoice".to_owned(),
            confidence: 0.9,
            user_confirmed: false,
        });
        neighbor.relationships.push(RankedRelationshipFact {
            relationship_type: "file_supplier".to_owned(),
            display_name: "Point P".to_owned(),
            confidence: 0.9,
            user_confirmed: false,
        });
        neighbor.relationships.push(RankedRelationshipFact {
            relationship_type: "file_supplier".to_owned(),
            display_name: "Autre fournisseur".to_owned(),
            confidence: 1.0,
            user_confirmed: true,
        });
        neighbor.vector_similarity = Some(0.99);

        let interpretation = QueryInterpretation {
            document_type: Some("invoice".to_owned()),
            supplier: Some("point p".to_owned()),
            amount: Some(crate::AmountIntent {
                minimum_minor: Some(126_000),
                maximum_minor: Some(154_000),
                target_minor: Some(140_000),
                currency: Some("EUR".to_owned()),
                approximate: true,
            }),
            ..QueryInterpretation::default()
        };
        let ranked = rank_hybrid_candidates(
            vec![neighbor, correct],
            &interpretation,
            HybridRankingPolicy::default(),
        );
        assert_eq!(ranked[0].filename, "poor-name.pdf");
        assert!(
            !ranked
                .iter()
                .any(|result| result.filename == "semantic-neighbor.pdf")
        );
    }

    #[test]
    fn explicit_rule_match_adds_bounded_explained_boost() {
        let mut preferred = candidate("preferred.pdf");
        preferred.lexical_score = Some(0.5);
        preferred.explicit_rule_boost = 0.15;
        preferred.explicit_rule_reasons =
            vec!["Matched your rule: Point P invoices stay in projects.".to_owned()];
        let mut ordinary = candidate("ordinary.pdf");
        ordinary.lexical_score = Some(0.5);
        let interpretation = QueryInterpretation {
            lexical_text: "invoice".to_owned(),
            ..QueryInterpretation::default()
        };

        let ranked = rank_hybrid_candidates(
            vec![ordinary, preferred],
            &interpretation,
            HybridRankingPolicy::default(),
        );
        assert_eq!(ranked[0].filename, "preferred.pdf");
        assert!(ranked[0].relevance <= 1.0);
        assert!(
            ranked[0]
                .why_matched
                .iter()
                .any(|reason| reason.starts_with("Matched your rule:"))
        );
    }

    #[test]
    fn confirmed_identity_mismatch_overrides_stale_machine_match() {
        let mut stale = candidate("stale-machine-link.pdf");
        stale.vector_similarity = Some(0.99);
        stale.relationships.extend([
            RankedRelationshipFact {
                relationship_type: "file_supplier".to_owned(),
                display_name: "Point P".to_owned(),
                confidence: 0.95,
                user_confirmed: false,
            },
            RankedRelationshipFact {
                relationship_type: "file_supplier".to_owned(),
                display_name: "Confirmed Other".to_owned(),
                confidence: 1.0,
                user_confirmed: true,
            },
        ]);
        let interpretation = QueryInterpretation {
            party: Some("Point P".to_owned()),
            ..QueryInterpretation::default()
        };

        let ranked =
            rank_hybrid_candidates(vec![stale], &interpretation, HybridRankingPolicy::default());
        assert!(ranked.is_empty());
    }

    #[test]
    fn confirmed_amount_correction_overrides_stale_machine_entity() {
        let mut stale = candidate("stale-amount.pdf");
        stale.vector_similarity = Some(0.99);
        stale.amounts.extend([
            RankedAmountFact {
                amount_minor: 140_000,
                currency: Some("EUR".to_owned()),
                user_confirmed: false,
            },
            RankedAmountFact {
                amount_minor: 160_000,
                currency: Some("EUR".to_owned()),
                user_confirmed: true,
            },
        ]);
        let interpretation = QueryInterpretation {
            amount: Some(crate::AmountIntent {
                minimum_minor: Some(126_000),
                maximum_minor: Some(154_000),
                target_minor: Some(140_000),
                currency: Some("EUR".to_owned()),
                approximate: true,
            }),
            ..QueryInterpretation::default()
        };

        let ranked =
            rank_hybrid_candidates(vec![stale], &interpretation, HybridRankingPolicy::default());
        assert!(ranked.is_empty());
    }

    #[test]
    fn inferred_party_mismatch_does_not_hide_an_exact_lexical_match() {
        let mut lexical = candidate("j-stale-two.txt");
        lexical.lexical_score = Some(0.9);
        lexical.relationships.push(RankedRelationshipFact {
            relationship_type: "file_supplier".to_owned(),
            display_name: "Confirmed Supplier".to_owned(),
            confidence: 1.0,
            user_confirmed: true,
        });
        let interpretation = QueryInterpretation {
            lexical_text: "j-stale-two.txt".to_owned(),
            party: Some("j-stale-two.txt".to_owned()),
            ..QueryInterpretation::default()
        };

        let ranked = rank_hybrid_candidates(
            vec![lexical],
            &interpretation,
            HybridRankingPolicy::default(),
        );
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].filename, "j-stale-two.txt");
    }

    #[test]
    fn page_support_types_remain_constructible() {
        let page = SearchPage {
            query: String::new(),
            page: 0,
            page_size: 50,
            total: 0,
            has_more: false,
            results: Vec::new(),
            interpreted_query: Vec::new(),
            embeddings: EmbeddingSearchStatus::default(),
            timings: SearchTimings::default(),
        };
        assert!(page.results.is_empty());
    }
}
