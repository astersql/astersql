// Copyright 2026 AsterSQL.

use crate::analyze_col::{
    AnalyzeColumnResult, AnalyzeColumnsExec, analyzeColumnRuntime, analyzeContext, analyzeInfo,
    analyzeJob, analyzeRequest, analyzeRequestSpec, analyzeTransportSpec, baseAnalyzeExec,
    columnInfo, keyRange, memoryTracker, notifyErrorWaitGroupWrapper, selectResult, tableInfo,
    virtualColumnSchema, waitGroupWrapper,
};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct TestMemoryTracker;

impl memoryTracker for TestMemoryTracker {
    fn attach_to_statement(&self) -> AnalyzeColumnResult {
        Ok(())
    }

    fn detach(&self) {}
}

#[derive(Default)]
struct TestSelectResult;

impl selectResult for TestSelectResult {
    fn close(&mut self) -> AnalyzeColumnResult {
        Ok(())
    }
}

#[derive(Default)]
struct RecordingAnalyzeRuntime {
    requests: Mutex<Vec<analyzeRequestSpec>>,
}

impl analyzeColumnRuntime for RecordingAnalyzeRuntime {
    fn new_memory_tracker(
        &self,
        _plan_id: i64,
        _byte_limit: i64,
    ) -> AnalyzeColumnResult<Arc<dyn memoryTracker>> {
        Ok(Arc::new(TestMemoryTracker))
    }

    fn split_ranges_across_int64_boundary(
        &self,
        ranges: Vec<keyRange>,
        _keep_order: bool,
        _descending: bool,
        _split_common_handle: bool,
    ) -> (Vec<keyRange>, Vec<keyRange>) {
        (ranges, Vec::new())
    }

    fn build_request(
        &self,
        spec: analyzeRequestSpec,
        _memory_tracker: &dyn memoryTracker,
    ) -> AnalyzeColumnResult<Vec<u8>> {
        self.requests.lock().unwrap().push(spec);
        Ok(Vec::new())
    }

    fn analyze(
        &self,
        _context: &analyzeContext,
        _request: Vec<u8>,
        _transport: analyzeTransportSpec,
    ) -> AnalyzeColumnResult<Box<dyn selectResult>> {
        Ok(Box::new(TestSelectResult))
    }
}

fn column(name: &str, changing: bool, removing: bool) -> columnInfo {
    columnInfo {
        name: name.to_owned(),
        changing,
        removing,
        ..columnInfo::default()
    }
}

#[test]
fn non_temporary_column_count_matches_go_modify_column_semantics() {
    let table = tableInfo {
        columns: vec![
            column("a", false, false),
            column("_Col$_a_0", true, false),
            column("_Del$_b", false, true),
            column("c", false, false),
        ],
        ..tableInfo::default()
    };

    assert_eq!(table.nonTemporaryColumnCount(), 2);
}

#[test]
fn analyze_request_is_unordered_for_store_batching() {
    let runtime = Arc::new(RecordingAnalyzeRuntime::default());
    let mut executor = AnalyzeColumnsExec {
        baseAnalyzeExec: baseAnalyzeExec {
            tableID: 42,
            concurrency: 6,
            analyzeStoreBatchSize: 4,
            analyzeRequest: analyzeRequest::default(),
            options: BTreeMap::new(),
            job: Some(analyzeJob::default()),
            snapshot: 0,
            planID: 7,
            enableAnalyzeSnapshot: false,
            resourceGroupTagger: Vec::new(),
            resourceGroupName: String::new(),
            explicitRequestSourceType: String::new(),
            restrictedSQL: true,
            clientID: "test".into(),
            kvVariables: BTreeMap::new(),
            distSQLContextID: 9,
            runtime: runtime.clone(),
        },
        tableInfo: tableInfo::default(),
        colsInfo: Vec::new(),
        handleCols: None,
        commonHandle: None,
        resultHandler: None,
        indexes: Vec::new(),
        analyzeInfo: analyzeInfo::default(),
        samplingBuilderWg: notifyErrorWaitGroupWrapper::default(),
        samplingMergeWg: waitGroupWrapper::default(),
        schemaForVirtualColEval: virtualColumnSchema::default(),
        baseCount: 0,
        baseModifyCnt: 0,
        samplingStatsConcurrency: 0,
        memTracker: None,
    };

    executor
        .open(
            &analyzeContext { requestID: 1 },
            vec![keyRange {
                low: vec![1],
                high: vec![2],
            }],
        )
        .unwrap();

    let requests = runtime.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert!(!requests[0].keepOrder);
    assert_eq!(requests[0].concurrency, 6);
    assert_eq!(requests[0].storeBatchSize, 4);
    assert!(requests[0].allowBatchTaskDataMerge);
    assert!(requests[0].executeBatchTasksSerially);

    drop(requests);
    executor.baseAnalyzeExec.analyzeStoreBatchSize = 0;
    executor
        .buildResp(&analyzeContext { requestID: 2 }, Vec::new())
        .unwrap();
    let requests = runtime.requests.lock().unwrap();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[1].storeBatchSize, 0);
    assert!(!requests[1].allowBatchTaskDataMerge);
    assert!(!requests[1].executeBatchTasksSerially);
}
