//! Protocol v2 for the isolated filesystem operation executor.
//!
//! The coordinator can select only an operation already frozen into an
//! authenticated session envelope. It cannot send an arbitrary source or
//! destination path to the executor.

mod auth;
mod framing;
mod model;

pub use auth::{
    AuthenticatedOperationRequest, AuthenticatedOperationResponse, AuthenticationError,
    CoordinatorResponseVerifier, HandshakeRefusal, Hello, OpenSession, SessionKey, SessionOpened,
    derive_consent_authority_key, derive_session_key, sign_consent_attestation,
};
pub use framing::{
    CoordinatorHandshakeFrame, CoordinatorSessionFrame, ExecutorFrame, FrameError, read_frame,
    write_frame,
};
pub use model::{
    ApprovedOperationManifest, AttestedConsentManifest, CommittedJournalEventBinding,
    ConsentAttestationBinding, ExecutorAttemptAudit, ExecutorErrorClass, ExecutorOutcome,
    ExpectedFileStateManifest, FixedBytes32, FrozenPlanManifest, HexBytes,
    ImmutableExecutionEnvelope, NativeFileIdentityManifest, NativePathEncoding, NativePathManifest,
    OperationBinding, OperationDirection, OperationPrimitiveManifest, PlatformKindManifest,
    ProtocolRefusal, ProtocolRefusalCategory, RollbackEligibility, RollbackEligibilityState,
    RootBindingManifest, SafetyPolicyBindingManifest, SessionAuthorization, ValidationError,
    VolumeIdentityManifest,
};

/// Name of the OS-keystore entry shared by the trusted coordinator and child.
pub const ROOT_AUTHORITY_SECRET_NAME: &str = "operation-executor-auth-v2";
/// Tauri/keyring service used by the desktop application.
pub const ROOT_AUTHORITY_SECRET_SERVICE: &str = "com.workingname.organizer";
/// Session-scoped 0600 file path inherited by the isolated helper.
///
/// The coordinator already holds the root key. Ad-hoc macOS Keychain ACLs do
/// not share that item with a separately identified sidecar, so the parent
/// passes a temporary file path instead of asking the child to re-read the
/// keystore.
pub const ROOT_AUTHORITY_FILE_ENV: &str = "WORKING_NAME_EXECUTOR_ROOT_FILE";
/// Qualification-only crash injection. Ignored unless the coordinator sets it.
pub const QUALIFICATION_CRASH_ENV: &str = "WORKING_NAME_QUALIFICATION_CRASH";
pub const PROTOCOL_VERSION: u16 = 2;
pub const SCHEMA_VERSION: u16 = 2;
// A 10,000-file plan also contains directory and internal staging steps.
// Keep the structural bound above the qualification target; the independent
// frame-size limit remains the final memory bound.
pub const MAX_MANIFESTS: usize = 100_000;
pub const MAX_FRAME_BYTES: usize = 128 * 1024 * 1024;
pub const MAX_RELATIVE_PATH_BYTES: usize = 4_096;
pub const MAX_NATIVE_PATH_BYTES: usize = 65_536;
pub const MAX_IDENTITY_BYTES: usize = 4_096;
pub const MAX_TEXT_BYTES: usize = 2_048;
pub const MAX_ID_BYTES: usize = 128;
pub const MAX_CLOCK_SKEW_MS: i64 = 30_000;
pub const MAX_SESSION_LIFETIME_MS: i64 = 15 * 60 * 1_000;
pub const MAX_MESSAGE_LIFETIME_MS: i64 = 30_000;

pub(crate) const HELLO_MAC_DOMAIN: &[u8] = b"com.workingname.operation-executor/v2/hello-mac\0";
pub(crate) const OPEN_SESSION_MAC_DOMAIN: &[u8] =
    b"com.workingname.operation-executor/v2/open-session-mac\0";
pub(crate) const SESSION_KEY_DOMAIN: &[u8] = b"com.workingname.operation-executor/v2/session-key\0";
pub(crate) const CONSENT_KEY_DOMAIN: &[u8] =
    b"com.workingname.operation-executor/v2/consent-authority-key\0";
pub(crate) const CONSENT_ATTESTATION_MAC_DOMAIN: &[u8] =
    b"com.workingname.operation-executor/v2/consent-attestation-mac\0";
pub(crate) const SESSION_OPENED_MAC_DOMAIN: &[u8] =
    b"com.workingname.operation-executor/v2/session-opened-mac\0";
pub(crate) const REQUEST_MAC_DOMAIN: &[u8] = b"com.workingname.operation-executor/v2/request-mac\0";
pub(crate) const RESPONSE_MAC_DOMAIN: &[u8] =
    b"com.workingname.operation-executor/v2/response-mac\0";
pub(crate) const REFUSAL_MAC_DOMAIN: &[u8] =
    b"com.workingname.operation-executor/v2/handshake-refusal-mac\0";

#[cfg(test)]
mod tests;
