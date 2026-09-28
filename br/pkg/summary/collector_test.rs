// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

//! Go-equivalent tests for `br/pkg/summary/collector_test.go`.
//!
//! 对齐 Go `TestSumDurationInt`：用自定义 logger 截获 `Summary` 输出的 Field，
//! 验证同名 Duration/Int 会在 collector 内累加后再写出。
//! 成功路径还会附带 success/total costs 等固定字段，故捕获条数大于业务字段数。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::{Field, LogCollector, NewLogCollector, units, zap};

/// Corresponds to Go `TestSumDurationInt`.
///
/// Custom logger captures zap-equivalent fields from `Summary("foo")` and asserts
/// that repeated `CollectDuration` / `CollectInt` aggregate by key.
/// 场景：`a` 只收一次；`b`/`c` 各收两次，期望汇总为 2s 与 4。
#[test]
fn test_sum_duration_int() {
    // fields 对应 Go 闭包捕获的 []zap.Field，用来承接 Summary("foo") 输出的所有字段。
    let fields: Arc<Mutex<Vec<Field>>> = Arc::new(Mutex::new(Vec::new()));
    let capture = Arc::clone(&fields);
    // logger 等价 Go 里注入的 logFunc：忽略 msg，只累积字段切片。
    let logger = Arc::new(move |_msg: &str, fs: &[Field]| {
        capture.lock().expect("fields lock").extend_from_slice(fs);
    });

    let mut col = NewLogCollector(logger);
    col.CollectDuration("a", Duration::from_secs(1));
    col.CollectDuration("b", Duration::from_secs(1));
    col.CollectDuration("b", Duration::from_secs(1));
    col.CollectInt("c", 2);
    col.CollectInt("c", 2);
    // SetSuccessStatus(true) 决定 Summary 走成功模板，否则会输出 failure 相关字段。
    col.SetSuccessStatus(true);
    col.Summary("foo");

    let fields = fields.lock().expect("fields lock").clone();
    // 7 = 业务字段 a/b/c + success 路径附加的固定摘要字段（与 Go 断言一致）。
    assert_eq!(fields.len(), 7);

    // assert_contains 对应 Go 内联闭包：按 Key 查找字段，并要求整条 Field 完全一致。
    let assert_contains = |expected: Field| {
        for f in &fields {
            if f.key == expected.key {
                assert_eq!(f, &expected, "field {:?} mismatch", expected.key);
                return;
            }
        }
        panic!("{:?} is not in {:?}", expected, fields);
    };

    // 同名键累加：a=1s、b=1+1=2s、c=2+2=4，与 Go zap.Duration/Int 期望值对齐。
    assert_contains(zap::Duration("a", Duration::from_secs(1)));
    assert_contains(zap::Duration("b", Duration::from_secs(2)));
    assert_contains(zap::Int("c", 4));
}

/// `docker/go-units.HumanSize` uses four significant digits, including for
/// sub-byte rates produced by the summary's average-speed calculation.
#[test]
fn test_human_size_keeps_four_significant_digits_for_small_values() {
    assert_eq!(units::HumanSize(0.000_123_456), "0.0001235B");
    assert_eq!(units::HumanSize(1_048_576.0), "1.049MB");
    assert_eq!(units::HumanSize(1.0e28), "1e+04YB");
}
