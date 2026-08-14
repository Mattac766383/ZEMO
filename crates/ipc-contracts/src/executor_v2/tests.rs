use super::*;
use std::io::Cursor;

const NOW: i64 = 1_000_000;
const ROOT_KEY: [u8; 32] = [7; 32];

#[test]
fn authenticated_frames_round_trip_and_tampering_fails() {
    let envelope = envelope();
    let hello = Hello::signed(42, FixedBytes32::from_bytes([1; 32]), NOW, &ROOT_KEY)
        .unwrap_or_else(|error| panic!("hello should sign: {error}"));
    hello
        .verify(&ROOT_KEY, NOW)
        .unwrap_or_else(|error| panic!("hello should verify: {error}"));
    let open = open_session(&hello, envelope);
    open.verify(&hello, &ROOT_KEY, NOW)
        .unwrap_or_else(|error| panic!("session should verify: {error}"));
    let session_key = derive_session_key(&ROOT_KEY, &hello, &open)
        .unwrap_or_else(|error| panic!("session key should derive: {error}"));
    let binding = OperationBinding {
        operation_id: open.envelope.operations[0].operation_id.clone(),
        direction: OperationDirection::Forward,
        journal_intent: journal_intent(2),
    };
    let request = AuthenticatedOperationRequest::signed(
        &open,
        1,
        FixedBytes32::from_bytes([4; 32]),
        NOW,
        NOW + 1_000,
        binding.clone(),
        &session_key,
    )
    .unwrap_or_else(|error| panic!("request should sign: {error}"));
    request
        .verify(&open, 1, &session_key, NOW)
        .unwrap_or_else(|error| panic!("request should verify: {error}"));
    let response = AuthenticatedOperationResponse::signed(
        &open,
        1,
        FixedBytes32::from_bytes([5; 32]),
        NOW,
        NOW + 1_000,
        binding.clone(),
        ExecutorOutcome::ProvenNotApplied {
            code: "test_refusal".to_owned(),
            detail: "No filesystem mutation was attempted.".to_owned(),
            audit: ExecutorAttemptAudit {
                attempt_count: 1,
                error_class: None,
            },
        },
        &session_key,
    )
    .unwrap_or_else(|error| panic!("response should sign: {error}"));
    response
        .verify(&open, 1, &binding, &session_key, NOW)
        .unwrap_or_else(|error| panic!("response should verify: {error}"));
    let mut response_verifier = CoordinatorResponseVerifier::new();
    response_verifier
        .verify_next(&response, &open, &binding, &session_key, NOW)
        .unwrap_or_else(|error| panic!("first response should verify once: {error}"));
    let replayed_nonce_response = AuthenticatedOperationResponse::signed(
        &open,
        2,
        response.message_nonce,
        NOW,
        NOW + 1_000,
        binding.clone(),
        ExecutorOutcome::ProvenNotApplied {
            code: "test_refusal".to_owned(),
            detail: "No filesystem mutation was attempted.".to_owned(),
            audit: ExecutorAttemptAudit {
                attempt_count: 1,
                error_class: None,
            },
        },
        &session_key,
    )
    .unwrap_or_else(|error| panic!("second response should sign: {error}"));
    assert!(matches!(
        response_verifier.verify_next(&replayed_nonce_response, &open, &binding, &session_key, NOW),
        Err(AuthenticationError::NonceReplay)
    ));

    let mut tampered = serde_json::to_value(&response)
        .unwrap_or_else(|error| panic!("response should encode: {error}"));
    tampered["outcome"]["detail"] = serde_json::json!("tampered");
    let tampered: AuthenticatedOperationResponse = serde_json::from_value(tampered)
        .unwrap_or_else(|error| panic!("tampered response remains syntactically valid: {error}"));
    assert!(matches!(
        tampered.verify(&open, 1, &binding, &session_key, NOW),
        Err(AuthenticationError::InvalidMac)
    ));

    let mut tampered_audit = serde_json::to_value(&response)
        .unwrap_or_else(|error| panic!("response should encode: {error}"));
    tampered_audit["outcome"]["audit"]["attempt_count"] = serde_json::json!(2);
    let tampered_audit: AuthenticatedOperationResponse = serde_json::from_value(tampered_audit)
        .unwrap_or_else(|error| panic!("tampered audit remains syntactically valid: {error}"));
    assert!(matches!(
        tampered_audit.verify(&open, 1, &binding, &session_key, NOW),
        Err(AuthenticationError::InvalidMac)
    ));
}

