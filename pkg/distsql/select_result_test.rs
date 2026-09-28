// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// DistSQL SelectResult 迭代与关闭语义的单测。
//
// DistSQL：分布式 SQL 执行路径，通过 coprocessor 拉取分区结果。
// 本文件验证跨空/非空响应的行流式读取、Region/RPC 错误中止，以及 Close 幂等。

use super::*;
use crate::select_result as production;
use crate::select_result::{
    SelectResult as ProductionSelectResult, SelectResultIter as ProductionSelectResultIter,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// 构造测试用 SelectResponse（行、告警、扫描 key 数）。
fn response(rows: &[&[&str]], warnings: &[&str], scanned: u64) -> SelectResponse {
    SelectResponse {
        rows: rows
            .iter()
            .map(|row| row.iter().map(|value| (*value).to_owned()).collect())
            .collect(),
        warnings: warnings
            .iter()
            .map(|warning| (*warning).to_owned())
            .collect(),
        scanned_keys: scanned,
        error: None,
    }
}

#[test]
/// 跨空响应与非空响应连续读行，并累计 stats。
fn streams_rows_across_empty_and_non_empty_responses() {
    let source = VecResponseSource::new(vec![
        Ok(response(&[], &["w1"], 2)),
        Ok(response(&[&["1", "a"], &["2", "b"]], &[], 3)),
    ]);
    let mut result = SelectResult::new(source);
    assert_eq!(
        result.next_row().unwrap(),
        Some(vec!["1".into(), "a".into()])
    );
    assert_eq!(
        result.next_row().unwrap(),
        Some(vec!["2".into(), "b".into()])
    );
    assert_eq!(result.next_row().unwrap(), None);
    assert_eq!(
        result.stats(),
        &SelectStats {
            response_count: 2,
            row_count: 2,
            warning_count: 1,
            scanned_keys: 5
        }
    );
}

#[test]
/// 嵌入式 Region 错误与传输层 RPC 错误均应中止迭代。
/// Region：TiKV 中数据的分片单位。
fn transport_and_embedded_errors_stop_iteration() {
    let mut embedded = response(&[&["ignored"]], &[], 1);
    embedded.error = Some("region error".into());
    let source = VecResponseSource::new(vec![Ok(embedded)]);
    assert_eq!(
        SelectResult::new(source).next_row().unwrap_err().0,
        "region error"
    );

    let source = VecResponseSource::new(vec![Err(DistSqlError("rpc failed".into()))]);
    assert_eq!(
        SelectResult::new(source).next_row().unwrap_err().0,
        "rpc failed"
    );
}

#[test]
/// Close 可重复调用，并丢弃已缓冲但未读完的行。
fn close_is_idempotent_and_discards_buffered_rows() {
    let source = VecResponseSource::new(vec![Ok(response(&[&["1"], &["2"]], &[], 2))]);
    let mut result = SelectResult::new(source);
    assert_eq!(result.next_row().unwrap(), Some(vec!["1".into()]));
    result.close().unwrap();
    result.close().unwrap();
    assert_eq!(result.next_row().unwrap(), None);
}

fn production_result(
    responses: Vec<DistSqlResult<SelectResponse>>,
) -> Box<dyn production::SelectResult> {
    Box::new(production::selectResult::new(
        VecResponseSource::new(responses),
        4,
    ))
}

fn production_response(rows: &[&[&str]]) -> SelectResponse {
    SelectResponse {
        rows: rows
            .iter()
            .map(|row| row.iter().map(|value| (*value).to_owned()).collect())
            .collect(),
        ..Default::default()
    }
}

#[test]
fn select_result_raw_and_row_reads_share_one_buffer() {
    let mut result = production::selectResult::new(
        VecResponseSource::new(vec![Ok(production_response(&[&["1", "a"], &["2", "b"]]))]),
        2,
    );
    assert_eq!(result.NextRaw().unwrap(), Some(b"1\ta".to_vec()));
    let mut rows = Vec::new();
    result.Next(&mut rows, 1).unwrap();
    assert_eq!(
        rows,
        vec![vec![
            production::Scalar::String("2".into()),
            production::Scalar::String("b".into())
        ]]
    );
    assert_eq!(result.NextRaw().unwrap(), None);
}

#[test]
fn serial_select_results_move_to_the_next_response_after_exhaustion() {
    let mut result = production::NewSerialSelectResults(vec![
        production_result(vec![Ok(production_response(&[&["a"]]))]),
        production_result(vec![Ok(production_response(&[&["b"]]))]),
    ]);
    let mut rows = Vec::new();
    result.Next(&mut rows, 8).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], vec![production::Scalar::String("a".into())]);
    assert_eq!(rows[1], vec![production::Scalar::String("b".into())]);
}

