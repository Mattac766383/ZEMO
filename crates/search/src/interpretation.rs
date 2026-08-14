use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

const MAX_QUERY_CHARS: usize = 512;
const MAX_RELATION_CHARS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryClock {
    pub year: i32,
    pub month: u8,
    pub day: u8,
}

impl QueryClock {
    #[must_use]
    pub const fn new(year: i32, month: u8, day: u8) -> Self {
        Self { year, month, day }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryChip {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AmountIntent {
    pub minimum_minor: Option<i64>,
    pub maximum_minor: Option<i64>,
    pub target_minor: Option<i64>,
    pub currency: Option<String>,
    pub approximate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DateIntent {
    pub from: String,
    pub to: String,
    pub year: i32,
    pub month: Option<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryInterpretation {
    pub lexical_text: String,
    pub document_type: Option<String>,
    pub context: Option<String>,
    pub supplier: Option<String>,
    pub customer: Option<String>,
    pub project: Option<String>,
    pub party: Option<String>,
    pub amount: Option<AmountIntent>,
    pub date: Option<DateIntent>,
    pub chips: Vec<QueryChip>,
}

#[must_use]
pub fn interpret_query(
    query: &str,
    clock: QueryClock,
    disabled_intents: &[String],
) -> QueryInterpretation {
    let bounded = query.chars().take(MAX_QUERY_CHARS).collect::<String>();
    let normalized = normalize_search_text(&bounded);
    let disabled = disabled_intents
        .iter()
        .map(|value| value.to_ascii_lowercase())
        .collect::<HashSet<_>>();
    let mut interpretation = QueryInterpretation::default();
    let mut consumed = Vec::<String>::new();

    if !disabled.contains("document_type")
        && let Some((value, label, aliases)) = parse_document_type(&normalized)
    {
        interpretation.document_type = Some(value.to_owned());
        interpretation.chips.push(QueryChip {
            id: "document_type".to_owned(),
            kind: "document_type".to_owned(),
            label: label.to_owned(),
            value: value.to_owned(),
        });
        consumed.extend(aliases.iter().map(|value| (*value).to_owned()));
    }

    if !disabled.contains("context")
        && let Some((value, label, aliases)) = parse_context(&normalized)
    {
        interpretation.context = Some(value.to_owned());
        interpretation.chips.push(QueryChip {
            id: "context".to_owned(),
            kind: "context".to_owned(),
            label: label.to_owned(),
            value: value.to_owned(),
        });
        consumed.extend(aliases.iter().map(|value| (*value).to_owned()));
    }

    if !disabled.contains("amount")
        && let Some((amount, matched)) = parse_amount(&normalized)
    {
        interpretation.chips.push(QueryChip {
            id: "amount".to_owned(),
            kind: "amount".to_owned(),
            label: amount_label(&amount),
            value: amount
                .target_minor
                .or(amount.minimum_minor)
                .unwrap_or_default()
                .to_string(),
        });
        interpretation.amount = Some(amount);
        consumed.push(matched);
    }

    if !disabled.contains("date")
        && let Some((date, matched)) = parse_date(&normalized, clock)
    {
        interpretation.chips.push(QueryChip {
            id: "date".to_owned(),
            kind: "date".to_owned(),
            label: date_label(&date),
            value: date.from.clone(),
        });
        interpretation.date = Some(date);
        consumed.push(matched);
    }

    for (kind, value, matched) in parse_explicit_relationships(&normalized) {
        if disabled.contains(kind) {
            continue;
        }
        let value = bounded_relation(&value);
        if value.is_empty() {
            continue;
        }
        match kind {
            "supplier" if interpretation.supplier.is_none() => {
                interpretation.supplier = Some(value.clone());
            }
            "customer" if interpretation.customer.is_none() => {
                interpretation.customer = Some(value.clone());
            }
            "project" if interpretation.project.is_none() => {
                interpretation.project = Some(value.clone());
            }
            _ => continue,
        }
        interpretation.chips.push(QueryChip {
            id: kind.to_owned(),
            kind: kind.to_owned(),
            label: relationship_label(kind, &value),
            value,
        });
        consumed.push(matched);
    }

    let party_remainder = lexical_remainder(&normalized, &consumed);
    let mut lexical_text = party_remainder.clone();
    if lexical_text.is_empty() && !normalized.is_empty() {
        lexical_text = lexical_remainder(&normalized, &[]);
    }
    if !disabled.contains("party")
        && interpretation.supplier.is_none()
        && interpretation.customer.is_none()
        && !looks_like_file_query(&normalized)
        && let Some(party) = infer_party(&party_remainder)
    {
        interpretation.party = Some(party.clone());
        interpretation.chips.push(QueryChip {
            id: "party".to_owned(),
            kind: "party".to_owned(),
            label: party.clone(),
            value: party,
        });
    }
    interpretation.lexical_text = lexical_text;
    interpretation
}

#[must_use]
pub fn normalize_search_text(value: &str) -> String {
    let deaccented = value
        .nfkd()
        .filter(|character| !is_combining_mark(*character))
        .flat_map(char::to_lowercase)
        .map(|character| match character {
            '’' | '‘' | '`' => '\'',
            '\u{00a0}' | '\u{202f}' => ' ',
            _ => character,
        })
        .collect::<String>();
    deaccented
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_QUERY_CHARS)
        .collect()
}

fn parse_document_type(
    value: &str,
) -> Option<(&'static str, &'static str, &'static [&'static str])> {
    const DOCUMENT_TYPES: &[(&str, &str, &[&str])] = &[
        (
            "invoice",
            "Facture",
            &["facture", "factures", "invoice", "invoices"],
        ),
        ("quote", "Devis", &["devis", "quotation", "estimate"]),
        (
            "contract",
            "Contrat",
            &["contrat", "contrats", "contract", "contracts"],
        ),
        (
            "purchase_order",
            "Bon de commande",
            &["bon de commande", "bons de commande", "purchase order"],
        ),
        (
            "delivery_note",
            "Bon de livraison",
            &["bon de livraison", "delivery note"],
        ),
        (
            "bank_statement",
            "Relevé bancaire",
            &["releve bancaire", "releves bancaires", "bank statement"],
        ),
        (
            "tax_document",
            "Document fiscal",
            &[
                "document fiscal",
                "documents fiscaux",
                "impot",
                "tax document",
            ],
        ),
        (
            "payslip",
            "Fiche de paie",
            &["fiche de paie", "bulletin de paie", "payslip"],
        ),
        (
            "administrative_document",
            "Document administratif",
            &["document administratif", "documents administratifs"],
        ),
        (
            "receipt",
            "Reçu",
            &["recu", "recus", "ticket de caisse", "receipt"],
        ),
        ("report", "Rapport", &["rapport", "rapports", "report"]),
        ("letter", "Courrier", &["courrier", "lettre", "letter"]),
        ("photo", "Photo", &["photo", "photos", "image", "images"]),
        (
            "spreadsheet",
            "Tableur",
            &["tableur", "tableurs", "spreadsheet"],
        ),
        (
            "presentation",
            "Présentation",
            &["presentation", "presentations", "diaporama"],
        ),
    ];
    DOCUMENT_TYPES.iter().find_map(|(kind, label, aliases)| {
        aliases
            .iter()
            .any(|alias| contains_phrase(value, alias))
            .then_some((*kind, *label, *aliases))
    })
}

fn parse_context(value: &str) -> Option<(&'static str, &'static str, &'static [&'static str])> {
    const CONTEXTS: &[(&str, &str, &[&str])] = &[
        (
            "personal",
            "Personnel",
            &[
                "personnel",
                "personnels",
                "personnelle",
                "personnelles",
                "prive",
            ],
        ),
        (
            "business",
            "Professionnel",
            &[
                "professionnel",
                "professionnels",
                "professionnelle",
                "entreprise",
                "business",
            ],
        ),
    ];
    CONTEXTS.iter().find_map(|(kind, label, aliases)| {
        aliases
            .iter()
            .any(|alias| contains_phrase(value, alias))
            .then_some((*kind, *label, *aliases))
    })
}

fn parse_amount(value: &str) -> Option<(AmountIntent, String)> {
    let between = Regex::new(
        r"(?i)\bentre\s+([0-9][0-9\s.,]*)\s*(?:€|euros?|eur)?\s+(?:et|a)\s+([0-9][0-9\s.,]*)\s*(€|euros?|eur)?",
    )
    .ok()?;
    if let Some(captures) = between.captures(value) {
        let first = parse_major_minor(captures.get(1)?.as_str())?;
        let second = parse_major_minor(captures.get(2)?.as_str())?;
        let currency = captures
            .get(3)
            .map(|value| currency_code(value.as_str()))
            .or(Some("EUR".to_owned()));
        return Some((
            AmountIntent {
                minimum_minor: Some(first.min(second)),
                maximum_minor: Some(first.max(second)),
                target_minor: None,
                currency,
                approximate: false,
            },
            captures.get(0)?.as_str().to_owned(),
        ));
    }

    let comparison = Regex::new(
        r"(?i)\b(plus de|superieur(?:e)? a|au-dessus de|moins de|inferieur(?:e)? a|sous)\s+([0-9][0-9\s.,]*)\s*(€|euros?|eur)?",
    )
    .ok()?;
    if let Some(captures) = comparison.captures(value) {
        let amount = parse_major_minor(captures.get(2)?.as_str())?;
        let operator = captures.get(1)?.as_str();
        let is_minimum = matches!(
            operator,
            "plus de" | "superieur a" | "superieure a" | "au-dessus de"
        );
        return Some((
            AmountIntent {
                minimum_minor: is_minimum.then_some(amount.saturating_add(1)),
                maximum_minor: (!is_minimum).then_some(amount.saturating_sub(1)),
                target_minor: None,
                currency: captures.get(3).map_or(Some("EUR".to_owned()), |value| {
                    Some(currency_code(value.as_str()))
                }),
                approximate: false,
            },
            captures.get(0)?.as_str().to_owned(),
        ));
    }

    let single = Regex::new(
        r"(?i)(?:(?:d[' ]?)?(environ|autour de|approximativement|vers|~)\s*)?([0-9][0-9\s.,]*)\s*(€|euros?|eur)",
    )
    .ok()?;
    let captures = single.captures(value)?;
    let target = parse_major_minor(captures.get(2)?.as_str())?;
    let approximate = captures.get(1).is_some();
    let tolerance = if approximate {
        (target.unsigned_abs() / 10).max(1_000) as i64
    } else {
        0
    };
    Some((
        AmountIntent {
            minimum_minor: Some(target.saturating_sub(tolerance)),
            maximum_minor: Some(target.saturating_add(tolerance)),
            target_minor: Some(target),
            currency: Some(currency_code(captures.get(3)?.as_str())),
            approximate,
        },
        captures.get(0)?.as_str().to_owned(),
    ))
}

fn parse_major_minor(value: &str) -> Option<i64> {
    let compact = value.replace([' ', '\u{00a0}', '\u{202f}'], "");
    if compact.is_empty() {
        return None;
    }
    let comma = compact.rfind(',');
    let dot = compact.rfind('.');
    let separator = match (comma, dot) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(index), None) | (None, Some(index)) => {
            let trailing = compact.len().saturating_sub(index + 1);
            (trailing <= 2).then_some(index)
        }
        (None, None) => None,
    };
    let mut digits = String::new();
    let mut fractional = String::new();
    for (index, character) in compact.char_indices() {
        if character.is_ascii_digit() {
            if separator.is_some_and(|separator| index > separator) {
                fractional.push(character);
            } else {
                digits.push(character);
            }
        } else if character != ',' && character != '.' {
            return None;
        }
    }
    if digits.is_empty() {
        return None;
    }
    let major = digits.parse::<i64>().ok()?;
    let fraction = match fractional.len() {
        0 => 0,
        1 => fractional.parse::<i64>().ok()?.saturating_mul(10),
        _ => fractional
            .chars()
            .take(2)
            .collect::<String>()
            .parse::<i64>()
            .ok()?,
    };
    major.checked_mul(100)?.checked_add(fraction)
}

