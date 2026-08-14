use super::{
    CONSENT_ATTESTATION_MAC_DOMAIN, CONSENT_KEY_DOMAIN, HELLO_MAC_DOMAIN, MAX_CLOCK_SKEW_MS,
    MAX_MESSAGE_LIFETIME_MS, MAX_SESSION_LIFETIME_MS, OPEN_SESSION_MAC_DOMAIN, PROTOCOL_VERSION,
    REFUSAL_MAC_DOMAIN, REQUEST_MAC_DOMAIN, RESPONSE_MAC_DOMAIN, SCHEMA_VERSION,
    SESSION_KEY_DOMAIN, SESSION_OPENED_MAC_DOMAIN,
};
use super::{
    ConsentAttestationBinding, ExecutorOutcome, FixedBytes32, ImmutableExecutionEnvelope,
    OperationBinding, ProtocolRefusal, ProtocolRefusalCategory, SessionAuthorization,
    ValidationError,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use subtle::ConstantTimeEq;
use zeroize::Zeroize;

#[must_use]
pub fn derive_consent_authority_key(root_authority_key: &[u8; 32]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new_keyed(root_authority_key);
    hasher.update(CONSENT_KEY_DOMAIN);
    *hasher.finalize().as_bytes()
}

pub fn sign_consent_attestation(
    binding: &ConsentAttestationBinding,
    consent_authority_key: &[u8; 32],
) -> Result<FixedBytes32, AuthenticationError> {
    binding.validate()?;
    keyed_mac(
        CONSENT_ATTESTATION_MAC_DOMAIN,
        consent_authority_key,
        binding,
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub protocol_version: u16,
    pub schema_version: u16,
    pub worker_pid: u32,
    pub worker_nonce: FixedBytes32,
    pub issued_at_unix_ms: i64,
    pub mac: FixedBytes32,
}

impl Hello {
    pub fn signed(
        worker_pid: u32,
        worker_nonce: FixedBytes32,
        issued_at_unix_ms: i64,
        root_authority_key: &[u8; 32],
    ) -> Result<Self, AuthenticationError> {
        if worker_pid == 0 || worker_nonce.is_zero() || issued_at_unix_ms < 0 {
            return Err(AuthenticationError::InvalidBinding);
        }
        let material = HelloMacMaterial {
            protocol_version: PROTOCOL_VERSION,
            schema_version: SCHEMA_VERSION,
            worker_pid,
            worker_nonce,
            issued_at_unix_ms,
        };
        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            schema_version: SCHEMA_VERSION,
            worker_pid,
            worker_nonce,
            issued_at_unix_ms,
            mac: keyed_mac(HELLO_MAC_DOMAIN, root_authority_key, &material)?,
        })
    }

    pub fn verify(
        &self,
        root_authority_key: &[u8; 32],
        now_unix_ms: i64,
    ) -> Result<(), AuthenticationError> {
        require_versions(self.protocol_version, self.schema_version)?;
        if self.worker_pid == 0
            || self.worker_nonce.is_zero()
            || !within_clock_window(self.issued_at_unix_ms, now_unix_ms, MAX_CLOCK_SKEW_MS)
        {
            return Err(AuthenticationError::StaleOrInvalidTime);
        }
        let expected = keyed_mac(
            HELLO_MAC_DOMAIN,
            root_authority_key,
            &HelloMacMaterial {
                protocol_version: self.protocol_version,
                schema_version: self.schema_version,
                worker_pid: self.worker_pid,
                worker_nonce: self.worker_nonce,
                issued_at_unix_ms: self.issued_at_unix_ms,
            },
        )?;
        require_mac(&expected, &self.mac)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenSession {
    pub protocol_version: u16,
    pub schema_version: u16,
    pub child_pid: u32,
    pub coordinator_pid: u32,
    pub worker_nonce: FixedBytes32,
    pub coordinator_nonce: FixedBytes32,
    pub session_id: FixedBytes32,
    pub execution_id: String,
    pub plan_id: String,
    pub plan_digest: FixedBytes32,
    pub issued_at_unix_ms: i64,
    pub expires_at_unix_ms: i64,
    pub authorization: SessionAuthorization,
    pub envelope: ImmutableExecutionEnvelope,
    pub mac: FixedBytes32,
}

impl OpenSession {
    #[allow(clippy::too_many_arguments)]
    pub fn signed(
        child_pid: u32,
        coordinator_pid: u32,
        worker_nonce: FixedBytes32,
        coordinator_nonce: FixedBytes32,
        session_id: FixedBytes32,
        issued_at_unix_ms: i64,
        expires_at_unix_ms: i64,
        authorization: SessionAuthorization,
        envelope: ImmutableExecutionEnvelope,
        root_authority_key: &[u8; 32],
    ) -> Result<Self, AuthenticationError> {
        envelope.validate()?;
        authorization.validate(&envelope)?;
        if child_pid == 0
            || coordinator_pid == 0
            || worker_nonce.is_zero()
            || coordinator_nonce.is_zero()
            || session_id.is_zero()
        {
            return Err(AuthenticationError::InvalidBinding);
        }
        validate_session_time(
            issued_at_unix_ms,
            expires_at_unix_ms,
            &authorization,
            &envelope,
            issued_at_unix_ms,
        )?;
        verify_envelope_consent(
            &envelope,
            root_authority_key,
            issued_at_unix_ms,
            matches!(&authorization, SessionAuthorization::Forward),
        )?;
        let execution_id = envelope.execution_id.clone();
        let plan_id = envelope.plan.plan_id.clone();
        let plan_digest = envelope.plan.digest;
        let material = OpenSessionMacMaterial {
            protocol_version: PROTOCOL_VERSION,
            schema_version: SCHEMA_VERSION,
            child_pid,
            coordinator_pid,
            worker_nonce,
            coordinator_nonce,
            session_id,
            execution_id: &execution_id,
            plan_id: &plan_id,
            plan_digest,
            issued_at_unix_ms,
            expires_at_unix_ms,
            authorization: &authorization,
            envelope: &envelope,
        };
        let mac = keyed_mac(OPEN_SESSION_MAC_DOMAIN, root_authority_key, &material)?;
        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            schema_version: SCHEMA_VERSION,
            child_pid,
            coordinator_pid,
            worker_nonce,
            coordinator_nonce,
            session_id,
            execution_id,
            plan_id,
            plan_digest,
            issued_at_unix_ms,
            expires_at_unix_ms,
            authorization,
            envelope,
            mac,
        })
    }

    pub fn verify(
        &self,
        hello: &Hello,
        root_authority_key: &[u8; 32],
        now_unix_ms: i64,
    ) -> Result<(), AuthenticationError> {
        require_versions(self.protocol_version, self.schema_version)?;
        let expected = keyed_mac(
            OPEN_SESSION_MAC_DOMAIN,
            root_authority_key,
            &OpenSessionMacMaterial {
                protocol_version: self.protocol_version,
                schema_version: self.schema_version,
                child_pid: self.child_pid,
                coordinator_pid: self.coordinator_pid,
                worker_nonce: self.worker_nonce,
                coordinator_nonce: self.coordinator_nonce,
                session_id: self.session_id,
                execution_id: &self.execution_id,
                plan_id: &self.plan_id,
                plan_digest: self.plan_digest,
                issued_at_unix_ms: self.issued_at_unix_ms,
                expires_at_unix_ms: self.expires_at_unix_ms,
                authorization: &self.authorization,
                envelope: &self.envelope,
            },
        )?;
        require_mac(&expected, &self.mac)?;
        self.envelope.validate()?;
        self.authorization.validate(&self.envelope)?;
        if self.child_pid == 0
            || self.coordinator_pid == 0
            || self.worker_nonce.is_zero()
            || self.coordinator_nonce.is_zero()
            || self.session_id.is_zero()
            || self.child_pid != hello.worker_pid
            || self.worker_nonce != hello.worker_nonce
            || self.execution_id != self.envelope.execution_id
            || self.plan_id != self.envelope.plan.plan_id
            || self.plan_digest != self.envelope.plan.digest
        {
            return Err(AuthenticationError::InvalidBinding);
        }
        validate_session_time(
            self.issued_at_unix_ms,
            self.expires_at_unix_ms,
            &self.authorization,
            &self.envelope,
            now_unix_ms,
        )?;
        verify_envelope_consent(
            &self.envelope,
            root_authority_key,
            now_unix_ms,
            matches!(&self.authorization, SessionAuthorization::Forward),
        )
    }
}

