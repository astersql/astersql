// Copyright 2026 AsterSQL.

use crate::persistent::PersistentJobStore;
use crate::session::{Datum, SessionError, SessionState, WorkerSession};

struct ClaimSession {
    state: SessionState,
    current_job_id: String,
    updates: usize,
}

impl WorkerSession for ClaimSession {
    fn state(&self) -> &SessionState {
        &self.state
    }

    fn state_mut(&mut self) -> &mut SessionState {
        &mut self.state
    }

    fn execute(&mut self, sql: &str, args: &[Datum]) -> Result<Vec<Vec<Datum>>, SessionError> {
        if sql.starts_with("SELECT current_job_id") {
            assert!(sql.contains("current_job_id=%? FOR UPDATE NOWAIT"));
            return Ok(
                (args.get(2) == Some(&Datum::Text(self.current_job_id.clone())))
                    .then(|| vec![vec![Datum::Text(self.current_job_id.clone())]])
                    .unwrap_or_default(),
            );
        }
        if sql.starts_with("UPDATE mysql.tidb_ttl_table_status") {
            self.updates += 1;
        }
        Ok(Vec::new())
    }
}

#[test]
fn timeout_takeover_is_limited_to_current_timer_event() {
    let mut session = ClaimSession {
        state: SessionState::default(),
        current_job_id: "event-a".into(),
        updates: 0,
    };
    assert_eq!(
        PersistentJobStore::takeover_timeout_for_job(
            &mut session,
            7,
            "new-owner",
            300,
            240,
            Some("event-b"),
        )
        .unwrap(),
        None
    );
    assert_eq!(session.updates, 0);
    assert_eq!(
        PersistentJobStore::takeover_timeout_for_job(
            &mut session,
            7,
            "new-owner",
            300,
            240,
            Some("event-a"),
        )
        .unwrap(),
        Some("event-a".into())
    );
    assert_eq!(session.updates, 1);
}
