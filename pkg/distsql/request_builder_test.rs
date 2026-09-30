// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// `RequestBuilder`（基于 lib.rs 简化版）的单元测试。
//
// 验证 key range 合法性、重叠合并、扫描/副本选项透传，以及
// 降序扫描必须 keep_order、并发必须为正等约束。

use super::*;
use crate::request_builder as production;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn go_merge_42_request_builder_forwards_limiters_and_batch_controls() {
    let limiter = astersql_kv::NewCoprRequestLimiter(2).unwrap();
    let mut builder = production::RequestBuilder::new();
    let request = builder
        .SetCoprRequestLimiter(Arc::clone(&limiter))
        .SetStoreBatchSize(5)
        .SetAllowBatchTaskDataMerge(true)
        .SetExecuteBatchTasksSerially(true)
        .Build()
        .unwrap();
    assert!(Arc::ptr_eq(
        request.copr_request_limiter.as_ref().unwrap(),
        &limiter
    ));
    assert_eq!(request.store_batch_size, 5);
    assert!(request.allow_batch_task_data_merge);
    assert!(request.execute_batch_tasks_serially);
}

/// 构造半开区间 `[start, end)` 的辅助函数；非法区间会 panic（测试数据已知合法）。
fn range(start: &[u8], end: &[u8]) -> KeyRange {
    KeyRange::new(start, end).unwrap()
}

/// 拒绝空请求与非法 key range（start >= end）。
#[test]
fn rejects_invalid_ranges_and_empty_requests() {
    assert!(KeyRange::new(b"b", b"a").is_err());
    assert!(KeyRange::new(b"a", b"a").is_err());
    assert!(RequestBuilder::new(RequestType::Dag).build().is_err());
}

/// 重叠/乱序的 key range 应排序并合并为不相交区间。
#[test]
fn sorts_and_merges_overlapping_ranges() {
    let request = RequestBuilder::new(RequestType::Dag)
        .key_ranges(vec![
            range(b"d", b"f"),
            range(b"a", b"c"),
            range(b"b", b"e"),
            range(b"h", b"i"),
        ])
        .build()
        .unwrap();
    assert_eq!(
        request.key_ranges,
        vec![range(b"a", b"f"), range(b"h", b"i")]
    );
}

/// 扫描方向、流式、存储类型、start_ts、超时等选项应完整透传到 Request。
#[test]
fn carries_scan_and_replica_options() {
    let request = RequestBuilder::new(RequestType::Analyze)
        .key_ranges(vec![range(b"a", b"z")])
        .concurrency(8)
        .keep_order(true)
        .descending(true)
        .streaming(true)
        .store_type(StoreType::TiFlash)
        .start_ts(42)
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap();
    assert_eq!(request.request_type, RequestType::Analyze);
    assert_eq!(request.concurrency, 8);
    assert!(request.keep_order && request.descending && request.streaming);
    assert_eq!(request.store_type, StoreType::TiFlash);
    assert_eq!(request.start_ts, 42);
}

/// 降序扫描要求 keep_order；并发度必须为正整数。
#[test]
fn descending_requires_order_and_concurrency_must_be_positive() {
    let ranges = vec![range(b"a", b"z")];
    assert!(
        RequestBuilder::new(RequestType::Dag)
            .key_ranges(ranges.clone())
            .descending(true)
            .build()
            .is_err()
    );
    assert!(
        RequestBuilder::new(RequestType::Dag)
            .key_ranges(ranges)
            .concurrency(0)
            .build()
            .is_err()
    );
}

#[test]
fn table_handles_merge_consecutive_ranges_and_preserve_hints() {
    let (ranges, hints) = production::TableHandlesToKVRanges(
        15,
        &[0, 2, 3, 4, 5, 10, 11, 100, i64::MAX - 1, i64::MAX],
    );
    assert_eq!(hints, vec![1, 4, 2, 1, 2]);
    assert_eq!(ranges.len(), hints.len());
    assert!(ranges[1].start < ranges[1].end);
    assert!(ranges.last().unwrap().end > ranges.last().unwrap().start);
}

#[test]
fn partition_handles_merge_only_adjacent_handles_in_the_same_partition() {
    let (ranges, hints) = production::PartitionHandlesToKVRanges(&[
        (1, 0),
        (2, 2),
        (2, 3),
        (2, 4),
        (3, 5),
        (1, 10),
        (2, 11),
        (3, 100),
    ]);
    assert_eq!(hints, vec![1, 3, 1, 1, 1, 1]);
    assert_eq!(ranges.len(), hints.len());
}

#[test]
fn table_ranges_preserve_exclusive_endpoints_and_empty_ranges() {
    let ranges = production::TableRangesToKVRanges(
        13,
        &[
            production::HandleRange {
                low: 1,
                high: 2,
                low_exclusive: false,
                high_inclusive: true,
            },
            production::HandleRange {
                low: 34,
                high: 34,
                low_exclusive: true,
                high_inclusive: false,
            },
        ],
    );
    assert_eq!(ranges.len(), 2);
    assert_eq!(ranges[0].end, {
        let mut key = ranges[0].start[..11].to_vec();
        key.extend_from_slice(&((2_i64 as u64 ^ (1 << 63)) + 1).to_be_bytes());
        key
    });
    assert!(ranges[1].start >= ranges[1].end);
}

#[test]
fn index_ranges_expand_for_each_table_without_dropping_empty_ranges() {
    let ranges =
        production::IndexRangesToKVRanges(&[12, 13], 15, &[(vec![1], vec![2]), (vec![3], vec![3])])
            .unwrap();
    assert_eq!(ranges.len(), 4);
    assert_eq!(ranges[0].start[0], b't');
    assert_eq!(ranges[2].start[0], b't');
}

