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

#[test]
fn batch_history_transfer_rolls_back_at_each_sql_failure() {
    struct FailingBackend {
        fail_at: usize,
        calls: Arc<Mutex<Vec<String>>>,
    }
    impl SQLBackend for FailingBackend {
        fn execute(&self, sql: &str, _: Vec<Value>) -> Result<SQLResult, Error> {
            let mut calls = self.calls.lock().unwrap();
            calls.push(sql.into());
            if calls.len() == self.fail_at {
                return Err(Error::new("history SQL failed"));
            }
            Ok(SQLResult {
                rows: vec![],
                affected_rows: 1,
            })
        }
    }
    for fail_at in 2..=6 {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let manager = NewTaskManager(util::SessionPool::with_factory({
            let calls = calls.clone();
            move || {
                Ok(sessionctx::Context::with_backend(Arc::new(
                    FailingBackend {
                        fail_at,
                        calls: calls.clone(),
                    },
                )))
            }
        }));
        let task = proto::Task {
            TaskBase: proto::TaskBase {
                ID: 42,
                Key: "history-error".into(),
                Type: proto::TaskTypeExample,
                State: proto::TaskStateSucceed,
                Step: proto::StepDone,
                Priority: 512,
                RequiredSlots: 1,
                TargetScope: String::new(),
                CreateTime: SystemTime::UNIX_EPOCH,
                MaxNodeCount: 0,
                ExtraParams: proto::ExtraParams::default(),
                Keyspace: "SYSTEM".into(),
            },
            SchedulerID: String::new(),
            StartTime: SystemTime::UNIX_EPOCH,
            StateUpdateTime: SystemTime::UNIX_EPOCH,
            Meta: b"redacted".to_vec(),
            Error: None,
            ModifyParam: proto::ModifyParam {
                PrevState: proto::TaskStateSucceed,
                Modifications: vec![],
            },
        };
        assert!(
            manager
                .TransferTasks2History((), vec![task])
                .unwrap_err()
                .to_string()
                .contains("history SQL failed")
        );
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), fail_at + 1);
        assert_eq!(calls[0], "begin");
        assert_eq!(calls.last().unwrap(), "rollback");
        assert!(!calls.iter().any(|sql| sql == "commit"));
    }
}
