// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// DistSQL 核心行为的单元测试。
//
// 覆盖请求类型标签保留，以及有序扫描时 `SelectResult` 按响应批次展开行的顺序语义。

use super::*;
use crate::distsql as production;
use crate::request_builder as request_production;
use crate::select_result::{GetSelectResultConcurrency, SelectResult as _};
use astersql_util_execdetails::execdetails;
use std::sync::{Arc, Mutex};
use std::time::Duration;

static ANALYZE_CONFIG_LOCK: Mutex<()> = Mutex::new(());
struct CollectConfigGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
    prior: bool,
}
impl CollectConfigGuard {
    fn set(enabled: bool) -> Self {
        let lock = ANALYZE_CONFIG_LOCK.lock().unwrap();
        let flag = &astersql_config::get_global_config()
            .instance
            .enable_collect_execution_info;
        let prior = flag.load();
        flag.store(enabled);
        Self { _lock: lock, prior }
    }
}
impl Drop for CollectConfigGuard {
    fn drop(&mut self) {
        astersql_config::get_global_config()
            .instance
            .enable_collect_execution_info
            .store(self.prior);
    }
}

/// DAG / Analyze / Checksum 三类请求在 Build 后应保留各自的 `RequestType`。
#[test]
fn checksum_and_dag_requests_keep_their_type() {
    for request_type in [
        RequestType::Dag,
        RequestType::Analyze,
        RequestType::Checksum,
    ] {
        let request = RequestBuilder::new(request_type)
            .key_ranges(vec![KeyRange::new(b"table/1", b"table/2").unwrap()])
            .build()
            .unwrap();
        assert_eq!(request.request_type, request_type);
    }
}

/// 有序扫描：多批次响应应按到达顺序拼接行，不打乱行序。
#[test]
fn ordered_scan_keeps_response_row_order() {
    // 两个响应批次分别含 [a,b] 与 [c]，合并后应为 a→b→c。
    let source = VecResponseSource::new(vec![
        Ok(SelectResponse {
            rows: vec![vec!["a".into()], vec!["b".into()]],
            ..Default::default()
        }),
        Ok(SelectResponse {
            rows: vec![vec!["c".into()]],
            ..Default::default()
        }),
    ]);
    let mut result = SelectResult::new(source);
    let mut rows = Vec::new();
    while let Some(row) = result.next_row().unwrap() {
        rows.push(row);
    }
    assert_eq!(rows, vec![vec!["a"], vec!["b"], vec!["c"]]);
}

struct TestClient {
    responses: Vec<DistSqlResult<SelectResponse>>,
}

impl production::KvClient for TestClient {
    fn send(
        &self,
        _request: &request_production::KvRequest,
    ) -> DistSqlResult<Box<dyn ResponseSource>> {
        Ok(Box::new(VecResponseSource::new(self.responses.clone())))
    }
}

struct CapturingClient {
    request: Mutex<Option<request_production::KvRequest>>,
}

impl production::KvClient for CapturingClient {
    fn send(
        &self,
        request: &request_production::KvRequest,
    ) -> DistSqlResult<Box<dyn ResponseSource>> {
        *self.request.lock().unwrap() = Some(request.clone());
        Ok(Box::new(VecResponseSource::new(vec![])))
    }
}

fn production_request(request_type: RequestType) -> request_production::KvRequest {
    let mut builder = request_production::RequestBuilder::new();
    match request_type {
        RequestType::Dag => {
            builder.SetDAGRequest(vec![1]);
        }
        RequestType::Analyze => {
            builder.SetAnalyzeRequest(vec![1], request_production::IsolationLevel::Snapshot);
        }
        RequestType::Checksum => {
            builder.SetChecksumRequest(vec![1]);
        }
    }
    builder
        .SetKeyRanges(vec![KeyRange::new(b"a", b"b").unwrap()])
        .Build()
        .unwrap()
}

fn test_context(client: Arc<dyn production::KvClient>) -> production::DistSQLContext {
    production::DistSQLContext {
        client,
        query_cop_store_limiter: None,
        exec_details: None,
        runtime_stats: None,
        in_restricted_sql: false,
        enable_chunk_rpc: false,
        streaming: false,
        concurrency: 1,
        tiflash_max_threads: -1,
        tiflash_max_bytes_before_external_join: -1,
        tiflash_max_bytes_before_external_group_by: -1,
        tiflash_max_bytes_before_external_sort: -1,
        tiflash_max_query_memory_per_node: 0,
        tiflash_query_spill_ratio: 0.7,
        tiflash_use_hash_join_v2: false,
    }
}

