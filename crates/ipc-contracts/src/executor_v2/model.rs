use super::{
    MAX_ID_BYTES, MAX_IDENTITY_BYTES, MAX_MANIFESTS, MAX_NATIVE_PATH_BYTES,
    MAX_RELATIVE_PATH_BYTES, MAX_TEXT_BYTES, SCHEMA_VERSION,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FixedBytes32([u8; 32]);

impl FixedBytes32 {
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.0.iter().all(|byte| *byte == 0)
    }

    pub fn from_hex(value: &str) -> Result<Self, ValidationError> {
        decode_fixed_hex(value).map(Self)
    }

    #[must_use]
    pub fn to_hex(self) -> String {
        encode_hex(&self.0)
    }
}

impl std::fmt::Debug for FixedBytes32 {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&encode_hex(&self.0))
    }
}

impl Serialize for FixedBytes32 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&encode_hex(&self.0))
    }
}

impl<'de> Deserialize<'de> for FixedBytes32 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        decode_fixed_hex(&value)
            .map(Self)
            .map_err(de::Error::custom)
    }
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HexBytes(Vec<u8>);

impl HexBytes {
    pub fn new(bytes: Vec<u8>) -> Result<Self, ValidationError> {
        if bytes.len() > MAX_NATIVE_PATH_BYTES {
            return Err(ValidationError::BoundExceeded("hex bytes"));
        }
        Ok(Self(bytes))
    }

    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    #[must_use]
    pub fn into_vec(self) -> Vec<u8> {
        self.0
    }
}

impl std::fmt::Debug for HexBytes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&encode_hex(&self.0))
    }
}

impl Serialize for HexBytes {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&encode_hex(&self.0))
    }
}

