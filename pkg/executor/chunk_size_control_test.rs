// Copyright 2026 AsterSQL.

// Chunk（向量化批）大小控制相关测试。
//
// 验证即使 Coprocessor（下推计算组件）页面返回有延迟，每次 `Next(n)` 仍严格
// 按调用方请求的行数返回，且观测到的请求序列与调用顺序一致。

use std::time::{Duration, Instant};

use crate::table_readers_required_rows_test::{
    RequiredRowsBackend, required_rows_reader, signed_rows,
};

/// 延迟返回的后端仍按每次请求的 chunk 大小分页，且累计耗时不低于单次延迟×次数。
#[test]
fn delayed_coprocessor_pages_still_honor_each_requested_chunk_size() {
    let delay = Duration::from_millis(2);
    let backend = RequiredRowsBackend::new(signed_rows(6), delay);
    let observed = backend.clone();
    let mut reader = required_rows_reader(backend);
    reader.Open().unwrap();

    let started = Instant::now();
    // 依次请求 2、1、3 行，后端应精确按该序列供数。
    let batches = [
        reader.Next(2).unwrap(),
        reader.Next(1).unwrap(),
        reader.Next(3).unwrap(),
    ];
    assert_eq!(
        batches.iter().map(Vec::len).collect::<Vec<_>>(),
        vec![2, 1, 3]
    );
    assert_eq!(observed.requests(), vec![2, 1, 3]);
    // 三次独立延迟请求，总耗时至少 3×delay。
    assert!(started.elapsed() >= delay * 3);
    reader.Close().unwrap();
}
