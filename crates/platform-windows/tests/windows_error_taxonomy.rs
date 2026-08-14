use platform::{PlatformError, PlatformErrorClass, classify_windows_error_code};

#[test]
fn win32_codes_map_to_stable_portable_error_classes() {
    for (code, expected) in [
        (2, PlatformErrorClass::SourceMissing),
        (3, PlatformErrorClass::SourceMissing),
        (5, PlatformErrorClass::PermissionDenied),
        (32, PlatformErrorClass::SharingViolation),
        (33, PlatformErrorClass::LockViolation),
        (39, PlatformErrorClass::DiskFull),
        (112, PlatformErrorClass::DiskFull),
        (80, PlatformErrorClass::DestinationCollision),
        (183, PlatformErrorClass::DestinationCollision),
        (1_920, PlatformErrorClass::PathPolicyRefusal),
        (4_390, PlatformErrorClass::PathPolicyRefusal),
        (4_394, PlatformErrorClass::PathPolicyRefusal),
    ] {
        assert_eq!(classify_windows_error_code(code), expected, "Win32 {code}");
        let error = PlatformError::from_windows_code(code, false);
        assert_eq!(error.class(), expected, "Win32 {code}");
        assert_eq!(error.class().code(), expected.code(), "Win32 {code}");
    }

    let unknown = PlatformError::from_windows_code(0xffff, false);
    assert_eq!(unknown.class(), PlatformErrorClass::Io);
    assert!(matches!(
        unknown,
        PlatformError::Io(ref error) if error.raw_os_error() == Some(0xffff)
    ));
}

#[test]
fn disk_full_is_deterministically_classified_without_exhausting_a_real_volume() {
    // ERROR_HANDLE_DISK_FULL and ERROR_DISK_FULL are injected as numeric
    // Win32 outcomes. Native qualification deliberately never fills a disk.
    for code in [39, 112] {
        assert!(matches!(
            PlatformError::from_windows_code(code, false),
            PlatformError::DiskFull
        ));
    }
}

#[test]
fn only_pre_mutation_lock_failures_are_retryable() {
    for error in [
        PlatformError::SharingViolation,
        PlatformError::LockViolation,
    ] {
        assert!(error.retryable_before_mutation());
    }

    for error in [
        PlatformError::PermissionDenied,
        PlatformError::DiskFull,
        PlatformError::DestinationExists,
        PlatformError::SourceMissing,
        PlatformError::PathPolicyRefusal,
        PlatformError::AmbiguousMutationOutcome,
    ] {
        assert!(!error.retryable_before_mutation());
    }
}

#[test]
fn uncertain_native_outcomes_are_never_presented_as_retryable() {
    for code in [5, 32, 33, 39, 80, 112, 183, 0xffff] {
        let error = PlatformError::from_windows_code(code, true);
        assert!(matches!(error, PlatformError::AmbiguousMutationOutcome));
        assert!(!error.retryable_before_mutation());
    }
}