fn currency_code(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "$" | "dollar" | "dollars" | "usd" => "USD",
        "£" | "gbp" => "GBP",
        _ => "EUR",
    }
    .to_owned()
}

fn parse_date(value: &str, clock: QueryClock) -> Option<(DateIntent, String)> {
    let last_year = Regex::new(r"\b(?:l[' ]annee derniere|annee derniere|last year)\b").ok()?;
    if let Some(found) = last_year.find(value) {
        let year = clock.year.saturating_sub(1);
        return Some((
            DateIntent {
                from: format!("{year:04}-01-01"),
                to: format!("{year:04}-12-31"),
                year,
                month: None,
            },
            found.as_str().to_owned(),
        ));
    }

    const MONTHS: &[(&str, u8)] = &[
        ("janvier", 1),
        ("january", 1),
        ("fevrier", 2),
        ("february", 2),
        ("mars", 3),
        ("march", 3),
        ("avril", 4),
        ("april", 4),
        ("mai", 5),
        ("may", 5),
        ("juin", 6),
        ("june", 6),
        ("juillet", 7),
        ("july", 7),
        ("aout", 8),
        ("august", 8),
        ("septembre", 9),
        ("september", 9),
        ("octobre", 10),
        ("october", 10),
        ("novembre", 11),
        ("november", 11),
        ("decembre", 12),
        ("december", 12),
    ];
    for (name, month) in MONTHS {
        let pattern =
            Regex::new(&format!(r"\b{name}\s+(19[0-9]{{2}}|20[0-9]{{2}}|2100)\b")).ok()?;
        if let Some(captures) = pattern.captures(value) {
            let year = captures.get(1)?.as_str().parse::<i32>().ok()?;
            let last_day = days_in_month(year, *month);
            return Some((
                DateIntent {
                    from: format!("{year:04}-{month:02}-01"),
                    to: format!("{year:04}-{month:02}-{last_day:02}"),
                    year,
                    month: Some(*month),
                },
                captures.get(0)?.as_str().to_owned(),
            ));
        }
    }

    let year_pattern = Regex::new(r"\b(19[0-9]{2}|20[0-9]{2}|2100)\b").ok()?;
    let found = year_pattern.captures(value)?;
    let year = found.get(1)?.as_str().parse::<i32>().ok()?;
    Some((
        DateIntent {
            from: format!("{year:04}-01-01"),
            to: format!("{year:04}-12-31"),
            year,
            month: None,
        },
        found.get(0)?.as_str().to_owned(),
    ))
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        4 | 6 | 9 | 11 => 30,
        2 if year % 400 == 0 || (year % 4 == 0 && year % 100 != 0) => 29,
        2 => 28,
        _ => 31,
    }
}

