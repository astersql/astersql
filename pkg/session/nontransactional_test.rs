// Copyright 2026 AsterSQL.

use std::cmp::Ordering;

use crate::nontransactional::{
    ColumnInfo, Datum, DmlKind, DmlStatement, LogLevel, MetricKind, NonTransactionalDMLStmt,
    NonTransactionalError, NonTransactionalRuntime, Result, RuntimeRecordSet, SessionVars,
    buildShardJobs,
};

struct ScanRuntime {
    vars: SessionVars,
    scan_calls: usize,
}

impl ScanRuntime {
    fn new() -> Self {
        Self {
            vars: SessionVars {
                read_staleness: 0,
                bulk_dml_enabled: false,
                autocommit: true,
                in_transaction: false,
                global_batch_dml_enabled: false,
                dml_batch_size: 0,
                batch_delete: false,
                batch_insert: false,
                weak_read_consistency: false,
                snapshot_ts: 0,
                select_limit: 7,
                max_execution_time: 11,
                memory_quota_query: 0,
                ignore_error: false,
                redact_log: "OFF".to_owned(),
                current_db: "test".to_owned(),
                max_chunk_size: 32,
            },
            scan_calls: 0,
        }
    }
}

impl NonTransactionalRuntime for ScanRuntime {
    fn session_vars(&self) -> &SessionVars {
        &self.vars
    }

    fn session_vars_mut(&mut self) -> &mut SessionVars {
        &mut self.vars
    }

    fn preprocess(&mut self, _statement: &mut NonTransactionalDMLStmt) -> Result<()> {
        Ok(())
    }

    fn increment_metric(&mut self, _metric: MetricKind) {}

    fn attach_memory_tracker(&mut self, _quota: i64) -> Result<()> {
        Ok(())
    }

    fn consume_memory(&mut self, _bytes: i64) -> Result<()> {
        Ok(())
    }

    fn detach_memory_tracker(&mut self) -> Result<()> {
        Ok(())
    }

    fn scan_shard_values(&mut self, _sql: &str) -> Result<Vec<Datum>> {
        self.scan_calls += 1;
        assert_eq!(self.vars.select_limit, u64::MAX);
        assert_eq!(self.vars.max_execution_time, 0);
        Ok(Vec::new())
    }

    fn compare_shard_values(
        &mut self,
        left: &Datum,
        right: &Datum,
        _column: Option<&ColumnInfo>,
    ) -> Result<Ordering> {
        left.compare(right, false)
    }

    fn execute_dml(&mut self, _sql: &str) -> Result<Option<Box<dyn RuntimeRecordSet>>> {
        Ok(None)
    }

    fn is_cancelled(&self) -> bool {
        false
    }

    fn cancellation_error(&self) -> NonTransactionalError {
        NonTransactionalError::new("cancelled")
    }

    fn log(&mut self, _level: LogLevel, _message: &str) {}
}

fn statement_with_limit(limit: i64) -> NonTransactionalDMLStmt {
    NonTransactionalDMLStmt {
        dml_stmt: DmlStatement {
            kind: DmlKind::Delete,
            table_refs: None,
            where_condition: None,
            has_limit: false,
            has_order_by: false,
            sql_template: "DELETE FROM t WHERE {WHERE}".to_owned(),
        },
        shard_column: None,
        limit,
        dry_run: 0,
    }
}

#[test]
fn invalid_batch_size_is_checked_after_the_go_equivalent_scan() {
    let mut runtime = ScanRuntime::new();
    let error = buildShardJobs(
        &statement_with_limit(0),
        &mut runtime,
        "SELECT `_tidb_rowid` FROM `test`.`t`",
        None,
    )
    .expect_err("zero batch size must be rejected");

    assert_eq!(runtime.scan_calls, 1);
    assert_eq!(runtime.vars.select_limit, 7);
    assert_eq!(runtime.vars.max_execution_time, 11);
    assert_eq!(
        error.message,
        "Non-transactional DML, batch size should be positive"
    );
}