pub struct SessionKey([u8; 32]);

impl SessionKey {
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl std::fmt::Debug for SessionKey {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("SessionKey(<redacted>)")
    }
}

impl Drop for SessionKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionOpened {
    pub protocol_version: u16,
    pub schema_version: u16,
    pub session_id: FixedBytes32,
    pub worker_pid: u32,
    pub coordinator_pid: u32,
    pub worker_nonce: FixedBytes32,
    pub coordinator_nonce: FixedBytes32,
    pub response_nonce: FixedBytes32,
    pub issued_at_unix_ms: i64,
    pub mac: FixedBytes32,
}

impl SessionOpened {
    pub fn signed(
        open_session: &OpenSession,
        response_nonce: FixedBytes32,
        issued_at_unix_ms: i64,
        session_key: &SessionKey,
    ) -> Result<Self, AuthenticationError> {
        if response_nonce.is_zero() || issued_at_unix_ms < 0 {
            return Err(AuthenticationError::InvalidBinding);
        }
        let material = SessionOpenedMacMaterial {
            protocol_version: PROTOCOL_VERSION,
            schema_version: SCHEMA_VERSION,
            session_id: open_session.session_id,
            worker_pid: open_session.child_pid,
            coordinator_pid: open_session.coordinator_pid,
            worker_nonce: open_session.worker_nonce,
            coordinator_nonce: open_session.coordinator_nonce,
            response_nonce,
            issued_at_unix_ms,
        };
        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            schema_version: SCHEMA_VERSION,
            session_id: open_session.session_id,
            worker_pid: open_session.child_pid,
            coordinator_pid: open_session.coordinator_pid,
            worker_nonce: open_session.worker_nonce,
            coordinator_nonce: open_session.coordinator_nonce,
            response_nonce,
            issued_at_unix_ms,
            mac: keyed_mac(SESSION_OPENED_MAC_DOMAIN, session_key.as_bytes(), &material)?,
        })
    }

    pub fn verify(
        &self,
        open_session: &OpenSession,
        session_key: &SessionKey,
        now_unix_ms: i64,
    ) -> Result<(), AuthenticationError> {
        require_versions(self.protocol_version, self.schema_version)?;
        let expected = keyed_mac(
            SESSION_OPENED_MAC_DOMAIN,
            session_key.as_bytes(),
            &SessionOpenedMacMaterial {
                protocol_version: self.protocol_version,
                schema_version: self.schema_version,
                session_id: self.session_id,
                worker_pid: self.worker_pid,
                coordinator_pid: self.coordinator_pid,
                worker_nonce: self.worker_nonce,
                coordinator_nonce: self.coordinator_nonce,
                response_nonce: self.response_nonce,
                issued_at_unix_ms: self.issued_at_unix_ms,
            },
        )?;
        require_mac(&expected, &self.mac)?;
        if self.session_id != open_session.session_id
            || self.worker_pid != open_session.child_pid
            || self.coordinator_pid != open_session.coordinator_pid
            || self.worker_nonce != open_session.worker_nonce
            || self.coordinator_nonce != open_session.coordinator_nonce
            || self.response_nonce.is_zero()
            || !within_clock_window(self.issued_at_unix_ms, now_unix_ms, MAX_CLOCK_SKEW_MS)
            || now_unix_ms >= open_session.expires_at_unix_ms
        {
            return Err(AuthenticationError::InvalidBinding);
        }
        Ok(())
    }
}