#[test]
fn go_merge_42_select_keeps_explicit_and_query_limiters_independent() {
    let client = Arc::new(CapturingClient {
        request: Mutex::new(None),
    });
    let query_limiter = astersql_kv::NewQueryCopStoreLimiter(3).unwrap();
    let explicit_limiter = astersql_kv::NewCoprRequestLimiter(7).unwrap();
    let context = production::DistSQLContext {
        client: client.clone(),
        query_cop_store_limiter: Some(query_limiter.clone()),
        exec_details: None,
        runtime_stats: None,
        in_restricted_sql: false,
        enable_chunk_rpc: false,
        streaming: false,
        concurrency: 1,
        tiflash_max_threads: -1,
        tiflash_max_bytes_before_external_join: -1,
        tiflash_max_bytes_before_external_group_by: -1,
        tiflash_max_bytes_before_external_sort: -1,
        tiflash_max_query_memory_per_node: 0,
        tiflash_query_spill_ratio: 0.7,
        tiflash_use_hash_join_v2: false,
    };
    for store in [StoreType::TiKv, StoreType::TiFlash] {
        let mut request = production_request(RequestType::Dag);
        request.store_type = store;
        request.copr_request_limiter = Some(explicit_limiter.clone());
        let _result = production::Select(&context, &request, &[]).unwrap();
        let sent = client.request.lock().unwrap();
        let sent = sent.as_ref().unwrap();
        assert!(Arc::ptr_eq(
            sent.copr_request_limiter.as_ref().unwrap(),
            &explicit_limiter
        ));
        assert!(Arc::ptr_eq(
            sent.query_cop_store_limiter.as_ref().unwrap(),
            &query_limiter
        ));
        assert!(request.query_cop_store_limiter.is_none());
    }
    let no_query_limit = production::DistSQLContext {
        query_cop_store_limiter: None,
        ..context
    };
    let mut request = production_request(RequestType::Dag);
    request.copr_request_limiter = Some(explicit_limiter.clone());
    let _result = production::Select(&no_query_limit, &request, &[]).unwrap();
    let sent = client.request.lock().unwrap();
    let sent = sent.as_ref().unwrap();
    assert!(sent.query_cop_store_limiter.is_none());
    assert!(Arc::ptr_eq(
        sent.copr_request_limiter.as_ref().unwrap(),
        &explicit_limiter
    ));
}