impl<'de> Deserialize<'de> for HexBytes {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        if value.len() > MAX_NATIVE_PATH_BYTES.saturating_mul(2) {
            return Err(de::Error::custom("hex byte string exceeds protocol bound"));
        }
        decode_hex(&value)
            .and_then(Self::new)
            .map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativePathEncoding {
    WindowsUtf16Le,
    UnixBytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformKindManifest {
    Windows,
    MacOs,
    Linux,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativePathManifest {
    pub encoding: NativePathEncoding,
    pub bytes: HexBytes,
}

impl NativePathManifest {
    fn validate(&self, label: &'static str) -> Result<(), ValidationError> {
        if self.bytes.as_slice().is_empty() || self.bytes.as_slice().len() > MAX_NATIVE_PATH_BYTES {
            return Err(ValidationError::InvalidField(label));
        }
        if self.encoding == NativePathEncoding::WindowsUtf16Le
            && !self.bytes.as_slice().len().is_multiple_of(2)
        {
            return Err(ValidationError::InvalidField(label));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VolumeIdentityManifest {
    pub platform: PlatformKindManifest,
    pub stable_identifier: String,
    pub filesystem_type: Option<String>,
    pub case_sensitive: bool,
    pub removable: bool,
    pub local: bool,
}

impl VolumeIdentityManifest {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_string(
            &self.stable_identifier,
            MAX_TEXT_BYTES,
            "volume stable identifier",
        )?;
        if let Some(value) = &self.filesystem_type {
            validate_string(value, 128, "filesystem type")?;
        }
        if !self.local || self.removable {
            return Err(ValidationError::InvalidField("root volume eligibility"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RootBindingManifest {
    pub canonical_path: NativePathManifest,
    pub display_path: String,
    pub volume: VolumeIdentityManifest,
}

impl RootBindingManifest {
    fn validate(&self) -> Result<(), ValidationError> {
        self.canonical_path.validate("canonical root")?;
        validate_string(
            &self.display_path,
            MAX_NATIVE_PATH_BYTES,
            "root display path",
        )?;
        self.volume.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SafetyPolicyBindingManifest {
    pub version: String,
    pub maximum_rehash_bytes: u64,
    pub allow_qualified_case_only_rename: bool,
    pub digest: FixedBytes32,
}

impl SafetyPolicyBindingManifest {
    fn validate(&self) -> Result<(), ValidationError> {
        validate_string(&self.version, MAX_TEXT_BYTES, "safety policy version")?;
        if self.version != domain::EXECUTION_SAFETY_POLICY_VERSION
            || self.maximum_rehash_bytes == 0
            || self.maximum_rehash_bytes > domain::MAX_EXECUTION_VERIFICATION_BYTES
        {
            return Err(ValidationError::InvalidField("safety policy binding"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeFileIdentityManifest {
    pub volume: VolumeIdentityManifest,
    pub object_key: HexBytes,
    pub parent_key: HexBytes,
    pub leaf_name: NativePathManifest,
    pub link_count: u32,
    pub reparse_tag: Option<u32>,
}

impl NativeFileIdentityManifest {
    fn validate(&self, root_volume: &VolumeIdentityManifest) -> Result<(), ValidationError> {
        self.volume.validate()?;
        if &self.volume != root_volume
            || self.object_key.as_slice().is_empty()
            || self.object_key.as_slice().len() > MAX_IDENTITY_BYTES
            || self.parent_key.as_slice().len() > MAX_IDENTITY_BYTES
            || self.link_count != 1
            || self.reparse_tag.is_some()
        {
            return Err(ValidationError::InvalidField("expected native identity"));
        }
        self.leaf_name.validate("expected native leaf name")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedFileStateManifest {
    pub native_identity: NativeFileIdentityManifest,
    pub byte_size: u64,
    pub modified_at_ns: Option<i64>,
    pub attributes: u64,
    pub content_digest: FixedBytes32,
}

impl ExpectedFileStateManifest {
    fn validate(&self, root_volume: &VolumeIdentityManifest) -> Result<(), ValidationError> {
        self.native_identity.validate(root_volume)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OperationPrimitiveManifest {
    CreateDirectory {
        destination_relative_path: String,
    },
    SameVolumeMove {
        source_relative_path: String,
        destination_relative_path: String,
        original_source_relative_path: String,
        expected_source: ExpectedFileStateManifest,
    },
    SameVolumeRename {
        source_relative_path: String,
        destination_relative_path: String,
        original_source_relative_path: String,
        expected_source: ExpectedFileStateManifest,
    },
    SameVolumeMoveAndRename {
        source_relative_path: String,
        destination_relative_path: String,
        original_source_relative_path: String,
        expected_source: ExpectedFileStateManifest,
    },
    InternalStage {
        source_relative_path: String,
        destination_relative_path: String,
        original_source_relative_path: String,
        expected_source: ExpectedFileStateManifest,
    },
}

impl OperationPrimitiveManifest {
    pub fn validate(&self, root_volume: &VolumeIdentityManifest) -> Result<(), ValidationError> {
        match self {
            Self::CreateDirectory {
                destination_relative_path,
            } => validate_relative_path(destination_relative_path),
            Self::SameVolumeMove {
                source_relative_path,
                destination_relative_path,
                original_source_relative_path,
                expected_source,
            }
            | Self::SameVolumeRename {
                source_relative_path,
                destination_relative_path,
                original_source_relative_path,
                expected_source,
            }
            | Self::SameVolumeMoveAndRename {
                source_relative_path,
                destination_relative_path,
                original_source_relative_path,
                expected_source,
            }
            | Self::InternalStage {
                source_relative_path,
                destination_relative_path,
                original_source_relative_path,
                expected_source,
            } => {
                validate_relative_path(source_relative_path)?;
                validate_relative_path(destination_relative_path)?;
                validate_relative_path(original_source_relative_path)?;
                if source_relative_path == destination_relative_path {
                    return Err(ValidationError::InvalidField(
                        "operation source and destination",
                    ));
                }
                if matches!(self, Self::InternalStage { .. })
                    && !destination_relative_path.starts_with(".supremacy-staging/")
                {
                    return Err(ValidationError::InvalidField(
                        "internal staging destination",
                    ));
                }
                expected_source.validate(root_volume)
            }
        }
    }

    #[must_use]
    pub const fn permits_rollback(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovedOperationManifest {
    pub operation_id: String,
    pub proposal_operation_id: Option<String>,
    pub sequence: u32,
    pub dependencies: Vec<String>,
    pub primitive: OperationPrimitiveManifest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenPlanManifest {
    pub material_version: u32,
    pub plan_id: String,
    pub proposal_id: String,
    pub proposal_revision_id: String,
    pub proposal_revision: u32,
    pub source_snapshot_version: String,
    pub approved_operation_ids: Vec<String>,
    pub operation_count: u64,
    pub approval_timestamp: String,
    pub user_confirmed: bool,
    pub digest: FixedBytes32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttestedConsentManifest {
    pub issued_at_unix_ms: i64,
    pub expires_at_unix_ms: i64,
    pub attested_at_unix_ms: i64,
    pub consent_nonce: FixedBytes32,
    pub attestation_mac: FixedBytes32,
}

impl AttestedConsentManifest {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.issued_at_unix_ms < 0
            || self.attested_at_unix_ms < self.issued_at_unix_ms
            || self.expires_at_unix_ms <= self.attested_at_unix_ms
        {
            return Err(ValidationError::InvalidField("consent time range"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsentAttestationBinding {
    pub material_version: u32,
    pub plan_digest: FixedBytes32,
    pub execution_id: String,
    pub plan_id: String,
    pub proposal_id: String,
    pub proposal_revision_id: String,
    pub proposal_revision: u32,
    pub approved_operation_ids: Vec<String>,
    pub approved_operation_count: u64,
    pub source_snapshot_version: String,
    pub root_id: String,
    pub destination_root: RootBindingManifest,
    pub safety_policy: SafetyPolicyBindingManifest,
    pub consent_nonce: FixedBytes32,
    pub issued_at_unix_ms: i64,
    pub expires_at_unix_ms: i64,
}

impl ConsentAttestationBinding {
    pub fn try_from_execution_detail(
        detail: &domain::ExecutionDetail,
        consent_nonce: FixedBytes32,
        issued_at_unix_ms: i64,
        expires_at_unix_ms: i64,
    ) -> Result<Self, ValidationError> {
        let approval = &detail.session.approval;
        let binding = Self {
            material_version: approval.material_version,
            plan_digest: FixedBytes32::from_hex(&approval.digest_hex)?,
            execution_id: detail.session.id.to_string(),
            plan_id: approval.plan_id.to_string(),
            proposal_id: approval.proposal_id.to_string(),
            proposal_revision_id: approval.proposal_revision_id.to_string(),
            proposal_revision: approval.proposal_revision,
            approved_operation_ids: approval
                .approved_operation_ids
                .iter()
                .map(ToString::to_string)
                .collect(),
            approved_operation_count: approval.operation_count,
            source_snapshot_version: approval.source_snapshot_version.to_string(),
            root_id: detail.session.root_id.to_string(),
            destination_root: root_from_live(&approval.destination_root)?,
            safety_policy: SafetyPolicyBindingManifest {
                version: approval.safety_policy.version.clone(),
                maximum_rehash_bytes: approval.safety_policy.maximum_rehash_bytes,
                allow_qualified_case_only_rename: approval
                    .safety_policy
                    .allow_qualified_case_only_rename,
                digest: FixedBytes32::from_hex(&approval.safety_policy.digest_hex)?,
            },
            consent_nonce,
            issued_at_unix_ms,
            expires_at_unix_ms,
        };
        binding.validate()?;
        Ok(binding)
    }

    #[must_use]
    pub fn from_envelope(envelope: &ImmutableExecutionEnvelope) -> Self {
        Self {
            material_version: envelope.plan.material_version,
            plan_digest: envelope.plan.digest,
            execution_id: envelope.execution_id.clone(),
            plan_id: envelope.plan.plan_id.clone(),
            proposal_id: envelope.plan.proposal_id.clone(),
            proposal_revision_id: envelope.plan.proposal_revision_id.clone(),
            proposal_revision: envelope.plan.proposal_revision,
            approved_operation_ids: envelope.plan.approved_operation_ids.clone(),
            approved_operation_count: envelope.plan.operation_count,
            source_snapshot_version: envelope.plan.source_snapshot_version.clone(),
            root_id: envelope.root_id.clone(),
            destination_root: envelope.root_binding.clone(),
            safety_policy: envelope.safety_policy_binding.clone(),
            consent_nonce: envelope.consent.consent_nonce,
            issued_at_unix_ms: envelope.consent.issued_at_unix_ms,
            expires_at_unix_ms: envelope.consent.expires_at_unix_ms,
        }
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_domain_id::<domain::ExecutionId>(&self.execution_id, "execution id")?;
        validate_domain_id::<domain::PlanId>(&self.plan_id, "plan id")?;
        validate_domain_id::<domain::ProposalId>(&self.proposal_id, "proposal id")?;
        validate_domain_id::<domain::OrganizationRevisionId>(
            &self.proposal_revision_id,
            "proposal revision id",
        )?;
        validate_domain_id::<domain::ScanId>(
            &self.source_snapshot_version,
            "source snapshot version",
        )?;
        validate_domain_id::<domain::RootId>(&self.root_id, "root id")?;
        if self.material_version != domain::EXECUTION_PLAN_MATERIAL_VERSION
            || self.plan_digest.is_zero()
            || self.consent_nonce.is_zero()
            || self.issued_at_unix_ms < 0
            || self.expires_at_unix_ms <= self.issued_at_unix_ms
            || self.approved_operation_ids.is_empty()
            || self.approved_operation_ids.len() > MAX_MANIFESTS
            || self.approved_operation_count
                != u64::try_from(self.approved_operation_ids.len())
                    .map_err(|_| ValidationError::BoundExceeded("approved operation ids"))?
        {
            return Err(ValidationError::InvalidField("consent attestation binding"));
        }
        let mut ids = BTreeSet::new();
        let mut previous: Option<domain::ProposalItemId> = None;
        for id in &self.approved_operation_ids {
            let parsed = validate_domain_id::<domain::ProposalItemId>(id, "approved operation id")?;
            if previous.is_some_and(|value| value >= parsed) || !ids.insert(id) {
                return Err(ValidationError::DuplicateOrUnordered(
                    "approved operation ids",
                ));
            }
            previous = Some(parsed);
        }
        self.destination_root.validate()?;
        self.safety_policy.validate()?;
        if self.safety_policy.digest.is_zero() {
            return Err(ValidationError::InvalidField("safety policy digest"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImmutableExecutionEnvelope {
    pub schema_version: u16,
    pub execution_id: String,
    pub root_id: String,
    pub plan: FrozenPlanManifest,
    pub root_binding: RootBindingManifest,
    pub safety_policy_binding: SafetyPolicyBindingManifest,
    pub consent: AttestedConsentManifest,
    pub operations: Vec<ApprovedOperationManifest>,
}

impl ImmutableExecutionEnvelope {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(ValidationError::UnsupportedSchema);
        }
        validate_domain_id::<domain::ExecutionId>(&self.execution_id, "execution id")?;
        validate_domain_id::<domain::RootId>(&self.root_id, "root id")?;
        validate_domain_id::<domain::PlanId>(&self.plan.plan_id, "plan id")?;
        validate_domain_id::<domain::ProposalId>(&self.plan.proposal_id, "proposal id")?;
        validate_domain_id::<domain::OrganizationRevisionId>(
            &self.plan.proposal_revision_id,
            "proposal revision id",
        )?;
        validate_domain_id::<domain::ScanId>(
            &self.plan.source_snapshot_version,
            "source snapshot version",
        )?;
        if self.plan.material_version != domain::EXECUTION_PLAN_MATERIAL_VERSION
            || !self.plan.user_confirmed
            || self.plan.digest.is_zero()
        {
            return Err(ValidationError::InvalidField("approved plan state"));
        }
        validate_string(
            &self.plan.approval_timestamp,
            MAX_TEXT_BYTES,
            "approval timestamp",
        )?;
        if self.plan.approved_operation_ids.is_empty()
            || self.plan.approved_operation_ids.len() > MAX_MANIFESTS
            || self.operations.is_empty()
            || self.operations.len() > MAX_MANIFESTS
            || self.plan.operation_count
                != u64::try_from(self.plan.approved_operation_ids.len())
                    .map_err(|_| ValidationError::BoundExceeded("approved operation ids"))?
        {
            return Err(ValidationError::BoundExceeded("operation manifests"));
        }

        let mut approved_ids = BTreeSet::new();
        let mut previous_approved: Option<domain::ProposalItemId> = None;
        for id in &self.plan.approved_operation_ids {
            let parsed = validate_domain_id::<domain::ProposalItemId>(id, "approved operation id")?;
            if previous_approved.is_some_and(|previous| previous >= parsed)
                || !approved_ids.insert(id.clone())
            {
                return Err(ValidationError::DuplicateOrUnordered(
                    "approved operation ids",
                ));
            }
            previous_approved = Some(parsed);
        }

        self.root_binding.validate()?;
        self.safety_policy_binding.validate()?;
        if self.safety_policy_binding.digest.is_zero() {
            return Err(ValidationError::InvalidField("safety policy digest"));
        }
        self.consent.validate()?;

        let mut operation_ids = BTreeSet::new();
        let mut observed_approved = BTreeMap::<String, usize>::new();
        for (index, operation) in self.operations.iter().enumerate() {
            validate_domain_id::<domain::OperationStepId>(&operation.operation_id, "operation id")?;
            let expected_sequence = u32::try_from(index)
                .map_err(|_| ValidationError::BoundExceeded("operation sequence"))?;
            if operation.sequence != expected_sequence
                || !operation_ids.insert(operation.operation_id.clone())
            {
                return Err(ValidationError::DuplicateOrUnordered("operation manifests"));
            }
            if operation.dependencies.len() > MAX_MANIFESTS {
                return Err(ValidationError::BoundExceeded("operation dependencies"));
            }
            let mut dependencies = BTreeSet::new();
            for dependency in &operation.dependencies {
                validate_domain_id::<domain::OperationStepId>(dependency, "dependency id")?;
                if !operation_ids.contains(dependency) || !dependencies.insert(dependency) {
                    return Err(ValidationError::InvalidField("operation dependency"));
                }
            }
            if let Some(proposal_operation_id) = &operation.proposal_operation_id {
                validate_domain_id::<domain::ProposalItemId>(
                    proposal_operation_id,
                    "proposal operation id",
                )?;
                if !approved_ids.contains(proposal_operation_id) {
                    return Err(ValidationError::InvalidField(
                        "operation outside approved set",
                    ));
                }
                *observed_approved
                    .entry(proposal_operation_id.clone())
                    .or_default() += 1;
            }
            operation.primitive.validate(&self.root_binding.volume)?;
            match operation.primitive {
                OperationPrimitiveManifest::CreateDirectory { .. }
                | OperationPrimitiveManifest::InternalStage { .. }
                    if operation.proposal_operation_id.is_some() =>
                {
                    return Err(ValidationError::InvalidField(
                        "internal operation proposal binding",
                    ));
                }
                OperationPrimitiveManifest::SameVolumeMove { .. }
                | OperationPrimitiveManifest::SameVolumeRename { .. }
                | OperationPrimitiveManifest::SameVolumeMoveAndRename { .. }
                    if operation.proposal_operation_id.is_none() =>
                {
                    return Err(ValidationError::InvalidField(
                        "approved operation proposal binding",
                    ));
                }
                _ => {}
            }
        }
        if approved_ids
            .iter()
            .any(|id| observed_approved.get(id).copied() != Some(1))
        {
            return Err(ValidationError::InvalidField(
                "exact approved operation set",
            ));
        }
        Ok(())
    }

    pub fn try_from_execution_detail(
        detail: &domain::ExecutionDetail,
    ) -> Result<Self, ValidationError> {
        Self::try_from_execution_detail_for(detail, EnvelopePurpose::Forward)
    }

    pub fn try_from_execution_detail_for_rollback(
        detail: &domain::ExecutionDetail,
    ) -> Result<Self, ValidationError> {
        Self::try_from_execution_detail_for(detail, EnvelopePurpose::Rollback)
    }

    fn try_from_execution_detail_for(
        detail: &domain::ExecutionDetail,
        purpose: EnvelopePurpose,
    ) -> Result<Self, ValidationError> {
        let approval = &detail.session.approval;
        if detail.session.id != approval.execution_id
            || detail.session.plan_id != approval.plan_id
            || detail.session.plan_digest_hex != approval.digest_hex
        {
            return Err(ValidationError::InvalidField("live execution state"));
        }
        match purpose {
            EnvelopePurpose::Forward
                if detail.session.status != domain::OrganizationExecutionStatus::Approved
                    || detail.session.consent.state != domain::ExecutionConsentState::Attested =>
            {
                return Err(ValidationError::InvalidField("live execution state"));
            }
            EnvelopePurpose::Rollback
                if !matches!(
                    detail.session.consent.state,
                    domain::ExecutionConsentState::Attested
                        | domain::ExecutionConsentState::Consumed
                        | domain::ExecutionConsentState::Expired
                ) =>
            {
                return Err(ValidationError::InvalidField("stored consent state"));
            }
            _ => {}
        }
        let approved = approval
            .approved_operation_ids
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let operations = detail
            .operations
            .iter()
            .filter(|operation| {
                operation
                    .proposal_operation_id
                    .is_none_or(|id| approved.contains(&id))
                    && !matches!(
                        operation.status,
                        domain::ExecutionOperationStatus::Planned
                            | domain::ExecutionOperationStatus::Blocked
                            | domain::ExecutionOperationStatus::Stale
                    )
            })
            .map(|operation| operation_from_live(operation, purpose))
            .collect::<Result<Vec<_>, _>>()?;

        let envelope = Self {
            schema_version: SCHEMA_VERSION,
            execution_id: detail.session.id.to_string(),
            root_id: detail.session.root_id.to_string(),
            plan: FrozenPlanManifest {
                material_version: approval.material_version,
                plan_id: approval.plan_id.to_string(),
                proposal_id: approval.proposal_id.to_string(),
                proposal_revision_id: approval.proposal_revision_id.to_string(),
                proposal_revision: approval.proposal_revision,
                source_snapshot_version: approval.source_snapshot_version.to_string(),
                approved_operation_ids: approval
                    .approved_operation_ids
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                operation_count: approval.operation_count,
                approval_timestamp: approval
                    .approval_timestamp
                    .clone()
                    .ok_or(ValidationError::InvalidField("approval timestamp"))?,
                user_confirmed: approval.user_confirmed,
                digest: FixedBytes32::from_hex(&approval.digest_hex)?,
            },
            root_binding: root_from_live(&approval.destination_root)?,
            safety_policy_binding: SafetyPolicyBindingManifest {
                version: approval.safety_policy.version.clone(),
                maximum_rehash_bytes: approval.safety_policy.maximum_rehash_bytes,
                allow_qualified_case_only_rename: approval
                    .safety_policy
                    .allow_qualified_case_only_rename,
                digest: FixedBytes32::from_hex(&approval.safety_policy.digest_hex)?,
            },
            consent: consent_from_live(&detail.session.consent, purpose)?,
            operations,
        };
        envelope.validate()?;
        Ok(envelope)
    }

    #[must_use]
    pub fn operation(&self, operation_id: &str) -> Option<&ApprovedOperationManifest> {
        self.operations
            .iter()
            .find(|operation| operation.operation_id == operation_id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EnvelopePurpose {
    Forward,
    Rollback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "direction", rename_all = "snake_case", deny_unknown_fields)]
pub enum OperationDirection {
    Forward,
    Rollback,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommittedJournalEventBinding {
    pub database_sequence: u64,
    pub database_event_digest: FixedBytes32,
    pub external_sequence: u64,
    pub external_event_digest: FixedBytes32,
}

impl CommittedJournalEventBinding {
    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.database_sequence != self.external_sequence
            || self.database_event_digest != self.external_event_digest
            || self.database_event_digest.is_zero()
        {
            return Err(ValidationError::InvalidField(
                "durable journal event binding",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RollbackEligibilityState {
    Applied,
    Recovered,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RollbackEligibility {
    pub operation_id: String,
    pub state: RollbackEligibilityState,
    pub applied_event: CommittedJournalEventBinding,
}

impl RollbackEligibility {
    fn validate(&self, envelope: &ImmutableExecutionEnvelope) -> Result<(), ValidationError> {
        validate_domain_id::<domain::OperationStepId>(&self.operation_id, "rollback operation id")?;
        self.applied_event.validate()?;
        if envelope.operation(&self.operation_id).is_none() {
            return Err(ValidationError::InvalidField(
                "rollback operation outside envelope",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionAuthorization {
    Forward,
    Rollback {
        eligible_operations: Vec<RollbackEligibility>,
    },
}

impl SessionAuthorization {
    pub fn validate(&self, envelope: &ImmutableExecutionEnvelope) -> Result<(), ValidationError> {
        match self {
            Self::Forward => Ok(()),
            Self::Rollback {
                eligible_operations,
            } => {
                if eligible_operations.is_empty() || eligible_operations.len() > MAX_MANIFESTS {
                    return Err(ValidationError::BoundExceeded(
                        "rollback eligible operations",
                    ));
                }
                let mut ids = BTreeSet::new();
                for eligibility in eligible_operations {
                    eligibility.validate(envelope)?;
                    if !ids.insert(&eligibility.operation_id) {
                        return Err(ValidationError::DuplicateOrUnordered(
                            "rollback eligible operations",
                        ));
                    }
                }
                Ok(())
            }
        }
    }

    #[must_use]
    pub fn permits(&self, operation_id: &str, direction: &OperationDirection) -> bool {
        match (self, direction) {
            (Self::Forward, OperationDirection::Forward) => true,
            (
                Self::Rollback {
                    eligible_operations,
                },
                OperationDirection::Rollback,
            ) => eligible_operations
                .iter()
                .any(|entry| entry.operation_id == operation_id),
            _ => false,
        }
    }

    #[must_use]
    pub fn rollback_eligible_ids(&self) -> BTreeSet<String> {
        match self {
            Self::Forward => BTreeSet::new(),
            Self::Rollback {
                eligible_operations,
            } => eligible_operations
                .iter()
                .map(|entry| entry.operation_id.clone())
                .collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationBinding {
    pub operation_id: String,
    pub direction: OperationDirection,
    pub journal_intent: CommittedJournalEventBinding,
}

impl OperationBinding {
    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_domain_id::<domain::OperationStepId>(&self.operation_id, "operation binding")?;
        self.journal_intent.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "category", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProtocolRefusalCategory {
    Protocol,
    Authentication,
    Replay,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolRefusal {
    pub category: ProtocolRefusalCategory,
    pub code: String,
    pub detail: String,
}

impl ProtocolRefusal {
    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_code_and_detail(&self.code, &self.detail)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutorErrorClass {
    SharingViolation,
    LockViolation,
    PermissionDenied,
    DiskFull,
    DestinationCollision,
    SourceMissing,
    PathPolicyRefusal,
    Precondition,
    VerificationLimit,
    Cancelled,
    Unsupported,
    Io,
    AmbiguousMutationOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutorAttemptAudit {
    pub attempt_count: u8,
    pub error_class: Option<ExecutorErrorClass>,
}

impl ExecutorAttemptAudit {
    fn validate(&self) -> Result<(), ValidationError> {
        if self.attempt_count == 0 || self.attempt_count > 3 {
            return Err(ValidationError::InvalidField("executor attempt count"));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExecutorOutcome {
    Success {
        applied_at_unix_ms: i64,
        observed_state_digest: FixedBytes32,
        audit: ExecutorAttemptAudit,
    },
    ProvenNotApplied {
        code: String,
        detail: String,
        audit: ExecutorAttemptAudit,
    },
    RecoveryRequired {
        code: String,
        detail: String,
        audit: ExecutorAttemptAudit,
    },
    ProtocolRefusal {
        refusal: ProtocolRefusal,
    },
}

impl ExecutorOutcome {
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::Success {
                applied_at_unix_ms, ..
            } if *applied_at_unix_ms < 0 => Err(ValidationError::InvalidField("applied timestamp")),
            Self::Success { audit, .. } => audit.validate(),
            Self::ProvenNotApplied {
                code,
                detail,
                audit,
            }
            | Self::RecoveryRequired {
                code,
                detail,
                audit,
            } => {
                validate_code_and_detail(code, detail)?;
                audit.validate()
            }
            Self::ProtocolRefusal { refusal } => refusal.validate(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ValidationError {
    #[error("unsupported executor protocol schema")]
    UnsupportedSchema,
    #[error("{0} exceeds its protocol bound")]
    BoundExceeded(&'static str),
    #[error("invalid protocol field: {0}")]
    InvalidField(&'static str),
    #[error("duplicate or unordered protocol field: {0}")]
    DuplicateOrUnordered(&'static str),
    #[error("invalid hexadecimal encoding")]
    InvalidHex,
}

fn operation_from_live(
    operation: &domain::ExecutionOperation,
    purpose: EnvelopePurpose,
) -> Result<ApprovedOperationManifest, ValidationError> {
    if purpose == EnvelopePurpose::Forward
        && operation.status != domain::ExecutionOperationStatus::PreflightOk
    {
        return Err(ValidationError::InvalidField(
            "live operation is not preflight-approved",
        ));
    }
    let primitive = match operation.kind {
        domain::ExecutionOperationKind::CreateDirectory => {
            if operation.source_relative_path.is_some()
                || operation.live_fingerprint.is_some()
                || operation.directory_existed_before != Some(false)
            {
                return Err(ValidationError::InvalidField("create-directory manifest"));
            }
            OperationPrimitiveManifest::CreateDirectory {
                destination_relative_path: operation.destination_relative_path.clone(),
            }
        }
        kind => {
            let source_relative_path = operation
                .source_relative_path
                .clone()
                .ok_or(ValidationError::InvalidField("operation source"))?;
            let original_source_relative_path = operation
                .original_source_relative_path
                .clone()
                .ok_or(ValidationError::InvalidField("original operation source"))?;
            let expected_fingerprint = match purpose {
                EnvelopePurpose::Forward => operation.live_fingerprint.as_ref(),
                EnvelopePurpose::Rollback => operation
                    .post_fingerprint
                    .as_ref()
                    .or(operation.live_fingerprint.as_ref()),
            }
            .ok_or(ValidationError::InvalidField(
                "direction-bound source fingerprint",
            ))?;
            let expected_source = expected_state_from_live(expected_fingerprint)?;
            let destination_relative_path = operation.destination_relative_path.clone();
            match kind {
                domain::ExecutionOperationKind::Move => {
                    OperationPrimitiveManifest::SameVolumeMove {
                        source_relative_path,
                        destination_relative_path,
                        original_source_relative_path,
                        expected_source,
                    }
                }
                domain::ExecutionOperationKind::Rename => {
                    OperationPrimitiveManifest::SameVolumeRename {
                        source_relative_path,
                        destination_relative_path,
                        original_source_relative_path,
                        expected_source,
                    }
                }
                domain::ExecutionOperationKind::MoveAndRename => {
                    OperationPrimitiveManifest::SameVolumeMoveAndRename {
                        source_relative_path,
                        destination_relative_path,
                        original_source_relative_path,
                        expected_source,
                    }
                }
                domain::ExecutionOperationKind::InternalStage => {
                    OperationPrimitiveManifest::InternalStage {
                        source_relative_path,
                        destination_relative_path,
                        original_source_relative_path,
                        expected_source,
                    }
                }
                domain::ExecutionOperationKind::CreateDirectory => unreachable!(),
            }
        }
    };
    Ok(ApprovedOperationManifest {
        operation_id: operation.id.to_string(),
        proposal_operation_id: operation.proposal_operation_id.map(|id| id.to_string()),
        sequence: operation.sequence,
        dependencies: operation
            .dependencies
            .iter()
            .map(ToString::to_string)
            .collect(),
        primitive,
    })
}

fn root_from_live(
    value: &domain::ExecutionRootBinding,
) -> Result<RootBindingManifest, ValidationError> {
    Ok(RootBindingManifest {
        canonical_path: native_path_from_live(&value.canonical_path)?,
        display_path: value.display_path.clone(),
        volume: volume_from_live(&value.volume),
    })
}

fn expected_state_from_live(
    value: &domain::FileFingerprint,
) -> Result<ExpectedFileStateManifest, ValidationError> {
    let modified_at_ns = value
        .modified_at_ns
        .map(i64::try_from)
        .transpose()
        .map_err(|_| ValidationError::InvalidField("modified timestamp"))?;
    Ok(ExpectedFileStateManifest {
        native_identity: NativeFileIdentityManifest {
            volume: volume_from_live(&value.native_identity.volume),
            object_key: HexBytes::new(value.native_identity.object_key.clone())?,
            parent_key: HexBytes::new(value.native_identity.parent_key.clone())?,
            leaf_name: native_path_from_live(&value.native_identity.leaf_name)?,
            link_count: value.native_identity.link_count,
            reparse_tag: value.native_identity.reparse_tag,
        },
        byte_size: value.byte_size,
        modified_at_ns,
        attributes: value.attributes,
        content_digest: FixedBytes32::from_bytes(
            value
                .content_digest
                .ok_or(ValidationError::InvalidField("content digest"))?,
        ),
    })
}

fn native_path_from_live(
    value: &domain::NativePath,
) -> Result<NativePathManifest, ValidationError> {
    Ok(NativePathManifest {
        encoding: match value.encoding {
            domain::PathEncoding::WindowsUtf16Le => NativePathEncoding::WindowsUtf16Le,
            domain::PathEncoding::UnixBytes => NativePathEncoding::UnixBytes,
        },
        bytes: HexBytes::new(value.bytes.clone())?,
    })
}

fn volume_from_live(value: &domain::VolumeIdentity) -> VolumeIdentityManifest {
    VolumeIdentityManifest {
        platform: match value.platform {
            domain::PlatformKind::Windows => PlatformKindManifest::Windows,
            domain::PlatformKind::MacOs => PlatformKindManifest::MacOs,
            domain::PlatformKind::Linux => PlatformKindManifest::Linux,
            domain::PlatformKind::Other => PlatformKindManifest::Other,
        },
        stable_identifier: value.stable_identifier.clone(),
        filesystem_type: value.filesystem_type.clone(),
        case_sensitive: value.case_sensitive,
        removable: value.removable,
        local: value.local,
    }
}

fn consent_from_live(
    value: &domain::ExecutionConsent,
    purpose: EnvelopePurpose,
) -> Result<AttestedConsentManifest, ValidationError> {
    let state_valid = match purpose {
        EnvelopePurpose::Forward => {
            value.state == domain::ExecutionConsentState::Attested
                && value.consumed_at_unix_ms.is_none()
        }
        EnvelopePurpose::Rollback => matches!(
            value.state,
            domain::ExecutionConsentState::Attested
                | domain::ExecutionConsentState::Consumed
                | domain::ExecutionConsentState::Expired
        ),
    };
    if !state_valid || value.invalidated_at_unix_ms.is_some() || value.invalidation_reason.is_some()
    {
        return Err(ValidationError::InvalidField("attested consent state"));
    }
    Ok(AttestedConsentManifest {
        issued_at_unix_ms: value
            .issued_at_unix_ms
            .ok_or(ValidationError::InvalidField("consent issue time"))?,
        expires_at_unix_ms: value
            .expires_at_unix_ms
            .ok_or(ValidationError::InvalidField("consent expiry"))?,
        attested_at_unix_ms: value
            .attested_at_unix_ms
            .ok_or(ValidationError::InvalidField("consent attestation time"))?,
        consent_nonce: FixedBytes32::from_bytes(
            value
                .nonce
                .ok_or(ValidationError::InvalidField("consent nonce"))?,
        ),
        attestation_mac: FixedBytes32::from_bytes(
            value
                .attestation_mac
                .ok_or(ValidationError::InvalidField("consent attestation MAC"))?,
        ),
    })
}

fn validate_domain_id<T>(value: &str, label: &'static str) -> Result<T, ValidationError>
where
    T: std::str::FromStr,
{
    validate_string(value, MAX_ID_BYTES, label)?;
    value
        .parse::<T>()
        .map_err(|_| ValidationError::InvalidField(label))
}

fn validate_string(
    value: &str,
    maximum: usize,
    label: &'static str,
) -> Result<(), ValidationError> {
    if value.is_empty() || value.len() > maximum || value.contains('\0') {
        return Err(ValidationError::InvalidField(label));
    }
    Ok(())
}

fn validate_relative_path(value: &str) -> Result<(), ValidationError> {
    validate_string(value, MAX_RELATIVE_PATH_BYTES, "relative path")?;
    if value.starts_with(['/', '\\'])
        || value.ends_with(['/', '\\'])
        || value.contains(':')
        || value.split(['/', '\\']).any(|component| {
            component.is_empty()
                || matches!(component, "." | "..")
                || component.ends_with([' ', '.'])
                || is_windows_device_component(component)
        })
    {
        return Err(ValidationError::InvalidField("relative path"));
    }
    Ok(())
}

fn is_windows_device_component(component: &str) -> bool {
    let stem = component
        .split_once('.')
        .map_or(component, |(stem, _)| stem)
        .trim_end_matches([' ', '.'])
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|suffix| {
                suffix.len() == 1
                    && suffix
                        .as_bytes()
                        .first()
                        .is_some_and(|digit| (b'1'..=b'9').contains(digit))
            })
}

fn validate_code_and_detail(code: &str, detail: &str) -> Result<(), ValidationError> {
    validate_string(code, 128, "response code")?;
    if !code
        .bytes()
        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
    {
        return Err(ValidationError::InvalidField("response code"));
    }
    validate_string(detail, MAX_TEXT_BYTES, "response detail")
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn decode_fixed_hex(value: &str) -> Result<[u8; 32], ValidationError> {
    if value.len() != 64 {
        return Err(ValidationError::InvalidHex);
    }
    let decoded = decode_hex(value)?;
    let mut output = [0_u8; 32];
    output.copy_from_slice(&decoded);
    Ok(output)
}

fn decode_hex(value: &str) -> Result<Vec<u8>, ValidationError> {
    if !value.len().is_multiple_of(2)
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_digit() && !(b'a'..=b'f').contains(&byte))
    {
        return Err(ValidationError::InvalidHex);
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16)
                .map_err(|_| ValidationError::InvalidHex)
        })
        .collect()
}
