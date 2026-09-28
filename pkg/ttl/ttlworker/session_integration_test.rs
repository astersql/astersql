// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// TTL pooled-session lifecycle integration tests. These mirror Go's injected
// setup/restore failures and verify that a damaged session is never reused.

use crate::session::{
    Datum, Row, SessionError, SessionState, WorkerSession, prepare_scan_session_checked,
    prepare_session_checked, restore_scan_session_checked, restore_session_checked,
};

#[derive(Default)]
struct FaultSession {
    state: SessionState,
    fault: Option<String>,
    panic: bool,
    unusable: bool,
    executed: Vec<String>,
}

impl WorkerSession for FaultSession {
    fn state(&self) -> &SessionState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut SessionState {
        &mut self.state
    }

    fn execute(&mut self, sql: &str, _: &[Datum]) -> Result<Vec<Row>, SessionError> {
        self.executed.push(sql.to_owned());
        if self
            .fault
            .as_ref()
            .is_some_and(|needle| sql.starts_with(needle))
        {
            if self.panic {
                panic!("{sql}");
            }
            return Err(SessionError::Execute("fault in test".into()));
        }
        Ok(vec![])
    }

    fn avoid_reuse(&mut self) {
        self.unusable = true;
    }
}

fn initial_session() -> FaultSession {
    let mut session = FaultSession::default();
    session.state.in_transaction = true;
    session.state.timezone_offset_seconds = 8 * 60 * 60;
    session.state.distsql_scan_concurrency = 123;
    session.state.enable_paging = true;
    session
        .state
        .variables
        .insert("tidb_retry_limit".into(), "10".into());
    session
        .state
        .variables
        .insert("tidb_enable_1pc".into(), "OFF".into());
    session
        .state
        .variables
        .insert("tidb_enable_async_commit".into(), "OFF".into());
    session
        .state
        .variables
        .insert("time_zone".into(), "+08:00".into());
    session
        .state
        .variables
        .insert("tidb_isolation_read_engines".into(), "tikv,tidb".into());
    session
}

#[test]
fn pooled_session_prepare_faults_and_panics_prevent_reuse() {
    for sql in [
        "set tidb_retry_limit=0",
        "set tidb_enable_1pc=ON",
        "set tidb_enable_async_commit=ON",
        "ROLLBACK",
        "set @@time_zone='UTC'",
        "set tidb_isolation_read_engines='tikv,tiflash,tidb'",
    ] {
        let mut session = initial_session();
        session.fault = Some(sql.into());
        assert_eq!(
            prepare_session_checked(&mut session),
            Err(SessionError::Execute("fault in test".into()))
        );
        assert!(session.unusable, "failed setup must discard session: {sql}");

        let mut session = initial_session();
        session.fault = Some(sql.into());
        session.panic = true;
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = prepare_session_checked(&mut session);
        }));
        assert!(panic.is_err());
        assert!(
            session.unusable,
            "panicking setup must discard session: {sql}"
        );
    }
}

#[test]
fn pooled_session_prepare_and_restore_preserve_every_setting() {
    let mut session = initial_session();
    let original = session.state.clone();
    let previous = prepare_session_checked(&mut session).unwrap();
    assert!(!session.state.in_transaction);
    assert_eq!(session.state.timezone_offset_seconds, 0);
    assert_eq!(session.state.variables["tidb_retry_limit"], "0");
    assert_eq!(session.state.variables["tidb_enable_1pc"], "ON");
    assert_eq!(session.state.variables["tidb_enable_async_commit"], "ON");
    assert_eq!(session.state.variables["time_zone"], "UTC");
    assert_eq!(
        session.state.variables["tidb_isolation_read_engines"],
        "tikv,tiflash,tidb"
    );
    restore_session_checked(&mut session, previous).unwrap();
    assert_eq!(session.state.variables, original.variables);
    assert_eq!(session.state.in_transaction, original.in_transaction);
    assert_eq!(
        session.state.timezone_offset_seconds,
        original.timezone_offset_seconds
    );
    assert!(!session.unusable);
}

#[test]
fn restore_fault_discards_session_and_stops_at_the_first_failure() {
    for prefix in [
        "set tidb_retry_limit=",
        "set tidb_enable_1pc=",
        "set tidb_enable_async_commit=",
        "set @@time_zone=",
        "set tidb_isolation_read_engines=",
    ] {
        let mut session = initial_session();
        let previous = prepare_session_checked(&mut session).unwrap();
        session.fault = Some(prefix.into());
        assert_eq!(
            restore_session_checked(&mut session, previous),
            Err(SessionError::Execute("fault in test".into()))
        );
        assert!(
            session.unusable,
            "failed restore must discard session: {prefix}"
        );
    }
}

#[test]
fn scan_setup_and_restore_cover_success_and_each_sql_failure() {
    let mut session = initial_session();
    let previous = prepare_scan_session_checked(&mut session).unwrap();
    assert!(session.state.internal_sql_scan_user_table);
    assert_eq!(session.state.distsql_scan_concurrency, 1);
    assert!(!session.state.enable_paging);
    restore_scan_session_checked(&mut session, previous).unwrap();
    assert!(!session.state.internal_sql_scan_user_table);
    assert_eq!(session.state.distsql_scan_concurrency, 123);
    assert!(session.state.enable_paging);

    for sql in [
        "set @@tidb_distsql_scan_concurrency=1",
        "set @@tidb_enable_paging=OFF",
    ] {
        let mut session = initial_session();
        session.fault = Some(sql.into());
        assert_eq!(
            prepare_scan_session_checked(&mut session),
            Err(SessionError::Execute("fault in test".into()))
        );
        assert!(!session.state.internal_sql_scan_user_table);
    }

    for prefix in [
        "set @@tidb_distsql_scan_concurrency=",
        "set @@tidb_enable_paging=",
    ] {
        let mut session = initial_session();
        let previous = prepare_scan_session_checked(&mut session).unwrap();
        let before = session.executed.len();
        session.fault = Some(prefix.into());
        assert!(restore_scan_session_checked(&mut session, previous).is_err());
        assert!(session.unusable);
        assert_eq!(session.executed.len() - before, 2, "both restores must run");
    }
}