struct AnalyzeSource {
    subset: Option<SelectResponse>,
    error: Option<DistSqlError>,
    unconsumed: Vec<CopRuntimeEvidence>,
    closed: Arc<std::sync::atomic::AtomicUsize>,
}
impl ResponseSource for AnalyzeSource {
    fn next_response(&mut self) -> DistSqlResult<Option<SelectResponse>> {
        if let Some(error) = self.error.take() {
            return Err(error);
        }
        Ok(self.subset.take())
    }
    fn next_response_with_error(&mut self) -> (Option<SelectResponse>, Option<DistSqlError>) {
        (self.subset.take(), self.error.take())
    }
    fn collect_unconsumed_cop_stats(&mut self) -> Vec<CopRuntimeEvidence> {
        std::mem::take(&mut self.unconsumed)
    }
    fn close(&mut self) -> DistSqlResult<()> {
        self.closed
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}
struct AnalyzeClient {
    source: Mutex<Option<AnalyzeSource>>,
    request: Mutex<Option<request_production::KvRequest>>,
    options: Mutex<Option<production::ClientSendOptions>>,
}
impl production::KvClient for AnalyzeClient {
    fn send(
        &self,
        request: &request_production::KvRequest,
    ) -> DistSqlResult<Box<dyn ResponseSource>> {
        *self.request.lock().unwrap() = Some(request.clone());
        Ok(Box::new(self.source.lock().unwrap().take().unwrap()))
    }
    fn send_with_options(
        &self,
        request: &request_production::KvRequest,
        options: &production::ClientSendOptions,
    ) -> DistSqlResult<Box<dyn ResponseSource>> {
        *self.options.lock().unwrap() = Some(*options);
        self.send(request)
    }
}
fn analyze_evidence(
    keys: i64,
    total: i64,
    bytes: i64,
    response_time: Duration,
) -> CopRuntimeEvidence {
    CopRuntimeEvidence {
        details: execdetails::CopExecDetails {
            ScanDetail: Some(execdetails::util::ScanDetail {
                ProcessedKeys: keys,
                TotalKeys: total,
                ProcessedKeysSize: bytes,
                ..Default::default()
            }),
            TimeDetail: execdetails::util::TimeDetail {
                ProcessTime: Duration::from_millis(3),
                WaitTime: Duration::from_millis(5),
            },
            ..Default::default()
        },
        response_time,
        ..Default::default()
    }
}
#[test]
fn go_merge_42_analyze_records_raw_details_once_and_estimates_each_request() {
    let _config = CollectConfigGuard::set(true);
    let details = Arc::new(execdetails::SyncExecDetails::default());
    let coll = Arc::new(Mutex::new(execdetails::RuntimeStatsColl::default()));
    let mut context = test_context(Arc::new(TestClient { responses: vec![] }));
    context.exec_details = Some(details.clone());
    context.runtime_stats = Some(coll.clone());
    let closed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let client = AnalyzeClient {
        source: Mutex::new(Some(AnalyzeSource {
            subset: Some(SelectResponse {
                raw_data: Some(b"analyze payload!".to_vec()),
                cop_stats: Some(analyze_evidence(13, 17, 19, Duration::from_millis(7))),
                ..Default::default()
            }),
            error: None,
            unconsumed: Vec::new(),
            closed: closed.clone(),
        })),
        request: Mutex::new(None),
        options: Mutex::new(None),
    };
    let request = production_request(RequestType::Analyze);
    let mut result = production::Analyze(&client, &request, true, &context, 42).unwrap();
    assert!(
        client
            .options
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .enable_collect_execution_info
    );
    assert_eq!(
        result.NextRaw().unwrap(),
        Some(b"analyze payload!".to_vec())
    );
    result.Close().unwrap();
    result.Close().unwrap();
    assert_eq!(closed.load(std::sync::atomic::Ordering::SeqCst), 1);
    let summary = details.GetExecDetails();
    assert_eq!(summary.RequestCount, 1);
    assert_eq!(summary.CopTime, Duration::ZERO);
    assert_eq!(
        summary
            .CopExecDetails
            .ScanDetail
            .as_ref()
            .unwrap()
            .ProcessedKeysSize,
        19
    );
    let coll = coll.lock().unwrap();
    assert!(coll.GetCopStats(42).is_some());
    assert!((coll.GetAnalyzeScanBytes(42).unwrap() - 19.0 / 13.0 * 17.0).abs() < 1e-9);
}

#[test]
fn go_merge_42_analyze_preserves_subset_stats_on_error_and_close() {
    let _config = CollectConfigGuard::set(true);
    let details = Arc::new(execdetails::SyncExecDetails::default());
    let coll = Arc::new(Mutex::new(execdetails::RuntimeStatsColl::default()));
    let mut context = test_context(Arc::new(TestClient { responses: vec![] }));
    context.exec_details = Some(details.clone());
    context.runtime_stats = Some(coll.clone());
    let client = AnalyzeClient {
        source: Mutex::new(Some(AnalyzeSource {
            subset: Some(SelectResponse {
                cop_stats: Some(analyze_evidence(2, 4, 6, Duration::from_millis(11))),
                ..Default::default()
            }),
            error: Some(DistSqlError("response error".into())),
            unconsumed: vec![analyze_evidence(1, 2, 3, Duration::ZERO)],
            closed: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        })),
        request: Mutex::new(None),
        options: Mutex::new(None),
    };
    let mut result = production::Analyze(
        &client,
        &production_request(RequestType::Analyze),
        true,
        &context,
        42,
    )
    .unwrap();
    assert_eq!(result.NextRaw().unwrap_err().0, "response error");
    assert_eq!(result.NextRaw().unwrap(), None);
    result.Close().unwrap();
    assert_eq!(details.GetExecDetails().RequestCount, 2);
    assert!((coll.lock().unwrap().GetAnalyzeScanBytes(42).unwrap() - 18.0).abs() < 1e-9);
}

#[test]
fn go_merge_42_analyze_respects_disabled_collection() {
    let _config = CollectConfigGuard::set(false);
    let details = Arc::new(execdetails::SyncExecDetails::default());
    let coll = Arc::new(Mutex::new(execdetails::RuntimeStatsColl::default()));
    let mut context = test_context(Arc::new(TestClient { responses: vec![] }));
    context.exec_details = Some(details.clone());
    context.runtime_stats = Some(coll.clone());
    let client = AnalyzeClient {
        source: Mutex::new(Some(AnalyzeSource {
            subset: Some(SelectResponse {
                raw_data: Some(b"payload".to_vec()),
                cop_stats: Some(analyze_evidence(1, 3, 2, Duration::from_millis(1))),
                ..Default::default()
            }),
            error: None,
            unconsumed: Vec::new(),
            closed: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        })),
        request: Mutex::new(None),
        options: Mutex::new(None),
    };
    let mut result = production::Analyze(
        &client,
        &production_request(RequestType::Analyze),
        true,
        &context,
        42,
    )
    .unwrap();
    assert!(
        !client
            .options
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .enable_collect_execution_info
    );
    assert_eq!(result.NextRaw().unwrap(), Some(b"payload".to_vec()));
    result.Close().unwrap();
    assert_eq!(details.GetExecDetails().RequestCount, 0);
    let coll = coll.lock().unwrap();
    assert!(coll.GetCopStats(42).is_none());
    assert!(coll.GetAnalyzeScanBytes(42).is_none());
    assert!(!coll.ExistsRootStats(42));
}

#[test]
fn go_merge_42_mpp_uses_current_direct_reporting_route() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let coll = Arc::new(Mutex::new(execdetails::RuntimeStatsColl::default()));
    let mut context = test_context(Arc::new(TestClient { responses: vec![] }));
    context.runtime_stats = Some(coll.clone());
    let directly = Arc::new(AtomicBool::new(true));
    let reports_directly: Arc<dyn Fn() -> bool + Send + Sync> = {
        let directly = directly.clone();
        Arc::new(move || directly.load(Ordering::SeqCst))
    };
    let response = |rows| SelectResponse {
        raw_data: Some(b"raw".to_vec()),
        execution_summaries: vec![Some(execdetails::tipb::ExecutorExecutionSummary {
            ExecutorId: "TableScan_10".into(),
            NumProducedRows: Some(rows),
            NumIterations: Some(1),
            TimeProcessedNs: Some(1),
            ..Default::default()
        })],
        ..Default::default()
    };
    let mut result = production::GenSelectResultFromMPPResponse(
        &context,
        Box::new(VecResponseSource::new(vec![
            Ok(response(3)),
            Ok(response(5)),
        ])),
        &[],
        &[10],
        10,
        reports_directly,
    );
    assert_eq!(result.NextRaw().unwrap(), Some(b"raw".to_vec()));
    assert!(!coll.lock().unwrap().GetTiFlashExecutionUnits(10).1);
    directly.store(false, Ordering::SeqCst);
    assert_eq!(result.NextRaw().unwrap(), Some(b"raw".to_vec()));
    assert_eq!(coll.lock().unwrap().GetTiFlashExecutionUnits(10).0.Rows, 5);
}