pub fn derive_session_key(
    root_authority_key: &[u8; 32],
    hello: &Hello,
    open_session: &OpenSession,
) -> Result<SessionKey, AuthenticationError> {
    let encoded = serde_json::to_vec(&SessionKeyMaterial {
        protocol_version: PROTOCOL_VERSION,
        schema_version: SCHEMA_VERSION,
        worker_pid: hello.worker_pid,
        coordinator_pid: open_session.coordinator_pid,
        worker_nonce: hello.worker_nonce,
        coordinator_nonce: open_session.coordinator_nonce,
        session_id: open_session.session_id,
        execution_id: &open_session.execution_id,
        plan_id: &open_session.plan_id,
        plan_digest: open_session.plan_digest,
        authorization: &open_session.authorization,
    })
    .map_err(|_| AuthenticationError::Serialization)?;
    let mut hasher = blake3::Hasher::new_keyed(root_authority_key);
    hasher.update(SESSION_KEY_DOMAIN);
    hasher.update(&encoded);
    Ok(SessionKey(*hasher.finalize().as_bytes()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthenticatedOperationRequest {
    pub protocol_version: u16,
    pub schema_version: u16,
    pub session_id: FixedBytes32,
    pub sequence: u64,
    pub message_nonce: FixedBytes32,
    pub sent_at_unix_ms: i64,
    pub expires_at_unix_ms: i64,
    pub execution_id: String,
    pub plan_id: String,
    pub plan_digest: FixedBytes32,
    pub operation: OperationBinding,
    pub mac: FixedBytes32,
}

impl AuthenticatedOperationRequest {
    #[allow(clippy::too_many_arguments)]
    pub fn signed(
        open_session: &OpenSession,
        sequence: u64,
        message_nonce: FixedBytes32,
        sent_at_unix_ms: i64,
        expires_at_unix_ms: i64,
        operation: OperationBinding,
        session_key: &SessionKey,
    ) -> Result<Self, AuthenticationError> {
        operation.validate()?;
        if sequence == 0 || message_nonce.is_zero() {
            return Err(AuthenticationError::InvalidBinding);
        }
        validate_message_time(
            sent_at_unix_ms,
            expires_at_unix_ms,
            open_session.expires_at_unix_ms,
            sent_at_unix_ms,
        )?;
        let material = OperationRequestMacMaterial {
            protocol_version: PROTOCOL_VERSION,
            schema_version: SCHEMA_VERSION,
            session_id: open_session.session_id,
            sequence,
            message_nonce,
            sent_at_unix_ms,
            expires_at_unix_ms,
            execution_id: &open_session.execution_id,
            plan_id: &open_session.plan_id,
            plan_digest: open_session.plan_digest,
            operation: &operation,
        };
        let mac = keyed_mac(REQUEST_MAC_DOMAIN, session_key.as_bytes(), &material)?;
        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            schema_version: SCHEMA_VERSION,
            session_id: open_session.session_id,
            sequence,
            message_nonce,
            sent_at_unix_ms,
            expires_at_unix_ms,
            execution_id: open_session.execution_id.clone(),
            plan_id: open_session.plan_id.clone(),
            plan_digest: open_session.plan_digest,
            operation,
            mac,
        })
    }

    pub fn verify(
        &self,
        open_session: &OpenSession,
        expected_sequence: u64,
        session_key: &SessionKey,
        now_unix_ms: i64,
    ) -> Result<(), AuthenticationError> {
        require_versions(self.protocol_version, self.schema_version)?;
        let expected = keyed_mac(
            REQUEST_MAC_DOMAIN,
            session_key.as_bytes(),
            &OperationRequestMacMaterial {
                protocol_version: self.protocol_version,
                schema_version: self.schema_version,
                session_id: self.session_id,
                sequence: self.sequence,
                message_nonce: self.message_nonce,
                sent_at_unix_ms: self.sent_at_unix_ms,
                expires_at_unix_ms: self.expires_at_unix_ms,
                execution_id: &self.execution_id,
                plan_id: &self.plan_id,
                plan_digest: self.plan_digest,
                operation: &self.operation,
            },
        )?;
        require_mac(&expected, &self.mac)?;
        self.operation.validate()?;
        if self.sequence == 0
            || self.message_nonce.is_zero()
            || self.session_id != open_session.session_id
            || self.execution_id != open_session.execution_id
            || self.plan_id != open_session.plan_id
            || self.plan_digest != open_session.plan_digest
        {
            return Err(AuthenticationError::InvalidBinding);
        }
        if self.sequence != expected_sequence {
            return Err(AuthenticationError::SequenceMismatch);
        }
        validate_message_time(
            self.sent_at_unix_ms,
            self.expires_at_unix_ms,
            open_session.expires_at_unix_ms,
            now_unix_ms,
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthenticatedOperationResponse {
    pub protocol_version: u16,
    pub schema_version: u16,
    pub session_id: FixedBytes32,
    pub sequence: u64,
    pub message_nonce: FixedBytes32,
    pub sent_at_unix_ms: i64,
    pub expires_at_unix_ms: i64,
    pub execution_id: String,
    pub plan_id: String,
    pub plan_digest: FixedBytes32,
    pub operation: OperationBinding,
    pub outcome: ExecutorOutcome,
    pub mac: FixedBytes32,
}

impl AuthenticatedOperationResponse {
    #[allow(clippy::too_many_arguments)]
    pub fn signed(
        open_session: &OpenSession,
        sequence: u64,
        message_nonce: FixedBytes32,
        sent_at_unix_ms: i64,
        expires_at_unix_ms: i64,
        operation: OperationBinding,
        outcome: ExecutorOutcome,
        session_key: &SessionKey,
    ) -> Result<Self, AuthenticationError> {
        operation.validate()?;
        outcome.validate()?;
        if sequence == 0 || message_nonce.is_zero() {
            return Err(AuthenticationError::InvalidBinding);
        }
        validate_message_time(
            sent_at_unix_ms,
            expires_at_unix_ms,
            open_session.expires_at_unix_ms,
            sent_at_unix_ms,
        )?;
        let material = OperationResponseMacMaterial {
            protocol_version: PROTOCOL_VERSION,
            schema_version: SCHEMA_VERSION,
            session_id: open_session.session_id,
            sequence,
            message_nonce,
            sent_at_unix_ms,
            expires_at_unix_ms,
            execution_id: &open_session.execution_id,
            plan_id: &open_session.plan_id,
            plan_digest: open_session.plan_digest,
            operation: &operation,
            outcome: &outcome,
        };
        let mac = keyed_mac(RESPONSE_MAC_DOMAIN, session_key.as_bytes(), &material)?;
        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            schema_version: SCHEMA_VERSION,
            session_id: open_session.session_id,
            sequence,
            message_nonce,
            sent_at_unix_ms,
            expires_at_unix_ms,
            execution_id: open_session.execution_id.clone(),
            plan_id: open_session.plan_id.clone(),
            plan_digest: open_session.plan_digest,
            operation,
            outcome,
            mac,
        })
    }

    pub fn verify(
        &self,
        open_session: &OpenSession,
        expected_sequence: u64,
        expected_operation: &OperationBinding,
        session_key: &SessionKey,
        now_unix_ms: i64,
    ) -> Result<(), AuthenticationError> {
        require_versions(self.protocol_version, self.schema_version)?;
        let expected = keyed_mac(
            RESPONSE_MAC_DOMAIN,
            session_key.as_bytes(),
            &OperationResponseMacMaterial {
                protocol_version: self.protocol_version,
                schema_version: self.schema_version,
                session_id: self.session_id,
                sequence: self.sequence,
                message_nonce: self.message_nonce,
                sent_at_unix_ms: self.sent_at_unix_ms,
                expires_at_unix_ms: self.expires_at_unix_ms,
                execution_id: &self.execution_id,
                plan_id: &self.plan_id,
                plan_digest: self.plan_digest,
                operation: &self.operation,
                outcome: &self.outcome,
            },
        )?;
        require_mac(&expected, &self.mac)?;
        self.operation.validate()?;
        self.outcome.validate()?;
        if self.sequence == 0
            || self.message_nonce.is_zero()
            || self.session_id != open_session.session_id
            || self.execution_id != open_session.execution_id
            || self.plan_id != open_session.plan_id
            || self.plan_digest != open_session.plan_digest
            || &self.operation != expected_operation
        {
            return Err(AuthenticationError::InvalidBinding);
        }
        if self.sequence != expected_sequence {
            return Err(AuthenticationError::SequenceMismatch);
        }
        validate_message_time(
            self.sent_at_unix_ms,
            self.expires_at_unix_ms,
            open_session.expires_at_unix_ms,
            now_unix_ms,
        )
    }
}

#[derive(Debug)]
pub struct CoordinatorResponseVerifier {
    next_sequence: u64,
    seen_nonces: BTreeSet<FixedBytes32>,
}

impl Default for CoordinatorResponseVerifier {
    fn default() -> Self {
        Self::new()
    }
}

impl CoordinatorResponseVerifier {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next_sequence: 1,
            seen_nonces: BTreeSet::new(),
        }
    }

    pub fn verify_next<'a>(
        &mut self,
        response: &'a AuthenticatedOperationResponse,
        open_session: &OpenSession,
        expected_operation: &OperationBinding,
        session_key: &SessionKey,
        now_unix_ms: i64,
    ) -> Result<&'a ExecutorOutcome, AuthenticationError> {
        response.verify(
            open_session,
            self.next_sequence,
            expected_operation,
            session_key,
            now_unix_ms,
        )?;
        if !self.seen_nonces.insert(response.message_nonce) {
            return Err(AuthenticationError::NonceReplay);
        }
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(AuthenticationError::SequenceMismatch)?;
        Ok(&response.outcome)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HandshakeRefusal {
    pub protocol_version: u16,
    pub schema_version: u16,
    pub worker_pid: u32,
    pub worker_nonce: FixedBytes32,
    pub response_nonce: FixedBytes32,
    pub issued_at_unix_ms: i64,
    pub refusal: ProtocolRefusal,
    pub mac: FixedBytes32,
}