fn parse_explicit_relationships(value: &str) -> Vec<(&'static str, String, String)> {
    const RELATION_MARKERS: &[(&str, &[&str])] = &[
        ("supplier", &["fournisseur", "supplier"]),
        ("customer", &["client", "customer"]),
        ("project", &["chantier", "projet", "project"]),
    ];
    let stop_words = [
        " environ ",
        " autour ",
        " entre ",
        " plus de ",
        " moins de ",
        " en janvier ",
        " en fevrier ",
        " en mars ",
        " en avril ",
        " en mai ",
        " en juin ",
        " en juillet ",
        " en aout ",
        " en septembre ",
        " en octobre ",
        " en novembre ",
        " en decembre ",
    ];
    let mut output = Vec::new();
    for (kind, markers) in RELATION_MARKERS {
        for marker in *markers {
            let needle = format!("{marker} ");
            let Some(start) = value.find(&needle) else {
                continue;
            };
            let value_start = start + needle.len();
            let tail = &value[value_start..];
            let end = stop_words
                .iter()
                .filter_map(|stop| tail.find(stop))
                .min()
                .unwrap_or(tail.len());
            let candidate = tail[..end]
                .trim_matches(|character: char| {
                    character.is_whitespace() || matches!(character, ',' | ';' | ':' | '.')
                })
                .strip_prefix("de ")
                .unwrap_or_else(|| {
                    tail[..end].trim_matches(|character: char| {
                        character.is_whitespace() || matches!(character, ',' | ';' | ':' | '.')
                    })
                })
                .to_owned();
            if !candidate.is_empty() && !starts_with_date_term(&candidate) {
                output.push((
                    *kind,
                    candidate,
                    value[start..value_start + end].trim().to_owned(),
                ));
            }
            break;
        }
    }
    output
}

