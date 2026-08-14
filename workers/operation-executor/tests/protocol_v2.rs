use ipc_contracts::executor_v2::{
    ApprovedOperationManifest, AttestedConsentManifest, AuthenticatedOperationRequest,
    CommittedJournalEventBinding, ConsentAttestationBinding, CoordinatorHandshakeFrame,
    CoordinatorSessionFrame, ExecutorAttemptAudit, ExecutorFrame, ExecutorOutcome,
    ExpectedFileStateManifest, FixedBytes32, FrozenPlanManifest, HexBytes,
    ImmutableExecutionEnvelope, NativeFileIdentityManifest, NativePathEncoding, NativePathManifest,
    OpenSession, OperationBinding, OperationDirection, OperationPrimitiveManifest,
    PlatformKindManifest, ProtocolRefusalCategory, RollbackEligibility, RollbackEligibilityState,
    RootBindingManifest, SCHEMA_VERSION, SafetyPolicyBindingManifest, SessionAuthorization,
    VolumeIdentityManifest, derive_consent_authority_key, derive_session_key, read_frame,
    sign_consent_attestation, write_frame,
};
use operation_executor::{
    Clock, ExecutorHandler, HandlerOutcome, NonceSource, ServerError, ServerExit, serve,
};
use std::{collections::VecDeque, io::Cursor};

const NOW: i64 = 1_000_000;
const ROOT_KEY: [u8; 32] = [7; 32];
const WORKER_PID: u32 = 42;

fn single_attempt_audit() -> ExecutorAttemptAudit {
    ExecutorAttemptAudit {
        attempt_count: 1,
        error_class: None,
    }
}

#[test]
fn valid_handshake_and_frame_reach_only_the_manifest_handler() {
    let hello = expected_hello();
    let open = open_session(&hello, envelope(), NOW, NOW + 10_000, &ROOT_KEY, WORKER_PID);
    let key = derive_session_key(&ROOT_KEY, &hello, &open)
        .unwrap_or_else(|error| panic!("session key should derive: {error}"));
    let binding = binding(&open);
    let request = AuthenticatedOperationRequest::signed(
        &open,
        1,
        FixedBytes32::from_bytes([4; 32]),
        NOW,
        NOW + 1_000,
        binding.clone(),
        &key,
    )
    .unwrap_or_else(|error| panic!("request should sign: {error}"));
    let input = input_bytes(&open, &[request]);
    let mut handler = MockHandler::success();

    let (exit, output) = run(&input, &mut handler);
    assert_eq!(exit, ServerExit::CoordinatorEof);
    assert_eq!(handler.calls, 1);
    let frames = output_frames(&output);
    assert_eq!(frames.len(), 3);
    let ExecutorFrame::OperationResponse(response) = &frames[2] else {
        panic!("third frame must be the authenticated operation response");
    };
    response
        .verify(&open, 1, &binding, &key, NOW)
        .unwrap_or_else(|error| panic!("response should verify: {error}"));
    assert!(matches!(response.outcome, ExecutorOutcome::Success { .. }));
}

#[test]
fn fresh_rollback_session_uses_root_authenticated_journal_eligibility() {
    let hello = expected_hello();
    let mut manifest = envelope();
    manifest.consent.issued_at_unix_ms = NOW - 100;
    manifest.consent.attested_at_unix_ms = NOW - 50;
    manifest.consent.expires_at_unix_ms = NOW - 1;
    resign_consent(&mut manifest);
    let operation_id = manifest.operations[0].operation_id.clone();
    let authorization = SessionAuthorization::Rollback {
        eligible_operations: vec![RollbackEligibility {
            operation_id: operation_id.clone(),
            state: RollbackEligibilityState::Recovered,
            applied_event: journal_intent(7),
        }],
    };
    let open = open_session_authorized(
        &hello,
        manifest,
        NOW,
        NOW + 10_000,
        &ROOT_KEY,
        WORKER_PID,
        authorization,
    );
    let key = derive_session_key(&ROOT_KEY, &hello, &open)
        .unwrap_or_else(|error| panic!("rollback session key should derive: {error}"));
    let binding = OperationBinding {
        operation_id,
        direction: OperationDirection::Rollback,
        journal_intent: journal_intent(8),
    };
    let request = request(&open, &key, 1, [4; 32], binding.clone());
    let mut handler = MockHandler::success();

    let (exit, output) = run(&input_bytes(&open, &[request]), &mut handler);
    assert_eq!(exit, ServerExit::CoordinatorEof);
    assert_eq!(handler.calls, 1);
    let frames = output_frames(&output);
    let ExecutorFrame::OperationResponse(response) = &frames[2] else {
        panic!("rollback must return an authenticated operation response");
    };
    response
        .verify(&open, 1, &binding, &key, NOW)
        .unwrap_or_else(|error| panic!("rollback response should verify: {error}"));
}

