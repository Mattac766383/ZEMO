#![cfg_attr(not(windows), allow(dead_code))]

use application::{
    ApprovedExecutorClient, ApprovedExecutorError, ApprovedExecutorSession, ExecutorDispatchResult,
    executor_nonce_hash, executor_response_digest, fresh_request_nonce,
    prepare_executor_request_identity,
};
use domain::{
    ExecutorRequestDirection, ExecutorRequestIdentity, ExecutorSessionIdentity,
    ExecutorSessionPurpose, OperationStepId,
};
use ipc_contracts::executor_v2::{
    AuthenticatedOperationRequest, CommittedJournalEventBinding, CoordinatorHandshakeFrame,
    CoordinatorResponseVerifier, CoordinatorSessionFrame, ExecutorFrame, FixedBytes32,
    ImmutableExecutionEnvelope, MAX_MESSAGE_LIFETIME_MS, MAX_SESSION_LIFETIME_MS, OpenSession,
    OperationBinding, OperationDirection, QUALIFICATION_CRASH_ENV, ROOT_AUTHORITY_FILE_ENV,
    SessionAuthorization, SessionKey, SessionOpened, derive_session_key, read_frame, write_frame,
};
use std::{
    collections::BTreeSet,
    fmt, io,
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

const DEFAULT_PROTOCOL_TIMEOUT: Duration = Duration::from_secs(30);
const CHILD_EXIT_GRACE: Duration = Duration::from_millis(250);

pub(crate) struct ProcessApprovedExecutorClient {
    executable: PathBuf,
    root_authority_key: Zeroizing<[u8; 32]>,
    timeout: Duration,
    qualification_crash: Option<&'static str>,
}

impl fmt::Debug for ProcessApprovedExecutorClient {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ProcessApprovedExecutorClient")
            .field("executable", &self.executable)
            .field("root_authority_key", &"<redacted>")
            .field("timeout", &self.timeout)
            .field("qualification_crash", &self.qualification_crash)
            .finish()
    }
}

impl ProcessApprovedExecutorClient {
    pub(crate) fn new(
        executable: PathBuf,
        root_authority_key: [u8; 32],
    ) -> Result<Self, ApprovedExecutorError> {
        validate_executable(&executable)?;
        Ok(Self {
            executable,
            root_authority_key: Zeroizing::new(root_authority_key),
            timeout: DEFAULT_PROTOCOL_TIMEOUT,
            qualification_crash: None,
        })
    }

    #[cfg(test)]
    pub(crate) fn with_qualification_crash(
        executable: PathBuf,
        root_authority_key: [u8; 32],
        phase: &'static str,
    ) -> Result<Self, ApprovedExecutorError> {
        let mut client = Self::new(executable, root_authority_key)?;
        client.qualification_crash = Some(phase);
        Ok(client)
    }
}