#[test]
fn go_merge_42_analyze_sums_estimates_before_flattening_requests() {
    let _config = CollectConfigGuard::set(true);
    let coll = Arc::new(Mutex::new(execdetails::RuntimeStatsColl::default()));
    let mut context = test_context(Arc::new(TestClient { responses: vec![] }));
    context.runtime_stats = Some(coll.clone());
    for (keys, total, bytes) in [(1, 10, 100), (9, 9, 9)] {
        let client = AnalyzeClient {
            source: Mutex::new(Some(AnalyzeSource {
                subset: Some(SelectResponse {
                    raw_data: Some(b"payload".to_vec()),
                    cop_stats: Some(analyze_evidence(keys, total, bytes, Duration::ZERO)),
                    ..Default::default()
                }),
                error: None,
                unconsumed: Vec::new(),
                closed: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            })),
            request: Mutex::new(None),
            options: Mutex::new(None),
        };
        let mut result = production::Analyze(
            &client,
            &production_request(RequestType::Analyze),
            true,
            &context,
            42,
        )
        .unwrap();
        result.NextRaw().unwrap();
        result.Close().unwrap();
    }
    assert_eq!(coll.lock().unwrap().GetAnalyzeScanBytes(42), Some(1009.0));
}

#[test]
fn select_sends_request_and_returns_all_rows() {
    let client = TestClient {
        responses: vec![Ok(SelectResponse {
            rows: vec![vec!["a".into()], vec!["b".into()]],
            ..Default::default()
        })],
    };
    let context = production::DistSQLContext {
        client: Arc::new(client),
        query_cop_store_limiter: None,
        exec_details: None,
        runtime_stats: None,
        in_restricted_sql: false,
        enable_chunk_rpc: false,
        streaming: false,
        concurrency: 3,
        tiflash_max_threads: -1,
        tiflash_max_bytes_before_external_join: -1,
        tiflash_max_bytes_before_external_group_by: -1,
        tiflash_max_bytes_before_external_sort: -1,
        tiflash_max_query_memory_per_node: 0,
        tiflash_query_spill_ratio: 0.7,
        tiflash_use_hash_join_v2: false,
    };
    let mut result =
        production::Select(&context, &production_request(RequestType::Dag), &[]).unwrap();
    assert_eq!(result.label, "dag");
    assert_eq!(result.sql_type, "general");
    assert_eq!(result.store_type, StoreType::TiKv);
    assert_eq!(result.row_len, 0);
    let mut rows = Vec::new();
    result.Next(&mut rows, 8).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(GetSelectResultConcurrency(&*result), Some((0, 0)));

    let restricted = production::DistSQLContext {
        in_restricted_sql: true,
        ..context.clone()
    };
    let mut paged_request = production_request(RequestType::Dag);
    paged_request.paging = true;
    let restricted_result =
        production::Select(&restricted, &paged_request, &["int".into()]).unwrap();
    assert_eq!(restricted_result.sql_type, "internal");
    assert_eq!(restricted_result.row_len, 1);
    assert!(restricted_result.paging);
}

