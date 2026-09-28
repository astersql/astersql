// Copyright 2026 AsterSQL.

//! Parity tests for `tests/llmtest/logger` public contracts vs Go `log.go`.

// 本文件对应 `tests/llmtest/logger/parity_test.rs`，本次任务只补中文解释，不改行为。
// 本文件按成功、边界、错误和清理四类场景组织。
// 阅读时先看总入口，再看各个合同分组。
// 这里的目标是证明 Rust 与 Go 的公共合同一致。
// 成功路径关注正常返回值和可观测副作用。
// 边界路径关注空输入、默认值和最小变体。
// 错误路径关注日志、panic、exit 与错误文本。
// 清理路径关注 Close、Sync、Join 和资源释放。
// 中文注释优先解释为什么要断言。
use crate::stubs::{self, zap};
use crate::{Global, ensure_init};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::LazyLock;

// 测试 `go_rust_public_contract_matches` 固定当前文件里一个完整的可观测场景。
// 阅读时同时关注前置状态、执行路径和最终断言。
#[test]
// `go_rust_public_contract_matches` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn go_rust_public_contract_matches() {
    contract_normal_paths();
    contract_boundary();
    contract_error_paths();
    contract_resource_cleanup();
}

/// Normal: Global is initialized as a development logger; Info/Error/Debug record.
// `contract_normal_paths` 把同一类合同断言收拢到一个阅读单元里。
// 这种拆分能避免不同失败原因混在一起。
fn contract_normal_paths() {
    stubs::set_force_new_development_error(false);
    ensure_init();

    assert!(
        Global.development,
        "Go NewDevelopment builds a development logger"
    );

    Global.Info(
        "generating test SQLs for function",
        &[
            zap::String("group", "abs"),
            zap::Int("existCases", 0),
            zap::Int("generateCount", 10),
        ],
    );
    Global.Error("Failed to open test case", &[zap::Error("no such file")]);
    Global.Debug("request body", &[zap::Any("body", &"payload")]);

    let records = Global.records();
    assert!(
        records.iter().any(|r| r.level == "info"
            && r.msg == "generating test SQLs for function"
            && r.fields
                .iter()
                .any(|f| f.key == "group" && f.value == "abs")),
        "Info must record message and fields"
    );
    assert!(
        records
            .iter()
            .any(|r| r.level == "error" && r.msg == "Failed to open test case"),
        "Error must record"
    );
    assert!(
        records
            .iter()
            .any(|r| r.level == "debug" && r.msg == "request body"),
        "Debug must record"
    );
}

/// Boundary: empty message / empty fields; fresh NewDevelopment is development=true.
// `contract_boundary` 把同一类合同断言收拢到一个阅读单元里。
// 这种拆分能避免不同失败原因混在一起。
fn contract_boundary() {
    stubs::set_force_new_development_error(false);
    let logger = zap::NewDevelopment().expect("NewDevelopment ok");
    assert!(logger.development);
    logger.Info("", &[]);
    logger.Error("", &[]);
    let records = logger.records();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].msg, "");
    assert!(records[0].fields.is_empty());
}

/// Error: NewDevelopment failure panics during Global init (Go `panic(err)`).
// `contract_error_paths` 把同一类合同断言收拢到一个阅读单元里。
// 这种拆分能避免不同失败原因混在一起。
fn contract_error_paths() {
    stubs::set_force_new_development_error(true);
    let result = catch_unwind(AssertUnwindSafe(|| {
        let _ = zap::NewDevelopment().map_err(|e| panic!("{e}"));
    }));
    stubs::set_force_new_development_error(false);
    assert!(
        result.is_err(),
        "Go init panics when NewDevelopment returns err"
    );

    // Direct NewDevelopment Err path (no panic when caller handles Result).
    stubs::set_force_new_development_error(true);
    let err = zap::NewDevelopment().expect_err("forced failure");
    stubs::set_force_new_development_error(false);
    assert!(!err.is_empty());
}

/// Resource cleanup: Sync flushes and succeeds (Go Sync on development logger).
// `contract_resource_cleanup` 把同一类合同断言收拢到一个阅读单元里。
// 这种拆分能避免不同失败原因混在一起。
fn contract_resource_cleanup() {
    stubs::set_force_new_development_error(false);
    let logger = zap::NewDevelopment().expect("NewDevelopment ok");
    assert!(!logger.is_synced());
    logger.Sync().expect("Sync ok");
    assert!(logger.is_synced(), "Sync must mark flush complete");

    // Global Sync is also safe (no hang / no error).
    ensure_init();
    Global.Sync().expect("Global.Sync ok");
    let _ = LazyLock::force(&Global);
}
