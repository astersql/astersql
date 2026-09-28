// Copyright 2026 AsterSQL.

use std::sync::Arc;

use astersql_errors as errors;

use crate::check_table_index::{
    CheckResult, FastCheckRuntime, FastCheckSession, FastCheckTableExec, IndexInfo, QueryRow,
    RecordData, SessionVars, TableMeta, groupByChecksum,
};

#[derive(Default)]
struct SnapshotRuntime;

impl FastCheckRuntime for SnapshotRuntime {
    fn OpenBase(&self) -> CheckResult {
        Ok(())
    }

    fn SetInvisibleIndexes(&self, _enabled: bool) {}

    fn SnapshotTS(&self) -> u64 {
        42
    }

    fn UserSessionVars(&self) -> SessionVars {
        SessionVars::default()
    }

    fn BucketSize(&self) -> usize {
        2
    }

    fn AcquireSystemSession(&self) -> CheckResult<Box<dyn FastCheckSession>> {
        unreachable!("the focused test initializes its session directly")
    }

    fn ReleaseSystemSession(&self, _session: Box<dyn FastCheckSession>) {}

    fn ForceBucketedCheck(&self) -> bool {
        false
    }

    fn DecodeRecord(
        &self,
        _row: &QueryRow,
        _table: &TableMeta,
        _index: &IndexInfo,
        _primary_key_columns: usize,
    ) -> CheckResult<RecordData> {
        unreachable!("record decoding is outside this focused test")
    }

    fn ReportInconsistency(
        &self,
        _table: &TableMeta,
        _index: &IndexInfo,
        _handle: &[u8],
        _index_record: Option<&RecordData>,
        _table_record: Option<&RecordData>,
    ) -> CheckResult {
        unreachable!("reporting is outside this focused test")
    }

    fn LogBucketDifference(
        &self,
        _table: &TableMeta,
        _index: &IndexInfo,
        _table_checksum: Option<&groupByChecksum>,
        _index_checksum: Option<&groupByChecksum>,
    ) {
    }
}

#[derive(Default)]
struct FailingSnapshotSession {
    variables: SessionVars,
    executed: Vec<String>,
}

impl FastCheckSession for FailingSnapshotSession {
    fn Variables(&self) -> SessionVars {
        self.variables.clone()
    }

    fn SetVariables(&mut self, variables: &SessionVars) {
        self.variables = variables.clone();
    }

    fn Execute(&mut self, sql: &str) -> CheckResult {
        self.executed.push(sql.to_owned());
        Err(errors::New("snapshot setup failed"))
    }

    fn Query(&mut self, _sql: &str, _row_limit: usize) -> CheckResult<Vec<QueryRow>> {
        unreachable!("querying is outside this focused test")
    }
}

#[test]
fn snapshot_setup_failure_is_best_effort_like_go() {
    let executor = FastCheckTableExec::new(
        Arc::new(SnapshotRuntime),
        "test".to_owned(),
        TableMeta::default(),
        Vec::new(),
    );
    let worker = executor.createWorker();
    let mut session = FailingSnapshotSession::default();

    let result = worker.initSessCtx(&mut session);

    assert!(
        result.is_ok(),
        "Go logs snapshot setup errors and continues"
    );
    assert_eq!(session.executed, ["set session tidb_snapshot = 42"]);
}