#[test]
fn wrong_open_mac_and_child_pid_are_refused_with_authenticated_frames() {
    let hello = expected_hello();
    let mut wrong_key_open =
        open_session(&hello, envelope(), NOW, NOW + 10_000, &ROOT_KEY, WORKER_PID);
    wrong_key_open.mac = FixedBytes32::from_bytes([88; 32]);
    let mut handler = MockHandler::success();
    let (exit, output) = run(&input_bytes(&wrong_key_open, &[]), &mut handler);
    assert_eq!(exit, ServerExit::Refused);
    assert_eq!(handler.calls, 0);
    assert_handshake_refusal(&output, &hello, ProtocolRefusalCategory::Authentication);

    let wrong_pid_open = open_session(
        &hello,
        envelope(),
        NOW,
        NOW + 10_000,
        &ROOT_KEY,
        WORKER_PID + 1,
    );
    let (exit, output) = run(&input_bytes(&wrong_pid_open, &[]), &mut handler);
    assert_eq!(exit, ServerExit::Refused);
    assert_handshake_refusal(&output, &hello, ProtocolRefusalCategory::Protocol);
}

#[test]
fn forged_request_mac_and_out_of_order_sequence_never_reach_handler() {
    let hello = expected_hello();
    let open = open_session(&hello, envelope(), NOW, NOW + 10_000, &ROOT_KEY, WORKER_PID);
    let key = derive_session_key(&ROOT_KEY, &hello, &open)
        .unwrap_or_else(|error| panic!("session key should derive: {error}"));
    let operation = binding(&open);

    let mut forged = request(&open, &key, 1, [4; 32], operation.clone());
    forged.mac = FixedBytes32::from_bytes([90; 32]);
    let mut handler = MockHandler::success();
    let (exit, output) = run(&input_bytes(&open, &[forged]), &mut handler);
    assert_eq!(exit, ServerExit::Refused);
    assert_eq!(handler.calls, 0);
    assert_authenticated_protocol_response(
        &output,
        2,
        &open,
        &key,
        1,
        &operation,
        ProtocolRefusalCategory::Authentication,
        "authentication_failed",
    );

    let out_of_order = request(&open, &key, 2, [5; 32], operation.clone());
    let mut handler = MockHandler::success();
    let (exit, output) = run(&input_bytes(&open, &[out_of_order]), &mut handler);
    assert_eq!(exit, ServerExit::Refused);
    assert_eq!(handler.calls, 0);
    assert_authenticated_protocol_response(
        &output,
        2,
        &open,
        &key,
        1,
        &operation,
        ProtocolRefusalCategory::Replay,
        "sequence_replay",
    );
}

#[test]
fn sequence_and_nonce_replays_are_rejected_before_a_second_handler_call() {
    let hello = expected_hello();
    let open = open_session(&hello, envelope(), NOW, NOW + 10_000, &ROOT_KEY, WORKER_PID);
    let key = derive_session_key(&ROOT_KEY, &hello, &open)
        .unwrap_or_else(|error| panic!("session key should derive: {error}"));
    let binding = binding(&open);
    let first = request(&open, &key, 1, [4; 32], binding.clone());
    let repeated_sequence = request(&open, &key, 1, [5; 32], binding.clone());
    let mut handler = MockHandler::success();
    let (exit, output) = run(
        &input_bytes(&open, &[first.clone(), repeated_sequence]),
        &mut handler,
    );
    assert_eq!(exit, ServerExit::Refused);
    assert_eq!(handler.calls, 1);
    assert_protocol_response(&output, 3, ProtocolRefusalCategory::Replay);

    let nonce_replay = request(&open, &key, 2, [4; 32], binding.clone());
    let mut handler = MockHandler::success();
    let (exit, output) = run(&input_bytes(&open, &[first, nonce_replay]), &mut handler);
    assert_eq!(exit, ServerExit::Refused);
    assert_eq!(handler.calls, 1);
    assert_protocol_response(&output, 3, ProtocolRefusalCategory::Replay);

    let first = request(&open, &key, 1, [4; 32], binding.clone());
    let operation_replay = request(&open, &key, 2, [6; 32], binding);
    let mut handler = MockHandler::success();
    let (exit, output) = run(
        &input_bytes(&open, &[first, operation_replay]),
        &mut handler,
    );
    assert_eq!(exit, ServerExit::Refused);
    assert_eq!(handler.calls, 1);
    assert_protocol_response(&output, 3, ProtocolRefusalCategory::Replay);
}