#[test]
fn select_with_runtime_stats_preserves_request_behavior() {
    let client = TestClient { responses: vec![] };
    let context = production::DistSQLContext {
        client: Arc::new(client),
        query_cop_store_limiter: None,
        exec_details: None,
        runtime_stats: None,
        in_restricted_sql: false,
        enable_chunk_rpc: false,
        streaming: true,
        concurrency: 9,
        tiflash_max_threads: -1,
        tiflash_max_bytes_before_external_join: -1,
        tiflash_max_bytes_before_external_group_by: -1,
        tiflash_max_bytes_before_external_sort: -1,
        tiflash_max_query_memory_per_node: 0,
        tiflash_query_spill_ratio: 0.7,
        tiflash_use_hash_join_v2: false,
    };
    let result = production::SelectWithRuntimeStats(
        &context,
        &production_request(RequestType::Dag),
        &[],
        &[1, 2],
        3,
    )
    .unwrap();
    assert_eq!(result.cop_plan_ids, vec![1, 2]);
    assert_eq!(result.root_plan_id, 3);
    assert_eq!(GetSelectResultConcurrency(&*result), Some((0, 0)));
}

#[test]
fn analyze_and_checksum_forward_requests_without_rechecking_payload_kind() {
    let client = TestClient { responses: vec![] };
    let context = test_context(Arc::new(TestClient { responses: vec![] }));
    let dag = production_request(RequestType::Dag);
    let analyze = production::Analyze(&client, &dag, true, &context, 0).unwrap();
    assert_eq!(analyze.label, "analyze");
    assert_eq!(analyze.sql_type, "internal");
    assert_eq!(analyze.store_type, StoreType::TiKv);

    let checksum = production::Checksum(&client, &dag).unwrap();
    assert_eq!(checksum.label, "checksum");
    assert_eq!(checksum.sql_type, "general");
    assert_eq!(checksum.store_type, StoreType::TiKv);
}