fn lexical_remainder(value: &str, consumed: &[String]) -> String {
    let mut remainder = value.to_owned();
    let mut ordered = consumed
        .iter()
        .filter(|item| !item.trim().is_empty())
        .collect::<Vec<_>>();
    ordered.sort_by_key(|item| std::cmp::Reverse(item.len()));
    for item in ordered {
        remainder = remainder.replace(item, " ");
    }
    let filler = HashSet::from([
        "retrouve",
        "retrouver",
        "trouve",
        "chercher",
        "recherche",
        "document",
        "documents",
        "fournisseur",
        "supplier",
        "client",
        "customer",
        "projet",
        "project",
        "chantier",
        "du",
        "de",
        "des",
        "la",
        "le",
        "les",
        "un",
        "une",
        "d",
        "l",
        "lie",
        "lies",
        "liee",
        "liees",
        "concernant",
        "environ",
        "autour",
        "approximativement",
        "pour",
        "vers",
        "en",
        "au",
        "aux",
        "a",
    ]);
    remainder
        .split(|character: char| !character.is_alphanumeric() && character != '-')
        .map(|token| token.trim_matches('-'))
        .filter(|token| !token.is_empty() && !filler.contains(token))
        .take(32)
        .collect::<Vec<_>>()
        .join(" ")
}