impl HandshakeRefusal {
    pub fn signed(
        hello: &Hello,
        response_nonce: FixedBytes32,
        issued_at_unix_ms: i64,
        refusal: ProtocolRefusal,
        root_authority_key: &[u8; 32],
    ) -> Result<Self, AuthenticationError> {
        refusal.validate()?;
        if response_nonce.is_zero() || issued_at_unix_ms < 0 {
            return Err(AuthenticationError::InvalidBinding);
        }
        let material = HandshakeRefusalMacMaterial {
            protocol_version: PROTOCOL_VERSION,
            schema_version: SCHEMA_VERSION,
            worker_pid: hello.worker_pid,
            worker_nonce: hello.worker_nonce,
            response_nonce,
            issued_at_unix_ms,
            refusal: &refusal,
        };
        let mac = keyed_mac(REFUSAL_MAC_DOMAIN, root_authority_key, &material)?;
        Ok(Self {
            protocol_version: PROTOCOL_VERSION,
            schema_version: SCHEMA_VERSION,
            worker_pid: hello.worker_pid,
            worker_nonce: hello.worker_nonce,
            response_nonce,
            issued_at_unix_ms,
            refusal,
            mac,
        })
    }

    pub fn verify(
        &self,
        hello: &Hello,
        root_authority_key: &[u8; 32],
        now_unix_ms: i64,
    ) -> Result<(), AuthenticationError> {
        require_versions(self.protocol_version, self.schema_version)?;
        let expected = keyed_mac(
            REFUSAL_MAC_DOMAIN,
            root_authority_key,
            &HandshakeRefusalMacMaterial {
                protocol_version: self.protocol_version,
                schema_version: self.schema_version,
                worker_pid: self.worker_pid,
                worker_nonce: self.worker_nonce,
                response_nonce: self.response_nonce,
                issued_at_unix_ms: self.issued_at_unix_ms,
                refusal: &self.refusal,
            },
        )?;
        require_mac(&expected, &self.mac)?;
        self.refusal.validate()?;
        if self.response_nonce.is_zero()
            || self.worker_pid != hello.worker_pid
            || self.worker_nonce != hello.worker_nonce
        {
            return Err(AuthenticationError::InvalidBinding);
        }
        if !within_clock_window(self.issued_at_unix_ms, now_unix_ms, MAX_CLOCK_SKEW_MS) {
            return Err(AuthenticationError::StaleOrInvalidTime);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthenticationError {
    #[error("unsupported executor protocol version")]
    UnsupportedVersion,
    #[error("executor message authentication failed")]
    InvalidMac,
    #[error("executor message binding is invalid")]
    InvalidBinding,
    #[error("executor message sequence is invalid")]
    SequenceMismatch,
    #[error("executor message nonce was replayed")]
    NonceReplay,
    #[error("executor message is stale or has an invalid clock range")]
    StaleOrInvalidTime,
    #[error("executor protocol material could not be serialized")]
    Serialization,
    #[error(transparent)]
    Validation(#[from] ValidationError),
}

impl AuthenticationError {
    #[must_use]
    pub const fn refusal_category(&self) -> ProtocolRefusalCategory {
        match self {
            Self::InvalidMac => ProtocolRefusalCategory::Authentication,
            Self::SequenceMismatch | Self::NonceReplay => ProtocolRefusalCategory::Replay,
            Self::UnsupportedVersion
            | Self::InvalidBinding
            | Self::StaleOrInvalidTime
            | Self::Serialization
            | Self::Validation(_) => ProtocolRefusalCategory::Protocol,
        }
    }

    #[must_use]
    pub const fn refusal_code(&self) -> &'static str {
        match self {
            Self::UnsupportedVersion => "unsupported_protocol",
            Self::InvalidMac => "authentication_failed",
            Self::InvalidBinding => "binding_mismatch",
            Self::SequenceMismatch => "sequence_replay",
            Self::NonceReplay => "message_nonce_replay",
            Self::StaleOrInvalidTime => "stale_or_invalid_time",
            Self::Serialization => "protocol_serialization_failed",
            Self::Validation(_) => "protocol_validation_failed",
        }
    }
}

#[derive(Serialize)]
struct HelloMacMaterial {
    protocol_version: u16,
    schema_version: u16,
    worker_pid: u32,
    worker_nonce: FixedBytes32,
    issued_at_unix_ms: i64,
}

#[derive(Serialize)]
struct OpenSessionMacMaterial<'a> {
    protocol_version: u16,
    schema_version: u16,
    child_pid: u32,
    coordinator_pid: u32,
    worker_nonce: FixedBytes32,
    coordinator_nonce: FixedBytes32,
    session_id: FixedBytes32,
    execution_id: &'a str,
    plan_id: &'a str,
    plan_digest: FixedBytes32,
    issued_at_unix_ms: i64,
    expires_at_unix_ms: i64,
    authorization: &'a SessionAuthorization,
    envelope: &'a ImmutableExecutionEnvelope,
}

#[derive(Serialize)]
struct SessionKeyMaterial<'a> {
    protocol_version: u16,
    schema_version: u16,
    worker_pid: u32,
    coordinator_pid: u32,
    worker_nonce: FixedBytes32,
    coordinator_nonce: FixedBytes32,
    session_id: FixedBytes32,
    execution_id: &'a str,
    plan_id: &'a str,
    plan_digest: FixedBytes32,
    authorization: &'a SessionAuthorization,
}

#[derive(Serialize)]
struct SessionOpenedMacMaterial {
    protocol_version: u16,
    schema_version: u16,
    session_id: FixedBytes32,
    worker_pid: u32,
    coordinator_pid: u32,
    worker_nonce: FixedBytes32,
    coordinator_nonce: FixedBytes32,
    response_nonce: FixedBytes32,
    issued_at_unix_ms: i64,
}

#[derive(Serialize)]
struct OperationRequestMacMaterial<'a> {
    protocol_version: u16,
    schema_version: u16,
    session_id: FixedBytes32,
    sequence: u64,
    message_nonce: FixedBytes32,
    sent_at_unix_ms: i64,
    expires_at_unix_ms: i64,
    execution_id: &'a str,
    plan_id: &'a str,
    plan_digest: FixedBytes32,
    operation: &'a OperationBinding,
}

