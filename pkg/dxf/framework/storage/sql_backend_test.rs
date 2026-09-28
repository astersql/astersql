// Copyright 2026 AsterSQL.

use super::*;

struct Backend {
    calls: Arc<Mutex<Vec<String>>>,
}
impl SQLBackend for Backend {
    fn execute(&self, sql: &str, args: Vec<Value>) -> Result<SQLResult, Error> {
        self.calls.lock().unwrap().push(sql.to_owned());
        if sql == "fail" {
            return Err(Error::new("backend error"));
        }
        Ok(SQLResult {
            rows: vec![chunk::Row::new(args)],
            affected_rows: 7,
        })
    }
}
#[test]
fn sql_backend_preserves_rows_affected_counts_and_transaction_boundaries() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let created = Arc::new(AtomicUsize::new(0));
    let manager = NewTaskManager(util::SessionPool::with_factory({
        let calls = calls.clone();
        let created = created.clone();
        move || {
            created.fetch_add(1, Ordering::SeqCst);
            Ok(sessionctx::Context::with_backend(Arc::new(Backend {
                calls: calls.clone(),
            })))
        }
    }));
    manager
        .WithNewTxn((), |session| {
            let rows =
                sqlexec::ExecSQL((), session.GetSQLExecutor(), "write", vec![42_i64.into()])?;
            assert_eq!(rows[0].GetInt64(0), 42);
            assert_eq!(session.GetSessionVars().StmtCtx.AffectedRows(), 7);
            Ok(())
        })
        .unwrap();
    let failure = manager
        .WithNewTxn((), |session| {
            sqlexec::ExecSQL((), session.GetSQLExecutor(), "fail", vec![])?;
            Ok(())
        })
        .unwrap_err();
    assert!(failure.to_string().contains("backend error"));
    assert_eq!(
        *calls.lock().unwrap(),
        ["begin", "write", "commit", "begin", "fail", "rollback"]
    );
    assert_eq!(
        created.load(Ordering::SeqCst),
        2,
        "leases must not share a live transaction"
    );
}