fn starts_with_date_term(value: &str) -> bool {
    const DATE_TERMS: &[&str] = &[
        "janvier",
        "fevrier",
        "mars",
        "avril",
        "mai",
        "juin",
        "juillet",
        "aout",
        "septembre",
        "octobre",
        "novembre",
        "decembre",
        "annee",
        "last year",
    ];
    value
        .split_whitespace()
        .next()
        .is_some_and(|first| first.chars().all(|character| character.is_ascii_digit()))
        || DATE_TERMS.iter().any(|term| value.starts_with(term))
}

fn infer_party(value: &str) -> Option<String> {
    let tokens = value
        .split_whitespace()
        .filter(|token| token.chars().any(char::is_alphabetic))
        .take(8)
        .collect::<Vec<_>>();
    if tokens.is_empty() {
        None
    } else {
        Some(bounded_relation(&tokens.join(" ")))
    }
}

fn looks_like_file_query(value: &str) -> bool {
    value.contains('/')
        || value.contains('\\')
        || value.rsplit_once('.').is_some_and(|(stem, extension)| {
            !stem.trim().is_empty()
                && (2..=10).contains(&extension.chars().count())
                && extension.chars().all(char::is_alphanumeric)
        })
}

fn contains_phrase(value: &str, phrase: &str) -> bool {
    let padded = format!(" {value} ");
    padded.contains(&format!(" {phrase} "))
}

fn bounded_relation(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_RELATION_CHARS)
        .collect()
}