#[test]
fn analyze_sends_the_request_concurrency_to_transport() {
    let client = CapturingClient {
        request: Mutex::new(None),
    };
    let mut request = production_request(RequestType::Analyze);
    request.concurrency = 7;

    let context = test_context(Arc::new(TestClient { responses: vec![] }));
    let result = production::Analyze(&client, &request, true, &context, 0).unwrap();

    assert_eq!(GetSelectResultConcurrency(&*result), Some((7, 0)));
    let sent = client.request.lock().unwrap().clone().unwrap();
    assert_eq!(sent.concurrency, 7);
    assert_eq!(sent.request_source, "stats");
}

#[test]
fn analyze_marks_the_sent_request_as_internal_stats() {
    let client = CapturingClient {
        request: Mutex::new(None),
    };
    let context = test_context(Arc::new(TestClient { responses: vec![] }));
    production::Analyze(
        &client,
        &production_request(RequestType::Analyze),
        false,
        &context,
        0,
    )
    .unwrap();
    assert_eq!(
        client
            .request
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .request_source,
        "stats"
    );
}

#[test]
fn mpp_result_uses_the_default_result_iterator() {
    let context = test_context(Arc::new(TestClient { responses: vec![] }));
    let mut result = production::GenSelectResultFromMPPResponse(
        &context,
        Box::new(VecResponseSource::new(vec![Ok(SelectResponse {
            rows: vec![vec!["mpp".into()]],
            ..Default::default()
        })])),
        &[],
        &[10],
        10,
        Arc::new(|| false),
    );
    assert_eq!(result.label, "mpp");
    assert_eq!(result.cop_plan_ids, vec![10]);
    assert_eq!(result.root_plan_id, 10);
    assert_eq!(result.store_type, StoreType::TiFlash);
    let mut rows = Vec::new();
    result.Next(&mut rows, 1).unwrap();
    assert_eq!(rows.len(), 1);
}

#[test]
fn tiflash_metadata_matches_go_sentinel_and_quota_rules() {
    let context = production::DistSQLContext {
        client: Arc::new(TestClient { responses: vec![] }),
        query_cop_store_limiter: None,
        exec_details: None,
        runtime_stats: None,
        in_restricted_sql: false,
        enable_chunk_rpc: false,
        streaming: false,
        concurrency: 1,
        tiflash_max_threads: -1,
        tiflash_max_bytes_before_external_join: -2,
        tiflash_max_bytes_before_external_group_by: 10,
        tiflash_max_bytes_before_external_sort: -1,
        tiflash_max_query_memory_per_node: 0,
        tiflash_query_spill_ratio: 0.75,
        tiflash_use_hash_join_v2: true,
    };
    let mut metadata = Vec::new();
    production::SetTiFlashConfVarsInContext(&context, &mut metadata);
    assert_eq!(
        metadata,
        vec![
            (
                "tidb_max_bytes_before_tiflash_external_join".into(),
                "-2".into()
            ),
            (
                "tidb_max_bytes_before_tiflash_external_group_by".into(),
                "10".into()
            ),
            ("tiflash_mem_quota_query_per_node".into(), "0".into()),
            ("tiflash_query_spill_ratio".into(), "0.75".into()),
            ("tiflash_use_hash_join_v2".into(), "true".into()),
        ]
    );
}

#[test]
fn encode_type_requires_chunk_flag_and_records_system_endian() {
    let context = production::DistSQLContext {
        client: Arc::new(TestClient { responses: vec![] }),
        query_cop_store_limiter: None,
        exec_details: None,
        runtime_stats: None,
        in_restricted_sql: false,
        enable_chunk_rpc: true,
        streaming: false,
        concurrency: 1,
        tiflash_max_threads: -1,
        tiflash_max_bytes_before_external_join: -1,
        tiflash_max_bytes_before_external_group_by: -1,
        tiflash_max_bytes_before_external_sort: -1,
        tiflash_max_query_memory_per_node: 0,
        tiflash_query_spill_ratio: 0.7,
        tiflash_use_hash_join_v2: false,
    };
    let mut request = production::DAGRequest::default();
    production::SetEncodeType(&context, &mut request);
    assert_eq!(request.encode_type, production::EncodeType::Chunk);
    assert_eq!(
        request.chunk_memory_layout,
        Some(production::GetSystemEndian())
    );

    let disabled = production::DistSQLContext {
        enable_chunk_rpc: false,
        ..context
    };
    production::SetEncodeType(&disabled, &mut request);
    assert_eq!(request.encode_type, production::EncodeType::Default);
}