#[test]
fn wrong_execution_plan_and_unapproved_operation_never_reach_handler() {
    let hello = expected_hello();
    let open = open_session(&hello, envelope(), NOW, NOW + 10_000, &ROOT_KEY, WORKER_PID);
    let key = derive_session_key(&ROOT_KEY, &hello, &open)
        .unwrap_or_else(|error| panic!("session key should derive: {error}"));
    let binding = binding(&open);

    let mut wrong_execution_open = open.clone();
    wrong_execution_open.execution_id = domain::ExecutionId::new().to_string();
    let wrong_execution = request(&wrong_execution_open, &key, 1, [4; 32], binding.clone());
    let mut handler = MockHandler::success();
    let (exit, _) = run(&input_bytes(&open, &[wrong_execution]), &mut handler);
    assert_eq!(exit, ServerExit::Refused);
    assert_eq!(handler.calls, 0);

    let mut wrong_plan_open = open.clone();
    wrong_plan_open.plan_id = domain::PlanId::new().to_string();
    let wrong_plan = request(&wrong_plan_open, &key, 1, [5; 32], binding.clone());
    let (exit, _) = run(&input_bytes(&open, &[wrong_plan]), &mut handler);
    assert_eq!(exit, ServerExit::Refused);
    assert_eq!(handler.calls, 0);

    let mut wrong_digest_open = open.clone();
    wrong_digest_open.plan_digest = FixedBytes32::from_bytes([55; 32]);
    let wrong_digest = request(&wrong_digest_open, &key, 1, [6; 32], binding);
    let (exit, _) = run(&input_bytes(&open, &[wrong_digest]), &mut handler);
    assert_eq!(exit, ServerExit::Refused);
    assert_eq!(handler.calls, 0);

    let unapproved = request(
        &open,
        &key,
        1,
        [7; 32],
        OperationBinding {
            operation_id: domain::OperationStepId::new().to_string(),
            direction: OperationDirection::Forward,
            journal_intent: journal_intent(2),
        },
    );
    let (exit, output) = run(&input_bytes(&open, &[unapproved]), &mut handler);
    assert_eq!(exit, ServerExit::Refused);
    assert_eq!(handler.calls, 0);
    assert_protocol_response(&output, 2, ProtocolRefusalCategory::Protocol);

    let wrong_direction = request(
        &open,
        &key,
        1,
        [8; 32],
        OperationBinding {
            operation_id: open.envelope.operations[0].operation_id.clone(),
            direction: OperationDirection::Rollback,
            journal_intent: journal_intent(2),
        },
    );
    let (exit, output) = run(&input_bytes(&open, &[wrong_direction]), &mut handler);
    assert_eq!(exit, ServerExit::Refused);
    assert_eq!(handler.calls, 0);
    assert_protocol_response(&output, 2, ProtocolRefusalCategory::Protocol);
}

#[test]
fn stale_session_and_coordinator_eof_exit_without_mutation() {
    let hello = expected_hello();
    let stale = open_session(
        &hello,
        envelope(),
        NOW - 30_001,
        NOW + 1_000,
        &ROOT_KEY,
        WORKER_PID,
    );
    let mut handler = MockHandler::success();
    let (exit, output) = run(&input_bytes(&stale, &[]), &mut handler);
    assert_eq!(exit, ServerExit::Refused);
    assert_eq!(handler.calls, 0);
    assert_handshake_refusal(&output, &hello, ProtocolRefusalCategory::Protocol);

    let (exit, output) = run(&[], &mut handler);
    assert_eq!(exit, ServerExit::CoordinatorEof);
    assert_eq!(handler.calls, 0);
    assert_eq!(output_frames(&output).len(), 1);
}

