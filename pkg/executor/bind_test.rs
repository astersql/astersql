// Copyright 2026 AsterSQL.

use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::bind::{Binding, SQLBindBackend, SQLBindExec, SQLBindOpDetail, SQLBindOpType};

#[derive(Default)]
struct PanicBackend {
    restored_context: bool,
}

impl SQLBindBackend<()> for PanicBackend {
    type Error = String;
    type StatementContext = ();

    fn error(&self, message: String) -> Self::Error {
        message
    }

    fn normalize_digest_for_binding(&self, sql: &str) -> String {
        sql.to_owned()
    }

    fn drop_session_bindings(&mut self, _: &[String]) -> Result<(), Self::Error> {
        Ok(())
    }

    fn drop_global_bindings(&mut self, _: &[String]) -> (u64, Result<(), Self::Error>) {
        (0, Ok(()))
    }

    fn add_affected_rows(&mut self, _: u64) {}

    fn set_global_binding_status(&mut self, _: &str, _: &str) -> (bool, Result<(), Self::Error>) {
        (true, Ok(()))
    }

    fn append_warning(&mut self, _: &str) {}

    fn take_statement_context(&mut self) -> Self::StatementContext {}

    fn current_set_var_hint_restore(&self) -> Vec<(String, String)> {
        Vec::new()
    }

    fn add_set_var_hint_restore(&mut self, _: &mut Self::StatementContext, _: String, _: String) {}

    fn restore_statement_context(&mut self, _: Self::StatementContext) {
        self.restored_context = true;
    }

    fn create_session_bindings(&mut self, _: &(), _: &[Binding]) -> Result<(), Self::Error> {
        panic!("create binding panic")
    }

    fn create_global_bindings(&mut self, _: &(), _: &[Binding]) -> Result<(), Self::Error> {
        panic!("create binding panic")
    }

    fn load_bindings(&mut self, _: bool, _: bool) -> Result<(), Self::Error> {
        Ok(())
    }

    fn broadcast(&mut self, _: &(), _: &str) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[test]
fn create_binding_restores_statement_context_when_backend_panics() {
    let mut executor = SQLBindExec {
        BaseExecutor: PanicBackend::default(),
        isGlobal: false,
        sqlBindOp: SQLBindOpType::Create,
        details: vec![SQLBindOpDetail {
            NormdOrigSQL: "select * from t".into(),
            Db: "test".into(),
            BindSQL: "select /*+ use_index(t, primary) */ * from t".into(),
            Charset: "utf8mb4".into(),
            Collation: "utf8mb4_bin".into(),
            NewStatus: String::new(),
            Source: "manual".into(),
            SQLDigest: "sql-digest".into(),
            PlanDigest: "plan-digest".into(),
        }],
        isFromRemote: false,
    };

    let panic = catch_unwind(AssertUnwindSafe(|| executor.createSQLBind(&())));

    assert!(panic.is_err());
    assert!(executor.BaseExecutor.restored_context);
}