impl ApprovedExecutorClient for ProcessApprovedExecutorClient {
    fn open_session(
        &self,
        envelope: ImmutableExecutionEnvelope,
        authorization: SessionAuthorization,
    ) -> Result<Box<dyn ApprovedExecutorSession>, ApprovedExecutorError> {
        envelope
            .validate()
            .map_err(|error| ApprovedExecutorError::Unavailable(error.to_string()))?;
        authorization
            .validate(&envelope)
            .map_err(|error| ApprovedExecutorError::Unavailable(error.to_string()))?;

        let mut transport = ChildTransport::spawn(
            &self.executable,
            &self.root_authority_key,
            self.qualification_crash,
        )?;
        let now = now_unix_ms()?;
        let hello = verify_child_hello(
            transport.recv(self.timeout)?,
            transport.child_id(),
            &self.root_authority_key,
            now,
        )?;

        let expires_at_unix_ms = match &authorization {
            SessionAuthorization::Forward => now
                .saturating_add(MAX_SESSION_LIFETIME_MS)
                .min(envelope.consent.expires_at_unix_ms),
            SessionAuthorization::Rollback { .. } => now.saturating_add(MAX_SESSION_LIFETIME_MS),
        };
        let open = OpenSession::signed(
            transport.child_id(),
            std::process::id(),
            hello.worker_nonce,
            fresh_nonce()?,
            fresh_nonce()?,
            now,
            expires_at_unix_ms,
            authorization,
            envelope,
            &self.root_authority_key,
        )
        .map_err(|error| ApprovedExecutorError::Unavailable(format!("open-signed: {error}")))?;
        let session_key = derive_session_key(&self.root_authority_key, &hello, &open)
            .map_err(|error| ApprovedExecutorError::Unavailable(format!("session-key: {error}")))?;
        transport
            .send(&CoordinatorHandshakeFrame::OpenSession(open.clone()))
            .map_err(|error| ApprovedExecutorError::Unavailable(error.to_string()))?;
        let opened = verify_session_opened(
            transport.recv(self.timeout)?,
            &hello,
            &open,
            &session_key,
            &self.root_authority_key,
            now_unix_ms()?,
        )?;
        let identity = ExecutorSessionIdentity {
            session_id: open.session_id.to_hex(),
            execution_id: open.execution_id.parse().map_err(|_| {
                ApprovedExecutorError::Unavailable("invalid execution identity".to_owned())
            })?,
            plan_id: open.plan_id.parse().map_err(|_| {
                ApprovedExecutorError::Unavailable("invalid plan identity".to_owned())
            })?,
            plan_digest_hex: open.plan_digest.to_hex(),
            purpose: match &open.authorization {
                SessionAuthorization::Forward => ExecutorSessionPurpose::Forward,
                SessionAuthorization::Rollback { .. } => ExecutorSessionPurpose::Rollback,
            },
            coordinator_pid: open.coordinator_pid,
            child_pid: Some(open.child_pid),
            worker_nonce_hash_hex: executor_nonce_hash(open.worker_nonce.as_bytes()),
            coordinator_nonce_hash_hex: executor_nonce_hash(open.coordinator_nonce.as_bytes()),
            response_nonce_hash_hex: Some(executor_nonce_hash(opened.response_nonce.as_bytes())),
            opened_at_unix_ms: open.issued_at_unix_ms,
        };

        Ok(Box::new(ProcessApprovedExecutorSession {
            transport,
            open,
            identity,
            session_key,
            next_sequence: 1,
            response_verifier: CoordinatorResponseVerifier::new(),
            attempted: BTreeSet::new(),
            prepared: None,
            poisoned: false,
            timeout: self.timeout,
        }))
    }
}

fn verify_child_hello(
    frame: ExecutorFrame,
    child_id: u32,
    root_authority_key: &[u8; 32],
    now_unix_ms: i64,
) -> Result<ipc_contracts::executor_v2::Hello, ApprovedExecutorError> {
    let ExecutorFrame::Hello(hello) = frame else {
        return Err(ApprovedExecutorError::Unavailable(
            "executor did not begin with an authenticated hello".to_owned(),
        ));
    };
    hello
        .verify(root_authority_key, now_unix_ms)
        .map_err(|error| ApprovedExecutorError::Unavailable(format!("hello: {error}")))?;
    if hello.worker_pid != child_id {
        return Err(ApprovedExecutorError::Unavailable(
            "executor hello PID does not match the spawned child".to_owned(),
        ));
    }
    Ok(hello)
}

fn verify_session_opened(
    frame: ExecutorFrame,
    hello: &ipc_contracts::executor_v2::Hello,
    open: &OpenSession,
    session_key: &SessionKey,
    root_authority_key: &[u8; 32],
    now_unix_ms: i64,
) -> Result<SessionOpened, ApprovedExecutorError> {
    match frame {
        ExecutorFrame::SessionOpened(opened) => {
            opened
                .verify(open, session_key, now_unix_ms)
                .map_err(|error| {
                    ApprovedExecutorError::Unavailable(format!("session opened: {error}"))
                })?;
            Ok(opened)
        }
        ExecutorFrame::HandshakeRefusal(refusal) => {
            refusal
                .verify(hello, root_authority_key, now_unix_ms)
                .map_err(|error| {
                    ApprovedExecutorError::Unavailable(format!("handshake refusal: {error}"))
                })?;
            Err(ApprovedExecutorError::Unavailable(format!(
                "executor refused the authenticated session: {}",
                refusal.refusal.code
            )))
        }
        _ => Err(ApprovedExecutorError::Unavailable(
            "executor did not acknowledge the authenticated session".to_owned(),
        )),
    }
}

