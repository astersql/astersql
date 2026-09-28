// Copyright 2026 AsterSQL.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::asynctestkit::NewAsyncTestKit;
use crate::{Database, DbValue, ExecutionResult, QueryRows, TestResult};

#[derive(Default)]
struct SessionState {
    close_count: AtomicUsize,
    statements: Mutex<Vec<String>>,
}

struct Store {
    session: Arc<SessionState>,
}

struct Session {
    state: Arc<SessionState>,
}

impl Database for Store {
    fn execute(&self, _sql: &str, _arguments: &[DbValue]) -> TestResult<ExecutionResult> {
        unreachable!("the store must be used only to create a session")
    }

    fn query(&self, _sql: &str, _arguments: &[DbValue]) -> TestResult<QueryRows> {
        unreachable!("the store must be used only to create a session")
    }

    fn create_session(&self) -> TestResult<Option<Arc<dyn Database>>> {
        Ok(Some(Arc::new(Session {
            state: Arc::clone(&self.session),
        })))
    }
}

impl Database for Session {
    fn execute(&self, sql: &str, _arguments: &[DbValue]) -> TestResult<ExecutionResult> {
        self.state.statements.lock().unwrap().push(sql.to_owned());
        Ok(ExecutionResult::default())
    }

    fn query(&self, sql: &str, _arguments: &[DbValue]) -> TestResult<QueryRows> {
        self.state.statements.lock().unwrap().push(sql.to_owned());
        Ok(QueryRows {
            columns: vec!["value".to_owned()],
            rows: vec![vec![DbValue::String("ok".to_owned())]],
        })
    }

    fn close(&self) -> TestResult {
        self.state.close_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn worker_serializes_commands_and_closes_its_session() {
    let state = Arc::new(SessionState::default());
    let store: Arc<dyn Database> = Arc::new(Store {
        session: Arc::clone(&state),
    });

    {
        let testkit = NewAsyncTestKit(store);
        testkit.Exec("insert", Vec::new()).unwrap();
        let result = testkit.MustQuery("select", Vec::new());
        result.Check(vec![vec!["ok"]]);
        testkit.Sync();
    }

    assert_eq!(
        *state.statements.lock().unwrap(),
        ["insert".to_owned(), "select".to_owned()]
    );
    assert_eq!(state.close_count.load(Ordering::SeqCst), 1);
}