#[test]
fn expired_session_and_request_exit_without_handler_calls() {
    let hello = expected_hello();
    let expired_open = open_session(&hello, envelope(), NOW - 1_000, NOW, &ROOT_KEY, WORKER_PID);
    let mut handler = MockHandler::success();
    let (exit, output) = run(&input_bytes(&expired_open, &[]), &mut handler);
    assert_eq!(exit, ServerExit::Refused);
    assert_eq!(handler.calls, 0);
    assert_handshake_refusal(&output, &hello, ProtocolRefusalCategory::Protocol);

    let open = open_session(&hello, envelope(), NOW, NOW + 10_000, &ROOT_KEY, WORKER_PID);
    let key = derive_session_key(&ROOT_KEY, &hello, &open)
        .unwrap_or_else(|error| panic!("session key should derive: {error}"));
    let operation = binding(&open);
    let expired_request = AuthenticatedOperationRequest::signed(
        &open,
        1,
        FixedBytes32::from_bytes([4; 32]),
        NOW - 1_000,
        NOW,
        operation.clone(),
        &key,
    )
    .unwrap_or_else(|error| panic!("historical request should sign: {error}"));
    let (exit, output) = run(&input_bytes(&open, &[expired_request]), &mut handler);
    assert_eq!(exit, ServerExit::Refused);
    assert_eq!(handler.calls, 0);
    assert_authenticated_protocol_response(
        &output,
        2,
        &open,
        &key,
        1,
        &operation,
        ProtocolRefusalCategory::Protocol,
        "stale_or_invalid_time",
    );
}

#[test]
fn source_and_destination_tampering_breaks_envelope_authentication() {
    for tamper_source in [true, false] {
        let hello = expected_hello();
        let mut open = open_session(&hello, envelope(), NOW, NOW + 10_000, &ROOT_KEY, WORKER_PID);
        let OperationPrimitiveManifest::SameVolumeMove {
            source_relative_path,
            destination_relative_path,
            ..
        } = &mut open.envelope.operations[0].primitive
        else {
            panic!("fixture should contain a move");
        };
        if tamper_source {
            *source_relative_path = "incoming/forged.txt".to_owned();
        } else {
            *destination_relative_path = "organized/forged.txt".to_owned();
        }
        let mut handler = MockHandler::success();

        let (exit, output) = run(&input_bytes(&open, &[]), &mut handler);

        assert_eq!(exit, ServerExit::Refused);
        assert_eq!(handler.calls, 0);
        assert_handshake_refusal(&output, &hello, ProtocolRefusalCategory::Authentication);
    }
}

#[test]
fn truncated_and_unknown_frames_are_refused_without_handler_calls() {
    let hello = expected_hello();
    let truncated_handshake = raw_frame_with_declared_length(8, b"{");
    let mut handler = MockHandler::success();
    let (exit, output) = run(&truncated_handshake, &mut handler);
    assert_eq!(exit, ServerExit::Refused);
    assert_eq!(handler.calls, 0);
    assert_handshake_refusal_at(
        &output,
        &hello,
        1,
        2,
        ProtocolRefusalCategory::Protocol,
        "frame_truncated",
    );

    let open = open_session(&hello, envelope(), NOW, NOW + 10_000, &ROOT_KEY, WORKER_PID);
    let mut truncated_session = input_bytes(&open, &[]);
    truncated_session.extend_from_slice(&raw_frame_with_declared_length(8, b"{"));
    let mut handler = MockHandler::success();
    let (exit, output) = run(&truncated_session, &mut handler);
    assert_eq!(exit, ServerExit::Refused);
    assert_eq!(handler.calls, 0);
    assert_handshake_refusal_at(
        &output,
        &hello,
        2,
        3,
        ProtocolRefusalCategory::Protocol,
        "frame_truncated",
    );

    let key = derive_session_key(&ROOT_KEY, &hello, &open)
        .unwrap_or_else(|error| panic!("session key should derive: {error}"));
    let request = request(&open, &key, 1, [4; 32], binding(&open));
    let mut unknown_session = input_bytes(&open, &[]);
    let encoded = session_frame_bytes(&request);
    unknown_session.extend_from_slice(&with_unknown_top_level_field(&encoded));
    let mut handler = MockHandler::success();
    let (exit, output) = run(&unknown_session, &mut handler);
    assert_eq!(exit, ServerExit::Refused);
    assert_eq!(handler.calls, 0);
    assert_handshake_refusal_at(
        &output,
        &hello,
        2,
        3,
        ProtocolRefusalCategory::Protocol,
        "frame_invalid",
    );
}