struct ProcessApprovedExecutorSession<T = ChildTransport> {
    transport: T,
    open: OpenSession,
    identity: ExecutorSessionIdentity,
    session_key: SessionKey,
    next_sequence: u64,
    response_verifier: CoordinatorResponseVerifier,
    attempted: BTreeSet<(OperationStepId, bool)>,
    prepared: Option<ExecutorRequestIdentity>,
    poisoned: bool,
    timeout: Duration,
}

trait SessionTransport: Send {
    fn send_operation(&mut self, frame: &CoordinatorSessionFrame) -> Result<(), io::Error>;

    fn recv_operation(&self, timeout: Duration) -> Result<ExecutorFrame, ApprovedExecutorError>;
}

impl<T> ApprovedExecutorSession for ProcessApprovedExecutorSession<T>
where
    T: SessionTransport,
{
    fn identity(&self) -> &ExecutorSessionIdentity {
        &self.identity
    }

    fn prepare_operation(
        &mut self,
        operation_id: OperationStepId,
        direction: OperationDirection,
    ) -> Result<ExecutorRequestIdentity, ApprovedExecutorError> {
        if self.poisoned || self.prepared.is_some() {
            return Err(ApprovedExecutorError::Ambiguous(
                "the executor session cannot prepare another request".to_owned(),
            ));
        }
        let rollback = matches!(&direction, OperationDirection::Rollback);
        if !self.attempted.insert((operation_id, rollback)) {
            return Err(ApprovedExecutorError::Ambiguous(
                "an operation direction cannot be attempted twice in one child session".to_owned(),
            ));
        }
        let operation_id_text = operation_id.to_string();
        if self.open.envelope.operation(&operation_id_text).is_none()
            || !self
                .open
                .authorization
                .permits(&operation_id_text, &direction)
        {
            return Err(ApprovedExecutorError::Ambiguous(
                "operation is outside the immutable session authorization".to_owned(),
            ));
        }
        let request = prepare_executor_request_identity(
            &self.identity,
            operation_id,
            direction,
            self.next_sequence,
            fresh_request_nonce()?,
        )?;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or_else(|| ApprovedExecutorError::Ambiguous("sequence overflow".to_owned()))?;
        self.prepared = Some(request.clone());
        Ok(request)
    }

    fn dispatch_prepared(
        &mut self,
        request: ExecutorRequestIdentity,
        journal_intent: CommittedJournalEventBinding,
    ) -> Result<ExecutorDispatchResult, ApprovedExecutorError> {
        if self.poisoned || self.prepared.take().as_ref() != Some(&request) {
            self.poisoned = true;
            return Err(ApprovedExecutorError::Ambiguous(
                "the committed request does not match the prepared session permit".to_owned(),
            ));
        }
        journal_intent
            .validate()
            .map_err(|error| ApprovedExecutorError::Ambiguous(error.to_string()))?;
        let direction = match request.direction {
            ExecutorRequestDirection::Forward => OperationDirection::Forward,
            ExecutorRequestDirection::Rollback => OperationDirection::Rollback,
        };
        let operation = OperationBinding {
            operation_id: request.operation_id.to_string(),
            direction,
            journal_intent,
        };
        let now =
            now_unix_ms().map_err(|error| ApprovedExecutorError::Ambiguous(error.to_string()))?;
        let authenticated = AuthenticatedOperationRequest::signed(
            &self.open,
            request.request_sequence,
            FixedBytes32::from_bytes(request.request_nonce),
            now,
            now.saturating_add(MAX_MESSAGE_LIFETIME_MS)
                .min(self.open.expires_at_unix_ms),
            operation.clone(),
            &self.session_key,
        )
        .map_err(|error| ApprovedExecutorError::Ambiguous(error.to_string()))?;
        if let Err(error) = self
            .transport
            .send_operation(&CoordinatorSessionFrame::ExecuteOperation(authenticated))
        {
            self.poisoned = true;
            return Err(ApprovedExecutorError::Ambiguous(error.to_string()));
        }
        let ExecutorFrame::OperationResponse(response) = self
            .transport
            .recv_operation(self.timeout)
            .inspect_err(|_| {
                self.poisoned = true;
            })?
        else {
            self.poisoned = true;
            return Err(ApprovedExecutorError::Ambiguous(
                "executor returned an unexpected frame after dispatch".to_owned(),
            ));
        };
        let outcome = self
            .response_verifier
            .verify_next(
                &response,
                &self.open,
                &operation,
                &self.session_key,
                now_unix_ms()
                    .map_err(|error| ApprovedExecutorError::Ambiguous(error.to_string()))?,
            )
            .map_err(|error| {
                self.poisoned = true;
                ApprovedExecutorError::Ambiguous(error.to_string())
            })?
            .clone();
        let response_digest_hex = executor_response_digest(&request, &outcome)?;
        Ok(ExecutorDispatchResult {
            outcome,
            response_digest_hex,
        })
    }
}