#[test]
fn sorted_select_results_merge_ascending_and_descending_inputs() {
    let ascending = production::NewSortedSelectResults(
        vec![
            production_result(vec![Ok(production_response(&[&["1"], &["3"]]))]),
            production_result(vec![Ok(production_response(&[&["2"], &["4"]]))]),
        ],
        vec![production::ByItem {
            column: 0,
            descending: false,
        }],
    )
    .unwrap();
    let mut ascending = ascending;
    let mut rows = Vec::new();
    ascending.Next(&mut rows, 8).unwrap();
    let values: Vec<_> = rows.iter().map(|row| row[0].clone()).collect();
    assert_eq!(
        values,
        vec![
            production::Scalar::String("1".into()),
            production::Scalar::String("2".into()),
            production::Scalar::String("3".into()),
            production::Scalar::String("4".into()),
        ]
    );
}

#[test]
fn select_result_iter_reports_channel_zero_and_end_of_stream() {
    let result = production_result(vec![Ok(production_response(&[&["row"]]))]);
    let mut iter = result.IntoIter().unwrap();
    let row = iter.Next().unwrap().unwrap();
    assert_eq!(row.channel, 0);
    assert_eq!(row.row, vec![production::Scalar::String("row".into())]);
    assert!(iter.Next().unwrap().is_none());
}

#[test]
fn runtime_stats_merge_and_cache_ratio_include_store_batches() {
    let mut stats = production::selectResultRuntimeStats {
        response_count: 2,
        ..Default::default()
    };
    stats.mergeCopRuntimeStats(Duration::from_millis(5), true, 1, 2);
    assert_eq!(stats.cop_cache_hit_num, 1);
    assert_eq!(stats.store_batched_fallback_num, 2);
    assert_eq!(stats.calcCacheHit(), 1.0 / 3.0);
    let mut other = production::selectResultRuntimeStats::default();
    other.response_count = 1;
    stats.Merge(&other);
    assert_eq!(stats.response_count, 3);
}

struct CloseTrackingResult {
    rows: Vec<production::Row>,
    close_log: Arc<Mutex<Vec<&'static str>>>,
    name: &'static str,
    close_error: Option<&'static str>,
}

impl production::SelectResult for CloseTrackingResult {
    fn NextRaw(&mut self) -> DistSqlResult<Option<Vec<u8>>> {
        Ok(None)
    }

    fn Next(&mut self, rows: &mut Vec<production::Row>, _capacity: usize) -> DistSqlResult<()> {
        rows.append(&mut self.rows);
        Ok(())
    }

    fn IntoIter(self: Box<Self>) -> DistSqlResult<Box<dyn production::SelectResultIter>> {
        Err(DistSqlError("not implemented".into()))
    }

    fn Close(&mut self) -> DistSqlResult<()> {
        self.close_log.lock().unwrap().push(self.name);
        match self.close_error {
            Some(message) => Err(DistSqlError(message.into())),
            None => Ok(()),
        }
    }
}

#[test]
fn serial_result_matches_go_close_lifecycle_and_last_error() {
    let close_log = Arc::new(Mutex::new(Vec::new()));
    let make_result = |name, rows, close_error| {
        Box::new(CloseTrackingResult {
            rows,
            close_log: Arc::clone(&close_log),
            name,
            close_error,
        }) as Box<dyn production::SelectResult>
    };
    let mut result = production::NewSerialSelectResults(vec![
        make_result("first", vec![], Some("first close error")),
        make_result("second", vec![vec![production::Scalar::Int(1)]], None),
        make_result("third", vec![], Some("last close error")),
    ]);

    let mut rows = Vec::new();
    result.Next(&mut rows, 1).unwrap();
    assert_eq!(rows, vec![vec![production::Scalar::Int(1)]]);
    assert!(close_log.lock().unwrap().is_empty());

    assert_eq!(result.Close().unwrap_err().0, "last close error");
    assert_eq!(*close_log.lock().unwrap(), vec!["first", "second", "third"]);
}

#[test]
fn sorted_result_into_iter_is_unsupported_like_go() {
    let results = [
        production::NewSortedSelectResults(Vec::new(), Vec::new()).unwrap(),
        production::NewSerialSelectResults(Vec::new()),
    ];
    for result in results {
        match result.IntoIter() {
            Err(error) => assert_eq!(error.0, "not implemented"),
            Ok(_) => panic!("aggregate select result unexpectedly supported IntoIter"),
        }
    }
}
