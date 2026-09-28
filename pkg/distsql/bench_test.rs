// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

// DistSQL 选择结果解码的基准场景回归测试。

use super::*;

fn assert_benchmark_response_is_fully_consumed(batch: usize, total_rows: usize) {
    let responses = (0..total_rows)
        .step_by(batch)
        .map(|start| {
            let end = (start + batch).min(total_rows);
            let rows = (start..end)
                .map(|index| vec![index.to_string(); 4])
                .collect();
            Ok(SelectResponse {
                rows,
                scanned_keys: (end - start) as u64,
                ..Default::default()
            })
        })
        .collect();
    let source = VecResponseSource::new(responses);
    let mut result = SelectResult::new(source);
    let mut count = 0;
    while result.next_row().unwrap().is_some() {
        count += 1;
    }
    assert_eq!(count, total_rows);
    assert_eq!(result.stats().response_count, total_rows.div_ceil(batch));
    assert_eq!(result.stats().row_count, total_rows);
    assert_eq!(result.stats().scanned_keys, total_rows as u64);
    assert!(result.next_row().unwrap().is_none());
}

/// 对应 Go `BenchmarkSelectResponseChunk_BigResponse` 的 4000×20000 场景。
#[test]
fn benchmark_select_response_chunk_big_response_contract() {
    assert_benchmark_response_is_fully_consumed(4000, 20000);
}

/// 对应 Go `BenchmarkSelectResponseChunk_SmallResponse` 的 32×3200 场景。
#[test]
fn benchmark_select_response_chunk_small_response_contract() {
    assert_benchmark_response_is_fully_consumed(32, 3200);
}
