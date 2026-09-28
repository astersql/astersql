// Copyright 2026 AsterSQL.

// `pkg/domain/domain_utils_test.go` parity tests.

/// The domain errors must retain the public MySQL error codes used by clients.
#[test]
fn error_codes_match_go() {
    use crate::domain::{ERR_INFO_SCHEMA_CHANGED, ERR_INFO_SCHEMA_EXPIRED};
    use astersql_util_dbterror::terror::ToSQLError;

    assert_eq!(ToSQLError(&ERR_INFO_SCHEMA_EXPIRED).Code, 8027);
    assert_eq!(ToSQLError(&ERR_INFO_SCHEMA_CHANGED).Code, 8028);
}

/// A node must detect a lost PD connection before its server-ID lease expires.
#[test]
fn server_id_timeout_order_matches_go() {
    use crate::domain::{LOST_CONNECTION_TO_PD_TIMEOUT, SERVER_ID_TTL};

    assert!(LOST_CONNECTION_TO_PD_TIMEOUT < SERVER_ID_TTL);
}