#[test]
fn handler_refusal_and_ambiguity_remain_distinct_authenticated_outcomes() {
    let hello = expected_hello();
    let open = open_session(&hello, envelope(), NOW, NOW + 10_000, &ROOT_KEY, WORKER_PID);
    let key = derive_session_key(&ROOT_KEY, &hello, &open)
        .unwrap_or_else(|error| panic!("session key should derive: {error}"));
    let request = request(&open, &key, 1, [4; 32], binding(&open));

    let mut refused = MockHandler::with(HandlerOutcome::ProvenNotApplied {
        code: "precondition_failed".to_owned(),
        detail: "The handler proved that no mutation occurred.".to_owned(),
        audit: single_attempt_audit(),
    });
    let (exit, output) = run(
        &input_bytes(&open, std::slice::from_ref(&request)),
        &mut refused,
    );
    assert_eq!(exit, ServerExit::CoordinatorEof);
    let frames = output_frames(&output);
    let ExecutorFrame::OperationResponse(response) = &frames[2] else {
        panic!("handler refusal must produce an operation response");
    };
    assert!(matches!(
        response.outcome,
        ExecutorOutcome::ProvenNotApplied { .. }
    ));

    let mut ambiguous = MockHandler::with(HandlerOutcome::RecoveryRequired {
        code: "mutation_state_ambiguous".to_owned(),
        detail: "The handler cannot prove whether native mutation occurred.".to_owned(),
        audit: single_attempt_audit(),
    });
    let (exit, output) = run(&input_bytes(&open, &[request]), &mut ambiguous);
    assert_eq!(exit, ServerExit::RecoveryRequired);
    let frames = output_frames(&output);
    let ExecutorFrame::OperationResponse(response) = &frames[2] else {
        panic!("ambiguous handler result must produce an operation response");
    };
    assert!(matches!(
        response.outcome,
        ExecutorOutcome::RecoveryRequired { .. }
    ));
}

fn request(
    open: &OpenSession,
    key: &ipc_contracts::executor_v2::SessionKey,
    sequence: u64,
    nonce: [u8; 32],
    operation: OperationBinding,
) -> AuthenticatedOperationRequest {
    AuthenticatedOperationRequest::signed(
        open,
        sequence,
        FixedBytes32::from_bytes(nonce),
        NOW,
        NOW + 1_000,
        operation,
        key,
    )
    .unwrap_or_else(|error| panic!("request should sign: {error}"))
}

fn input_bytes(open: &OpenSession, requests: &[AuthenticatedOperationRequest]) -> Vec<u8> {
    let mut input = Vec::new();
    write_frame(
        &mut input,
        &CoordinatorHandshakeFrame::OpenSession(open.clone()),
    )
    .unwrap_or_else(|error| panic!("open frame should encode: {error}"));
    for request in requests {
        input.extend_from_slice(&session_frame_bytes(request));
    }
    input
}

fn session_frame_bytes(request: &AuthenticatedOperationRequest) -> Vec<u8> {
    let mut encoded = Vec::new();
    write_frame(
        &mut encoded,
        &CoordinatorSessionFrame::ExecuteOperation(request.clone()),
    )
    .unwrap_or_else(|error| panic!("request frame should encode: {error}"));
    encoded
}

fn raw_frame_with_declared_length(declared_length: u32, payload: &[u8]) -> Vec<u8> {
    let mut encoded = declared_length.to_be_bytes().to_vec();
    encoded.extend_from_slice(payload);
    encoded
}