fn amount_label(amount: &AmountIntent) -> String {
    let currency = amount.currency.as_deref().unwrap_or("EUR");
    let symbol = if currency == "EUR" { "€" } else { currency };
    if let Some(target) = amount.target_minor {
        let prefix = if amount.approximate { "~" } else { "" };
        return format!("{prefix}{}{symbol}", format_major(target));
    }
    match (amount.minimum_minor, amount.maximum_minor) {
        (Some(minimum), Some(maximum)) => {
            format!(
                "{}–{}{symbol}",
                format_major(minimum),
                format_major(maximum)
            )
        }
        (Some(minimum), None) => format!("≥{}{symbol}", format_major(minimum)),
        (None, Some(maximum)) => format!("≤{}{symbol}", format_major(maximum)),
        (None, None) => symbol.to_owned(),
    }
}

fn format_major(minor: i64) -> String {
    let major = minor / 100;
    let fraction = minor.unsigned_abs() % 100;
    if fraction == 0 {
        major.to_string()
    } else {
        format!("{major},{fraction:02}")
    }
}

fn date_label(date: &DateIntent) -> String {
    date.month.map_or_else(
        || date.year.to_string(),
        |month| {
            const MONTH_LABELS: [&str; 12] = [
                "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.",
                "nov.", "déc.",
            ];
            format!(
                "{} {}",
                MONTH_LABELS[usize::from(month.saturating_sub(1))],
                date.year
            )
        },
    )
}

fn relationship_label(kind: &str, value: &str) -> String {
    match kind {
        "supplier" => format!("Fournisseur {value}"),
        "customer" => format!("Client {value}"),
        "project" => format!("Projet {value}"),
        _ => value.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLOCK: QueryClock = QueryClock::new(2026, 8, 11);

    #[test]
    fn understands_invoice_supplier_amount_and_project() {
        let parsed = interpret_query(
            "Retrouve la facture Point P d'environ 1 400 € du chantier Martin",
            CLOCK,
            &[],
        );
        assert_eq!(parsed.document_type.as_deref(), Some("invoice"));
        assert_eq!(parsed.project.as_deref(), Some("martin"));
        assert_eq!(parsed.party.as_deref(), Some("point p"));
        assert_eq!(
            parsed
                .amount
                .as_ref()
                .and_then(|amount| amount.target_minor),
            Some(140_000)
        );
        assert!(
            parsed
                .amount
                .as_ref()
                .is_some_and(|amount| amount.approximate)
        );
    }

    #[test]
    fn understands_date_and_amount_ranges_locally() {
        let date = interpret_query("facture fournisseur de juin 2026", CLOCK, &[]);
        assert_eq!(
            date.date,
            Some(DateIntent {
                from: "2026-06-01".to_owned(),
                to: "2026-06-30".to_owned(),
                year: 2026,
                month: Some(6),
            })
        );
        assert_eq!(date.supplier, None);
        assert_eq!(date.party, None);
        let last_year = interpret_query("contrats de l'année dernière", CLOCK, &[]);
        assert_eq!(last_year.date.as_ref().map(|date| date.year), Some(2025));
        let amount = interpret_query("entre 500 et 1500 euros", CLOCK, &[]);
        let amount = amount
            .amount
            .unwrap_or_else(|| panic!("amount should parse"));
        assert_eq!(amount.minimum_minor, Some(50_000));
        assert_eq!(amount.maximum_minor, Some(150_000));
        let above = interpret_query("plus de 1000 €", CLOCK, &[])
            .amount
            .unwrap_or_else(|| panic!("lower-bound amount should parse"));
        assert_eq!(above.minimum_minor, Some(100_001));
        assert_eq!(above.maximum_minor, None);
    }

    #[test]
    fn disabled_chip_restores_plain_lexical_search() {
        let parsed = interpret_query("facture Point P", CLOCK, &["document_type".to_owned()]);
        assert_eq!(parsed.document_type, None);
        assert!(parsed.lexical_text.contains("facture"));
    }

    #[test]
    fn filename_queries_are_not_treated_as_confirmed_party_requests() {
        let parsed = interpret_query("j-stale-two.txt", CLOCK, &[]);
        assert_eq!(parsed.party, None);
        assert_eq!(parsed.lexical_text, "j-stale-two txt");
    }
}