#[test]
fn request_mac_and_expiry_fail_closed() {
    let hello = Hello::signed(42, FixedBytes32::from_bytes([1; 32]), NOW, &ROOT_KEY)
        .unwrap_or_else(|error| panic!("hello should sign: {error}"));
    let open = open_session(&hello, envelope());
    let session_key = derive_session_key(&ROOT_KEY, &hello, &open)
        .unwrap_or_else(|error| panic!("session key should derive: {error}"));
    let binding = OperationBinding {
        operation_id: open.envelope.operations[0].operation_id.clone(),
        direction: OperationDirection::Forward,
        journal_intent: journal_intent(2),
    };
    let request = AuthenticatedOperationRequest::signed(
        &open,
        1,
        FixedBytes32::from_bytes([4; 32]),
        NOW,
        NOW + 1_000,
        binding.clone(),
        &session_key,
    )
    .unwrap_or_else(|error| panic!("request should sign: {error}"));

    let mut forged = request.clone();
    forged.mac = FixedBytes32::from_bytes([90; 32]);
    assert!(matches!(
        forged.verify(&open, 1, &session_key, NOW),
        Err(AuthenticationError::InvalidMac)
    ));

    let expired = AuthenticatedOperationRequest::signed(
        &open,
        1,
        FixedBytes32::from_bytes([5; 32]),
        NOW - 1_000,
        NOW,
        binding,
        &session_key,
    )
    .unwrap_or_else(|error| panic!("historical request should sign: {error}"));
    assert!(matches!(
        expired.verify(&open, 1, &session_key, NOW),
        Err(AuthenticationError::StaleOrInvalidTime)
    ));

    let mut wrong_version = request;
    wrong_version.protocol_version = PROTOCOL_VERSION + 1;
    assert!(matches!(
        wrong_version.verify(&open, 1, &session_key, NOW),
        Err(AuthenticationError::UnsupportedVersion)
    ));
}

#[test]
fn response_is_bound_to_session_sequence_and_exact_operation() {
    let hello = Hello::signed(42, FixedBytes32::from_bytes([1; 32]), NOW, &ROOT_KEY)
        .unwrap_or_else(|error| panic!("hello should sign: {error}"));
    let open = open_session(&hello, envelope());
    let session_key = derive_session_key(&ROOT_KEY, &hello, &open)
        .unwrap_or_else(|error| panic!("session key should derive: {error}"));
    let binding = OperationBinding {
        operation_id: open.envelope.operations[0].operation_id.clone(),
        direction: OperationDirection::Forward,
        journal_intent: journal_intent(2),
    };

    let wrong_operation = OperationBinding {
        operation_id: domain::OperationStepId::new().to_string(),
        direction: OperationDirection::Forward,
        journal_intent: journal_intent(2),
    };
    let wrong_operation_response = AuthenticatedOperationResponse::signed(
        &open,
        1,
        FixedBytes32::from_bytes([20; 32]),
        NOW,
        NOW + 1_000,
        wrong_operation,
        refusal_outcome(),
        &session_key,
    )
    .unwrap_or_else(|error| panic!("alternate response should sign: {error}"));
    assert!(matches!(
        wrong_operation_response.verify(&open, 1, &binding, &session_key, NOW),
        Err(AuthenticationError::InvalidBinding)
    ));

    let out_of_order = AuthenticatedOperationResponse::signed(
        &open,
        2,
        FixedBytes32::from_bytes([21; 32]),
        NOW,
        NOW + 1_000,
        binding.clone(),
        refusal_outcome(),
        &session_key,
    )
    .unwrap_or_else(|error| panic!("out-of-order response should sign: {error}"));
    assert!(matches!(
        out_of_order.verify(&open, 1, &binding, &session_key, NOW),
        Err(AuthenticationError::SequenceMismatch)
    ));

    let mut wrong_execution_open = open.clone();
    wrong_execution_open.execution_id = domain::ExecutionId::new().to_string();
    let wrong_execution_response = AuthenticatedOperationResponse::signed(
        &wrong_execution_open,
        1,
        FixedBytes32::from_bytes([22; 32]),
        NOW,
        NOW + 1_000,
        binding.clone(),
        refusal_outcome(),
        &session_key,
    )
    .unwrap_or_else(|error| panic!("wrong-session response should sign: {error}"));
    assert!(matches!(
        wrong_execution_response.verify(&open, 1, &binding, &session_key, NOW),
        Err(AuthenticationError::InvalidBinding)
    ));

    let expired = AuthenticatedOperationResponse::signed(
        &open,
        1,
        FixedBytes32::from_bytes([23; 32]),
        NOW - 1_000,
        NOW,
        binding.clone(),
        refusal_outcome(),
        &session_key,
    )
    .unwrap_or_else(|error| panic!("historical response should sign: {error}"));
    assert!(matches!(
        expired.verify(&open, 1, &binding, &session_key, NOW),
        Err(AuthenticationError::StaleOrInvalidTime)
    ));
}

