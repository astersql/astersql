// Copyright 2026 AsterSQL.

use super::*;

#[test]
fn privilege_error_kinds_match_go_standard_errors() {
    let cases = [
        (
            ErrInvalidPrivilegeType,
            "ErrInvalidPrivilegeType",
            8050,
            "unknown privilege type %s",
        ),
        (
            ErrNonexistingGrant,
            "ErrNonexistingGrant",
            1141,
            "There is no such grant defined for user '%-.48s' on host '%-.255s'",
        ),
        (
            ErrLoadPrivilege,
            "ErrLoadPrivilege",
            8049,
            "Load privilege table fail: %s",
        ),
        (
            ErrAccessDenied,
            "ErrAccessDenied",
            1045,
            "Access denied for user '%-.48s'@'%-.255s' (using password: %s)",
        ),
        (
            ErrAccountHasBeenLocked,
            "ErrAccountHasBeenLocked",
            3118,
            "Access denied for user '%s'@'%s'. Account is locked.",
        ),
        (
            ErUserAccessDeniedForUserAccountBlockedByPasswordLock,
            "ErUserAccessDeniedForUserAccountBlockedByPasswordLock",
            3955,
            "Access denied for user '%s'@'%s'. Account is blocked for %s day(s) (%s day(s) remaining) due to %d consecutive failed logins.",
        ),
        (
            ErrMustChangePasswordLogin,
            "ErrMustChangePasswordLogin",
            1862,
            "Your password has expired. To log in you must change it using a client that supports expired passwords.",
        ),
    ];

    for (kind, name, code, message_template) in cases {
        assert_eq!(kind.name(), name);
        assert_eq!(kind.code(), code);
        assert_eq!(kind.message_template(), message_template);
    }
}