impl SessionTransport for ChildTransport {
    fn send_operation(&mut self, frame: &CoordinatorSessionFrame) -> Result<(), io::Error> {
        self.send(frame)
    }

    fn recv_operation(&self, timeout: Duration) -> Result<ExecutorFrame, ApprovedExecutorError> {
        self.recv_ambiguous(timeout)
    }
}

struct ChildTransport {
    child: Child,
    stdin: Option<ChildStdin>,
    receiver: Receiver<Result<ExecutorFrame, String>>,
    reader: Option<JoinHandle<()>>,
    root_file: Option<PathBuf>,
}

impl ChildTransport {
    fn spawn(
        executable: &Path,
        root_authority_key: &[u8; 32],
        qualification_crash: Option<&str>,
    ) -> Result<Self, ApprovedExecutorError> {
        let root_file = write_session_root_file(root_authority_key)?;
        let mut command = Command::new(executable);
        command
            .env_remove(QUALIFICATION_CRASH_ENV)
            .env(ROOT_AUTHORITY_FILE_ENV, &root_file)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(phase) = qualification_crash {
            command.env(QUALIFICATION_CRASH_ENV, phase);
        }
        let mut child = command.spawn().map_err(|error| {
            let _ = std::fs::remove_file(&root_file);
            ApprovedExecutorError::Unavailable(error.to_string())
        })?;
        let stdin = child.stdin.take().ok_or_else(|| {
            let _ = std::fs::remove_file(&root_file);
            ApprovedExecutorError::Unavailable("executor stdin pipe is unavailable".to_owned())
        })?;
        let mut stdout = child.stdout.take().ok_or_else(|| {
            let _ = std::fs::remove_file(&root_file);
            ApprovedExecutorError::Unavailable("executor stdout pipe is unavailable".to_owned())
        })?;
        let (sender, receiver) = mpsc::sync_channel(8);
        let reader = thread::Builder::new()
            .name("operation-executor-protocol-reader".to_owned())
            .spawn(move || {
                loop {
                    let frame = match read_frame(&mut stdout) {
                        Ok(Some(frame)) => Ok(frame),
                        Ok(None) => Err("executor closed its stdout pipe".to_owned()),
                        Err(error) => Err(error.to_string()),
                    };
                    let terminal = frame.is_err();
                    if sender.send(frame).is_err() || terminal {
                        break;
                    }
                }
            })
            .map_err(|error| {
                let _ = std::fs::remove_file(&root_file);
                ApprovedExecutorError::Unavailable(error.to_string())
            })?;
        Ok(Self {
            child,
            stdin: Some(stdin),
            receiver,
            reader: Some(reader),
            root_file: Some(root_file),
        })
    }

    fn child_id(&self) -> u32 {
        self.child.id()
    }

