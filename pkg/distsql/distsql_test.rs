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
use std::sync::{Arc, Mutex};

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
    let dag = production_request(RequestType::Dag);
    let analyze = production::Analyze(&client, &dag, true).unwrap();
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

    let result = production::Analyze(&client, &request, true).unwrap();

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
    production::Analyze(&client, &production_request(RequestType::Analyze), false).unwrap();
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
    let mut result = production::GenSelectResultFromMPPResponse(
        Box::new(VecResponseSource::new(vec![Ok(SelectResponse {
            rows: vec![vec!["mpp".into()]],
            ..Default::default()
        })])),
        &[],
        &[10],
        10,
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