fn with_unknown_top_level_field(frame: &[u8]) -> Vec<u8> {
    assert!(frame.len() > 5, "encoded frame should contain JSON");
    let declared = usize::try_from(u32::from_be_bytes(
        frame[..4]
            .try_into()
            .unwrap_or_else(|_| panic!("frame should contain a four-byte length")),
    ))
    .unwrap_or_else(|error| panic!("frame length should fit usize: {error}"));
    assert_eq!(declared, frame.len() - 4);
    let payload = &frame[4..];
    assert_eq!(payload.last(), Some(&b'}'));
    let mut mutated = payload[..payload.len() - 1].to_vec();
    mutated.extend_from_slice(br#","unexpected":true}"#);
    raw_frame_with_declared_length(
        u32::try_from(mutated.len())
            .unwrap_or_else(|error| panic!("test frame should fit u32: {error}")),
        &mutated,
    )
}

fn run(input: &[u8], handler: &mut MockHandler) -> (ServerExit, Vec<u8>) {
    let mut reader = Cursor::new(input);
    let mut output = Vec::new();
    let mut nonces = TestNonces {
        values: VecDeque::from(vec![
            FixedBytes32::from_bytes([1; 32]),
            FixedBytes32::from_bytes([50; 32]),
            FixedBytes32::from_bytes([51; 32]),
            FixedBytes32::from_bytes([52; 32]),
        ]),
    };
    let exit = serve(
        &mut reader,
        &mut output,
        &ROOT_KEY,
        WORKER_PID,
        &FixedClock,
        &mut nonces,
        handler,
    )
    .unwrap_or_else(|error| panic!("server should complete deterministically: {error}"));
    (exit, output)
}

fn output_frames(output: &[u8]) -> Vec<ExecutorFrame> {
    let mut reader = Cursor::new(output);
    let mut frames = Vec::new();
    while let Some(frame) = read_frame(&mut reader)
        .unwrap_or_else(|error| panic!("output frame should decode: {error}"))
    {
        frames.push(frame);
    }
    frames
}

fn assert_handshake_refusal(
    output: &[u8],
    hello: &ipc_contracts::executor_v2::Hello,
    category: ProtocolRefusalCategory,
) {
    let frames = output_frames(output);
    assert_eq!(frames.len(), 2);
    let ExecutorFrame::HandshakeRefusal(refusal) = &frames[1] else {
        panic!("second frame must be a handshake refusal");
    };
    refusal
        .verify(hello, &ROOT_KEY, NOW)
        .unwrap_or_else(|error| panic!("refusal should authenticate: {error}"));
    assert_eq!(refusal.refusal.category, category);
}

fn assert_handshake_refusal_at(
    output: &[u8],
    hello: &ipc_contracts::executor_v2::Hello,
    refusal_index: usize,
    expected_frames: usize,
    category: ProtocolRefusalCategory,
    code: &str,
) {
    let frames = output_frames(output);
    assert_eq!(frames.len(), expected_frames);
    let ExecutorFrame::HandshakeRefusal(refusal) = &frames[refusal_index] else {
        panic!("selected frame must be a handshake refusal");
    };
    refusal
        .verify(hello, &ROOT_KEY, NOW)
        .unwrap_or_else(|error| panic!("refusal should authenticate: {error}"));
    assert_eq!(refusal.refusal.category, category);
    assert_eq!(refusal.refusal.code, code);
}

fn assert_protocol_response(
    output: &[u8],
    response_index: usize,
    category: ProtocolRefusalCategory,
) {
    let frames = output_frames(output);
    let ExecutorFrame::OperationResponse(response) = &frames[response_index] else {
        panic!("selected frame must be an operation response");
    };
    let ExecutorOutcome::ProtocolRefusal { refusal } = &response.outcome else {
        panic!("operation response must be a protocol refusal");
    };
    assert_eq!(refusal.category, category);
}

#[allow(clippy::too_many_arguments)]
fn assert_authenticated_protocol_response(
    output: &[u8],
    response_index: usize,
    open: &OpenSession,
    key: &ipc_contracts::executor_v2::SessionKey,
    sequence: u64,
    operation: &OperationBinding,
    category: ProtocolRefusalCategory,
    code: &str,
) {
    let frames = output_frames(output);
    let ExecutorFrame::OperationResponse(response) = &frames[response_index] else {
        panic!("selected frame must be an operation response");
    };
    response
        .verify(open, sequence, operation, key, NOW)
        .unwrap_or_else(|error| panic!("protocol response should authenticate: {error}"));
    let ExecutorOutcome::ProtocolRefusal { refusal } = &response.outcome else {
        panic!("operation response must be a protocol refusal");
    };
    assert_eq!(refusal.category, category);
    assert_eq!(refusal.code, code);
}

fn expected_hello() -> ipc_contracts::executor_v2::Hello {
    ipc_contracts::executor_v2::Hello::signed(
        WORKER_PID,
        FixedBytes32::from_bytes([1; 32]),
        NOW,
        &ROOT_KEY,
    )
    .unwrap_or_else(|error| panic!("expected hello should sign: {error}"))
}

fn open_session(
    hello: &ipc_contracts::executor_v2::Hello,
    envelope: ImmutableExecutionEnvelope,
    issued_at: i64,
    expires_at: i64,
    key: &[u8; 32],
    child_pid: u32,
) -> OpenSession {
    open_session_authorized(
        hello,
        envelope,
        issued_at,
        expires_at,
        key,
        child_pid,
        SessionAuthorization::Forward,
    )
}

#[allow(clippy::too_many_arguments)]
fn open_session_authorized(
    hello: &ipc_contracts::executor_v2::Hello,
    envelope: ImmutableExecutionEnvelope,
    issued_at: i64,
    expires_at: i64,
    key: &[u8; 32],
    child_pid: u32,
    authorization: SessionAuthorization,
) -> OpenSession {
    OpenSession::signed(
        child_pid,
        21,
        hello.worker_nonce,
        FixedBytes32::from_bytes([2; 32]),
        FixedBytes32::from_bytes([3; 32]),
        issued_at,
        expires_at,
        authorization,
        envelope,
        key,
    )
    .unwrap_or_else(|error| panic!("session should sign: {error}"))
}

fn binding(open: &OpenSession) -> OperationBinding {
    OperationBinding {
        operation_id: open.envelope.operations[0].operation_id.clone(),
        direction: OperationDirection::Forward,
        journal_intent: journal_intent(2),
    }
}

fn journal_intent(sequence: u64) -> CommittedJournalEventBinding {
    CommittedJournalEventBinding {
        database_sequence: sequence,
        database_event_digest: FixedBytes32::from_bytes([31; 32]),
        external_sequence: sequence,
        external_event_digest: FixedBytes32::from_bytes([31; 32]),
    }
}

#[derive(Debug)]
struct FixedClock;

impl Clock for FixedClock {
    fn now_unix_ms(&self) -> Result<i64, ServerError> {
        Ok(NOW)
    }
}

#[derive(Debug)]
struct TestNonces {
    values: VecDeque<FixedBytes32>,
}

impl NonceSource for TestNonces {
    fn next_nonce(&mut self) -> Result<FixedBytes32, ServerError> {
        self.values.pop_front().ok_or(ServerError::Randomness)
    }
}

#[derive(Debug)]
struct MockHandler {
    calls: usize,
    outcomes: VecDeque<HandlerOutcome>,
}

impl MockHandler {
    fn success() -> Self {
        Self::with(HandlerOutcome::Success {
            observed_state_digest: FixedBytes32::from_bytes([99; 32]),
            audit: single_attempt_audit(),
        })
    }

    fn with(outcome: HandlerOutcome) -> Self {
        Self {
            calls: 0,
            outcomes: VecDeque::from(vec![outcome]),
        }
    }
}

impl ExecutorHandler for MockHandler {
    fn handle(
        &mut self,
        _envelope: &ImmutableExecutionEnvelope,
        _operation: &ApprovedOperationManifest,
        _direction: &OperationDirection,
    ) -> HandlerOutcome {
        self.calls += 1;
        self.outcomes
            .pop_front()
            .unwrap_or_else(|| HandlerOutcome::Success {
                observed_state_digest: FixedBytes32::from_bytes([99; 32]),
                audit: single_attempt_audit(),
            })
    }
}

fn envelope() -> ImmutableExecutionEnvelope {
    let proposal_operation_id = domain::ProposalItemId::new().to_string();
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
        execution_id: domain::ExecutionId::new().to_string(),
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
                .unwrap_or_else(|error| panic!("root bytes: {error}")),
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
            operation_id: domain::OperationStepId::new().to_string(),
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