#[test]
fn request_builder_allows_zero_value_request_like_go() {
    let request = production::RequestBuilder::new().Build().unwrap();
    assert_eq!(request.request_type, RequestType::Dag);
    assert_eq!(request.payload, production::RequestPayload::Empty);
    assert!(request.key_ranges.is_empty());
    assert_eq!(request.concurrency, 0);
}

#[test]
fn request_builder_dag_copies_scan_options() {
    let request = production::RequestBuilder::new()
        .SetKeyRanges(vec![range(b"a", b"b")])
        .SetDAGRequest(vec![1, 2, 3])
        .SetStartTS(42)
        .SetDesc(true)
        .SetKeepOrder(true)
        .SetStoreType(StoreType::TiFlash)
        .SetConcurrency(8)
        .Build()
        .unwrap();
    assert_eq!(
        request.payload,
        production::RequestPayload::Dag(vec![1, 2, 3])
    );
    assert_eq!(request.start_ts, 42);
    assert!(request.descending && request.keep_order);
    assert_eq!(request.store_type, StoreType::TiFlash);
    assert_eq!(request.concurrency, 8);
}

#[test]
fn request_builder_analyze_and_checksum_disable_cache() {
    let analyze = production::RequestBuilder::new()
        .SetAnalyzeRequest(vec![1], production::IsolationLevel::ReadCommitted)
        .Build()
        .unwrap();
    assert_eq!(analyze.request_type, RequestType::Analyze);
    assert_eq!(analyze.priority, production::Priority::Low);
    assert!(analyze.not_fill_cache);

    let checksum = production::RequestBuilder::new()
        .SetChecksumRequest(vec![2])
        .Build()
        .unwrap();
    assert_eq!(checksum.request_type, RequestType::Checksum);
    assert!(checksum.not_fill_cache);
}

#[test]
fn request_builder_session_concurrency_is_a_cap_not_an_overwrite() {
    let vars = production::SessionVars {
        concurrency: 4,
        ..Default::default()
    };
    let capped = production::RequestBuilder::new()
        .SetConcurrency(8)
        .SetFromSessionVars(&vars)
        .SetDAGRequest(vec![])
        .Build()
        .unwrap();
    assert_eq!(capped.concurrency, 4);

    let filled = production::RequestBuilder::new()
        .SetFromSessionVars(&vars)
        .SetDAGRequest(vec![])
        .Build()
        .unwrap();
    assert_eq!(filled.concurrency, 4);
}

#[test]
fn request_builder_is_single_use_after_success() {
    let mut builder = production::RequestBuilder::new();
    assert!(builder.Build().is_ok());
    assert!(builder.Build().is_err());
}

#[test]
fn txn_scope_checker_is_called_for_partition_ranges() {
    struct Checker;
    impl production::TxnScopeChecker for Checker {
        fn verify_txn_scope(&self, scope: &str, physical_table_id: i64) -> bool {
            scope == "dc1" && physical_table_id == 7
        }
    }
    let ok = production::RequestBuilder::new()
        .SetTxnScope("dc1")
        .SetFromInfoSchema(std::sync::Arc::new(Checker))
        .SetPartitionIDAndRanges(vec![production::PartitionIDAndRanges {
            partition_id: 7,
            ranges: vec![range(b"a", b"b")],
        }])
        .Build();
    assert!(ok.is_ok());
    let bad = production::RequestBuilder::new()
        .SetTxnScope("dc1")
        .SetFromInfoSchema(std::sync::Arc::new(Checker))
        .SetPartitionIDAndRanges(vec![production::PartitionIDAndRanges {
            partition_id: 8,
            ranges: vec![range(b"a", b"b")],
        }])
        .Build();
    assert!(bad.is_err());
}

#[test]
fn txn_scope_zero_value_and_global_match_go_exactly() {
    struct RejectAll;
    impl production::TxnScopeChecker for RejectAll {
        fn verify_txn_scope(&self, _scope: &str, _physical_table_id: i64) -> bool {
            false
        }
    }

    let checker = RejectAll;
    assert!(production::VerifyTxnScope("", 7, &checker));
    assert!(production::VerifyTxnScope("global", 7, &checker));
    assert!(!production::VerifyTxnScope("GLOBAL", 7, &checker));

    let request = production::RequestBuilder::new()
        .SetTxnScope("")
        .SetFromInfoSchema(std::sync::Arc::new(RejectAll))
        .SetPartitionIDAndRanges(vec![production::PartitionIDAndRanges {
            partition_id: 7,
            ranges: vec![range(b"a", b"b")],
        }])
        .Build();
    assert!(request.is_ok());
}

#[test]
fn split_ranges_keep_signed_and_unsigned_order_contract() {
    let ranges = vec![
        production::HandleRange {
            low: -2,
            high: -1,
            low_exclusive: false,
            high_inclusive: true,
        },
        production::HandleRange {
            low: 1,
            high: 2,
            low_exclusive: false,
            high_inclusive: true,
        },
    ];
    let (signed, unsigned) =
        production::SplitRangesAcrossInt64Boundary(&ranges, true, false, false);
    assert_eq!(signed, vec![ranges[0].clone()]);
    assert_eq!(unsigned, vec![ranges[1].clone()]);
    let (descending_unsigned, descending_signed) =
        production::SplitRangesAcrossInt64Boundary(&ranges, true, true, false);
    assert_eq!(descending_unsigned, unsigned);
    assert_eq!(descending_signed, signed);
}

#[test]
fn build_table_ranges_contains_record_and_index_prefixes() {
    let ranges = production::BuildTableRanges(7, &[1, 2]);
    assert_eq!(ranges.len(), 3);
    assert!(ranges[0].start.starts_with(b"t"));
    assert!(ranges[1].start.windows(2).any(|window| window == b"_i"));
}
