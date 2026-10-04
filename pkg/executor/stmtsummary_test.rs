// Copyright 2026 AsterSQL.

// 语句摘要（statement summary）辅助逻辑的单元测试。

use std::sync::{Arc, Mutex};

use crate::stmtsummary::{
    CLUSTER_TABLE_STATEMENTS_SUMMARY, CoarseTimeRange, RetrieverMode, RowsPuller,
    StatementSummaryRuntime, StatementsSummaryExtractor, StmtSummaryRetriever,
    TABLE_STATEMENTS_SUMMARY, TABLE_STATEMENTS_SUMMARY_EVICTED, TABLE_STATEMENTS_SUMMARY_HISTORY,
    buildStmtSummaryRetriever, buildTimeRanges, isClusterTable, isCurrentTable, isEvictedTable,
    isHistoryTable, newRowsReader, newSimpleRowsReader,
};

#[derive(Default)]
struct PullState {
    batches: Vec<Vec<i32>>,
    next: usize,
    closes: usize,
}

struct TestPuller(Arc<Mutex<PullState>>);

impl RowsPuller<i32, String> for TestPuller {
    fn rows(&mut self) -> Result<Vec<i32>, String> {
        let mut state = self.0.lock().unwrap();
        let rows = state.batches.get(state.next).cloned().unwrap_or_default();
        state.next += 1;
        Ok(rows)
    }

    fn close(&mut self) -> Result<(), String> {
        self.0.lock().unwrap().closes += 1;
        Ok(())
    }
}

struct TestRuntime {
    persistent: bool,
    process_privilege: bool,
    memory_rows: Vec<i32>,
    memory_reads: usize,
    evicted_row: Option<i32>,
    pull_state: Arc<Mutex<PullState>>,
}

impl TestRuntime {
    fn persistent(memory_rows: Vec<i32>, batches: Vec<Vec<i32>>) -> Self {
        Self {
            persistent: true,
            process_privilege: true,
            memory_rows,
            memory_reads: 0,
            evicted_row: None,
            pull_state: Arc::new(Mutex::new(PullState {
                batches,
                ..PullState::default()
            })),
        }
    }
}

impl StatementSummaryRuntime for TestRuntime {
    type Context = ();
    type Row = i32;
    type Column = usize;
    type Digests = Vec<String>;
    type Error = String;

    fn persistent_enabled(&self) -> bool {
        self.persistent
    }
    fn digests_empty(&self, digests: &Self::Digests) -> bool {
        digests.is_empty()
    }
    fn has_process_privilege(&self, _context: &Self::Context) -> bool {
        self.process_privilege
    }
    fn process_privilege_denied(&self) -> Self::Error {
        "PROCESS denied".to_owned()
    }
    fn statement_summary_error(&self, error: astersql_errors::SharedError) -> Self::Error {
        assert!(astersql_util_dbterror_plannererrors::ErrNotSupportedYet.Equal(Some(&error)));
        error.to_string()
    }
    fn instance_address(&self, _context: &Self::Context) -> Result<String, Self::Error> {
        Ok("127.0.0.1:4000".to_owned())
    }
    fn append_host_info(
        &mut self,
        _context: &mut Self::Context,
        rows: Vec<Self::Row>,
    ) -> Result<Vec<Self::Row>, Self::Error> {
        Ok(rows.into_iter().map(|row| row + 1000).collect())
    }
    fn adjust_columns(
        &self,
        rows: Vec<Self::Row>,
        _columns: &[Self::Column],
        _table_name: &str,
    ) -> Vec<Self::Row> {
        rows
    }
    fn legacy_evicted_rows(&self) -> Vec<Self::Row> {
        self.evicted_row.into_iter().collect()
    }
    fn legacy_summary_rows(
        &mut self,
        _context: &mut Self::Context,
        _table_name: &str,
        _columns: &[Self::Column],
        _digests: Option<&Self::Digests>,
        _instance_address: String,
        _process_privilege: bool,
    ) -> Result<Vec<Self::Row>, Self::Error> {
        Ok(self.memory_rows.clone())
    }
    fn persistent_evicted_row(&self) -> Option<Self::Row> {
        self.evicted_row
    }
    fn persistent_memory_rows(
        &mut self,
        _context: &mut Self::Context,
        _columns: &[Self::Column],
        _digests: Option<&Self::Digests>,
        _time_ranges: Option<&[crate::stmtsummary::StmtTimeRange]>,
        _instance_address: String,
        _process_privilege: bool,
    ) -> Result<Vec<Self::Row>, Self::Error> {
        self.memory_reads += 1;
        Ok(self.memory_rows.clone())
    }
    fn persistent_history_puller(
        &mut self,
        _context: &mut Self::Context,
        _columns: &[Self::Column],
        _digests: Option<&Self::Digests>,
        _time_ranges: Option<&[crate::stmtsummary::StmtTimeRange]>,
        _instance_address: String,
        _process_privilege: bool,
    ) -> Result<Box<dyn RowsPuller<Self::Row, Self::Error>>, Self::Error> {
        Ok(Box::new(TestPuller(self.pull_state.clone())))
    }
}

fn retriever(runtime: TestRuntime, table: &str) -> StmtSummaryRetriever<TestRuntime> {
    buildStmtSummaryRetriever(runtime, table.to_owned(), vec![], None)
}