#[test]
fn wrong_keys_pids_execution_and_plan_bindings_are_rejected() {
    let hello = Hello::signed(42, FixedBytes32::from_bytes([1; 32]), NOW, &ROOT_KEY)
        .unwrap_or_else(|error| panic!("hello should sign: {error}"));
    assert!(matches!(
        hello.verify(&[8; 32], NOW),
        Err(AuthenticationError::InvalidMac)
    ));

    let wrong_pid = OpenSession::signed(
        99,
        21,
        hello.worker_nonce,
        FixedBytes32::from_bytes([2; 32]),
        FixedBytes32::from_bytes([3; 32]),
        NOW,
        NOW + 10_000,
        SessionAuthorization::Forward,
        envelope(),
        &ROOT_KEY,
    )
    .unwrap_or_else(|error| panic!("wrong PID session should sign: {error}"));
    assert!(matches!(
        wrong_pid.verify(&hello, &ROOT_KEY, NOW),
        Err(AuthenticationError::InvalidBinding)
    ));

    let open = open_session(&hello, envelope());
    let session_key = derive_session_key(&ROOT_KEY, &hello, &open)
        .unwrap_or_else(|error| panic!("session key should derive: {error}"));
    let binding = OperationBinding {
        operation_id: open.envelope.operations[0].operation_id.clone(),
        direction: OperationDirection::Forward,
        journal_intent: journal_intent(2),
    };

    let mut wrong_execution_open = open.clone();
    wrong_execution_open.execution_id = domain::ExecutionId::new().to_string();
    let wrong_execution = AuthenticatedOperationRequest::signed(
        &wrong_execution_open,
        1,
        FixedBytes32::from_bytes([11; 32]),
        NOW,
        NOW + 1_000,
        binding.clone(),
        &session_key,
    )
    .unwrap_or_else(|error| panic!("request should sign: {error}"));
    assert!(matches!(
        wrong_execution.verify(&open, 1, &session_key, NOW),
        Err(AuthenticationError::InvalidBinding)
    ));

    let mut wrong_plan_open = open.clone();
    wrong_plan_open.plan_id = domain::PlanId::new().to_string();
    let wrong_plan = AuthenticatedOperationRequest::signed(
        &wrong_plan_open,
        1,
        FixedBytes32::from_bytes([12; 32]),
        NOW,
        NOW + 1_000,
        binding,
        &session_key,
    )
    .unwrap_or_else(|error| panic!("request should sign: {error}"));
    assert!(matches!(
        wrong_plan.verify(&open, 1, &session_key, NOW),
        Err(AuthenticationError::InvalidBinding)
    ));
}