    fn send<T: serde::Serialize>(&mut self, frame: &T) -> Result<(), io::Error> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "executor stdin is closed"))?;
        write_frame(stdin, frame).map_err(io::Error::other)
    }

    fn recv(&self, timeout: Duration) -> Result<ExecutorFrame, ApprovedExecutorError> {
        match self.receiver.recv_timeout(timeout) {
            Ok(Ok(frame)) => Ok(frame),
            Ok(Err(error)) => Err(ApprovedExecutorError::Unavailable(error)),
            Err(RecvTimeoutError::Timeout) => Err(ApprovedExecutorError::Unavailable(
                "executor protocol timed out".to_owned(),
            )),
            Err(RecvTimeoutError::Disconnected) => Err(ApprovedExecutorError::Unavailable(
                "executor protocol reader disconnected".to_owned(),
            )),
        }
    }

    fn recv_ambiguous(&self, timeout: Duration) -> Result<ExecutorFrame, ApprovedExecutorError> {
        self.recv(timeout)
            .map_err(|error| ApprovedExecutorError::Ambiguous(error.to_string()))
    }

    fn close_and_reap(&mut self) {
        self.stdin.take();
        let deadline = Instant::now() + CHILD_EXIT_GRACE;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                Ok(None) | Err(_) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
        if let Some(path) = self.root_file.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

impl Drop for ChildTransport {
    fn drop(&mut self) {
        self.close_and_reap();
    }
}

pub(crate) fn resolve_packaged_sidecar(
    resource_dir: &Path,
    current_executable: &Path,
) -> Result<PathBuf, ApprovedExecutorError> {
    let executable_parent = current_executable.parent().ok_or_else(|| {
        ApprovedExecutorError::Unavailable("application executable has no parent".to_owned())
    })?;
    let file_name = if cfg!(windows) {
        "operation-executor.exe"
    } else {
        "operation-executor"
    };
    let candidates = [
        executable_parent.join(file_name),
        resource_dir.join(file_name),
        resource_dir.join("binaries").join(file_name),
    ];
    for candidate in candidates {
        if validate_executable(&candidate).is_ok() {
            let canonical = candidate
                .canonicalize()
                .map_err(|error| ApprovedExecutorError::Unavailable(error.to_string()))?;
            let trusted_executable = executable_parent
                .canonicalize()
                .map_err(|error| ApprovedExecutorError::Unavailable(error.to_string()))?;
            let trusted_resources = resource_dir
                .canonicalize()
                .map_err(|error| ApprovedExecutorError::Unavailable(error.to_string()))?;
            if canonical.starts_with(&trusted_executable)
                || canonical.starts_with(&trusted_resources)
            {
                return Ok(canonical);
            }
        }
    }
    Err(ApprovedExecutorError::Unavailable(
        "packaged operation executor was not found in a trusted application location".to_owned(),
    ))
}

fn write_session_root_file(
    root_authority_key: &[u8; 32],
) -> Result<PathBuf, ApprovedExecutorError> {
    let mut suffix = [0_u8; 16];
    getrandom::fill(&mut suffix)
        .map_err(|error| ApprovedExecutorError::Unavailable(error.to_string()))?;
    let path = std::env::temp_dir().join(format!(
        "supremacy-executor-root-{}-{}",
        std::process::id(),
        suffix.iter().fold(String::new(), |mut acc, byte| {
            use std::fmt::Write;
            let _ = write!(acc, "{byte:02x}");
            acc
        })
    ));
    privacy::persist_shared_executor_root_to(&path, root_authority_key)
        .map_err(|error| ApprovedExecutorError::Unavailable(error.to_string()))?;
    Ok(path)
}

fn validate_executable(path: &Path) -> Result<(), ApprovedExecutorError> {
    if !path.is_absolute() {
        return Err(ApprovedExecutorError::Unavailable(
            "executor path must be absolute".to_owned(),
        ));
    }
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| ApprovedExecutorError::Unavailable(error.to_string()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(ApprovedExecutorError::Unavailable(
            "executor path is linked or is not a regular file".to_owned(),
        ));
    }
    Ok(())
}

fn fresh_nonce() -> Result<FixedBytes32, ApprovedExecutorError> {
    loop {
        let mut nonce = [0_u8; 32];
        getrandom::fill(&mut nonce)
            .map_err(|error| ApprovedExecutorError::Unavailable(error.to_string()))?;
        let nonce = FixedBytes32::from_bytes(nonce);
        if !nonce.is_zero() {
            return Ok(nonce);
        }
    }
}

fn now_unix_ms() -> Result<i64, ApprovedExecutorError> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| ApprovedExecutorError::Unavailable(error.to_string()))?
        .as_millis();
    i64::try_from(millis).map_err(|error| ApprovedExecutorError::Unavailable(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ipc_contracts::executor_v2::{
        ApprovedOperationManifest, AttestedConsentManifest, AuthenticatedOperationResponse,
        ExecutorOutcome, FrozenPlanManifest, Hello, HexBytes, NativePathEncoding,
        NativePathManifest, OperationPrimitiveManifest, PROTOCOL_VERSION, PlatformKindManifest,
        RootBindingManifest, SCHEMA_VERSION, SafetyPolicyBindingManifest, VolumeIdentityManifest,
    };
    use std::{
        collections::VecDeque,
        sync::{Arc, Mutex},
    };

    struct MockSessionTransport {
        frames: Mutex<VecDeque<Result<ExecutorFrame, String>>>,
        sent: Arc<Mutex<Vec<CoordinatorSessionFrame>>>,
    }

    impl SessionTransport for MockSessionTransport {
        fn send_operation(&mut self, frame: &CoordinatorSessionFrame) -> Result<(), io::Error> {
            self.sent
                .lock()
                .unwrap_or_else(|_| panic!("sent-frame lock should not be poisoned"))
                .push(frame.clone());
            Ok(())
        }

        fn recv_operation(
            &self,
            _timeout: Duration,
        ) -> Result<ExecutorFrame, ApprovedExecutorError> {
            self.frames
                .lock()
                .unwrap_or_else(|_| panic!("frame lock should not be poisoned"))
                .pop_front()
                .unwrap_or_else(|| Err("mock transport timed out".to_owned()))
                .map_err(ApprovedExecutorError::Ambiguous)
        }
    }

    #[test]
    fn child_hello_requires_root_mac_and_spawned_pid() {
        let now =
            now_unix_ms().unwrap_or_else(|error| panic!("test clock should be available: {error}"));
        let hello = Hello::signed(41, FixedBytes32::from_bytes([14; 32]), now, &[15; 32])
            .unwrap_or_else(|error| panic!("test hello should sign: {error}"));
        assert!(
            verify_child_hello(ExecutorFrame::Hello(hello.clone()), 41, &[15; 32], now).is_ok()
        );
        assert!(matches!(
            verify_child_hello(ExecutorFrame::Hello(hello.clone()), 42, &[15; 32], now),
            Err(ApprovedExecutorError::Unavailable(_))
        ));
        assert!(matches!(
            verify_child_hello(ExecutorFrame::Hello(hello), 41, &[16; 32], now),
            Err(ApprovedExecutorError::Unavailable(_))
        ));
    }

    #[test]
    fn mock_transport_verifies_response_mac_and_durable_intent_binding() {
        let (mut session, sent, operation_id, intent) = session_fixture(false);

        let prepared = session
            .prepare_operation(operation_id, OperationDirection::Forward)
            .unwrap_or_else(|error| panic!("operation should prepare: {error}"));
        assert_eq!(prepared.request_sequence, 1);
        assert_eq!(prepared.request_id.len(), 64);
        assert_eq!(prepared.request_digest_hex.len(), 64);
        assert_eq!(session.identity().session_id.len(), 64);
        assert!(
            sent.lock()
                .unwrap_or_else(|_| panic!("sent-frame lock should not be poisoned"))
                .is_empty(),
            "preparing a permit must not dispatch it"
        );
        let result = session
            .dispatch_prepared(prepared.clone(), intent.clone())
            .unwrap_or_else(|error| panic!("authenticated response should succeed: {error}"));
        assert!(matches!(result.outcome, ExecutorOutcome::Success { .. }));
        let sent = sent
            .lock()
            .unwrap_or_else(|_| panic!("sent-frame lock should not be poisoned"));
        let CoordinatorSessionFrame::ExecuteOperation(wire_request) = &sent[0];
        assert_eq!(wire_request.operation.journal_intent, intent);
        drop(sent);
        assert!(matches!(
            session.dispatch_prepared(prepared, intent),
            Err(ApprovedExecutorError::Ambiguous(_))
        ));
    }

    #[test]
    fn mock_transport_maps_bad_mac_to_ambiguous_without_replay() {
        let (mut session, sent, operation_id, intent) = session_fixture(true);
        let request = session
            .prepare_operation(operation_id, OperationDirection::Forward)
            .unwrap_or_else(|error| panic!("operation should prepare: {error}"));

        assert!(matches!(
            session.dispatch_prepared(request, intent),
            Err(ApprovedExecutorError::Ambiguous(_))
        ));
        assert_eq!(
            sent.lock()
                .unwrap_or_else(|_| panic!("sent-frame lock should not be poisoned"))
                .len(),
            1
        );
    }

    #[test]
    fn mock_transport_maps_executor_eof_to_ambiguous_without_replay() {
        let (mut session, sent, operation_id, intent) = session_fixture(false);
        session.transport.frames = Mutex::new(VecDeque::from([Err(
            "executor EOF before acknowledgement".to_owned(),
        )]));
        let request = session
            .prepare_operation(operation_id, OperationDirection::Forward)
            .unwrap_or_else(|error| panic!("operation should prepare: {error}"));

        assert!(matches!(
            session.dispatch_prepared(request, intent),
            Err(ApprovedExecutorError::Ambiguous(_))
        ));
        assert_eq!(
            sent.lock()
                .unwrap_or_else(|_| panic!("sent-frame lock should not be poisoned"))
                .len(),
            1
        );
        assert!(matches!(
            session.prepare_operation(operation_id, OperationDirection::Forward),
            Err(ApprovedExecutorError::Ambiguous(_))
        ));
    }

    #[test]
    fn mock_transport_never_dispatches_an_operation_outside_the_envelope() {
        let (mut session, sent, _operation_id, _intent) = session_fixture(false);
        assert!(matches!(
            session.prepare_operation(OperationStepId::new(), OperationDirection::Forward),
            Err(ApprovedExecutorError::Ambiguous(_))
        ));
        assert!(
            sent.lock()
                .unwrap_or_else(|_| panic!("sent-frame lock should not be poisoned"))
                .is_empty()
        );
    }

    type MockSession = ProcessApprovedExecutorSession<MockSessionTransport>;

    fn session_fixture(
        corrupt_response_mac: bool,
    ) -> (
        MockSession,
        Arc<Mutex<Vec<CoordinatorSessionFrame>>>,
        OperationStepId,
        CommittedJournalEventBinding,
    ) {
        let operation_id = OperationStepId::new();
        let envelope = minimal_envelope(operation_id);
        let now =
            now_unix_ms().unwrap_or_else(|error| panic!("test clock should be available: {error}"));
        let open = OpenSession {
            protocol_version: PROTOCOL_VERSION,
            schema_version: SCHEMA_VERSION,
            child_pid: 7,
            coordinator_pid: 8,
            worker_nonce: FixedBytes32::from_bytes([1; 32]),
            coordinator_nonce: FixedBytes32::from_bytes([2; 32]),
            session_id: FixedBytes32::from_bytes([3; 32]),
            execution_id: envelope.execution_id.clone(),
            plan_id: envelope.plan.plan_id.clone(),
            plan_digest: envelope.plan.digest,
            issued_at_unix_ms: now,
            expires_at_unix_ms: now + 10_000,
            authorization: SessionAuthorization::Forward,
            envelope,
            mac: FixedBytes32::from_bytes([4; 32]),
        };
        let hello = Hello {
            protocol_version: PROTOCOL_VERSION,
            schema_version: SCHEMA_VERSION,
            worker_pid: open.child_pid,
            worker_nonce: open.worker_nonce,
            issued_at_unix_ms: now,
            mac: FixedBytes32::from_bytes([5; 32]),
        };
        let session_key = derive_session_key(&[42; 32], &hello, &open)
            .unwrap_or_else(|error| panic!("test session key should derive: {error}"));
        let identity = session_identity_fixture(&open, now);
        let intent = CommittedJournalEventBinding {
            database_sequence: 2,
            database_event_digest: FixedBytes32::from_bytes([6; 32]),
            external_sequence: 2,
            external_event_digest: FixedBytes32::from_bytes([6; 32]),
        };
        let operation = OperationBinding {
            operation_id: operation_id.to_string(),
            direction: OperationDirection::Forward,
            journal_intent: intent.clone(),
        };
        let mut response = AuthenticatedOperationResponse::signed(
            &open,
            1,
            FixedBytes32::from_bytes([7; 32]),
            now,
            now + 5_000,
            operation,
            ExecutorOutcome::Success {
                applied_at_unix_ms: now,
                observed_state_digest: FixedBytes32::from_bytes([8; 32]),
                audit: ipc_contracts::executor_v2::ExecutorAttemptAudit {
                    attempt_count: 1,
                    error_class: None,
                },
            },
            &session_key,
        )
        .unwrap_or_else(|error| panic!("test response should sign: {error}"));
        if corrupt_response_mac {
            response.mac = FixedBytes32::from_bytes([9; 32]);
        }
        let sent = Arc::new(Mutex::new(Vec::new()));
        (
            ProcessApprovedExecutorSession {
                transport: MockSessionTransport {
                    frames: Mutex::new(VecDeque::from([Ok(ExecutorFrame::OperationResponse(
                        response,
                    ))])),
                    sent: sent.clone(),
                },
                open,
                session_key,
                next_sequence: 1,
                identity,
                response_verifier: CoordinatorResponseVerifier::new(),
                attempted: BTreeSet::new(),
                prepared: None,
                poisoned: false,
                timeout: Duration::from_millis(1),
            },
            sent,
            operation_id,
            intent,
        )
    }

    fn session_identity_fixture(open: &OpenSession, now: i64) -> ExecutorSessionIdentity {
        ExecutorSessionIdentity {
            session_id: open.session_id.to_hex(),
            execution_id: open
                .execution_id
                .parse()
                .unwrap_or_else(|error| panic!("test execution id should parse: {error}")),
            plan_id: open
                .plan_id
                .parse()
                .unwrap_or_else(|error| panic!("test plan id should parse: {error}")),
            plan_digest_hex: open.plan_digest.to_hex(),
            purpose: ExecutorSessionPurpose::Forward,
            coordinator_pid: open.coordinator_pid,
            child_pid: Some(open.child_pid),
            worker_nonce_hash_hex: executor_nonce_hash(open.worker_nonce.as_bytes()),
            coordinator_nonce_hash_hex: executor_nonce_hash(open.coordinator_nonce.as_bytes()),
            response_nonce_hash_hex: Some(executor_nonce_hash(&[12; 32])),
            opened_at_unix_ms: now,
        }
    }

    fn minimal_envelope(operation_id: OperationStepId) -> ImmutableExecutionEnvelope {
        ImmutableExecutionEnvelope {
            schema_version: SCHEMA_VERSION,
            execution_id: "00000000-0000-4000-8000-000000000001".to_owned(),
            root_id: "00000000-0000-4000-8000-000000000002".to_owned(),
            plan: FrozenPlanManifest {
                material_version: 2,
                plan_id: "00000000-0000-4000-8000-000000000003".to_owned(),
                proposal_id: "00000000-0000-4000-8000-000000000004".to_owned(),
                proposal_revision_id: "00000000-0000-4000-8000-000000000005".to_owned(),
                proposal_revision: 1,
                source_snapshot_version: "00000000-0000-4000-8000-000000000006".to_owned(),
                approved_operation_ids: vec!["00000000-0000-4000-8000-000000000007".to_owned()],
                operation_count: 1,
                approval_timestamp: "2026-08-11T00:00:00Z".to_owned(),
                user_confirmed: true,
                digest: FixedBytes32::from_bytes([10; 32]),
            },
            root_binding: RootBindingManifest {
                canonical_path: NativePathManifest {
                    encoding: NativePathEncoding::UnixBytes,
                    bytes: HexBytes::new(vec![b'/'])
                        .unwrap_or_else(|error| panic!("test path should encode: {error}")),
                },
                display_path: "/".to_owned(),
                volume: VolumeIdentityManifest {
                    platform: PlatformKindManifest::Other,
                    stable_identifier: "test-volume".to_owned(),
                    filesystem_type: Some("test".to_owned()),
                    case_sensitive: true,
                    removable: false,
                    local: true,
                },
            },
            safety_policy_binding: SafetyPolicyBindingManifest {
                version: domain::EXECUTION_SAFETY_POLICY_VERSION.to_owned(),
                digest: FixedBytes32::from_bytes([11; 32]),
                maximum_rehash_bytes: domain::MAX_EXECUTION_VERIFICATION_BYTES,
                allow_qualified_case_only_rename: false,
            },
            consent: AttestedConsentManifest {
                issued_at_unix_ms: 1,
                expires_at_unix_ms: i64::MAX,
                attested_at_unix_ms: 2,
                consent_nonce: FixedBytes32::from_bytes([12; 32]),
                attestation_mac: FixedBytes32::from_bytes([13; 32]),
            },
            operations: vec![ApprovedOperationManifest {
                operation_id: operation_id.to_string(),
                proposal_operation_id: None,
                sequence: 0,
                dependencies: Vec::new(),
                primitive: OperationPrimitiveManifest::CreateDirectory {
                    destination_relative_path: "safe".to_owned(),
                },
            }],
        }
    }
}