#[test]
fn statement_summary_reader_pages_rows_and_classifies_tables() {
    let mut reader = newSimpleRowsReader::<_, String>((1..=7).collect());
    assert_eq!(reader.read(3).unwrap(), vec![1, 2, 3]);
    assert_eq!(reader.read(3).unwrap(), vec![4, 5, 6]);
    assert_eq!(reader.read(3).unwrap(), vec![7]);
    assert!(reader.read(3).unwrap().is_empty());
    assert!(isCurrentTable(TABLE_STATEMENTS_SUMMARY));
    assert!(isHistoryTable(TABLE_STATEMENTS_SUMMARY_HISTORY));
    assert!(isEvictedTable(TABLE_STATEMENTS_SUMMARY_EVICTED));
    assert!(isClusterTable(CLUSTER_TABLE_STATEMENTS_SUMMARY));
    assert_eq!(
        buildTimeRanges(Some(CoarseTimeRange {
            start_unix: 10,
            end_unix: 20
        }))
        .unwrap()[0],
        crate::stmtsummary::StmtTimeRange { begin: 10, end: 20 }
    );
}

#[test]
fn rows_reader_combines_memory_and_history_and_closes_at_eof() {
    let state = Arc::new(Mutex::new(PullState {
        batches: vec![vec![3, 4], vec![]],
        ..PullState::default()
    }));
    let mut reader = newRowsReader(vec![1, 2], Box::new(TestPuller(state.clone())));
    assert_eq!(reader.read(1024).unwrap(), vec![1, 2]);
    assert_eq!(reader.read(1024).unwrap(), vec![3, 4]);
    assert!(reader.read(1024).unwrap().is_empty());
    assert_eq!(state.lock().unwrap().closes, 1);
    assert!(reader.puller.is_none());
}

#[test]
fn persistent_retriever_matches_current_evicted_and_history_paths() {
    let mut current = retriever(
        TestRuntime::persistent(vec![1, 2, 3], vec![]),
        TABLE_STATEMENTS_SUMMARY,
    );
    assert_eq!(current.retrieve(&mut ()).unwrap(), vec![1, 2, 3]);
    assert!(current.retrieve(&mut ()).unwrap().is_empty());

    let mut evicted_runtime = TestRuntime::persistent(vec![], vec![]);
    evicted_runtime.evicted_row = Some(2);
    let mut evicted = retriever(evicted_runtime, TABLE_STATEMENTS_SUMMARY_EVICTED);
    assert_eq!(evicted.retrieve(&mut ()).unwrap(), vec![2]);

    let history_runtime = TestRuntime::persistent(vec![1, 2, 3], vec![vec![4, 5, 6, 7], vec![]]);
    let pull_state = history_runtime.pull_state.clone();
    let mut history = retriever(history_runtime, TABLE_STATEMENTS_SUMMARY_HISTORY);
    let mut results = Vec::new();
    loop {
        let rows = history.retrieve(&mut ()).unwrap();
        if rows.is_empty() {
            break;
        }
        results.extend(rows);
    }
    assert_eq!(results, vec![1, 2, 3, 4, 5, 6, 7]);
    assert_eq!(pull_state.lock().unwrap().closes, 1);
}

#[test]
fn builder_and_evicted_privilege_match_go_contract() {
    let dummy = buildStmtSummaryRetriever(
        TestRuntime::persistent(vec![1], vec![]),
        TABLE_STATEMENTS_SUMMARY.to_owned(),
        vec![],
        Some(StatementsSummaryExtractor {
            digests: Some(Vec::new()),
            coarse_time_range: None,
            skip_request: true,
        }),
    );
    assert_eq!(dummy.mode, RetrieverMode::Dummy);
    assert!(dummy.digests.is_none());

    let mut denied_runtime = TestRuntime::persistent(vec![], vec![]);
    denied_runtime.process_privilege = false;
    denied_runtime.evicted_row = Some(1);
    let mut denied = retriever(denied_runtime, TABLE_STATEMENTS_SUMMARY_EVICTED);
    assert_eq!(denied.retrieve(&mut ()).unwrap_err(), "PROCESS denied");

    let mut cluster_runtime = TestRuntime::persistent(vec![], vec![]);
    cluster_runtime.evicted_row = Some(1);
    let mut cluster = retriever(
        cluster_runtime,
        crate::stmtsummary::CLUSTER_TABLE_STATEMENTS_SUMMARY_EVICTED,
    );
    assert_eq!(cluster.retrieve(&mut ()).unwrap(), vec![1001]);
}

#[test]
fn persistent_cumulative_tables_return_unsupported_without_initializing_reader() {
    for table in [
        crate::stmtsummary::TABLE_TIDB_STATEMENTS_STATS,
        crate::stmtsummary::CLUSTER_TABLE_TIDB_STATEMENTS_STATS,
    ] {
        let mut reader = retriever(TestRuntime::persistent(vec![1], vec![]), table);
        reader.close().unwrap();
        for _ in 0..2 {
            assert_eq!(
                reader.retrieve(&mut ()).unwrap_err(),
                "[planner:1235]This version of TiDB doesn't yet support 'cumulative statement summary table with persistent mode (v2)'"
            );
            assert!(reader.rows_reader.is_none());
            assert_eq!(reader.runtime.memory_reads, 0);
        }
        reader.close().unwrap();
        let mut runtime = TestRuntime::persistent(vec![1], vec![]);
        runtime.persistent = false;
        assert_eq!(
            retriever(runtime, table).retrieve(&mut ()).unwrap(),
            vec![1]
        );
    }
}