#[test]
fn stale_expired_sessions_and_unknown_fields_fail_closed() {
    let hello = Hello::signed(42, FixedBytes32::from_bytes([1; 32]), NOW, &ROOT_KEY)
        .unwrap_or_else(|error| panic!("hello should sign: {error}"));
    let stale = OpenSession::signed(
        hello.worker_pid,
        21,
        hello.worker_nonce,
        FixedBytes32::from_bytes([2; 32]),
        FixedBytes32::from_bytes([3; 32]),
        NOW - MAX_CLOCK_SKEW_MS - 1,
        NOW + 1_000,
        SessionAuthorization::Forward,
        envelope(),
        &ROOT_KEY,
    )
    .unwrap_or_else(|error| panic!("stale session material should sign: {error}"));
    assert!(matches!(
        stale.verify(&hello, &ROOT_KEY, NOW),
        Err(AuthenticationError::StaleOrInvalidTime)
    ));

    let expired = OpenSession::signed(
        hello.worker_pid,
        21,
        hello.worker_nonce,
        FixedBytes32::from_bytes([2; 32]),
        FixedBytes32::from_bytes([3; 32]),
        NOW - 1_000,
        NOW,
        SessionAuthorization::Forward,
        envelope(),
        &ROOT_KEY,
    )
    .unwrap_or_else(|error| panic!("expired session material should sign: {error}"));
    assert!(matches!(
        expired.verify(&hello, &ROOT_KEY, NOW),
        Err(AuthenticationError::StaleOrInvalidTime)
    ));

    let valid = CoordinatorHandshakeFrame::OpenSession(open_session(&hello, envelope()));
    let mut value = serde_json::to_value(valid)
        .unwrap_or_else(|error| panic!("frame should serialize: {error}"));
    value["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<CoordinatorHandshakeFrame>(value).is_err());

    let valid = CoordinatorHandshakeFrame::OpenSession(open_session(&hello, envelope()));
    let mut value = serde_json::to_value(valid)
        .unwrap_or_else(|error| panic!("frame should serialize: {error}"));
    value["payload"]["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<CoordinatorHandshakeFrame>(value).is_err());

    let open = open_session(&hello, envelope());
    let session_key = derive_session_key(&ROOT_KEY, &hello, &open)
        .unwrap_or_else(|error| panic!("session key should derive: {error}"));
    let request = AuthenticatedOperationRequest::signed(
        &open,
        1,
        FixedBytes32::from_bytes([4; 32]),
        NOW,
        NOW + 1_000,
        OperationBinding {
            operation_id: open.envelope.operations[0].operation_id.clone(),
            direction: OperationDirection::Forward,
            journal_intent: journal_intent(2),
        },
        &session_key,
    )
    .unwrap_or_else(|error| panic!("request should sign: {error}"));
    let valid = CoordinatorSessionFrame::ExecuteOperation(request.clone());
    let mut value = serde_json::to_value(valid)
        .unwrap_or_else(|error| panic!("session frame should serialize: {error}"));
    value["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<CoordinatorSessionFrame>(value).is_err());

    let valid = CoordinatorSessionFrame::ExecuteOperation(request.clone());
    let mut value = serde_json::to_value(valid)
        .unwrap_or_else(|error| panic!("session frame should serialize: {error}"));
    value["payload"]["operation"]["unexpected"] = serde_json::json!(true);
    assert!(serde_json::from_value::<CoordinatorSessionFrame>(value).is_err());

    let valid = CoordinatorSessionFrame::ExecuteOperation(request);
    let mut value = serde_json::to_value(valid)
        .unwrap_or_else(|error| panic!("session frame should serialize: {error}"));
    value["payload"]["mac"] = serde_json::json!("00");
    assert!(serde_json::from_value::<CoordinatorSessionFrame>(value).is_err());
}

#[test]
fn clean_eof_truncated_and_bounded_frames_are_distinguished() {
    let clean_eof = read_frame::<_, CoordinatorHandshakeFrame>(&mut Cursor::new(Vec::<u8>::new()))
        .unwrap_or_else(|error| panic!("clean EOF should not be a frame error: {error}"));
    assert!(clean_eof.is_none());

    for truncated in [vec![0], vec![0, 0, 0], vec![0, 0, 0, 4, b'{', b'}']] {
        let result = read_frame::<_, CoordinatorHandshakeFrame>(&mut Cursor::new(truncated));
        assert!(matches!(result, Err(FrameError::Truncated)));
    }

    let empty = 0_u32.to_be_bytes().to_vec();
    let result = read_frame::<_, CoordinatorHandshakeFrame>(&mut Cursor::new(empty));
    assert!(matches!(result, Err(FrameError::Oversized)));

    let oversized = u32::try_from(MAX_FRAME_BYTES + 1)
        .unwrap_or_else(|error| panic!("configured frame bound fits u32: {error}"))
        .to_be_bytes()
        .to_vec();
    let result = read_frame::<_, CoordinatorHandshakeFrame>(&mut Cursor::new(oversized));
    assert!(matches!(result, Err(FrameError::Oversized)));
}

#[test]
fn malformed_protocol_values_are_rejected_before_use() {
    let hello = Hello::signed(42, FixedBytes32::from_bytes([1; 32]), NOW, &ROOT_KEY)
        .unwrap_or_else(|error| panic!("hello should sign: {error}"));
    let open = open_session(&hello, envelope());
    let session_key = derive_session_key(&ROOT_KEY, &hello, &open)
        .unwrap_or_else(|error| panic!("session key should derive: {error}"));
    let binding = OperationBinding {
        operation_id: open.envelope.operations[0].operation_id.clone(),
        direction: OperationDirection::Forward,
        journal_intent: journal_intent(2),
    };

    assert!(matches!(
        AuthenticatedOperationRequest::signed(
            &open,
            0,
            FixedBytes32::from_bytes([4; 32]),
            NOW,
            NOW + 1_000,
            binding.clone(),
            &session_key,
        ),
        Err(AuthenticationError::InvalidBinding)
    ));
    assert!(matches!(
        AuthenticatedOperationRequest::signed(
            &open,
            1,
            FixedBytes32::from_bytes([0; 32]),
            NOW,
            NOW + 1_000,
            binding.clone(),
            &session_key,
        ),
        Err(AuthenticationError::InvalidBinding)
    ));

    let mut invalid_id = binding.clone();
    invalid_id.operation_id = "not-an-operation-id".to_owned();
    assert!(matches!(
        AuthenticatedOperationRequest::signed(
            &open,
            1,
            FixedBytes32::from_bytes([5; 32]),
            NOW,
            NOW + 1_000,
            invalid_id,
            &session_key,
        ),
        Err(AuthenticationError::Validation(_))
    ));

    let mut invalid_journal = binding;
    invalid_journal.journal_intent.external_sequence += 1;
    assert!(matches!(
        AuthenticatedOperationRequest::signed(
            &open,
            1,
            FixedBytes32::from_bytes([6; 32]),
            NOW,
            NOW + 1_000,
            invalid_journal,
            &session_key,
        ),
        Err(AuthenticationError::Validation(_))
    ));

    assert!(serde_json::from_value::<FixedBytes32>(serde_json::json!("AA".repeat(32))).is_err());
    assert!(
        serde_json::from_value::<HexBytes>(serde_json::json!(
            "00".repeat(MAX_NATIVE_PATH_BYTES + 1)
        ))
        .is_err()
    );
}

#[test]
fn worker_manifest_rejects_windows_path_forms_and_invalid_hash_bounds() {
    for invalid in [
        "../escape.txt",
        "safe/file.txt:stream",
        "safe/CON.txt",
        "safe/trailing.",
        r"\\server\share\file.txt",
    ] {
        let mut candidate = envelope();
        let OperationPrimitiveManifest::SameVolumeMove {
            destination_relative_path,
            ..
        } = &mut candidate.operations[0].primitive
        else {
            panic!("fixture should contain a move");
        };
        *destination_relative_path = invalid.to_owned();
        assert!(
            candidate.validate().is_err(),
            "unsafe worker path should fail: {invalid}"
        );
    }

    let mut candidate = envelope();
    let OperationPrimitiveManifest::SameVolumeMove {
        destination_relative_path,
        ..
    } = &mut candidate.operations[0].primitive
    else {
        panic!("fixture should contain a move");
    };
    *destination_relative_path = "a".repeat(MAX_RELATIVE_PATH_BYTES + 1);
    assert!(candidate.validate().is_err());

    for invalid_bound in [0, domain::MAX_EXECUTION_VERIFICATION_BYTES + 1] {
        let mut candidate = envelope();
        candidate.safety_policy_binding.maximum_rehash_bytes = invalid_bound;
        assert!(candidate.validate().is_err());
    }
}

#[test]
fn hard_frame_bound_accommodates_ten_thousand_bounded_manifests() {
    let mut envelope = envelope();
    let template = envelope.operations[0].primitive.clone();
    let mut approved_ids = (0..MAX_MANIFESTS)
        .map(|_| domain::ProposalItemId::new().to_string())
        .collect::<Vec<_>>();
    approved_ids.sort();
    envelope
        .plan
        .approved_operation_ids
        .clone_from(&approved_ids);
    envelope.plan.operation_count = u64::try_from(MAX_MANIFESTS)
        .unwrap_or_else(|error| panic!("manifest bound fits u64: {error}"));
    envelope.operations = approved_ids
        .into_iter()
        .enumerate()
        .map(
            |(sequence, proposal_operation_id)| ApprovedOperationManifest {
                operation_id: domain::OperationStepId::new().to_string(),
                proposal_operation_id: Some(proposal_operation_id),
                sequence: u32::try_from(sequence)
                    .unwrap_or_else(|error| panic!("manifest sequence fits u32: {error}")),
                dependencies: Vec::new(),
                primitive: template.clone(),
            },
        )
        .collect();
    envelope
        .validate()
        .unwrap_or_else(|error| panic!("maximum representative envelope should validate: {error}"));
    resign_consent(&mut envelope);
    let hello = Hello::signed(42, FixedBytes32::from_bytes([1; 32]), NOW, &ROOT_KEY)
        .unwrap_or_else(|error| panic!("hello should sign: {error}"));
    let open = open_session(&hello, envelope);
    let mut encoded = Vec::new();
    write_frame(&mut encoded, &CoordinatorHandshakeFrame::OpenSession(open))
        .unwrap_or_else(|error| panic!("10k bounded manifests must fit the hard frame: {error}"));
    assert!(encoded.len() <= MAX_FRAME_BYTES + 4);
}

fn open_session(hello: &Hello, envelope: ImmutableExecutionEnvelope) -> OpenSession {
    OpenSession::signed(
        hello.worker_pid,
        21,
        hello.worker_nonce,
        FixedBytes32::from_bytes([2; 32]),
        FixedBytes32::from_bytes([3; 32]),
        NOW,
        NOW + 10_000,
        SessionAuthorization::Forward,
        envelope,
        &ROOT_KEY,
    )
    .unwrap_or_else(|error| panic!("session should sign: {error}"))
}

fn envelope() -> ImmutableExecutionEnvelope {
    let execution_id = domain::ExecutionId::new().to_string();
    let proposal_operation_id = domain::ProposalItemId::new().to_string();
    let operation_id = domain::OperationStepId::new().to_string();
    let volume = VolumeIdentityManifest {
        platform: PlatformKindManifest::Windows,
        stable_identifier: "volume-1".to_owned(),
        filesystem_type: Some("NTFS".to_owned()),
        case_sensitive: false,
        removable: false,
        local: true,
    };
    let mut envelope = ImmutableExecutionEnvelope {
        schema_version: SCHEMA_VERSION,
        execution_id,
        root_id: domain::RootId::new().to_string(),
        plan: FrozenPlanManifest {
            material_version: domain::EXECUTION_PLAN_MATERIAL_VERSION,
            plan_id: domain::PlanId::new().to_string(),
            proposal_id: domain::ProposalId::new().to_string(),
            proposal_revision_id: domain::OrganizationRevisionId::new().to_string(),
            proposal_revision: 1,
            source_snapshot_version: domain::ScanId::new().to_string(),
            approved_operation_ids: vec![proposal_operation_id.clone()],
            operation_count: 1,
            approval_timestamp: "2026-08-11T12:00:00Z".to_owned(),
            user_confirmed: true,
            digest: FixedBytes32::from_bytes([9; 32]),
        },
        root_binding: RootBindingManifest {
            canonical_path: NativePathManifest {
                encoding: NativePathEncoding::WindowsUtf16Le,
                bytes: HexBytes::new(
                    "C:\\safe"
                        .encode_utf16()
                        .flat_map(u16::to_le_bytes)
                        .collect(),
                )
                .unwrap_or_else(|error| panic!("root bytes should be bounded: {error}")),
            },
            display_path: "C:\\safe".to_owned(),
            volume: volume.clone(),
        },
        safety_policy_binding: SafetyPolicyBindingManifest {
            version: domain::EXECUTION_SAFETY_POLICY_VERSION.to_owned(),
            digest: FixedBytes32::from_bytes([8; 32]),
            maximum_rehash_bytes: domain::MAX_EXECUTION_VERIFICATION_BYTES,
            allow_qualified_case_only_rename: false,
        },
        consent: AttestedConsentManifest {
            issued_at_unix_ms: NOW - 100,
            expires_at_unix_ms: NOW + 20_000,
            attested_at_unix_ms: NOW - 50,
            consent_nonce: FixedBytes32::from_bytes([6; 32]),
            attestation_mac: FixedBytes32::from_bytes([7; 32]),
        },
        operations: vec![ApprovedOperationManifest {
            operation_id,
            proposal_operation_id: Some(proposal_operation_id),
            sequence: 0,
            dependencies: Vec::new(),
            primitive: OperationPrimitiveManifest::SameVolumeMove {
                source_relative_path: "incoming/file.txt".to_owned(),
                destination_relative_path: "organized/file.txt".to_owned(),
                original_source_relative_path: "incoming/file.txt".to_owned(),
                expected_source: ExpectedFileStateManifest {
                    native_identity: NativeFileIdentityManifest {
                        volume,
                        object_key: HexBytes::new(vec![1, 2, 3])
                            .unwrap_or_else(|error| panic!("object key: {error}")),
                        parent_key: HexBytes::new(vec![4, 5, 6])
                            .unwrap_or_else(|error| panic!("parent key: {error}")),
                        leaf_name: NativePathManifest {
                            encoding: NativePathEncoding::WindowsUtf16Le,
                            bytes: HexBytes::new(
                                "file.txt"
                                    .encode_utf16()
                                    .flat_map(u16::to_le_bytes)
                                    .collect(),
                            )
                            .unwrap_or_else(|error| panic!("leaf name: {error}")),
                        },
                        link_count: 1,
                        reparse_tag: None,
                    },
                    byte_size: 12,
                    modified_at_ns: Some(10),
                    attributes: 0,
                    content_digest: FixedBytes32::from_bytes([10; 32]),
                },
            },
        }],
    };
    resign_consent(&mut envelope);
    envelope
}

fn resign_consent(envelope: &mut ImmutableExecutionEnvelope) {
    let consent_key = derive_consent_authority_key(&ROOT_KEY);
    envelope.consent.attestation_mac = sign_consent_attestation(
        &ConsentAttestationBinding::from_envelope(envelope),
        &consent_key,
    )
    .unwrap_or_else(|error| panic!("test consent should sign: {error}"));
}

fn journal_intent(sequence: u64) -> CommittedJournalEventBinding {
    CommittedJournalEventBinding {
        database_sequence: sequence,
        database_event_digest: FixedBytes32::from_bytes([31; 32]),
        external_sequence: sequence,
        external_event_digest: FixedBytes32::from_bytes([31; 32]),
    }
}

fn refusal_outcome() -> ExecutorOutcome {
    ExecutorOutcome::ProvenNotApplied {
        code: "test_refusal".to_owned(),
        detail: "No filesystem mutation was attempted.".to_owned(),
        audit: ExecutorAttemptAudit {
            attempt_count: 1,
            error_class: None,
        },
    }
}