#[derive(Serialize)]
struct OperationResponseMacMaterial<'a> {
    protocol_version: u16,
    schema_version: u16,
    session_id: FixedBytes32,
    sequence: u64,
    message_nonce: FixedBytes32,
    sent_at_unix_ms: i64,
    expires_at_unix_ms: i64,
    execution_id: &'a str,
    plan_id: &'a str,
    plan_digest: FixedBytes32,
    operation: &'a OperationBinding,
    outcome: &'a ExecutorOutcome,
}

#[derive(Serialize)]
struct HandshakeRefusalMacMaterial<'a> {
    protocol_version: u16,
    schema_version: u16,
    worker_pid: u32,
    worker_nonce: FixedBytes32,
    response_nonce: FixedBytes32,
    issued_at_unix_ms: i64,
    refusal: &'a ProtocolRefusal,
}

fn keyed_mac<T: Serialize>(
    domain: &[u8],
    key: &[u8; 32],
    material: &T,
) -> Result<FixedBytes32, AuthenticationError> {
    let encoded = serde_json::to_vec(material).map_err(|_| AuthenticationError::Serialization)?;
    let mut hasher = blake3::Hasher::new_keyed(key);
    hasher.update(domain);
    hasher.update(&encoded);
    Ok(FixedBytes32::from_bytes(*hasher.finalize().as_bytes()))
}

