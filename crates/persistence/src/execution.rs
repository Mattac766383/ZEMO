use crate::{Database, PersistenceError, from_sql_u64, to_sql_u64};
use domain::{
    ApprovedExecutionPlan, ExecutionConsent, ExecutionConsentState, ExecutionDetail, ExecutionId,
    ExecutionOperation, ExecutionOperationKind, ExecutionOperationStatus, ExecutionRecoveryState,
    ExecutionRetentionMetadata, ExecutionRootBinding, ExecutionSafetyPolicyBinding,
    ExecutionSession, ExecutionSummary, ExecutorRequestDirection, ExecutorRequestFact,
    ExecutorRequestIdentity, ExecutorRequestState, ExecutorSessionFact, ExecutorSessionIdentity,
    ExecutorSessionPurpose, FileFingerprint, JournalEventKind, NativePath, OperationJournalEvent,
    OperationStepId, OrganizationExecutionStatus, PathEncoding, ProposalItemId, VolumeIdentity,
    WorkspaceId,
};
use rusqlite::{OptionalExtension, Row, Transaction, params};
use std::collections::BTreeSet;
use uuid::Uuid;

impl Database {
    pub fn persist_prepared_execution(
        &self,
        detail: &ExecutionDetail,
    ) -> Result<(), PersistenceError> {
        validate_prepared_execution(detail)?;
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        validate_approved_proposal(&transaction, detail)?;
        insert_execution_session(&transaction, &detail.session)?;
        insert_approval_snapshot(
            &transaction,
            &detail.session.approval,
            &detail.session.created_at,
        )?;
        insert_execution_consent(&transaction, &detail.session)?;
        for operation in &detail.operations {
            insert_execution_operation(&transaction, operation)?;
        }
        insert_execution_retention(&transaction, &detail.session)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn issue_execution_consent_challenge(
        &self,
        execution_id: ExecutionId,
        nonce: [u8; 32],
        issued_at_unix_ms: i64,
        expires_at_unix_ms: i64,
    ) -> Result<ExecutionDetail, PersistenceError> {
        if expires_at_unix_ms <= issued_at_unix_ms {
            return Err(PersistenceError::InvalidExecution);
        }
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE local_execution_consents
             SET state = 'pending',
                 issued_at_unix_ms = ?2,
                 expires_at_unix_ms = ?3,
                 attested_at_unix_ms = NULL,
                 consumed_at_unix_ms = NULL,
                 invalidated_at_unix_ms = NULL,
                 invalidation_reason = NULL,
                 nonce = ?4,
                 attestation_mac = NULL,
                 state_changed_at_unix_ms = ?2
             WHERE execution_id = ?1
               AND material_version = ?5
               AND state IN ('pending', 'expired')",
            params![
                execution_id.to_string(),
                issued_at_unix_ms,
                expires_at_unix_ms,
                nonce.as_slice(),
                i64::from(domain::EXECUTION_PLAN_MATERIAL_VERSION),
            ],
        )?;
        if changed != 1 {
            return Err(PersistenceError::InvalidExecution);
        }
        let session_changed = transaction.execute(
            "UPDATE local_execution_sessions
             SET status = 'awaiting_confirmation',
                 user_confirmed = 0,
                 approved_at = NULL
             WHERE id = ?1
               AND status = 'awaiting_confirmation'
               AND preflight_ok_count > 0",
            [execution_id.to_string()],
        )?;
        let snapshot_changed = transaction.execute(
            "UPDATE local_execution_approval_snapshots
             SET user_confirmed = 0,
                 approved_at = NULL
             WHERE execution_id = ?1",
            [execution_id.to_string()],
        )?;
        if session_changed != 1 || snapshot_changed != 1 {
            return Err(PersistenceError::InvalidExecution);
        }
        transaction.commit()?;
        drop(connection);
        self.execution_detail(execution_id)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn attest_execution_consent(
        &self,
        execution_id: ExecutionId,
        nonce: [u8; 32],
        issued_at_unix_ms: i64,
        expires_at_unix_ms: i64,
        attestation_mac: [u8; 32],
        attested_at_unix_ms: i64,
    ) -> Result<ExecutionDetail, PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE local_execution_consents
             SET state = 'attested',
                 attested_at_unix_ms = ?5,
                 attestation_mac = ?6,
                 state_changed_at_unix_ms = ?5
             WHERE execution_id = ?1
               AND state = 'pending'
               AND nonce = ?2
               AND issued_at_unix_ms = ?3
               AND expires_at_unix_ms = ?4
               AND ?5 BETWEEN issued_at_unix_ms AND expires_at_unix_ms
               AND attestation_mac IS NULL",
            params![
                execution_id.to_string(),
                nonce.as_slice(),
                issued_at_unix_ms,
                expires_at_unix_ms,
                attested_at_unix_ms,
                attestation_mac.as_slice(),
            ],
        )?;
        if changed != 1 {
            return Err(PersistenceError::InvalidExecution);
        }
        let session_changed = transaction.execute(
            "UPDATE local_execution_sessions
             SET status = 'approved',
                 user_confirmed = 1,
                 approved_at = ?2
             WHERE id = ?1
               AND status = 'awaiting_confirmation'
               AND preflight_ok_count > 0
               AND user_confirmed = 0",
            params![execution_id.to_string(), attested_at_unix_ms.to_string()],
        )?;
        let snapshot_changed = transaction.execute(
            "UPDATE local_execution_approval_snapshots
             SET user_confirmed = 1,
                 approved_at = ?2
             WHERE execution_id = ?1 AND user_confirmed = 0",
            params![execution_id.to_string(), attested_at_unix_ms.to_string()],
        )?;
        if session_changed != 1 || snapshot_changed != 1 {
            return Err(PersistenceError::InvalidExecution);
        }
        transaction.commit()?;
        drop(connection);
        self.execution_detail(execution_id)
    }

    pub fn clear_execution_consent_challenge(
        &self,
        execution_id: ExecutionId,
        nonce: [u8; 32],
        issued_at_unix_ms: i64,
        expires_at_unix_ms: i64,
        now_unix_ms: i64,
    ) -> Result<bool, PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE local_execution_consents
             SET issued_at_unix_ms = NULL,
                 expires_at_unix_ms = NULL,
                 nonce = NULL,
                 state_changed_at_unix_ms = ?5
             WHERE execution_id = ?1
               AND state = 'pending'
               AND nonce = ?2
               AND issued_at_unix_ms = ?3
               AND expires_at_unix_ms = ?4
               AND attestation_mac IS NULL",
            params![
                execution_id.to_string(),
                nonce.as_slice(),
                issued_at_unix_ms,
                expires_at_unix_ms,
                now_unix_ms,
            ],
        )?;
        Ok(changed == 1)
    }

    pub fn expire_execution_consent(
        &self,
        execution_id: ExecutionId,
        now_unix_ms: i64,
    ) -> Result<bool, PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE local_execution_consents
             SET state = 'expired',
                 state_changed_at_unix_ms = ?2
             WHERE execution_id = ?1
               AND state IN ('pending', 'attested')
               AND expires_at_unix_ms IS NOT NULL
               AND expires_at_unix_ms <= ?2",
            params![execution_id.to_string(), now_unix_ms],
        )?;
        if changed == 1 {
            reset_legacy_execution_approval(&transaction, execution_id)?;
        }
        transaction.commit()?;
        Ok(changed == 1)
    }

    pub fn invalidate_execution_consent(
        &self,
        execution_id: ExecutionId,
        reason: &str,
        now_unix_ms: i64,
    ) -> Result<bool, PersistenceError> {
        let reason = reason.chars().take(256).collect::<String>();
        if reason.is_empty() {
            return Err(PersistenceError::InvalidExecution);
        }
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE local_execution_consents
             SET state = 'invalidated',
                 invalidated_at_unix_ms = ?3,
                 invalidation_reason = ?2,
                 state_changed_at_unix_ms = ?3
             WHERE execution_id = ?1
               AND state IN ('pending', 'attested', 'expired')",
            params![execution_id.to_string(), reason, now_unix_ms],
        )?;
        if changed == 1 {
            reset_legacy_execution_approval(&transaction, execution_id)?;
        }
        transaction.commit()?;
        Ok(changed == 1)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn consume_execution_consent(
        &self,
        execution_id: ExecutionId,
        nonce: [u8; 32],
        issued_at_unix_ms: i64,
        expires_at_unix_ms: i64,
        attestation_mac: [u8; 32],
        consumed_at_unix_ms: i64,
    ) -> Result<ExecutionDetail, PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE local_execution_consents
             SET state = 'consumed',
                 consumed_at_unix_ms = ?6,
                 state_changed_at_unix_ms = ?6
             WHERE execution_id = ?1
               AND state = 'attested'
               AND nonce = ?2
               AND issued_at_unix_ms = ?3
               AND expires_at_unix_ms = ?4
               AND attestation_mac = ?5
               AND ?6 <= expires_at_unix_ms",
            params![
                execution_id.to_string(),
                nonce.as_slice(),
                issued_at_unix_ms,
                expires_at_unix_ms,
                attestation_mac.as_slice(),
                consumed_at_unix_ms,
            ],
        )?;
        if changed != 1 {
            return Err(PersistenceError::InvalidExecution);
        }
        let session_changed = transaction.execute(
            "UPDATE local_execution_sessions
             SET status = 'running',
                 started_at = COALESCE(started_at, ?2)
             WHERE id = ?1 AND status = 'approved'",
            params![execution_id.to_string(), consumed_at_unix_ms.to_string()],
        )?;
        if session_changed != 1 {
            return Err(PersistenceError::InvalidExecution);
        }
        transaction.commit()?;
        drop(connection);
        self.execution_detail(execution_id)
    }

    pub fn execution_detail(
        &self,
        execution_id: ExecutionId,
    ) -> Result<ExecutionDetail, PersistenceError> {
        let connection = self.lock()?;
        execution_detail_from_connection(&connection, execution_id)
    }

    pub fn execution_history(
        &self,
        workspace_id: WorkspaceId,
        limit: usize,
    ) -> Result<Vec<ExecutionSession>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id
             FROM local_execution_sessions
             WHERE workspace_id = ?1
             ORDER BY created_at DESC
             LIMIT ?2",
        )?;
        let rows = statement.query_map(
            params![
                workspace_id.to_string(),
                i64::try_from(limit.clamp(1, 100))
                    .map_err(|_| PersistenceError::NumericOverflow)?
            ],
            |row| row.get::<_, String>(0),
        )?;
        let mut output = Vec::new();
        for row in rows {
            output.push(execution_session_from_connection(
                &connection,
                row?.parse()?,
            )?);
        }
        Ok(output)
    }

    pub fn blocking_execution_exists(&self) -> Result<bool, PersistenceError> {
        let connection = self.lock()?;
        let count: i64 = connection.query_row(
            "SELECT COUNT(*)
             FROM local_execution_sessions
             WHERE status IN (
                'prepared', 'awaiting_confirmation', 'approved', 'running', 'paused',
                'recovery_required', 'recovery_available', 'recovery_ambiguous', 'rolling_back'
             )",
            [],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn execution_proposal_approval_is_current(
        &self,
        execution_id: ExecutionId,
    ) -> Result<bool, PersistenceError> {
        let connection = self.lock()?;
        let valid: i64 = connection.query_row(
            "SELECT EXISTS(
                SELECT 1
                FROM local_execution_sessions AS execution
                JOIN local_organization_proposals AS proposal
                  ON proposal.id = execution.proposal_id
                JOIN local_organization_proposal_revisions AS revision
                  ON revision.id = proposal.current_revision_id
                WHERE execution.id = ?1
                  AND proposal.status = 'approved_for_future_apply'
                  AND proposal.current_revision_id = execution.proposal_revision_id
                  AND revision.revision_number = execution.proposal_revision
             )",
            [execution_id.to_string()],
            |row| row.get(0),
        )?;
        Ok(valid != 0)
    }

    pub fn recovery_execution_ids(&self) -> Result<Vec<ExecutionId>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id
             FROM local_execution_sessions
             WHERE status IN (
                'running', 'rolling_back', 'recovery_required',
                'recovery_available', 'recovery_ambiguous'
             )
             ORDER BY created_at",
        )?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(row?.parse()?)).collect()
    }

    pub fn execution_ids_with_journal(&self) -> Result<Vec<ExecutionId>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT id
             FROM local_execution_sessions
             WHERE journal_sequence >= 0
             ORDER BY created_at, id",
        )?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| Ok(row?.parse()?)).collect()
    }

    pub fn mark_interrupted_executions_for_recovery(&self) -> Result<u64, PersistenceError> {
        let connection = self.lock()?;
        let changed = connection.execute(
            "UPDATE local_execution_sessions
             SET status = 'recovery_required',
                 recovery_state = 'recovery_required',
                 error_message = COALESCE(
                    error_message,
                    'Execution was interrupted before a durable terminal state.'
                 )
             WHERE status IN ('running', 'rolling_back')",
            [],
        )?;
        u64::try_from(changed).map_err(|_| PersistenceError::NumericOverflow)
    }

    pub fn mark_execution_recovery_required(
        &self,
        execution_id: ExecutionId,
        message: &str,
    ) -> Result<bool, PersistenceError> {
        let connection = self.lock()?;
        let message = message.chars().take(2_048).collect::<String>();
        let changed = connection.execute(
            "UPDATE local_execution_sessions
             SET status = 'recovery_required',
                 recovery_state = 'recovery_required',
                 current_operation = NULL,
                 error_message = ?2
             WHERE id = ?1 AND status IN ('running', 'rolling_back')",
            params![execution_id.to_string(), message],
        )?;
        Ok(changed == 1)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn persist_execution_event(
        &self,
        event: &OperationJournalEvent,
        canonical_data_json: &str,
        operation_status: Option<ExecutionOperationStatus>,
        session_status: Option<OrganizationExecutionStatus>,
        post_fingerprint: Option<&FileFingerprint>,
        error_code: Option<&str>,
        error_message: Option<&str>,
        created_at: &str,
    ) -> Result<(), PersistenceError> {
        self.persist_execution_event_inner(
            event,
            canonical_data_json,
            operation_status,
            session_status,
            post_fingerprint,
            error_code,
            error_message,
            created_at,
            None,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn persist_executor_request_intent(
        &self,
        event: &OperationJournalEvent,
        canonical_data_json: &str,
        operation_status: ExecutionOperationStatus,
        session_status: OrganizationExecutionStatus,
        request: &ExecutorRequestIdentity,
        created_at: &str,
    ) -> Result<(), PersistenceError> {
        self.persist_execution_event_inner(
            event,
            canonical_data_json,
            Some(operation_status),
            Some(session_status),
            None,
            None,
            None,
            created_at,
            Some(request),
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn persist_execution_event_with_request_proof(
        &self,
        event: &OperationJournalEvent,
        canonical_data_json: &str,
        operation_status: Option<ExecutionOperationStatus>,
        session_status: Option<OrganizationExecutionStatus>,
        post_fingerprint: Option<&FileFingerprint>,
        error_code: Option<&str>,
        error_message: Option<&str>,
        created_at: &str,
        request_id: &str,
        proof_state: ExecutorRequestState,
    ) -> Result<(), PersistenceError> {
        self.persist_execution_event_inner(
            event,
            canonical_data_json,
            operation_status,
            session_status,
            post_fingerprint,
            error_code,
            error_message,
            created_at,
            None,
            Some((request_id, proof_state)),
        )
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(never)]
    fn persist_execution_event_inner(
        &self,
        event: &OperationJournalEvent,
        canonical_data_json: &str,
        operation_status: Option<ExecutionOperationStatus>,
        session_status: Option<OrganizationExecutionStatus>,
        post_fingerprint: Option<&FileFingerprint>,
        error_code: Option<&str>,
        error_message: Option<&str>,
        created_at: &str,
        request_intent: Option<&ExecutorRequestIdentity>,
        request_proof: Option<(&str, ExecutorRequestState)>,
    ) -> Result<(), PersistenceError> {
        if serde_json::from_str::<serde_json::Value>(canonical_data_json).is_err()
            || event.payload != canonical_data_json.as_bytes()
            || event.payload_digest != *blake3::hash(canonical_data_json.as_bytes()).as_bytes()
        {
            return Err(PersistenceError::InvalidExecution);
        }
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let (last_sequence, chain_head): (i64, Option<String>) = transaction.query_row(
            "SELECT journal_sequence, journal_chain_head
             FROM local_execution_sessions
             WHERE id = ?1",
            [event.execution_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        let expected_sequence = last_sequence.saturating_add(1);
        let expected_previous = chain_head.as_deref().map(decode_digest).transpose()?;
        if i64::try_from(event.sequence).map_err(|_| PersistenceError::NumericOverflow)?
            != expected_sequence
            || !event.verify(expected_previous)
        {
            return Err(PersistenceError::InvalidExecution);
        }
        transaction.execute(
            "INSERT INTO local_execution_journal(
                execution_id, sequence_number, operation_id, event_kind,
                canonical_data_json, payload_digest, previous_entry_digest,
                entry_digest, occurred_at_unix_ms, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                event.execution_id.to_string(),
                i64::try_from(event.sequence).map_err(|_| PersistenceError::NumericOverflow)?,
                event.step_id.map(|value| value.to_string()),
                journal_kind_name(event.kind),
                canonical_data_json,
                encode_digest(event.payload_digest),
                event.previous_event_digest.map(encode_digest),
                encode_digest(event.event_digest),
                event.occurred_at_unix_ms,
                created_at,
            ],
        )?;
        if let Some(request) = request_intent {
            if request.execution_id != event.execution_id
                || event.step_id != Some(request.operation_id)
                || !matches!(
                    event.kind,
                    JournalEventKind::IntentDurable | JournalEventKind::RollbackIntent
                )
                || request.request_id.len() != 64
                || request.session_id.len() != 64
                || request.request_digest_hex.len() != 64
                || request.request_sequence == 0
                || request.request_nonce.iter().all(|byte| *byte == 0)
            {
                return Err(PersistenceError::InvalidExecution);
            }
            let payload = serde_json::from_str::<serde_json::Value>(canonical_data_json)
                .map_err(|_| PersistenceError::InvalidExecution)?;
            let stored_request = payload
                .get("executor_request")
                .ok_or(PersistenceError::InvalidExecution)?;
            if stored_request
                .get("request_id")
                .and_then(|value| value.as_str())
                != Some(request.request_id.as_str())
                || stored_request
                    .get("session_id")
                    .and_then(|value| value.as_str())
                    != Some(request.session_id.as_str())
                || stored_request
                    .get("request_sequence")
                    .and_then(serde_json::Value::as_u64)
                    != Some(request.request_sequence)
                || stored_request
                    .get("request_digest")
                    .and_then(|value| value.as_str())
                    != Some(request.request_digest_hex.as_str())
            {
                return Err(PersistenceError::InvalidExecution);
            }
            let session_purpose: String = transaction.query_row(
                "SELECT purpose
                 FROM local_executor_sessions
                 WHERE session_id = ?1 AND execution_id = ?2",
                params![request.session_id, request.execution_id.to_string()],
                |row| row.get(0),
            )?;
            if session_purpose != request.direction.database_name() {
                return Err(PersistenceError::InvalidExecution);
            }
            let mut prior_statement = transaction.prepare(
                "SELECT state
                 FROM local_executor_requests
                 WHERE execution_id = ?1 AND operation_id = ?2 AND direction = ?3",
            )?;
            let prior_states = prior_statement
                .query_map(
                    params![
                        request.execution_id.to_string(),
                        request.operation_id.to_string(),
                        request.direction.database_name(),
                    ],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<Result<Vec<_>, _>>()?;
            drop(prior_statement);
            if (!prior_states.is_empty() && request.direction == ExecutorRequestDirection::Forward)
                || prior_states.iter().any(|state| {
                    !matches!(state.as_str(), "proven_not_applied" | "proven_not_started")
                })
            {
                return Err(PersistenceError::InvalidExecution);
            }
            transaction.execute(
                "INSERT INTO local_executor_requests(
                    request_id, session_id, execution_id, operation_id, direction,
                    request_sequence, request_nonce, request_digest,
                    intent_event_sequence, intent_event_digest, state, prepared_at
                 ) VALUES (
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'intent_durable', ?11
                 )",
                params![
                    request.request_id,
                    request.session_id,
                    request.execution_id.to_string(),
                    request.operation_id.to_string(),
                    request.direction.database_name(),
                    to_sql_u64(request.request_sequence)?,
                    request.request_nonce.as_slice(),
                    request.request_digest_hex,
                    to_sql_u64(event.sequence)?,
                    encode_digest(event.event_digest),
                    created_at,
                ],
            )?;
        }
        transaction.execute(
            "UPDATE local_execution_sessions
             SET journal_sequence = ?2,
                 journal_chain_head = ?3,
                 status = COALESCE(?4, status),
                 recovery_state = CASE
                    WHEN ?4 IN (
                        'completed', 'partial', 'failed', 'cancelled',
                        'rolled_back', 'rollback_partial'
                    ) THEN 'recovery_not_required'
                    ELSE recovery_state
                 END,
                 current_operation = CASE
                    WHEN ?4 = 'running' AND ?6 IS NOT NULL THEN (
                        SELECT destination_relative_path
                        FROM local_execution_operations
                        WHERE execution_id = ?1 AND id = ?6
                    )
                    WHEN ?4 IN (
                        'completed', 'partial', 'failed', 'cancelled', 'paused',
                        'recovery_required', 'recovery_available', 'recovery_ambiguous',
                        'rolled_back', 'rollback_partial'
                    ) THEN NULL
                    ELSE current_operation
                 END,
                 error_message = COALESCE(?5, error_message),
                 started_at = CASE
                    WHEN ?4 = 'running' THEN COALESCE(started_at, ?7)
                    ELSE started_at
                 END,
                 completed_at = CASE
                    WHEN ?4 IN ('completed', 'partial', 'failed', 'cancelled')
                    THEN ?7
                    ELSE completed_at
                 END,
                 rolled_back_at = CASE
                    WHEN ?4 IN ('rolled_back', 'rollback_partial') THEN ?7
                    ELSE rolled_back_at
                 END
             WHERE id = ?1",
            params![
                event.execution_id.to_string(),
                i64::try_from(event.sequence).map_err(|_| PersistenceError::NumericOverflow)?,
                encode_digest(event.event_digest),
                session_status.map(OrganizationExecutionStatus::database_name),
                error_message,
                event.step_id.map(|value| value.to_string()),
                created_at,
            ],
        )?;
        if session_status.is_some_and(|status| {
            matches!(
                status,
                OrganizationExecutionStatus::Completed
                    | OrganizationExecutionStatus::Partial
                    | OrganizationExecutionStatus::Failed
                    | OrganizationExecutionStatus::Cancelled
                    | OrganizationExecutionStatus::RolledBack
                    | OrganizationExecutionStatus::RollbackPartial
            )
        }) {
            transaction.execute(
                "UPDATE local_execution_recovery
                 SET recovery_state = 'recovery_not_required',
                     resolved_at = ?2
                 WHERE execution_id = ?1",
                params![event.execution_id.to_string(), created_at],
            )?;
        }
        if let Some(step_id) = event.step_id {
            let fingerprint_json = post_fingerprint
                .map(serde_json::to_string)
                .transpose()
                .map_err(|_| PersistenceError::InvalidExecution)?;
            let changed = transaction.execute(
                "UPDATE local_execution_operations
                 SET status = COALESCE(?3, status),
                     post_fingerprint_json = COALESCE(?4, post_fingerprint_json),
                     error_code = COALESCE(?5, error_code),
                     error_message = COALESCE(?6, error_message),
                     started_at = CASE
                        WHEN ?3 IN ('running', 'rolling_back')
                        THEN COALESCE(started_at, ?7)
                        ELSE started_at
                     END,
                     completed_at = CASE
                        WHEN ?3 IN ('applied', 'failed', 'skipped', 'recovered')
                        THEN ?7
                        ELSE completed_at
                     END,
                     rolled_back_at = CASE
                        WHEN ?3 = 'rolled_back' THEN ?7 ELSE rolled_back_at
                     END
                 WHERE execution_id = ?1 AND id = ?2",
                params![
                    event.execution_id.to_string(),
                    step_id.to_string(),
                    operation_status.map(ExecutionOperationStatus::database_name),
                    fingerprint_json,
                    error_code,
                    error_message,
                    created_at,
                ],
            )?;
            if changed != 1 {
                return Err(PersistenceError::InvalidExecution);
            }
            if matches!(
                operation_status,
                Some(ExecutionOperationStatus::Applied | ExecutionOperationStatus::Recovered)
            ) {
                insert_rollback_record(&transaction, event.execution_id, step_id, created_at)?;
            }
        }
        refresh_execution_counts(&transaction, event.execution_id)?;
        if let Some((request_id, proof_state)) = request_proof {
            transition_executor_request_state(
                &transaction,
                request_id,
                proof_state,
                created_at,
                true,
            )?;
        }
        refresh_execution_retention(&transaction, event.execution_id, session_status, created_at)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn persist_executor_session(
        &self,
        identity: &ExecutorSessionIdentity,
    ) -> Result<(), PersistenceError> {
        if identity.session_id.len() != 64
            || identity.plan_digest_hex.len() != 64
            || identity.coordinator_pid == 0
            || identity.child_pid == Some(0)
            || identity.worker_nonce_hash_hex.len() != 64
            || identity.coordinator_nonce_hash_hex.len() != 64
            || identity
                .response_nonce_hash_hex
                .as_ref()
                .is_some_and(|value| value.len() != 64)
            || identity.opened_at_unix_ms < 0
        {
            return Err(PersistenceError::InvalidExecution);
        }
        let connection = self.lock()?;
        let changed = connection.execute(
            "INSERT INTO local_executor_sessions(
                session_id, execution_id, plan_id, plan_digest, purpose,
                coordinator_pid, child_pid, worker_nonce_hash,
                coordinator_nonce_hash, response_nonce_hash, opened_at_unix_ms
             )
             SELECT ?1, execution.id, execution.plan_id, execution.plan_digest, ?5,
                    ?6, ?7, ?8, ?9, ?10, ?11
             FROM local_execution_sessions AS execution
             WHERE execution.id = ?2
               AND execution.plan_id = ?3
               AND execution.plan_digest = ?4",
            params![
                identity.session_id,
                identity.execution_id.to_string(),
                identity.plan_id.to_string(),
                identity.plan_digest_hex,
                identity.purpose.database_name(),
                i64::from(identity.coordinator_pid),
                identity.child_pid.map(i64::from),
                identity.worker_nonce_hash_hex,
                identity.coordinator_nonce_hash_hex,
                identity.response_nonce_hash_hex,
                identity.opened_at_unix_ms,
            ],
        )?;
        if changed != 1 {
            return Err(PersistenceError::InvalidExecution);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_executor_response(
        &self,
        request_id: &str,
        response_digest_hex: &str,
        outcome_class: &str,
        attempt_count: Option<u8>,
        error_class: Option<&str>,
        state: ExecutorRequestState,
        recorded_at: &str,
    ) -> Result<(), PersistenceError> {
        if request_id.len() != 64
            || response_digest_hex.len() != 64
            || !matches!(
                outcome_class,
                "success"
                    | "proven_not_applied"
                    | "recovery_required"
                    | "protocol_refusal"
                    | "transport_ambiguous"
            )
            || attempt_count.is_some_and(|value| !(1..=3).contains(&value))
            || error_class.is_some_and(|value| value.is_empty() || value.len() > 128)
            || !matches!(
                state,
                ExecutorRequestState::AcknowledgedSuccess
                    | ExecutorRequestState::ProvenNotApplied
                    | ExecutorRequestState::RecoveryRequired
            )
        {
            return Err(PersistenceError::InvalidExecution);
        }
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let current = executor_request_state(&transaction, request_id)?;
        if !current.may_transition_to(state) {
            return Err(PersistenceError::InvalidExecution);
        }
        let changed = transaction.execute(
            "UPDATE local_executor_requests
             SET state = ?2,
                 response_digest = ?3,
                 outcome_class = ?4,
                 attempt_count = ?5,
                 error_class = ?6,
                 response_recorded_at = ?7
             WHERE request_id = ?1 AND state = ?8 AND response_digest IS NULL",
            params![
                request_id,
                state.database_name(),
                response_digest_hex,
                outcome_class,
                attempt_count.map(i64::from),
                error_class,
                recorded_at,
                current.database_name(),
            ],
        )?;
        if changed != 1 {
            return Err(PersistenceError::InvalidExecution);
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn transition_executor_request_proof(
        &self,
        request_id: &str,
        proof_state: ExecutorRequestState,
        recorded_at: &str,
    ) -> Result<(), PersistenceError> {
        if !matches!(
            proof_state,
            ExecutorRequestState::ProvenNotStarted
                | ExecutorRequestState::ProvenApplied
                | ExecutorRequestState::Ambiguous
                | ExecutorRequestState::RecoveryRequired
        ) {
            return Err(PersistenceError::InvalidExecution);
        }
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        transition_executor_request_state(
            &transaction,
            request_id,
            proof_state,
            recorded_at,
            true,
        )?;
        transaction.commit()?;
        Ok(())
    }

    pub fn executor_session_facts(
        &self,
        execution_id: ExecutionId,
    ) -> Result<Vec<ExecutorSessionFact>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT session_id, plan_id, plan_digest, purpose, coordinator_pid,
                    child_pid, worker_nonce_hash, coordinator_nonce_hash,
                    response_nonce_hash, opened_at_unix_ms
             FROM local_executor_sessions
             WHERE execution_id = ?1
             ORDER BY opened_at_unix_ms, session_id",
        )?;
        let rows = statement.query_map([execution_id.to_string()], |row| {
            Ok(StoredExecutorSessionFact {
                session_id: row.get(0)?,
                plan_id: row.get(1)?,
                plan_digest: row.get(2)?,
                purpose: row.get(3)?,
                coordinator_pid: row.get(4)?,
                child_pid: row.get(5)?,
                worker_nonce_hash: row.get(6)?,
                coordinator_nonce_hash: row.get(7)?,
                response_nonce_hash: row.get(8)?,
                opened_at_unix_ms: row.get(9)?,
            })
        })?;
        let mut facts = Vec::new();
        for row in rows {
            let row = row?;
            facts.push(ExecutorSessionFact {
                session_id: row.session_id,
                execution_id,
                plan_id: row.plan_id.parse()?,
                plan_digest_hex: row.plan_digest,
                purpose: parse_executor_session_purpose(&row.purpose)?,
                coordinator_pid: u32::try_from(row.coordinator_pid)
                    .map_err(|_| PersistenceError::NumericOverflow)?,
                child_pid: row
                    .child_pid
                    .map(u32::try_from)
                    .transpose()
                    .map_err(|_| PersistenceError::NumericOverflow)?,
                worker_nonce_hash_hex: row.worker_nonce_hash,
                coordinator_nonce_hash_hex: row.coordinator_nonce_hash,
                response_nonce_hash_hex: row.response_nonce_hash,
                opened_at_unix_ms: row.opened_at_unix_ms,
            });
        }
        Ok(facts)
    }

    pub fn executor_request_facts(
        &self,
        execution_id: ExecutionId,
    ) -> Result<Vec<ExecutorRequestFact>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT request_id, session_id, operation_id, direction, request_sequence,
                    request_nonce, request_digest, intent_event_sequence, intent_event_digest, state,
                    response_digest, outcome_class, attempt_count, error_class
             FROM local_executor_requests
             WHERE execution_id = ?1
             ORDER BY intent_event_sequence, request_id",
        )?;
        let rows = statement.query_map([execution_id.to_string()], |row| {
            Ok(StoredExecutorRequestFact {
                request_id: row.get(0)?,
                session_id: row.get(1)?,
                operation_id: row.get(2)?,
                direction: row.get(3)?,
                request_sequence: row.get(4)?,
                request_nonce: row.get(5)?,
                request_digest: row.get(6)?,
                intent_event_sequence: row.get(7)?,
                intent_event_digest: row.get(8)?,
                state: row.get(9)?,
                response_digest: row.get(10)?,
                outcome_class: row.get(11)?,
                attempt_count: row.get(12)?,
                error_class: row.get(13)?,
            })
        })?;
        let mut facts = Vec::new();
        for row in rows {
            let row = row?;
            let request_nonce: [u8; 32] = row
                .request_nonce
                .as_slice()
                .try_into()
                .map_err(|_| PersistenceError::InvalidExecution)?;
            facts.push(ExecutorRequestFact {
                request_id: row.request_id,
                session_id: row.session_id,
                operation_id: row.operation_id.parse()?,
                direction: parse_executor_request_direction(&row.direction)?,
                request_sequence: from_sql_u64(row.request_sequence)?,
                request_nonce_hash_hex: domain::executor_nonce_hash(&request_nonce),
                request_digest_hex: row.request_digest,
                intent_event_sequence: from_sql_u64(row.intent_event_sequence)?,
                intent_event_digest_hex: row.intent_event_digest,
                state: parse_executor_request_state(&row.state)?,
                response_digest_hex: row.response_digest,
                outcome_class: row.outcome_class,
                attempt_count: row
                    .attempt_count
                    .map(u8::try_from)
                    .transpose()
                    .map_err(|_| PersistenceError::NumericOverflow)?,
                error_class: row.error_class,
            });
        }
        Ok(facts)
    }

    pub fn execution_retention_metadata(
        &self,
        execution_id: ExecutionId,
    ) -> Result<ExecutionRetentionMetadata, PersistenceError> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT finalized_at, journal_retention_reason, rollback_retention_reason,
                        minimum_retain_until, active_recovery, rollback_eligible,
                        cleanup_eligible_at, cleanup_eligibility_reason, updated_at
                 FROM local_execution_retention
                 WHERE execution_id = ?1",
                [execution_id.to_string()],
                |row| {
                    Ok(ExecutionRetentionMetadata {
                        execution_id,
                        finalized_at: row.get(0)?,
                        journal_retention_reason: row.get(1)?,
                        rollback_retention_reason: row.get(2)?,
                        minimum_retain_until: row.get(3)?,
                        active_recovery: row.get::<_, i64>(4)? != 0,
                        rollback_eligible: row.get::<_, i64>(5)? != 0,
                        cleanup_eligible_at: row.get(6)?,
                        cleanup_eligibility_reason: row.get(7)?,
                        updated_at: row.get(8)?,
                    })
                },
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)
    }

    pub fn request_execution_pause(
        &self,
        execution_id: ExecutionId,
    ) -> Result<bool, PersistenceError> {
        self.set_execution_control(execution_id, true, false)
    }

    pub fn request_execution_cancel(
        &self,
        execution_id: ExecutionId,
    ) -> Result<bool, PersistenceError> {
        self.set_execution_control(execution_id, false, true)
    }

    pub fn cancel_unstarted_execution(
        &self,
        execution_id: ExecutionId,
        completed_at: &str,
    ) -> Result<bool, PersistenceError> {
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        let changed = transaction.execute(
            "UPDATE local_execution_sessions
             SET status = 'cancelled',
                 cancel_requested = 1,
                 completed_at = ?2,
                 rollback_available = 0
             WHERE id = ?1
               AND status IN ('awaiting_confirmation', 'approved', 'paused')
               AND applied_count = 0
               AND NOT EXISTS(
                    SELECT 1 FROM local_execution_operations
                    WHERE execution_id = ?1 AND status IN ('running', 'applied', 'recovered')
               )",
            params![execution_id.to_string(), completed_at],
        )?;
        if changed == 1 {
            refresh_execution_retention(
                &transaction,
                execution_id,
                Some(OrganizationExecutionStatus::Cancelled),
                completed_at,
            )?;
        }
        transaction.commit()?;
        Ok(changed == 1)
    }

    pub fn execution_control(
        &self,
        execution_id: ExecutionId,
    ) -> Result<(bool, bool), PersistenceError> {
        let connection = self.lock()?;
        connection
            .query_row(
                "SELECT pause_requested, cancel_requested
                 FROM local_execution_sessions
                 WHERE id = ?1",
                [execution_id.to_string()],
                |row| Ok((row.get::<_, i64>(0)? != 0, row.get::<_, i64>(1)? != 0)),
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)
    }

    pub fn clear_execution_pause(&self, execution_id: ExecutionId) -> Result<(), PersistenceError> {
        let connection = self.lock()?;
        let changed = connection.execute(
            "UPDATE local_execution_sessions
             SET pause_requested = 0
             WHERE id = ?1 AND status = 'paused'",
            [execution_id.to_string()],
        )?;
        if changed != 1 {
            return Err(PersistenceError::InvalidExecution);
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn persist_recovery_assessment(
        &self,
        execution_id: ExecutionId,
        state: ExecutionRecoveryState,
        not_started: u64,
        applied: u64,
        ambiguous: u64,
        details_json: &str,
        assessed_at: &str,
    ) -> Result<(), PersistenceError> {
        if serde_json::from_str::<serde_json::Value>(details_json).is_err() {
            return Err(PersistenceError::InvalidExecution);
        }
        let session_status = match state {
            ExecutionRecoveryState::RecoveryNotRequired => "paused",
            ExecutionRecoveryState::RecoveryAvailable => "recovery_available",
            ExecutionRecoveryState::RecoveryRequired => "recovery_required",
            ExecutionRecoveryState::RecoveryAmbiguous => "recovery_ambiguous",
        };
        let connection = self.lock()?;
        let transaction = connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO local_execution_recovery(
                execution_id, recovery_state, not_started_count, applied_count,
                ambiguous_count, details_json, assessed_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(execution_id) DO UPDATE SET
                recovery_state = excluded.recovery_state,
                not_started_count = excluded.not_started_count,
                applied_count = excluded.applied_count,
                ambiguous_count = excluded.ambiguous_count,
                details_json = excluded.details_json,
                assessed_at = excluded.assessed_at,
                resolved_at = NULL",
            params![
                execution_id.to_string(),
                state.database_name(),
                to_sql_u64(not_started)?,
                to_sql_u64(applied)?,
                to_sql_u64(ambiguous)?,
                details_json,
                assessed_at,
            ],
        )?;
        transaction.execute(
            "UPDATE local_execution_sessions
             SET status = ?2,
                 recovery_state = ?3
             WHERE id = ?1",
            params![
                execution_id.to_string(),
                session_status,
                state.database_name()
            ],
        )?;
        refresh_execution_counts(&transaction, execution_id)?;
        refresh_execution_retention(&transaction, execution_id, None, assessed_at)?;
        transaction.commit()?;
        Ok(())
    }

    pub fn validate_execution_journal(
        &self,
        execution_id: ExecutionId,
    ) -> Result<bool, PersistenceError> {
        let events = self.execution_journal_events(execution_id)?;
        let mut previous = None;
        for (index, event) in events.iter().enumerate() {
            if event.sequence != u64::try_from(index).unwrap_or(u64::MAX) || !event.verify(previous)
            {
                return Ok(false);
            }
            previous = Some(event.event_digest);
        }
        let connection = self.lock()?;
        let stored: Option<String> = connection
            .query_row(
                "SELECT journal_chain_head
                 FROM local_execution_sessions
                 WHERE id = ?1",
                [execution_id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .ok_or(PersistenceError::NotFound)?;
        Ok(stored.as_deref().map(decode_digest).transpose()? == previous)
    }

    pub fn execution_journal_events(
        &self,
        execution_id: ExecutionId,
    ) -> Result<Vec<OperationJournalEvent>, PersistenceError> {
        let connection = self.lock()?;
        let mut statement = connection.prepare(
            "SELECT sequence_number, operation_id, event_kind, canonical_data_json,
                    payload_digest, previous_entry_digest, entry_digest, occurred_at_unix_ms
             FROM local_execution_journal
             WHERE execution_id = ?1
             ORDER BY sequence_number",
        )?;
        let rows = statement.query_map([execution_id.to_string()], |row| {
            Ok(StoredJournalRow {
                sequence: row.get(0)?,
                operation_id: row.get(1)?,
                kind: row.get(2)?,
                payload: row.get(3)?,
                payload_digest: row.get(4)?,
                previous_digest: row.get(5)?,
                digest: row.get(6)?,
                occurred_at_unix_ms: row.get(7)?,
            })
        })?;
        let mut output = Vec::new();
        for row in rows {
            let row = row?;
            output.push(OperationJournalEvent {
                execution_id,
                sequence: u64::try_from(row.sequence)
                    .map_err(|_| PersistenceError::NumericOverflow)?,
                step_id: row.operation_id.map(|value| value.parse()).transpose()?,
                kind: parse_journal_kind(&row.kind)?,
                payload: row.payload.into_bytes(),
                payload_digest: decode_digest(&row.payload_digest)?,
                previous_event_digest: row
                    .previous_digest
                    .as_deref()
                    .map(decode_digest)
                    .transpose()?,
                event_digest: decode_digest(&row.digest)?,
                occurred_at_unix_ms: row.occurred_at_unix_ms,
            });
        }
        Ok(output)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn add_execution_error(
        &self,
        execution_id: ExecutionId,
        operation_id: Option<OperationStepId>,
        category: domain::ExecutionFailureCategory,
        code: &str,
        user_message: &str,
        technical_details: Option<&str>,
        created_at: &str,
    ) -> Result<(), PersistenceError> {
        let connection = self.lock()?;
        connection.execute(
            "INSERT INTO local_execution_errors(
                id, execution_id, operation_id, category, error_code,
                user_message, technical_details, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                Uuid::now_v7().to_string(),
                execution_id.to_string(),
                operation_id.map(|value| value.to_string()),
                category.database_name(),
                code,
                user_message,
                technical_details,
                created_at,
            ],
        )?;
        Ok(())
    }

    fn set_execution_control(
        &self,
        execution_id: ExecutionId,
        pause: bool,
        cancel: bool,
    ) -> Result<bool, PersistenceError> {
        let connection = self.lock()?;
        let changed = connection.execute(
            "UPDATE local_execution_sessions
             SET pause_requested = CASE WHEN ?2 = 1 THEN 1 ELSE pause_requested END,
                 cancel_requested = CASE WHEN ?3 = 1 THEN 1 ELSE cancel_requested END
             WHERE id = ?1 AND status = 'running'",
            params![
                execution_id.to_string(),
                i64::from(pause),
                i64::from(cancel)
            ],
        )?;
        Ok(changed == 1)
    }
}

fn reset_legacy_execution_approval(
    transaction: &Transaction<'_>,
    execution_id: ExecutionId,
) -> Result<(), PersistenceError> {
    let session_changed = transaction.execute(
        "UPDATE local_execution_sessions
         SET status = 'awaiting_confirmation',
             user_confirmed = 0,
             approved_at = NULL
         WHERE id = ?1
           AND status IN ('awaiting_confirmation', 'approved')",
        [execution_id.to_string()],
    )?;
    let snapshot_changed = transaction.execute(
        "UPDATE local_execution_approval_snapshots
         SET user_confirmed = 0,
             approved_at = NULL
         WHERE execution_id = ?1",
        [execution_id.to_string()],
    )?;
    if session_changed != 1 || snapshot_changed != 1 {
        return Err(PersistenceError::InvalidExecution);
    }
    Ok(())
}

fn validate_prepared_execution(detail: &ExecutionDetail) -> Result<(), PersistenceError> {
    let session = &detail.session;
    if session.status != OrganizationExecutionStatus::AwaitingConfirmation
        || session.recovery_state != ExecutionRecoveryState::RecoveryNotRequired
        || session.consent != ExecutionConsent::pending()
        || session.approval.material_version != domain::EXECUTION_PLAN_MATERIAL_VERSION
        || session.approval.execution_id != session.id
        || session.approval.plan_id != session.plan_id
        || session.approval.proposal_id != session.proposal_id
        || session.approval.proposal_revision_id != session.proposal_revision_id
        || session.approval.proposal_revision != session.proposal_revision
        || session.approval.source_snapshot_version != session.source_scan_id
        || session.approval.user_confirmed
        || session.plan_digest_hex.len() != 64
        || session.approval.digest_hex != session.plan_digest_hex
        || session.approval.safety_policy.version.is_empty()
        || session.approval.safety_policy.version.len() > 64
        || session.approval.safety_policy.digest_hex.len() != 64
        || session.approval.destination_root.canonical_path.is_empty()
        || session.approval.destination_root.display_path.is_empty()
        || session.approval.destination_root.display_path.len() > 4_096
        || detail.operations.is_empty()
    {
        return Err(PersistenceError::InvalidExecution);
    }
    let approved = session
        .approval
        .approved_operation_ids
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    if approved.len() != session.approval.approved_operation_ids.len()
        || u64::try_from(approved.len()).map_err(|_| PersistenceError::NumericOverflow)?
            != session.approval.operation_count
        || session.approval.operation_count != session.summary.preflight_ok
    {
        return Err(PersistenceError::InvalidExecution);
    }
    let planned = detail
        .operations
        .iter()
        .filter_map(|operation| {
            (operation.status == ExecutionOperationStatus::PreflightOk
                && operation.kind != ExecutionOperationKind::InternalStage)
                .then_some(operation.proposal_operation_id)
                .flatten()
        })
        .collect::<BTreeSet<_>>();
    if approved != planned {
        return Err(PersistenceError::InvalidExecution);
    }
    for (index, operation) in detail.operations.iter().enumerate() {
        if operation.execution_id != session.id
            || usize::try_from(operation.sequence).ok() != Some(index)
        {
            return Err(PersistenceError::InvalidExecution);
        }
    }
    Ok(())
}

fn validate_approved_proposal(
    transaction: &Transaction<'_>,
    detail: &ExecutionDetail,
) -> Result<(), PersistenceError> {
    let session = &detail.session;
    let current: Option<(String, String, i64)> = transaction
        .query_row(
            "SELECT status, current_revision_id,
                    (SELECT revision_number
                     FROM local_organization_proposal_revisions
                     WHERE id = current_revision_id)
             FROM local_organization_proposals
             WHERE id = ?1",
            [session.proposal_id.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((status, revision_id, revision)) = current else {
        return Err(PersistenceError::NotFound);
    };
    if status != "approved_for_future_apply"
        || revision_id != session.proposal_revision_id.to_string()
        || u32::try_from(revision).map_err(|_| PersistenceError::NumericOverflow)?
            != session.proposal_revision
    {
        return Err(PersistenceError::InvalidExecution);
    }
    for operation_id in &session.approval.approved_operation_ids {
        let stored = transaction
            .query_row(
                "SELECT operation_kind, source_relative_path,
                        proposed_destination_json, proposed_name, user_override
                 FROM local_organization_proposal_operations
                 WHERE id = ?1
                   AND revision_id = ?2",
                params![
                    operation_id.to_string(),
                    session.proposal_revision_id.to_string()
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()?;
        let valid = stored.is_some_and(
            |(kind, source, destination_json, proposed_name, user_override)| {
                if matches!(kind.as_str(), "move_proposal" | "rename_proposal") {
                    return true;
                }
                if kind != "keep_in_place" {
                    return false;
                }
                let Ok(destination) = serde_json::from_str::<Vec<String>>(&destination_json) else {
                    return false;
                };
                let destination = destination
                    .into_iter()
                    .chain(std::iter::once(proposed_name))
                    .collect::<Vec<_>>()
                    .join("/");
                let source = source.replace('\\', "/");
                source != destination
                    && source.to_lowercase() == destination.to_lowercase()
                    && user_override != 0
            },
        );
        if !valid {
            return Err(PersistenceError::InvalidExecution);
        }
    }
    Ok(())
}

fn insert_execution_session(
    transaction: &Transaction<'_>,
    session: &ExecutionSession,
) -> Result<(), PersistenceError> {
    let summary = &session.summary;
    transaction.execute(
        "INSERT INTO local_execution_sessions(
            id, plan_id, proposal_id, proposal_revision_id, proposal_revision,
            workspace_id, root_id, source_scan_id, status, recovery_state,
            plan_digest, approved_operation_count, affected_file_count,
            folder_count, move_count, rename_count, unchanged_count,
            conflict_count, needs_review_count, preflight_ok_count, blocked_count,
            skipped_count, applied_count, failed_count, rolled_back_count,
            rollback_blocked_count, rollback_failed_count,
            confirmation_phrase_required, user_confirmed, rollback_available,
            current_operation, created_at, approved_at, started_at, completed_at,
            rolled_back_at, error_message
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13,
            ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25,
            ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?34, ?35, ?36, ?37
         )",
        params![
            session.id.to_string(),
            session.plan_id.to_string(),
            session.proposal_id.to_string(),
            session.proposal_revision_id.to_string(),
            i64::from(session.proposal_revision),
            session.workspace_id.to_string(),
            session.root_id.to_string(),
            session.source_scan_id.to_string(),
            session.status.database_name(),
            session.recovery_state.database_name(),
            session.plan_digest_hex,
            to_sql_u64(session.approval.operation_count)?,
            to_sql_u64(summary.affected_files)?,
            to_sql_u64(summary.folders_to_create)?,
            to_sql_u64(summary.files_to_move)?,
            to_sql_u64(summary.files_to_rename)?,
            to_sql_u64(summary.files_unchanged)?,
            to_sql_u64(summary.conflicts)?,
            to_sql_u64(summary.needs_review)?,
            to_sql_u64(summary.preflight_ok)?,
            to_sql_u64(summary.blocked)?,
            to_sql_u64(summary.skipped)?,
            to_sql_u64(summary.applied)?,
            to_sql_u64(summary.failed)?,
            to_sql_u64(summary.rolled_back)?,
            to_sql_u64(summary.rollback_blocked)?,
            to_sql_u64(summary.rollback_failed)?,
            i64::from(session.confirmation_phrase_required),
            i64::from(session.approval.user_confirmed),
            i64::from(session.rollback_available),
            session.current_operation,
            session.created_at,
            session.approved_at,
            session.started_at,
            session.completed_at,
            session.rolled_back_at,
            session.error,
        ],
    )?;
    Ok(())
}

fn insert_approval_snapshot(
    transaction: &Transaction<'_>,
    approval: &ApprovedExecutionPlan,
    frozen_at: &str,
) -> Result<(), PersistenceError> {
    let operation_ids = serde_json::to_string(&approval.approved_operation_ids)
        .map_err(|_| PersistenceError::InvalidExecution)?;
    transaction.execute(
        "INSERT INTO local_execution_approval_snapshots(
            execution_id, plan_id, proposal_id, proposal_revision_id,
            proposal_revision, source_snapshot_version, approved_operation_ids_json,
            operation_count, user_confirmed, frozen_at, approved_at, snapshot_digest
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
        params![
            approval.execution_id.to_string(),
            approval.plan_id.to_string(),
            approval.proposal_id.to_string(),
            approval.proposal_revision_id.to_string(),
            i64::from(approval.proposal_revision),
            approval.source_snapshot_version.to_string(),
            operation_ids,
            to_sql_u64(approval.operation_count)?,
            i64::from(approval.user_confirmed),
            frozen_at,
            approval.approval_timestamp,
            approval.digest_hex,
        ],
    )?;
    Ok(())
}

fn insert_execution_consent(
    transaction: &Transaction<'_>,
    session: &ExecutionSession,
) -> Result<(), PersistenceError> {
    let approval = &session.approval;
    let path_encoding = match approval.destination_root.canonical_path.encoding {
        PathEncoding::WindowsUtf16Le => "windows_utf16_le",
        PathEncoding::UnixBytes => "unix_bytes",
    };
    let volume_json = serde_json::to_string(&approval.destination_root.volume)
        .map_err(|_| PersistenceError::InvalidExecution)?;
    transaction.execute(
        "INSERT INTO local_execution_consents(
            execution_id, material_version, state, safety_policy_version,
            safety_policy_digest, maximum_rehash_bytes,
            allow_qualified_case_only_rename, destination_root_path_encoding,
            destination_root_canonical, destination_root_display,
            destination_volume_json, state_changed_at_unix_ms
         ) VALUES (?1, ?2, 'pending', ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 0)",
        params![
            session.id.to_string(),
            i64::from(approval.material_version),
            approval.safety_policy.version,
            approval.safety_policy.digest_hex,
            to_sql_u64(approval.safety_policy.maximum_rehash_bytes)?,
            i64::from(approval.safety_policy.allow_qualified_case_only_rename),
            path_encoding,
            approval.destination_root.canonical_path.bytes,
            approval.destination_root.display_path,
            volume_json,
        ],
    )?;
    Ok(())
}

fn insert_execution_operation(
    transaction: &Transaction<'_>,
    operation: &ExecutionOperation,
) -> Result<(), PersistenceError> {
    let fingerprint = operation
        .live_fingerprint
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|_| PersistenceError::InvalidExecution)?;
    let preconditions = serde_json::to_string(&operation.preconditions)
        .map_err(|_| PersistenceError::InvalidExecution)?;
    let dependencies = serde_json::to_string(&operation.dependencies)
        .map_err(|_| PersistenceError::InvalidExecution)?;
    transaction.execute(
        "INSERT INTO local_execution_operations(
            id, execution_id, proposal_operation_id, operation_kind,
            source_relative_path, destination_relative_path,
            original_source_relative_path, expected_source_hash,
            expected_source_size, expected_source_modified_at,
            live_fingerprint_json, preconditions_json, dependencies_json,
            sequence_number, status, directory_existed_before, reason,
            error_code, error_message, started_at, completed_at, rolled_back_at
         ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11,
            ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22
         )",
        params![
            operation.id.to_string(),
            operation.execution_id.to_string(),
            operation
                .proposal_operation_id
                .map(|value| value.to_string()),
            operation.kind.database_name(),
            operation.source_relative_path,
            operation.destination_relative_path,
            operation.original_source_relative_path,
            operation.expected_source_hash,
            operation.expected_source_size.map(to_sql_u64).transpose()?,
            operation.expected_source_modified_at,
            fingerprint,
            preconditions,
            dependencies,
            i64::from(operation.sequence),
            operation.status.database_name(),
            operation.directory_existed_before.map(i64::from),
            operation.reason,
            operation.error_code,
            operation.error_message,
            operation.started_at,
            operation.completed_at,
            operation.rolled_back_at,
        ],
    )?;
    Ok(())
}

fn insert_execution_retention(
    transaction: &Transaction<'_>,
    session: &ExecutionSession,
) -> Result<(), PersistenceError> {
    transaction.execute(
        "INSERT INTO local_execution_retention(
            execution_id, finalized_at, journal_retention_reason,
            rollback_retention_reason, minimum_retain_until, active_recovery,
            rollback_eligible, cleanup_eligible_at,
            cleanup_eligibility_reason, updated_at
         ) VALUES (
            ?1, NULL, 'execution_not_finalized', 'no_applied_operations',
            NULL, 0, 0, NULL, 'execution_not_finalized', ?2
         )",
        params![session.id.to_string(), session.created_at],
    )?;
    Ok(())
}

fn insert_rollback_record(
    transaction: &Transaction<'_>,
    execution_id: ExecutionId,
    operation_id: OperationStepId,
    prepared_at: &str,
) -> Result<(), PersistenceError> {
    transaction.execute(
        "INSERT OR IGNORE INTO local_execution_rollback_records(
            execution_id, operation_id, rollback_order,
            source_relative_path, destination_relative_path,
            expected_fingerprint_json, status, prepared_at
         )
         SELECT execution_id, id, sequence_number,
                destination_relative_path,
                COALESCE(source_relative_path, destination_relative_path),
                post_fingerprint_json, 'available', ?3
         FROM local_execution_operations
         WHERE execution_id = ?1 AND id = ?2",
        params![
            execution_id.to_string(),
            operation_id.to_string(),
            prepared_at
        ],
    )?;
    Ok(())
}

fn refresh_execution_counts(
    transaction: &Transaction<'_>,
    execution_id: ExecutionId,
) -> Result<(), PersistenceError> {
    transaction.execute(
        "UPDATE local_execution_sessions
         SET applied_count = (
                SELECT COUNT(*) FROM local_execution_operations
                WHERE execution_id = ?1 AND status IN ('applied', 'recovered')
                  AND operation_kind NOT IN ('create_directory', 'internal_stage')
             ),
             blocked_count = (
                SELECT COUNT(*) FROM local_execution_operations
                WHERE execution_id = ?1 AND status IN ('blocked', 'stale')
             ),
             skipped_count = (
                SELECT COUNT(*) FROM local_execution_operations
                WHERE execution_id = ?1 AND status = 'skipped'
             ),
             failed_count = (
                SELECT COUNT(*) FROM local_execution_operations
                WHERE execution_id = ?1 AND status = 'failed'
             ),
             rolled_back_count = (
                SELECT COUNT(*) FROM local_execution_operations
                WHERE execution_id = ?1 AND status = 'rolled_back'
                  AND operation_kind NOT IN ('create_directory', 'internal_stage')
             ),
             rollback_blocked_count = (
                SELECT COUNT(*) FROM local_execution_operations
                WHERE execution_id = ?1 AND status = 'rollback_blocked'
             ),
             rollback_failed_count = (
                SELECT COUNT(*) FROM local_execution_operations
                WHERE execution_id = ?1 AND status = 'rollback_failed'
             ),
             rollback_available = CASE
                WHEN status IN ('rolled_back', 'rollback_partial', 'recovery_ambiguous') THEN 0
                ELSE EXISTS(
                    SELECT 1 FROM local_execution_operations
                    WHERE execution_id = ?1 AND status IN ('applied', 'recovered')
                )
             END
         WHERE id = ?1",
        [execution_id.to_string()],
    )?;
    Ok(())
}

fn refresh_execution_retention(
    transaction: &Transaction<'_>,
    execution_id: ExecutionId,
    _session_status: Option<OrganizationExecutionStatus>,
    updated_at: &str,
) -> Result<(), PersistenceError> {
    transaction.execute(
        "UPDATE local_execution_retention
         SET finalized_at = CASE
                WHEN (
                    SELECT status FROM local_execution_sessions WHERE id = ?1
                ) IN (
                    'completed', 'partial', 'failed', 'cancelled',
                    'rolled_back', 'rollback_partial'
                ) THEN COALESCE(finalized_at, ?2)
                ELSE finalized_at
             END,
             minimum_retain_until = CASE
                WHEN (
                    SELECT status FROM local_execution_sessions WHERE id = ?1
                ) IN (
                    'completed', 'partial', 'failed', 'cancelled',
                    'rolled_back', 'rollback_partial'
                ) THEN COALESCE(
                    minimum_retain_until,
                    strftime('%Y-%m-%dT%H:%M:%fZ', julianday(?2) + 30)
                )
                ELSE minimum_retain_until
             END,
             active_recovery = CASE
                WHEN (
                    SELECT recovery_state FROM local_execution_sessions WHERE id = ?1
                ) IN ('recovery_required', 'recovery_ambiguous') THEN 1
                ELSE 0
             END,
             rollback_eligible = CASE
                WHEN (
                    SELECT recovery_state FROM local_execution_sessions WHERE id = ?1
                ) IN ('recovery_required', 'recovery_ambiguous') THEN 0
                WHEN (
                    SELECT rollback_available FROM local_execution_sessions WHERE id = ?1
                ) = 1 THEN 1
                ELSE 0
             END,
             journal_retention_reason = CASE
                WHEN (
                    SELECT recovery_state FROM local_execution_sessions WHERE id = ?1
                ) IN ('recovery_required', 'recovery_ambiguous')
                    THEN 'recovery_evidence_required'
                WHEN (
                    SELECT rollback_available FROM local_execution_sessions WHERE id = ?1
                ) = 1 THEN 'rollback_evidence_required'
                ELSE 'execution_audit_history'
             END,
             rollback_retention_reason = CASE
                WHEN (
                    SELECT rollback_available FROM local_execution_sessions WHERE id = ?1
                ) = 1 THEN 'verified_applied_operations'
                ELSE 'no_rollback_eligible_operations'
             END,
             cleanup_eligible_at = CASE
                WHEN (
                    SELECT recovery_state FROM local_execution_sessions WHERE id = ?1
                ) IN ('recovery_required', 'recovery_ambiguous') THEN NULL
                WHEN (
                    SELECT rollback_available FROM local_execution_sessions WHERE id = ?1
                ) = 1 THEN NULL
                WHEN finalized_at IS NOT NULL THEN COALESCE(
                    minimum_retain_until,
                    strftime('%Y-%m-%dT%H:%M:%fZ', julianday(?2) + 30)
                )
                ELSE NULL
             END,
             cleanup_eligibility_reason = CASE
                WHEN (
                    SELECT recovery_state FROM local_execution_sessions WHERE id = ?1
                ) IN ('recovery_required', 'recovery_ambiguous')
                    THEN 'active_recovery_blocks_cleanup'
                WHEN (
                    SELECT rollback_available FROM local_execution_sessions WHERE id = ?1
                ) = 1 THEN 'rollback_eligibility_blocks_cleanup'
                WHEN finalized_at IS NULL THEN 'execution_not_finalized'
                ELSE 'eligible_after_minimum_retention'
             END,
             updated_at = ?2
         WHERE execution_id = ?1",
        params![execution_id.to_string(), updated_at],
    )?;
    Ok(())
}

fn execution_detail_from_connection(
    connection: &rusqlite::Connection,
    execution_id: ExecutionId,
) -> Result<ExecutionDetail, PersistenceError> {
    Ok(ExecutionDetail {
        session: execution_session_from_connection(connection, execution_id)?,
        operations: execution_operations_from_connection(connection, execution_id)?,
    })
}

fn execution_session_from_connection(
    connection: &rusqlite::Connection,
    execution_id: ExecutionId,
) -> Result<ExecutionSession, PersistenceError> {
    let row = connection
        .query_row(
            "SELECT plan_id, proposal_id, proposal_revision_id, proposal_revision,
                    workspace_id, root_id, source_scan_id, status, recovery_state,
                    plan_digest, affected_file_count, folder_count, move_count,
                    rename_count, unchanged_count, conflict_count, needs_review_count,
                    preflight_ok_count, applied_count, blocked_count, skipped_count,
                    failed_count, rolled_back_count, rollback_blocked_count,
                    rollback_failed_count, current_operation, rollback_available,
                    confirmation_phrase_required, created_at, approved_at, started_at,
                    completed_at, rolled_back_at, error_message
             FROM local_execution_sessions
             WHERE id = ?1",
            [execution_id.to_string()],
            stored_session_row,
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)?;
    let stored_consent = consent_from_connection(connection, execution_id)?;
    let approval = approval_from_connection(connection, execution_id, &stored_consent)?;
    Ok(ExecutionSession {
        id: execution_id,
        plan_id: row.plan_id.parse()?,
        proposal_id: row.proposal_id.parse()?,
        proposal_revision_id: row.proposal_revision_id.parse()?,
        proposal_revision: u32::try_from(row.proposal_revision)
            .map_err(|_| PersistenceError::NumericOverflow)?,
        workspace_id: row.workspace_id.parse()?,
        root_id: row.root_id.parse()?,
        source_scan_id: row.source_scan_id.parse()?,
        status: parse_session_status(&row.status)?,
        recovery_state: parse_recovery_state(&row.recovery_state)?,
        plan_digest_hex: row.plan_digest,
        approval,
        consent: stored_consent.consent,
        summary: ExecutionSummary {
            affected_files: from_sql_u64(row.affected_files)?,
            folders_to_create: from_sql_u64(row.folders)?,
            files_to_move: from_sql_u64(row.moves)?,
            files_to_rename: from_sql_u64(row.renames)?,
            files_unchanged: from_sql_u64(row.unchanged)?,
            conflicts: from_sql_u64(row.conflicts)?,
            needs_review: from_sql_u64(row.needs_review)?,
            preflight_ok: from_sql_u64(row.preflight_ok)?,
            applied: from_sql_u64(row.applied)?,
            blocked: from_sql_u64(row.blocked)?,
            skipped: from_sql_u64(row.skipped)?,
            failed: from_sql_u64(row.failed)?,
            rolled_back: from_sql_u64(row.rolled_back)?,
            rollback_blocked: from_sql_u64(row.rollback_blocked)?,
            rollback_failed: from_sql_u64(row.rollback_failed)?,
        },
        current_operation: row.current_operation,
        rollback_available: row.rollback_available != 0,
        confirmation_phrase_required: row.confirmation_phrase_required != 0,
        created_at: row.created_at,
        approved_at: row.approved_at,
        started_at: row.started_at,
        completed_at: row.completed_at,
        rolled_back_at: row.rolled_back_at,
        error: row.error,
    })
}

fn approval_from_connection(
    connection: &rusqlite::Connection,
    execution_id: ExecutionId,
    stored_consent: &StoredExecutionConsent,
) -> Result<ApprovedExecutionPlan, PersistenceError> {
    let row: (
        String,
        String,
        String,
        i64,
        String,
        String,
        i64,
        i64,
        Option<String>,
        String,
    ) = connection
        .query_row(
            "SELECT plan_id, proposal_id, proposal_revision_id, proposal_revision,
                        source_snapshot_version, approved_operation_ids_json,
                        operation_count, user_confirmed, approved_at, snapshot_digest
                 FROM local_execution_approval_snapshots
                 WHERE execution_id = ?1",
            [execution_id.to_string()],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                ))
            },
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)?;
    let operation_ids = serde_json::from_str::<Vec<ProposalItemId>>(&row.5)
        .map_err(|_| PersistenceError::InvalidExecution)?;
    Ok(ApprovedExecutionPlan {
        material_version: stored_consent.material_version,
        execution_id,
        plan_id: row.0.parse()?,
        proposal_id: row.1.parse()?,
        proposal_revision_id: row.2.parse()?,
        proposal_revision: u32::try_from(row.3).map_err(|_| PersistenceError::NumericOverflow)?,
        source_snapshot_version: row.4.parse()?,
        approved_operation_ids: operation_ids,
        operation_count: from_sql_u64(row.6)?,
        destination_root: stored_consent.destination_root.clone(),
        safety_policy: stored_consent.safety_policy.clone(),
        approval_timestamp: row.8,
        user_confirmed: row.7 != 0,
        digest_hex: row.9,
    })
}

#[derive(Debug)]
struct StoredExecutionConsent {
    material_version: u32,
    destination_root: ExecutionRootBinding,
    safety_policy: ExecutionSafetyPolicyBinding,
    consent: ExecutionConsent,
}

#[derive(Debug)]
struct StoredExecutionConsentRow {
    material_version: i64,
    state: String,
    issued_at_unix_ms: Option<i64>,
    expires_at_unix_ms: Option<i64>,
    attested_at_unix_ms: Option<i64>,
    consumed_at_unix_ms: Option<i64>,
    invalidated_at_unix_ms: Option<i64>,
    invalidation_reason: Option<String>,
    nonce: Option<Vec<u8>>,
    safety_policy_version: String,
    safety_policy_digest: String,
    maximum_rehash_bytes: i64,
    allow_qualified_case_only_rename: i64,
    root_path_encoding: String,
    root_canonical: Vec<u8>,
    root_display: String,
    volume_json: String,
    attestation_mac: Option<Vec<u8>>,
}

fn consent_from_connection(
    connection: &rusqlite::Connection,
    execution_id: ExecutionId,
) -> Result<StoredExecutionConsent, PersistenceError> {
    let row = connection
        .query_row(
            "SELECT material_version, state, issued_at_unix_ms, expires_at_unix_ms,
                    attested_at_unix_ms, consumed_at_unix_ms, invalidated_at_unix_ms,
                    invalidation_reason, nonce, safety_policy_version,
                    safety_policy_digest, maximum_rehash_bytes,
                    allow_qualified_case_only_rename, destination_root_path_encoding,
                    destination_root_canonical, destination_root_display,
                    destination_volume_json, attestation_mac
             FROM local_execution_consents
             WHERE execution_id = ?1",
            [execution_id.to_string()],
            |row| {
                Ok(StoredExecutionConsentRow {
                    material_version: row.get(0)?,
                    state: row.get(1)?,
                    issued_at_unix_ms: row.get(2)?,
                    expires_at_unix_ms: row.get(3)?,
                    attested_at_unix_ms: row.get(4)?,
                    consumed_at_unix_ms: row.get(5)?,
                    invalidated_at_unix_ms: row.get(6)?,
                    invalidation_reason: row.get(7)?,
                    nonce: row.get(8)?,
                    safety_policy_version: row.get(9)?,
                    safety_policy_digest: row.get(10)?,
                    maximum_rehash_bytes: row.get(11)?,
                    allow_qualified_case_only_rename: row.get(12)?,
                    root_path_encoding: row.get(13)?,
                    root_canonical: row.get(14)?,
                    root_display: row.get(15)?,
                    volume_json: row.get(16)?,
                    attestation_mac: row.get(17)?,
                })
            },
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)?;
    let path_encoding = match row.root_path_encoding.as_str() {
        "windows_utf16_le" => PathEncoding::WindowsUtf16Le,
        "unix_bytes" | "legacy_utf8" => PathEncoding::UnixBytes,
        _ => return Err(PersistenceError::InvalidExecution),
    };
    let destination_root = ExecutionRootBinding {
        canonical_path: NativePath {
            encoding: path_encoding,
            bytes: row.root_canonical,
        },
        display_path: row.root_display,
        volume: serde_json::from_str::<VolumeIdentity>(&row.volume_json)
            .map_err(|_| PersistenceError::InvalidExecution)?,
    };
    let safety_policy = ExecutionSafetyPolicyBinding {
        version: row.safety_policy_version,
        digest_hex: row.safety_policy_digest,
        maximum_rehash_bytes: from_sql_u64(row.maximum_rehash_bytes)?,
        allow_qualified_case_only_rename: row.allow_qualified_case_only_rename != 0,
    };
    let consent = ExecutionConsent {
        state: parse_consent_state(&row.state)?,
        issued_at_unix_ms: row.issued_at_unix_ms,
        expires_at_unix_ms: row.expires_at_unix_ms,
        attested_at_unix_ms: row.attested_at_unix_ms,
        consumed_at_unix_ms: row.consumed_at_unix_ms,
        invalidated_at_unix_ms: row.invalidated_at_unix_ms,
        invalidation_reason: row.invalidation_reason,
        nonce: row.nonce.map(decode_fixed_32).transpose()?,
        attestation_mac: row.attestation_mac.map(decode_fixed_32).transpose()?,
    };
    let stored = StoredExecutionConsent {
        material_version: u32::try_from(row.material_version)
            .map_err(|_| PersistenceError::NumericOverflow)?,
        destination_root,
        safety_policy,
        consent,
    };
    validate_stored_consent(&stored)?;
    Ok(stored)
}

fn validate_stored_consent(stored: &StoredExecutionConsent) -> Result<(), PersistenceError> {
    let consent = &stored.consent;
    let challenge_complete = consent.issued_at_unix_ms.is_some()
        && consent.expires_at_unix_ms.is_some()
        && consent.nonce.is_some();
    let challenge_empty = consent.issued_at_unix_ms.is_none()
        && consent.expires_at_unix_ms.is_none()
        && consent.nonce.is_none();
    if stored.material_version == 0
        || stored.destination_root.canonical_path.is_empty()
        || stored.destination_root.display_path.is_empty()
        || stored.safety_policy.version.is_empty()
        || stored.safety_policy.digest_hex.len() != 64
        || stored.safety_policy.maximum_rehash_bytes == 0
        || stored.safety_policy.maximum_rehash_bytes > domain::MAX_EXECUTION_VERIFICATION_BYTES
        || (!challenge_complete && !challenge_empty)
        || consent
            .expires_at_unix_ms
            .zip(consent.issued_at_unix_ms)
            .is_some_and(|(expires, issued)| expires <= issued)
        || consent.attested_at_unix_ms.is_some() != consent.attestation_mac.is_some()
    {
        return Err(PersistenceError::InvalidExecution);
    }
    match consent.state {
        ExecutionConsentState::Pending => {
            if consent.attested_at_unix_ms.is_some()
                || consent.consumed_at_unix_ms.is_some()
                || consent.invalidated_at_unix_ms.is_some()
            {
                return Err(PersistenceError::InvalidExecution);
            }
        }
        ExecutionConsentState::Attested => {
            if !challenge_complete
                || consent.attested_at_unix_ms.is_none()
                || consent.consumed_at_unix_ms.is_some()
                || consent.invalidated_at_unix_ms.is_some()
            {
                return Err(PersistenceError::InvalidExecution);
            }
        }
        ExecutionConsentState::Consumed => {
            if !challenge_complete
                || consent.attested_at_unix_ms.is_none()
                || consent.consumed_at_unix_ms.is_none()
                || consent.invalidated_at_unix_ms.is_some()
            {
                return Err(PersistenceError::InvalidExecution);
            }
        }
        ExecutionConsentState::Expired => {
            if !challenge_complete
                || consent.consumed_at_unix_ms.is_some()
                || consent.invalidated_at_unix_ms.is_some()
            {
                return Err(PersistenceError::InvalidExecution);
            }
        }
        ExecutionConsentState::Invalidated => {
            if consent.invalidated_at_unix_ms.is_none()
                || consent.invalidation_reason.is_none()
                || consent.consumed_at_unix_ms.is_some()
            {
                return Err(PersistenceError::InvalidExecution);
            }
        }
    }
    Ok(())
}

fn execution_operations_from_connection(
    connection: &rusqlite::Connection,
    execution_id: ExecutionId,
) -> Result<Vec<ExecutionOperation>, PersistenceError> {
    let mut statement = connection.prepare(
        "SELECT id, proposal_operation_id, operation_kind, source_relative_path,
                destination_relative_path, original_source_relative_path,
                expected_source_hash, expected_source_size, expected_source_modified_at,
                live_fingerprint_json, post_fingerprint_json, preconditions_json,
                dependencies_json, sequence_number, status, directory_existed_before,
                reason, error_code, error_message, started_at, completed_at, rolled_back_at
         FROM local_execution_operations
         WHERE execution_id = ?1
         ORDER BY sequence_number",
    )?;
    let rows = statement.query_map([execution_id.to_string()], stored_operation_row)?;
    let mut output = Vec::new();
    for row in rows {
        let row = row?;
        output.push(ExecutionOperation {
            id: row.id.parse()?,
            execution_id,
            proposal_operation_id: row
                .proposal_operation_id
                .map(|value| value.parse())
                .transpose()?,
            kind: parse_operation_kind(&row.kind)?,
            source_relative_path: row.source,
            destination_relative_path: row.destination,
            original_source_relative_path: row.original_source,
            expected_source_hash: row.expected_hash,
            expected_source_size: row.expected_size.map(from_sql_u64).transpose()?,
            expected_source_modified_at: row.expected_modified_at,
            live_fingerprint: row
                .fingerprint_json
                .map(|value| serde_json::from_str(&value))
                .transpose()
                .map_err(|_| PersistenceError::InvalidExecution)?,
            post_fingerprint: row
                .post_fingerprint_json
                .map(|value| serde_json::from_str(&value))
                .transpose()
                .map_err(|_| PersistenceError::InvalidExecution)?,
            preconditions: serde_json::from_str(&row.preconditions_json)
                .map_err(|_| PersistenceError::InvalidExecution)?,
            dependencies: serde_json::from_str(&row.dependencies_json)
                .map_err(|_| PersistenceError::InvalidExecution)?,
            sequence: u32::try_from(row.sequence).map_err(|_| PersistenceError::NumericOverflow)?,
            status: parse_operation_status(&row.status)?,
            directory_existed_before: row.directory_existed_before.map(|value| value != 0),
            reason: row.reason,
            error_code: row.error_code,
            error_message: row.error_message,
            started_at: row.started_at,
            completed_at: row.completed_at,
            rolled_back_at: row.rolled_back_at,
        });
    }
    Ok(output)
}

#[derive(Debug)]
struct StoredSessionRow {
    plan_id: String,
    proposal_id: String,
    proposal_revision_id: String,
    proposal_revision: i64,
    workspace_id: String,
    root_id: String,
    source_scan_id: String,
    status: String,
    recovery_state: String,
    plan_digest: String,
    affected_files: i64,
    folders: i64,
    moves: i64,
    renames: i64,
    unchanged: i64,
    conflicts: i64,
    needs_review: i64,
    preflight_ok: i64,
    applied: i64,
    blocked: i64,
    skipped: i64,
    failed: i64,
    rolled_back: i64,
    rollback_blocked: i64,
    rollback_failed: i64,
    current_operation: Option<String>,
    rollback_available: i64,
    confirmation_phrase_required: i64,
    created_at: String,
    approved_at: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
    rolled_back_at: Option<String>,
    error: Option<String>,
}

fn stored_session_row(row: &Row<'_>) -> rusqlite::Result<StoredSessionRow> {
    Ok(StoredSessionRow {
        plan_id: row.get(0)?,
        proposal_id: row.get(1)?,
        proposal_revision_id: row.get(2)?,
        proposal_revision: row.get(3)?,
        workspace_id: row.get(4)?,
        root_id: row.get(5)?,
        source_scan_id: row.get(6)?,
        status: row.get(7)?,
        recovery_state: row.get(8)?,
        plan_digest: row.get(9)?,
        affected_files: row.get(10)?,
        folders: row.get(11)?,
        moves: row.get(12)?,
        renames: row.get(13)?,
        unchanged: row.get(14)?,
        conflicts: row.get(15)?,
        needs_review: row.get(16)?,
        preflight_ok: row.get(17)?,
        applied: row.get(18)?,
        blocked: row.get(19)?,
        skipped: row.get(20)?,
        failed: row.get(21)?,
        rolled_back: row.get(22)?,
        rollback_blocked: row.get(23)?,
        rollback_failed: row.get(24)?,
        current_operation: row.get(25)?,
        rollback_available: row.get(26)?,
        confirmation_phrase_required: row.get(27)?,
        created_at: row.get(28)?,
        approved_at: row.get(29)?,
        started_at: row.get(30)?,
        completed_at: row.get(31)?,
        rolled_back_at: row.get(32)?,
        error: row.get(33)?,
    })
}

#[derive(Debug)]
struct StoredOperationRow {
    id: String,
    proposal_operation_id: Option<String>,
    kind: String,
    source: Option<String>,
    destination: String,
    original_source: Option<String>,
    expected_hash: Option<String>,
    expected_size: Option<i64>,
    expected_modified_at: Option<String>,
    fingerprint_json: Option<String>,
    post_fingerprint_json: Option<String>,
    preconditions_json: String,
    dependencies_json: String,
    sequence: i64,
    status: String,
    directory_existed_before: Option<i64>,
    reason: Option<String>,
    error_code: Option<String>,
    error_message: Option<String>,
    started_at: Option<String>,
    completed_at: Option<String>,
    rolled_back_at: Option<String>,
}

fn stored_operation_row(row: &Row<'_>) -> rusqlite::Result<StoredOperationRow> {
    Ok(StoredOperationRow {
        id: row.get(0)?,
        proposal_operation_id: row.get(1)?,
        kind: row.get(2)?,
        source: row.get(3)?,
        destination: row.get(4)?,
        original_source: row.get(5)?,
        expected_hash: row.get(6)?,
        expected_size: row.get(7)?,
        expected_modified_at: row.get(8)?,
        fingerprint_json: row.get(9)?,
        post_fingerprint_json: row.get(10)?,
        preconditions_json: row.get(11)?,
        dependencies_json: row.get(12)?,
        sequence: row.get(13)?,
        status: row.get(14)?,
        directory_existed_before: row.get(15)?,
        reason: row.get(16)?,
        error_code: row.get(17)?,
        error_message: row.get(18)?,
        started_at: row.get(19)?,
        completed_at: row.get(20)?,
        rolled_back_at: row.get(21)?,
    })
}

#[derive(Debug)]
struct StoredExecutorSessionFact {
    session_id: String,
    plan_id: String,
    plan_digest: String,
    purpose: String,
    coordinator_pid: i64,
    child_pid: Option<i64>,
    worker_nonce_hash: String,
    coordinator_nonce_hash: String,
    response_nonce_hash: Option<String>,
    opened_at_unix_ms: i64,
}

#[derive(Debug)]
struct StoredExecutorRequestFact {
    request_id: String,
    session_id: String,
    operation_id: String,
    direction: String,
    request_sequence: i64,
    request_nonce: Vec<u8>,
    request_digest: String,
    intent_event_sequence: i64,
    intent_event_digest: String,
    state: String,
    response_digest: Option<String>,
    outcome_class: Option<String>,
    attempt_count: Option<i64>,
    error_class: Option<String>,
}

#[derive(Debug)]
struct StoredJournalRow {
    sequence: i64,
    operation_id: Option<String>,
    kind: String,
    payload: String,
    payload_digest: String,
    previous_digest: Option<String>,
    digest: String,
    occurred_at_unix_ms: i64,
}

fn executor_request_state(
    transaction: &Transaction<'_>,
    request_id: &str,
) -> Result<ExecutorRequestState, PersistenceError> {
    let state = transaction
        .query_row(
            "SELECT state FROM local_executor_requests WHERE request_id = ?1",
            [request_id],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or(PersistenceError::NotFound)?;
    parse_executor_request_state(&state)
}

fn transition_executor_request_state(
    transaction: &Transaction<'_>,
    request_id: &str,
    next: ExecutorRequestState,
    recorded_at: &str,
    proof: bool,
) -> Result<(), PersistenceError> {
    let current = executor_request_state(transaction, request_id)?;
    if !current.may_transition_to(next) {
        return Err(PersistenceError::InvalidExecution);
    }
    let changed = transaction.execute(
        "UPDATE local_executor_requests
         SET state = ?2,
             proof_recorded_at = CASE
                WHEN ?3 = 1 THEN COALESCE(proof_recorded_at, ?4)
                ELSE proof_recorded_at
             END
         WHERE request_id = ?1 AND state = ?5",
        params![
            request_id,
            next.database_name(),
            i64::from(proof),
            recorded_at,
            current.database_name(),
        ],
    )?;
    if changed != 1 {
        return Err(PersistenceError::InvalidExecution);
    }
    Ok(())
}

fn parse_executor_session_purpose(value: &str) -> Result<ExecutorSessionPurpose, PersistenceError> {
    match value {
        "forward" => Ok(ExecutorSessionPurpose::Forward),
        "rollback" => Ok(ExecutorSessionPurpose::Rollback),
        _ => Err(PersistenceError::InvalidExecution),
    }
}

fn parse_executor_request_direction(
    value: &str,
) -> Result<ExecutorRequestDirection, PersistenceError> {
    match value {
        "forward" => Ok(ExecutorRequestDirection::Forward),
        "rollback" => Ok(ExecutorRequestDirection::Rollback),
        _ => Err(PersistenceError::InvalidExecution),
    }
}

fn parse_executor_request_state(value: &str) -> Result<ExecutorRequestState, PersistenceError> {
    match value {
        "intent_durable" => Ok(ExecutorRequestState::IntentDurable),
        "acknowledged_success" => Ok(ExecutorRequestState::AcknowledgedSuccess),
        "proven_not_applied" => Ok(ExecutorRequestState::ProvenNotApplied),
        "recovery_required" => Ok(ExecutorRequestState::RecoveryRequired),
        "proven_not_started" => Ok(ExecutorRequestState::ProvenNotStarted),
        "proven_applied" => Ok(ExecutorRequestState::ProvenApplied),
        "ambiguous" => Ok(ExecutorRequestState::Ambiguous),
        _ => Err(PersistenceError::InvalidExecution),
    }
}

fn parse_session_status(value: &str) -> Result<OrganizationExecutionStatus, PersistenceError> {
    match value {
        "prepared" => Ok(OrganizationExecutionStatus::Prepared),
        "awaiting_confirmation" => Ok(OrganizationExecutionStatus::AwaitingConfirmation),
        "approved" => Ok(OrganizationExecutionStatus::Approved),
        "running" => Ok(OrganizationExecutionStatus::Running),
        "paused" => Ok(OrganizationExecutionStatus::Paused),
        "cancelled" => Ok(OrganizationExecutionStatus::Cancelled),
        "completed" => Ok(OrganizationExecutionStatus::Completed),
        "partial" => Ok(OrganizationExecutionStatus::Partial),
        "failed" => Ok(OrganizationExecutionStatus::Failed),
        "recovery_required" => Ok(OrganizationExecutionStatus::RecoveryRequired),
        "recovery_available" => Ok(OrganizationExecutionStatus::RecoveryAvailable),
        "recovery_ambiguous" => Ok(OrganizationExecutionStatus::RecoveryAmbiguous),
        "rolling_back" => Ok(OrganizationExecutionStatus::RollingBack),
        "rolled_back" => Ok(OrganizationExecutionStatus::RolledBack),
        "rollback_partial" => Ok(OrganizationExecutionStatus::RollbackPartial),
        _ => Err(PersistenceError::InvalidExecution),
    }
}

fn parse_recovery_state(value: &str) -> Result<ExecutionRecoveryState, PersistenceError> {
    match value {
        "recovery_not_required" => Ok(ExecutionRecoveryState::RecoveryNotRequired),
        "recovery_available" => Ok(ExecutionRecoveryState::RecoveryAvailable),
        "recovery_required" => Ok(ExecutionRecoveryState::RecoveryRequired),
        "recovery_ambiguous" => Ok(ExecutionRecoveryState::RecoveryAmbiguous),
        _ => Err(PersistenceError::InvalidExecution),
    }
}

fn parse_consent_state(value: &str) -> Result<ExecutionConsentState, PersistenceError> {
    match value {
        "pending" => Ok(ExecutionConsentState::Pending),
        "attested" => Ok(ExecutionConsentState::Attested),
        "consumed" => Ok(ExecutionConsentState::Consumed),
        "expired" => Ok(ExecutionConsentState::Expired),
        "invalidated" => Ok(ExecutionConsentState::Invalidated),
        _ => Err(PersistenceError::InvalidExecution),
    }
}

fn parse_operation_kind(value: &str) -> Result<ExecutionOperationKind, PersistenceError> {
    match value {
        "create_directory" => Ok(ExecutionOperationKind::CreateDirectory),
        "move" => Ok(ExecutionOperationKind::Move),
        "rename" => Ok(ExecutionOperationKind::Rename),
        "move_and_rename" => Ok(ExecutionOperationKind::MoveAndRename),
        "internal_stage" => Ok(ExecutionOperationKind::InternalStage),
        _ => Err(PersistenceError::InvalidExecution),
    }
}

fn parse_operation_status(value: &str) -> Result<ExecutionOperationStatus, PersistenceError> {
    match value {
        "planned" => Ok(ExecutionOperationStatus::Planned),
        "preflight_ok" => Ok(ExecutionOperationStatus::PreflightOk),
        "blocked" => Ok(ExecutionOperationStatus::Blocked),
        "running" => Ok(ExecutionOperationStatus::Running),
        "applied" => Ok(ExecutionOperationStatus::Applied),
        "failed" => Ok(ExecutionOperationStatus::Failed),
        "skipped" => Ok(ExecutionOperationStatus::Skipped),
        "stale" => Ok(ExecutionOperationStatus::Stale),
        "recovered" => Ok(ExecutionOperationStatus::Recovered),
        "rolling_back" => Ok(ExecutionOperationStatus::RollingBack),
        "rolled_back" => Ok(ExecutionOperationStatus::RolledBack),
        "rollback_blocked" => Ok(ExecutionOperationStatus::RollbackBlocked),
        "rollback_failed" => Ok(ExecutionOperationStatus::RollbackFailed),
        _ => Err(PersistenceError::InvalidExecution),
    }
}

const fn journal_kind_name(kind: JournalEventKind) -> &'static str {
    match kind {
        JournalEventKind::ApprovedDurable => "approved_durable",
        JournalEventKind::IntentDurable => "intent_durable",
        JournalEventKind::PreconditionsValidated => "preconditions_validated",
        JournalEventKind::AppliedObserved => "applied_observed",
        JournalEventKind::StepFailed => "step_failed",
        JournalEventKind::ExecutionFinished => "execution_finished",
        JournalEventKind::RollbackIntent => "rollback_intent",
        JournalEventKind::RolledBackObserved => "rolled_back_observed",
        JournalEventKind::Conflict => "conflict",
    }
}

fn parse_journal_kind(value: &str) -> Result<JournalEventKind, PersistenceError> {
    match value {
        "approved_durable" => Ok(JournalEventKind::ApprovedDurable),
        "intent_durable" => Ok(JournalEventKind::IntentDurable),
        "preconditions_validated" => Ok(JournalEventKind::PreconditionsValidated),
        "applied_observed" => Ok(JournalEventKind::AppliedObserved),
        "step_failed" => Ok(JournalEventKind::StepFailed),
        "execution_finished" => Ok(JournalEventKind::ExecutionFinished),
        "rollback_intent" => Ok(JournalEventKind::RollbackIntent),
        "rolled_back_observed" => Ok(JournalEventKind::RolledBackObserved),
        "conflict" => Ok(JournalEventKind::Conflict),
        _ => Err(PersistenceError::InvalidExecution),
    }
}

fn encode_digest(value: [u8; 32]) -> String {
    value.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_digest(value: &str) -> Result<[u8; 32], PersistenceError> {
    if value.len() != 64 {
        return Err(PersistenceError::InvalidExecution);
    }
    let mut output = [0_u8; 32];
    for (index, slot) in output.iter_mut().enumerate() {
        *slot = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| PersistenceError::InvalidExecution)?;
    }
    Ok(output)
}

fn decode_fixed_32(value: Vec<u8>) -> Result<[u8; 32], PersistenceError> {
    value
        .try_into()
        .map_err(|_| PersistenceError::InvalidExecution)
}
