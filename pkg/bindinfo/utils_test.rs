// Copyright 2026 AsterSQL.

use crate::*;
use std::sync::Mutex;

struct FailingBeginContext;

impl BindingValidator for FailingBeginContext {
    fn validate_binding_sql(&self, _sql: &str) -> Result<()> {
        Ok(())
    }
}

impl BindingSqlContext for FailingBeginContext {
    fn execute(&self, sql: &str, _args: &[SqlValue]) -> Result<Vec<BindingRow>> {
        assert_eq!(sql, "BEGIN PESSIMISTIC");
        Err(BindError("begin failed".to_owned()))
    }

    fn plan_digest(&self, _schema: &str, _binding_sql: &str) -> Result<String> {
        Ok(String::new())
    }
}

#[derive(Default)]
struct RecordingPool {
    released: Mutex<usize>,
    destroyed: Mutex<usize>,
}

impl DestroyableSessionPool for RecordingPool {
    fn acquire(&self) -> Result<Box<dyn BindingSqlContext>> {
        Ok(Box::new(FailingBeginContext))
    }

    fn release(&self, _session: Box<dyn BindingSqlContext>) {
        *self.released.lock().unwrap() += 1;
    }

    fn destroy(&self, _session: Box<dyn BindingSqlContext>) {
        *self.destroyed.lock().unwrap() += 1;
    }
}

#[test]
fn call_with_sctx_destroys_session_when_begin_fails() {
    let pool = RecordingPool::default();
    let error = callWithSCtx(&pool, true, |_| Ok(())).unwrap_err();
    assert_eq!(error.0, "begin failed");
    assert_eq!(*pool.released.lock().unwrap(), 0);
    assert_eq!(*pool.destroyed.lock().unwrap(), 1);
}

#[test]
fn should_update_uses_elapsed_time_since_last_save() {
    let now = BindingTime::now();
    let interval = i64::try_from(MaxWriteInterval.as_micros()).unwrap();
    let last_saved = BindingTime(now.0 - interval);
    let last_used = BindingTime(last_saved.0 + 1);
    assert!(shouldUpdateBinding(Some(last_saved), Some(last_used)));
}

#[test]
fn new_binding_from_storage_normalizes_go_compatibility_fields() {
    let binding = newBindingFromStorage(BindingRow {
        DefaultDB: "Mixed_CASE_DB".to_owned(),
        Status: StatusUsing.to_owned(),
        ..BindingRow::default()
    });
    assert_eq!(binding.Db, "mixed_case_db");
    assert_eq!(binding.Status, StatusEnabled);
}

#[test]
fn generate_binding_sql_matches_go_statement_specific_injection() {
    let update = Statement {
        SQL: "explain update t set a = 1".to_owned(),
        ..Statement::default()
    };
    let update_sql = GenerateBindingSQL(&update, "use_index(t, idx)", "app");
    assert!(
        update_sql.starts_with("UPDATE /*+ use_index(t, idx) */"),
        "{update_sql}"
    );

    let insert = Statement {
        SQL: "insert into dst select * from src".to_owned(),
        ..Statement::default()
    };
    let insert_sql = GenerateBindingSQL(&insert, "use_index(src, idx)", "app");
    assert!(insert_sql.starts_with("INSERT INTO"));
    assert!(insert_sql.contains("SELECT /*+ use_index(src, idx) */"));

    let cte = Statement {
        SQL: "with cte as (select * from src) select * from cte".to_owned(),
        ..Statement::default()
    };
    let cte_sql = GenerateBindingSQL(&cte, "hash_join(cte)", "app");
    assert!(cte_sql.starts_with("WITH `cte` AS"));
    assert!(cte_sql.contains(") SELECT /*+ hash_join(cte) */"));
}