fn require_mac(
    expected: &FixedBytes32,
    presented: &FixedBytes32,
) -> Result<(), AuthenticationError> {
    if bool::from(expected.as_bytes().ct_eq(presented.as_bytes())) {
        Ok(())
    } else {
        Err(AuthenticationError::InvalidMac)
    }
}

fn require_versions(protocol_version: u16, schema_version: u16) -> Result<(), AuthenticationError> {
    if protocol_version == PROTOCOL_VERSION && schema_version == SCHEMA_VERSION {
        Ok(())
    } else {
        Err(AuthenticationError::UnsupportedVersion)
    }
}

fn validate_session_time(
    issued_at_unix_ms: i64,
    expires_at_unix_ms: i64,
    authorization: &SessionAuthorization,
    envelope: &ImmutableExecutionEnvelope,
    now_unix_ms: i64,
) -> Result<(), AuthenticationError> {
    if issued_at_unix_ms < 0
        || expires_at_unix_ms <= issued_at_unix_ms
        || expires_at_unix_ms.saturating_sub(issued_at_unix_ms) > MAX_SESSION_LIFETIME_MS
        || !within_clock_window(issued_at_unix_ms, now_unix_ms, MAX_CLOCK_SKEW_MS)
        || now_unix_ms >= expires_at_unix_ms
        || matches!(authorization, SessionAuthorization::Forward)
            && expires_at_unix_ms > envelope.consent.expires_at_unix_ms
    {
        return Err(AuthenticationError::StaleOrInvalidTime);
    }
    Ok(())
}

