use super::{Database, PersistenceError};
use domain::{
    LearningObservationId, LearningObservationInput, LocalRule, LocalRuleInput,
    RULE_SUGGESTION_THRESHOLD, RuleAction, RuleCondition, RuleFileMatch, RuleId, RuleOrigin,
    RuleSuggestion, RuleSuggestionId, RuleSuggestionSeed, RuleSuggestionStatus, WorkspaceId,
};
use rusqlite::{OptionalExtension, Transaction, params};
use std::collections::HashSet;

type RuleRow = (
    String,
    String,
    String,
    i64,
    i64,
    String,
    String,
    String,
    Option<String>,
    String,
    String,
);

type SuggestionRow = (
    String,
    String,
    String,
    i64,
    String,
    String,
    Option<String>,
    String,
    String,
);

impl Database {
    pub fn file_workspace_id(
        &self,
        file_id: domain::FileId,
    ) -> Result<WorkspaceId, PersistenceError> {
        let connection = self.lock()?;
        let value = connection
            .query_row(
                "SELECT workspace_id FROM files WHERE id = ?1",
                [file_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)?;
        value.parse().map_err(PersistenceError::InvalidIdentifier)
    }

    pub fn create_rule(
        &self,
        workspace_id: WorkspaceId,
        input: &LocalRuleInput,
    ) -> Result<LocalRule, PersistenceError> {
        validate_rule_input(input)?;
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let rule_id = insert_rule(
            &transaction,
            workspace_id,
            input,
            RuleOrigin::UserCreated,
            None,
        )?;
        transaction.commit()?;
        drop(connection);
        self.rule(workspace_id, rule_id)
    }

    pub fn update_rule(
        &self,
        workspace_id: WorkspaceId,
        rule_id: RuleId,
        input: &LocalRuleInput,
    ) -> Result<LocalRule, PersistenceError> {
        validate_rule_input(input)?;
        let conditions =
            serde_json::to_string(&input.conditions).map_err(|_| PersistenceError::InvalidRule)?;
        let action =
            serde_json::to_string(&input.action).map_err(|_| PersistenceError::InvalidRule)?;
        let connection = self.lock()?;
        let changed = connection.execute(
            "UPDATE local_user_rules
             SET name = ?3,
                 explanation = ?4,
                 enabled = ?5,
                 conditions_json = ?6,
                 action_kind = ?7,
                 action_json = ?8,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE workspace_id = ?1 AND id = ?2",
            params![
                workspace_id.to_string(),
                rule_id.to_string(),
                input.name.trim(),
                input.explanation.trim(),
                i64::from(input.enabled),
                conditions,
                input.action.database_name(),
                action,
            ],
        )?;
        if changed == 0 {
            return Err(PersistenceError::NotFound);
        }
        drop(connection);
        self.rule(workspace_id, rule_id)
    }

    pub fn set_rule_enabled(
        &self,
        workspace_id: WorkspaceId,
        rule_id: RuleId,
        enabled: bool,
    ) -> Result<LocalRule, PersistenceError> {
        let connection = self.lock()?;
        let changed = connection.execute(
            "UPDATE local_user_rules
             SET enabled = ?3, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE workspace_id = ?1 AND id = ?2",
            params![
                workspace_id.to_string(),
                rule_id.to_string(),
                i64::from(enabled)
            ],
        )?;
        if changed == 0 {
            return Err(PersistenceError::NotFound);
        }
        drop(connection);
        self.rule(workspace_id, rule_id)
    }

    pub fn delete_rule(
        &self,
        workspace_id: WorkspaceId,
        rule_id: RuleId,
    ) -> Result<(), PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let changed = transaction.execute(
            "DELETE FROM local_user_rules WHERE workspace_id = ?1 AND id = ?2",
            params![workspace_id.to_string(), rule_id.to_string()],
        )?;
        if changed == 0 {
            return Err(PersistenceError::NotFound);
        }
        normalize_rule_positions(&transaction, workspace_id)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn reorder_rules(
        &self,
        workspace_id: WorkspaceId,
        ordered_ids: &[RuleId],
    ) -> Result<Vec<LocalRule>, PersistenceError> {
        if ordered_ids.len() > 512
            || ordered_ids.iter().copied().collect::<HashSet<_>>().len() != ordered_ids.len()
        {
            return Err(PersistenceError::RuleConflict);
        }
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let existing = rule_ids(&transaction, workspace_id)?;
        if existing.len() != ordered_ids.len()
            || existing.iter().copied().collect::<HashSet<_>>()
                != ordered_ids.iter().copied().collect::<HashSet<_>>()
        {
            return Err(PersistenceError::RuleConflict);
        }
        for (position, rule_id) in ordered_ids.iter().enumerate() {
            transaction.execute(
                "UPDATE local_user_rules
                 SET position = ?3,
                     updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                 WHERE workspace_id = ?1 AND id = ?2",
                params![
                    workspace_id.to_string(),
                    rule_id.to_string(),
                    i64::try_from(position).map_err(|_| PersistenceError::NumericOverflow)?,
                ],
            )?;
        }
        transaction.commit()?;
        drop(connection);
        self.rules(workspace_id)
    }

    pub fn rule(
        &self,
        workspace_id: WorkspaceId,
        rule_id: RuleId,
    ) -> Result<LocalRule, PersistenceError> {
        let connection = self.lock()?;
        let row = connection
            .query_row(
                "SELECT
                    name, explanation, origin, position, enabled,
                    conditions_json, action_json, action_kind,
                    source_suggestion_id, created_at, updated_at
                 FROM local_user_rules
                 WHERE workspace_id = ?1 AND id = ?2",
                params![workspace_id.to_string(), rule_id.to_string()],
                read_rule_row,
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)?;
        decode_rule(workspace_id, rule_id, row)
    }

    pub fn rules(&self, workspace_id: WorkspaceId) -> Result<Vec<LocalRule>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT
                id, name, explanation, origin, position, enabled,
                conditions_json, action_json, action_kind,
                source_suggestion_id, created_at, updated_at
             FROM local_user_rules
             WHERE workspace_id = ?1
             ORDER BY position, id",
        )?;
        let rows = statement
            .query_map([workspace_id.to_string()], |row| {
                Ok((row.get::<_, String>(0)?, read_rule_row_offset(row, 1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(id, row)| decode_rule(workspace_id, id.parse::<RuleId>()?, row))
            .collect()
    }

    pub fn record_learning_observation(
        &self,
        workspace_id: WorkspaceId,
        observation: &LearningObservationInput,
        suggestion: Option<&RuleSuggestionSeed>,
    ) -> Result<Option<RuleSuggestion>, PersistenceError> {
        validate_observation(observation)?;
        if let Some(seed) = suggestion {
            validate_suggestion_seed(seed)?;
        }
        let evidence = serde_json::to_string(&observation.evidence)
            .map_err(|_| PersistenceError::InvalidRule)?;
        if evidence.len() > 16_384 {
            return Err(PersistenceError::InvalidRule);
        }
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT OR IGNORE INTO local_learning_observations(
                id, workspace_id, file_id, source_kind, source_ref,
                pattern_kind, pattern_key, evidence_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                LearningObservationId::new().to_string(),
                workspace_id.to_string(),
                observation.file_id.map(|value| value.to_string()),
                observation.source_kind.database_name(),
                observation.source_ref.trim(),
                observation.pattern_kind.database_name(),
                observation.pattern_key.trim(),
                evidence,
            ],
        )?;
        let evidence_count = transaction.query_row(
            "SELECT COUNT(*)
             FROM local_learning_observations
             WHERE workspace_id = ?1 AND pattern_kind = ?2 AND pattern_key = ?3",
            params![
                workspace_id.to_string(),
                observation.pattern_kind.database_name(),
                observation.pattern_key.trim()
            ],
            |row| row.get::<_, i64>(0),
        )?;
        let evidence_count =
            u64::try_from(evidence_count).map_err(|_| PersistenceError::NumericOverflow)?;
        if evidence_count < RULE_SUGGESTION_THRESHOLD {
            transaction.commit()?;
            return Ok(None);
        }
        let Some(seed) = suggestion else {
            transaction.commit()?;
            return Ok(None);
        };
        let signature = suggestion_signature(
            observation.pattern_kind.database_name(),
            observation.pattern_key.trim(),
        );
        let proposed_rule = serde_json::to_string(&seed.proposed_rule)
            .map_err(|_| PersistenceError::InvalidRule)?;
        let new_id = RuleSuggestionId::new();
        transaction.execute(
            "INSERT INTO local_rule_suggestions(
                id, workspace_id, signature, title, explanation,
                evidence_count, status, proposed_rule_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', ?7)
             ON CONFLICT(workspace_id, signature) DO UPDATE SET
                evidence_count = excluded.evidence_count,
                updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')",
            params![
                new_id.to_string(),
                workspace_id.to_string(),
                signature,
                seed.title.trim(),
                seed.explanation.trim(),
                i64::try_from(evidence_count).map_err(|_| PersistenceError::NumericOverflow)?,
                proposed_rule,
            ],
        )?;
        let suggestion_id = transaction.query_row(
            "SELECT id FROM local_rule_suggestions
             WHERE workspace_id = ?1 AND signature = ?2",
            params![workspace_id.to_string(), signature],
            |row| row.get::<_, String>(0),
        )?;
        transaction.commit()?;
        drop(connection);
        self.rule_suggestion(workspace_id, suggestion_id.parse()?)
            .map(Some)
    }

    pub fn rule_suggestions(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<Vec<RuleSuggestion>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT
                id, signature, title, explanation, evidence_count, status,
                proposed_rule_json, accepted_rule_id, created_at, updated_at
             FROM local_rule_suggestions
             WHERE workspace_id = ?1
             ORDER BY
                CASE status WHEN 'pending' THEN 0 WHEN 'accepted' THEN 1 ELSE 2 END,
                updated_at DESC, id",
        )?;
        let rows = statement
            .query_map([workspace_id.to_string()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    read_suggestion_row_offset(row, 1)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(id, row)| decode_suggestion(workspace_id, id.parse()?, row))
            .collect()
    }

    pub fn accept_rule_suggestion(
        &self,
        workspace_id: WorkspaceId,
        suggestion_id: RuleSuggestionId,
    ) -> Result<LocalRule, PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let proposed = transaction
            .query_row(
                "SELECT proposed_rule_json
                 FROM local_rule_suggestions
                 WHERE workspace_id = ?1 AND id = ?2 AND status = 'pending'",
                params![workspace_id.to_string(), suggestion_id.to_string()],
                |row| row.get::<_, String>(0),
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)?;
        let input = serde_json::from_str::<LocalRuleInput>(&proposed)
            .map_err(|_| PersistenceError::InvalidRule)?;
        validate_rule_input(&input)?;
        let rule_id = insert_rule(
            &transaction,
            workspace_id,
            &input,
            RuleOrigin::AcceptedSuggestion,
            Some(suggestion_id),
        )?;
        let changed = transaction.execute(
            "UPDATE local_rule_suggestions
             SET status = 'accepted',
                 accepted_rule_id = ?3,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE workspace_id = ?1 AND id = ?2 AND status = 'pending'",
            params![
                workspace_id.to_string(),
                suggestion_id.to_string(),
                rule_id.to_string()
            ],
        )?;
        if changed != 1 {
            return Err(PersistenceError::RuleConflict);
        }
        transaction.commit()?;
        drop(connection);
        self.rule(workspace_id, rule_id)
    }

    pub fn dismiss_rule_suggestion(
        &self,
        workspace_id: WorkspaceId,
        suggestion_id: RuleSuggestionId,
    ) -> Result<RuleSuggestion, PersistenceError> {
        let connection = self.lock()?;
        let changed = connection.execute(
            "UPDATE local_rule_suggestions
             SET status = 'dismissed',
                 accepted_rule_id = NULL,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
             WHERE workspace_id = ?1 AND id = ?2 AND status = 'pending'",
            params![workspace_id.to_string(), suggestion_id.to_string()],
        )?;
        if changed == 0 {
            return Err(PersistenceError::NotFound);
        }
        drop(connection);
        self.rule_suggestion(workspace_id, suggestion_id)
    }

    pub fn replace_rule_file_matches(
        &self,
        workspace_id: WorkspaceId,
        matches: &[RuleFileMatch],
    ) -> Result<(), PersistenceError> {
        if matches.len() > 100_000
            || matches.iter().any(|item| {
                item.workspace_id != workspace_id
                    || !(0.0..=0.25).contains(&item.boost)
                    || item.explanation.trim().is_empty()
                    || item.explanation.chars().count() > 512
            })
        {
            return Err(PersistenceError::InvalidRule);
        }
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        transaction.execute(
            "DELETE FROM local_rule_file_matches WHERE workspace_id = ?1",
            [workspace_id.to_string()],
        )?;
        {
            let mut statement = transaction.prepare(
                "INSERT INTO local_rule_file_matches(
                    rule_id, workspace_id, file_id, boost, explanation
                 )
                 SELECT ?1, ?2, ?3, ?4, ?5
                 WHERE EXISTS (
                    SELECT 1 FROM local_user_rules
                    WHERE id = ?1 AND workspace_id = ?2 AND enabled = 1
                 )",
            )?;
            for item in matches {
                statement.execute(params![
                    item.rule_id.to_string(),
                    workspace_id.to_string(),
                    item.file_id.to_string(),
                    item.boost,
                    item.explanation.trim(),
                ])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn replace_rule_file_matches_for_files(
        &self,
        workspace_id: WorkspaceId,
        file_ids: &[domain::FileId],
        matches: &[RuleFileMatch],
    ) -> Result<(), PersistenceError> {
        if matches.len() > 100_000
            || matches.iter().any(|item| {
                item.workspace_id != workspace_id
                    || !(0.0..=0.25).contains(&item.boost)
                    || item.explanation.trim().is_empty()
                    || item.explanation.chars().count() > 512
                    || !file_ids.contains(&item.file_id)
            })
        {
            return Err(PersistenceError::InvalidRule);
        }
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        for file_id in file_ids {
            transaction.execute(
                "DELETE FROM local_rule_file_matches
                 WHERE workspace_id = ?1 AND file_id = ?2",
                params![workspace_id.to_string(), file_id.to_string()],
            )?;
        }
        {
            let mut statement = transaction.prepare(
                "INSERT INTO local_rule_file_matches(
                    rule_id, workspace_id, file_id, boost, explanation
                 )
                 SELECT ?1, ?2, ?3, ?4, ?5
                 WHERE EXISTS (
                    SELECT 1 FROM local_user_rules
                    WHERE id = ?1 AND workspace_id = ?2 AND enabled = 1
                 )",
            )?;
            for item in matches {
                statement.execute(params![
                    item.rule_id.to_string(),
                    workspace_id.to_string(),
                    item.file_id.to_string(),
                    item.boost,
                    item.explanation.trim(),
                ])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn rule_file_matches(
        &self,
        workspace_id: WorkspaceId,
    ) -> Result<Vec<RuleFileMatch>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT rule_id, file_id, boost, explanation
             FROM local_rule_file_matches
             WHERE workspace_id = ?1
             ORDER BY file_id, boost DESC, rule_id",
        )?;
        let rows = statement
            .query_map([workspace_id.to_string()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(rule_id, file_id, boost, explanation)| {
                Ok(RuleFileMatch {
                    rule_id: rule_id.parse()?,
                    workspace_id,
                    file_id: file_id.parse()?,
                    boost,
                    explanation,
                })
            })
            .collect()
    }

    fn rule_suggestion(
        &self,
        workspace_id: WorkspaceId,
        suggestion_id: RuleSuggestionId,
    ) -> Result<RuleSuggestion, PersistenceError> {
        let connection = self.lock()?;
        let row = connection
            .query_row(
                "SELECT
                    signature, title, explanation, evidence_count, status,
                    proposed_rule_json, accepted_rule_id, created_at, updated_at
                 FROM local_rule_suggestions
                 WHERE workspace_id = ?1 AND id = ?2",
                params![workspace_id.to_string(), suggestion_id.to_string()],
                read_suggestion_row,
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)?;
        decode_suggestion(workspace_id, suggestion_id, row)
    }
}

fn insert_rule(
    transaction: &Transaction<'_>,
    workspace_id: WorkspaceId,
    input: &LocalRuleInput,
    origin: RuleOrigin,
    source_suggestion_id: Option<RuleSuggestionId>,
) -> Result<RuleId, PersistenceError> {
    let position = transaction.query_row(
        "SELECT COALESCE(MAX(position) + 1, 0)
         FROM local_user_rules WHERE workspace_id = ?1",
        [workspace_id.to_string()],
        |row| row.get::<_, i64>(0),
    )?;
    if position >= 512 {
        return Err(PersistenceError::InvalidRule);
    }
    let conditions =
        serde_json::to_string(&input.conditions).map_err(|_| PersistenceError::InvalidRule)?;
    let action = serde_json::to_string(&input.action).map_err(|_| PersistenceError::InvalidRule)?;
    let rule_id = RuleId::new();
    transaction.execute(
        "INSERT INTO local_user_rules(
            id, workspace_id, name, explanation, position, enabled,
            conditions_json, action_kind, action_json, origin, source_suggestion_id
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            rule_id.to_string(),
            workspace_id.to_string(),
            input.name.trim(),
            input.explanation.trim(),
            position,
            i64::from(input.enabled),
            conditions,
            input.action.database_name(),
            action,
            origin.database_name(),
            source_suggestion_id.map(|value| value.to_string()),
        ],
    )?;
    Ok(rule_id)
}

fn normalize_rule_positions(
    transaction: &Transaction<'_>,
    workspace_id: WorkspaceId,
) -> Result<(), PersistenceError> {
    let ids = rule_ids(transaction, workspace_id)?;
    for (position, id) in ids.iter().enumerate() {
        transaction.execute(
            "UPDATE local_user_rules SET position = ?3
             WHERE workspace_id = ?1 AND id = ?2",
            params![
                workspace_id.to_string(),
                id.to_string(),
                i64::try_from(position).map_err(|_| PersistenceError::NumericOverflow)?,
            ],
        )?;
    }
    Ok(())
}

fn rule_ids(
    transaction: &Transaction<'_>,
    workspace_id: WorkspaceId,
) -> Result<Vec<RuleId>, PersistenceError> {
    let mut statement = transaction.prepare(
        "SELECT id FROM local_user_rules
         WHERE workspace_id = ?1 ORDER BY position, id",
    )?;
    let values = statement
        .query_map([workspace_id.to_string()], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    values
        .into_iter()
        .map(|value| value.parse().map_err(PersistenceError::InvalidIdentifier))
        .collect()
}

fn read_rule_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RuleRow> {
    read_rule_row_offset(row, 0)
}

fn read_rule_row_offset(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<RuleRow> {
    Ok((
        row.get(offset)?,
        row.get(offset + 1)?,
        row.get(offset + 2)?,
        row.get(offset + 3)?,
        row.get(offset + 4)?,
        row.get(offset + 5)?,
        row.get(offset + 6)?,
        row.get(offset + 7)?,
        row.get(offset + 8)?,
        row.get(offset + 9)?,
        row.get(offset + 10)?,
    ))
}

fn decode_rule(
    workspace_id: WorkspaceId,
    rule_id: RuleId,
    row: RuleRow,
) -> Result<LocalRule, PersistenceError> {
    let (
        name,
        explanation,
        origin,
        position,
        enabled,
        conditions,
        action,
        action_kind,
        source_suggestion_id,
        created_at,
        updated_at,
    ) = row;
    let conditions = serde_json::from_str::<Vec<RuleCondition>>(&conditions)
        .map_err(|_| PersistenceError::InvalidRule)?;
    let action =
        serde_json::from_str::<RuleAction>(&action).map_err(|_| PersistenceError::InvalidRule)?;
    if action.database_name() != action_kind {
        return Err(PersistenceError::InvalidRule);
    }
    Ok(LocalRule {
        id: rule_id,
        workspace_id,
        name,
        explanation,
        position: u32::try_from(position).map_err(|_| PersistenceError::NumericOverflow)?,
        enabled: enabled != 0,
        conditions,
        action,
        origin: match origin.as_str() {
            "user_created" => RuleOrigin::UserCreated,
            "accepted_suggestion" => RuleOrigin::AcceptedSuggestion,
            _ => return Err(PersistenceError::InvalidRule),
        },
        source_suggestion_id: source_suggestion_id
            .map(|value| value.parse())
            .transpose()?,
        created_at,
        updated_at,
    })
}

fn read_suggestion_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SuggestionRow> {
    read_suggestion_row_offset(row, 0)
}

fn read_suggestion_row_offset(
    row: &rusqlite::Row<'_>,
    offset: usize,
) -> rusqlite::Result<SuggestionRow> {
    Ok((
        row.get(offset)?,
        row.get(offset + 1)?,
        row.get(offset + 2)?,
        row.get(offset + 3)?,
        row.get(offset + 4)?,
        row.get(offset + 5)?,
        row.get(offset + 6)?,
        row.get(offset + 7)?,
        row.get(offset + 8)?,
    ))
}

fn decode_suggestion(
    workspace_id: WorkspaceId,
    suggestion_id: RuleSuggestionId,
    row: SuggestionRow,
) -> Result<RuleSuggestion, PersistenceError> {
    let (
        signature,
        title,
        explanation,
        evidence_count,
        status,
        proposed_rule,
        accepted_rule_id,
        created_at,
        updated_at,
    ) = row;
    let status = match status.as_str() {
        "pending" => RuleSuggestionStatus::Pending,
        "accepted" => RuleSuggestionStatus::Accepted,
        "dismissed" => RuleSuggestionStatus::Dismissed,
        _ => return Err(PersistenceError::InvalidRule),
    };
    Ok(RuleSuggestion {
        id: suggestion_id,
        workspace_id,
        signature,
        title,
        explanation,
        evidence_count: u64::try_from(evidence_count)
            .map_err(|_| PersistenceError::NumericOverflow)?,
        status,
        proposed_rule: serde_json::from_str(&proposed_rule)
            .map_err(|_| PersistenceError::InvalidRule)?,
        accepted_rule_id: accepted_rule_id.map(|value| value.parse()).transpose()?,
        created_at,
        updated_at,
    })
}

fn validate_rule_input(input: &LocalRuleInput) -> Result<(), PersistenceError> {
    if input.name.trim().is_empty()
        || input.name.chars().count() > 120
        || input.explanation.trim().is_empty()
        || input.explanation.chars().count() > 512
        || input.conditions.is_empty()
        || input.conditions.len() > 8
        || serde_json::to_string(&input.conditions)
            .map_err(|_| PersistenceError::InvalidRule)?
            .len()
            > 16_384
        || serde_json::to_string(&input.action)
            .map_err(|_| PersistenceError::InvalidRule)?
            .len()
            > 8_192
    {
        return Err(PersistenceError::InvalidRule);
    }
    Ok(())
}

fn validate_observation(value: &LearningObservationInput) -> Result<(), PersistenceError> {
    if value.source_ref.trim().is_empty()
        || value.source_ref.chars().count() > 128
        || value.pattern_key.trim().is_empty()
        || value.pattern_key.chars().count() > 1024
    {
        return Err(PersistenceError::InvalidRule);
    }
    Ok(())
}

fn validate_suggestion_seed(value: &RuleSuggestionSeed) -> Result<(), PersistenceError> {
    if value.title.trim().is_empty()
        || value.title.chars().count() > 200
        || value.explanation.trim().is_empty()
        || value.explanation.chars().count() > 1024
    {
        return Err(PersistenceError::InvalidRule);
    }
    validate_rule_input(&value.proposed_rule)
}

fn suggestion_signature(pattern_kind: &str, pattern_key: &str) -> String {
    blake3::hash(format!("{pattern_kind}\0{pattern_key}").as_bytes())
        .to_hex()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DatabaseKey;
    use domain::{
        LearningPatternKind, LearningSourceKind, RuleCondition, RuleField, RuleOperator,
        SemanticRuleField,
    };

    fn input(name: &str, value: &str) -> LocalRuleInput {
        LocalRuleInput {
            name: name.to_owned(),
            explanation: format!("{name} was explicitly requested by the user."),
            enabled: true,
            conditions: vec![RuleCondition {
                field: RuleField::DocumentType,
                operator: RuleOperator::Equals,
                value: Some("invoice".to_owned()),
            }],
            action: RuleAction::SetSemanticField {
                field: SemanticRuleField::Context,
                value: value.to_owned(),
            },
        }
    }

    #[test]
    fn rule_crud_reorder_disable_and_delete_are_durable() {
        let database = Database::open_in_memory(&DatabaseKey::from_bytes([31; 32]))
            .unwrap_or_else(|error| panic!("database should open: {error}"));
        let workspace = database
            .create_workspace("Rules")
            .unwrap_or_else(|error| panic!("workspace should exist: {error}"));
        let first = database
            .create_rule(workspace.id, &input("First", "business"))
            .unwrap_or_else(|error| panic!("first rule should save: {error}"));
        let second = database
            .create_rule(workspace.id, &input("Second", "personal"))
            .unwrap_or_else(|error| panic!("second rule should save: {error}"));

        let ordered = database
            .reorder_rules(workspace.id, &[second.id, first.id])
            .unwrap_or_else(|error| panic!("rules should reorder: {error}"));
        assert_eq!(ordered[0].id, second.id);
        assert_eq!(ordered[0].position, 0);

        let disabled = database
            .set_rule_enabled(workspace.id, second.id, false)
            .unwrap_or_else(|error| panic!("rule should disable: {error}"));
        assert!(!disabled.enabled);
        database
            .delete_rule(workspace.id, first.id)
            .unwrap_or_else(|error| panic!("rule should delete: {error}"));
        let remaining = database
            .rules(workspace.id)
            .unwrap_or_else(|error| panic!("rules should load: {error}"));
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].position, 0);
    }

    #[test]
    fn repeated_observations_only_suggest_until_explicit_acceptance() {
        let database = Database::open_in_memory(&DatabaseKey::from_bytes([32; 32]))
            .unwrap_or_else(|error| panic!("database should open: {error}"));
        let workspace = database
            .create_workspace("Learning")
            .unwrap_or_else(|error| panic!("workspace should exist: {error}"));
        let seed = RuleSuggestionSeed {
            title: "Create a reusable invoice rule?".to_owned(),
            explanation: "Three equivalent corrections were recorded locally.".to_owned(),
            proposed_rule: input("Invoices are business", "business"),
        };
        for index in 0..2 {
            let suggestion = database
                .record_learning_observation(
                    workspace.id,
                    &LearningObservationInput {
                        file_id: None,
                        source_kind: LearningSourceKind::SemanticCorrection,
                        source_ref: format!("correction-{index}"),
                        pattern_kind: LearningPatternKind::SemanticField,
                        pattern_key: "document_type\0receipt\0invoice".to_owned(),
                        evidence: serde_json::json!({"index": index}),
                    },
                    Some(&seed),
                )
                .unwrap_or_else(|error| panic!("observation should save: {error}"));
            assert!(suggestion.is_none());
        }
        assert!(
            database
                .rules(workspace.id)
                .unwrap_or_else(|error| panic!("rules should load: {error}"))
                .is_empty()
        );
        let suggestion = database
            .record_learning_observation(
                workspace.id,
                &LearningObservationInput {
                    file_id: None,
                    source_kind: LearningSourceKind::SemanticCorrection,
                    source_ref: "correction-2".to_owned(),
                    pattern_kind: LearningPatternKind::SemanticField,
                    pattern_key: "document_type\0receipt\0invoice".to_owned(),
                    evidence: serde_json::json!({"index": 2}),
                },
                Some(&seed),
            )
            .unwrap_or_else(|error| panic!("third observation should save: {error}"))
            .unwrap_or_else(|| panic!("third observation should suggest"));
        assert_eq!(suggestion.evidence_count, 3);
        assert!(
            database
                .rules(workspace.id)
                .unwrap_or_else(|error| panic!("rules should load: {error}"))
                .is_empty()
        );

        let accepted = database
            .accept_rule_suggestion(workspace.id, suggestion.id)
            .unwrap_or_else(|error| panic!("accept should create a rule: {error}"));
        assert_eq!(accepted.origin, RuleOrigin::AcceptedSuggestion);
        assert_eq!(
            database
                .rules(workspace.id)
                .unwrap_or_else(|error| panic!("rules should load: {error}"))
                .len(),
            1
        );
    }
}
