// Copyright 2026 AsterSQL.

//! Go/Rust 行为对照：覆盖成功汇总、失败去重、取消分流、Summary 后重置与辅助格式化。
//! 不依赖真实 zap，用注入 logger 截获 Field，断言与 Go collector 语义一致。

use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug)]
struct SameMessageAsContextCanceled;

impl std::fmt::Display for SameMessageAsContextCanceled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("context canceled")
    }
}

impl std::error::Error for SameMessageAsContextCanceled {}

use crate::collector::{
    self, BackupDataSize, ContextCanceled, Field, FieldValue, LogCollector, NewLogCollector,
    SummaryValue, log_key_for, reset_global_collector_for_test, zap,
};
use crate::units;
use crate::{
    CollectDuration, CollectFailureUnit, CollectInt, CollectSuccessUnit, SetSuccessStatus, Succeed,
    Summary,
};

// 按整条 Field 等价比较，避免只比 key 漏掉累加后的 value 偏差。
fn field_contains(fields: &[Field], expected: &Field) -> bool {
    fields.iter().any(|field| field == expected)
}

#[test]
fn go_rust_public_contract_matches() {
    // 全局 collector 可能被其它测试污染，先复位再测包级 API。
    reset_global_collector_for_test();

    // 正常路径：同名 Duration/Int 累加后进入成功 Summary（对齐 TestSumDurationInt）。
    // normal: duration/int aggregation in success summary (TestSumDurationInt)
    let captured = Arc::new(Mutex::new(Vec::<Field>::new()));
    let capture = Arc::clone(&captured);
    // 注入 logger 只截获字段；msg 在本段不参与断言。
    let mut col = NewLogCollector(Arc::new(move |_msg, fields| {
        capture
            .lock()
            .expect("capture lock")
            .extend_from_slice(fields);
    }));
    // 故意对 b/c 重复 Collect，验证 map 内累加而非覆盖。
    col.CollectDuration("a", Duration::from_secs(1));
    col.CollectDuration("b", Duration::from_secs(1));
    col.CollectDuration("b", Duration::from_secs(1));
    col.CollectInt("c", 2);
    col.CollectInt("c", 2);
    // true → 成功模板；false 时本段断言会失效。
    col.SetSuccessStatus(true);
    col.Summary("foo");

    let fields = captured.lock().expect("capture lock").clone();
    // 7 条含业务字段与成功模板附加字段，与 Go 侧计数一致。
    assert_eq!(fields.len(), 7);
    // a 只采一次 → 1s；b 两次 → 2s；c 两次 → 4。
    assert!(field_contains(
        &fields,
        &zap::Duration("a", Duration::from_secs(1))
    ));
    assert!(field_contains(
        &fields,
        &zap::Duration("b", Duration::from_secs(2))
    ));
    assert!(field_contains(&fields, &zap::Int("c", 4)));

    // 边界：同一失败 unit 重复上报只保留首次 reason。
    // boundary: duplicate failure units only record the first reason
    let captured = Arc::new(Mutex::new(Vec::<Field>::new()));
    let capture = Arc::clone(&captured);
    let mut col = NewLogCollector(Arc::new(move |_msg, fields| {
        capture
            .lock()
            .expect("capture lock")
            .extend_from_slice(fields);
    }));
    col.CollectFailureUnit("range-a", Arc::new(std::io::Error::other("first")));
    // 第二次同名失败不得覆盖 "first"。
    col.CollectFailureUnit("range-a", Arc::new(std::io::Error::other("second")));
    col.SetSuccessStatus(false);
    col.Summary("backup");
    let fields = captured.lock().expect("capture lock").clone();
    // ranges-failed 按去重后的 unit 数计 1，而非调用次数。
    assert!(field_contains(&fields, &zap::Int("ranges-failed", 1)));
    assert!(fields.iter().any(|field| {
        field.key == "error" && field.value == FieldValue::Error("first".to_string())
    }));

    // 错误分流：普通失败打 unit error；ContextCanceled 单独计数、不进 unit-name 列表。
    // error: non-cancel failures emit unit error; canceled ones are counted separately
    let captured = Arc::new(Mutex::new((String::new(), Vec::<Field>::new())));
    let capture = Arc::clone(&captured);
    // 同时截获 msg，校验失败摘要标题形如 "<name> failed summary"。
    let mut col = NewLogCollector(Arc::new(move |msg, fields| {
        let mut guard = capture.lock().expect("capture lock");
        guard.0 = msg.to_string();
        guard.1.extend_from_slice(fields);
    }));
    // boom 为普通 IO 错误；range-b 用 ContextCanceled 模拟取消。
    col.CollectFailureUnit("range-a", Arc::new(std::io::Error::other("boom")));
    col.CollectFailureUnit(
        "range-b",
        Arc::new(ContextCanceled) as collector::SummaryError,
    );
    col.SetSuccessStatus(false);
    col.Summary("restore");
    let (msg, fields) = captured.lock().expect("capture lock").clone();
    // 失败标题固定拼 " failed summary"，name 来自 Summary 参数。
    assert_eq!(msg, "restore failed summary");
    // 非取消失败应带 unit-name=range-a 与 error=boom。
    assert!(fields.iter().any(|field| {
        field.key == "unit-name" && field.value == FieldValue::String("range-a".to_string())
    }));
    assert!(fields.iter().any(|field| {
        field.key == "error" && field.value == FieldValue::Error("boom".to_string())
    }));
    // 取消类失败不得以 unit-name=range-b 出现在错误明细里。
    assert!(!fields.iter().any(|field| field.key == "unit-name"
        && field.value == FieldValue::String("range-b".to_string())));

    // Go compares errors.Cause(reason) with the context.Canceled sentinel by identity.
    // A different error type with the same display text must remain a normal failure.
    let captured = Arc::new(Mutex::new(Vec::<Field>::new()));
    let capture = Arc::clone(&captured);
    let mut col = NewLogCollector(Arc::new(move |_msg, fields| {
        capture
            .lock()
            .expect("capture lock")
            .extend_from_slice(fields);
    }));
    col.CollectFailureUnit("lookalike", Arc::new(SameMessageAsContextCanceled));
    col.SetSuccessStatus(false);
    col.Summary("restore");
    let fields = captured.lock().expect("capture lock").clone();
    assert!(fields.iter().any(|field| {
        field.key == "unit-name" && field.value == FieldValue::String("lookalike".to_string())
    }));
    assert!(fields.iter().any(|field| {
        field.key == "error" && field.value == FieldValue::Error("context canceled".to_string())
    }));

    // 生命周期：Summary 后 duration map 清空；Succeed 跟踪最近一次成功标志。
    // resource / lifecycle: duration maps reset after Summary; succeed flag tracks status
    reset_global_collector_for_test();
    SetSuccessStatus(true);
    // 包级 Succeed 读 LAST_STATUS，与 collector 内 success 标志双写对齐 Go。
    assert!(Succeed());

    let captured = Arc::new(Mutex::new(Vec::<Field>::new()));
    let capture = Arc::clone(&captured);
    let mut col = NewLogCollector(Arc::new(move |_msg, fields| {
        capture
            .lock()
            .expect("capture lock")
            .extend_from_slice(fields);
    }));
    col.CollectDuration("phase", Duration::from_millis(100));
    col.SetSuccessStatus(true);
    col.Summary("task");
    // Summary 后重新采集，应只有新的 50ms，不能与旧 100ms 累加成 150ms。
    col.CollectDuration("phase", Duration::from_millis(50));
    captured.lock().expect("capture lock").clear();
    col.Summary("task");
    let fields = captured.lock().expect("capture lock").clone();
    assert!(field_contains(
        &fields,
        &zap::Duration("phase", Duration::from_millis(50))
    ));
    assert!(!field_contains(
        &fields,
        &zap::Duration("phase", Duration::from_millis(150))
    ));

    // 辅助函数：log key 空格转连字符；HumanSize 与 docker/go-units 展示对齐。
    // helper parity: log key formatting and human size
    assert_eq!(log_key_for("total kv"), "total-kv");
    assert_eq!(units::HumanSize(1_500.0), "1.5kB");
}