fn validate_message_time(
    sent_at_unix_ms: i64,
    expires_at_unix_ms: i64,
    session_expires_at_unix_ms: i64,
    now_unix_ms: i64,
) -> Result<(), AuthenticationError> {
    if sent_at_unix_ms < 0
        || expires_at_unix_ms <= sent_at_unix_ms
        || expires_at_unix_ms > session_expires_at_unix_ms
        || expires_at_unix_ms.saturating_sub(sent_at_unix_ms) > MAX_MESSAGE_LIFETIME_MS
        || !within_clock_window(sent_at_unix_ms, now_unix_ms, MAX_CLOCK_SKEW_MS)
        || now_unix_ms >= expires_at_unix_ms
    {
        return Err(AuthenticationError::StaleOrInvalidTime);
    }
    Ok(())
}

fn within_clock_window(value: i64, now: i64, window: i64) -> bool {
    value >= now.saturating_sub(window) && value <= now.saturating_add(window)
}

fn verify_envelope_consent(
    envelope: &ImmutableExecutionEnvelope,
    root_authority_key: &[u8; 32],
    now_unix_ms: i64,
    require_unexpired: bool,
) -> Result<(), AuthenticationError> {
    let binding = ConsentAttestationBinding::from_envelope(envelope);
    binding.validate()?;
    let mut consent_key = derive_consent_authority_key(root_authority_key);
    let expected = sign_consent_attestation(&binding, &consent_key)?;
    consent_key.zeroize();
    require_mac(&expected, &envelope.consent.attestation_mac)?;
    if envelope.consent.attested_at_unix_ms < envelope.consent.issued_at_unix_ms
        || (require_unexpired && now_unix_ms >= envelope.consent.expires_at_unix_ms)
    {
        return Err(AuthenticationError::StaleOrInvalidTime);
    }
    Ok(())
}
