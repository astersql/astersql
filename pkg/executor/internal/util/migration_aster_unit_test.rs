// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0

// `executor/internal/util` 迁移期单元测试。
//
// 覆盖 `UpdateExecutorTableID` 对各扫描变体、递归/非递归路径、Join 与未知协议的行为，
// 以及随机串字母表、调用方函数名探测与 spill 临时文件泄漏检查。

use std::collections::HashSet;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};

use astersql_executor_internal_util::{
    CheckNoLeakFiles, ExecType, Executor, GenerateRandomString, GetFunctionName, LETTER_BYTES,
    UpdateExecutorTableID, UpdateExecutorTableIDContext,
};

/// 验证 TableScan / PartitionTableScan / IndexScan 的 ID 改写及 recording 上下文。
#[test]
fn update_executor_table_id_handles_scan_variants_and_context() {
    let mut visited = HashSet::new();
    let mut table_scan = Executor::table_scan(7);
    // TableScan：取 partition_ids[0]，并记入 visited。
    UpdateExecutorTableID(
        UpdateExecutorTableIDContext::recording(&mut visited),
        Some(&mut table_scan),
        false,
        &[42, 43],
    )
    .unwrap();
    assert_eq!(table_scan, Executor::table_scan(42));
    assert_eq!(visited, HashSet::from([42]));

    let mut partition_scan = Executor::partition_table_scan(vec![1]);
    // PartitionTableScan：整表替换为传入的分区 ID 列表。
    UpdateExecutorTableID(
        UpdateExecutorTableIDContext::default(),
        Some(&mut partition_scan),
        false,
        &[42, 43],
    )
    .unwrap();
    assert_eq!(partition_scan, Executor::partition_table_scan(vec![42, 43]));

    let mut index_scan = Executor::index_scan(9);
    UpdateExecutorTableID(
        UpdateExecutorTableIDContext::default(),
        Some(&mut index_scan),
        false,
        &[42],
    )
    .unwrap();
    assert_eq!(index_scan, Executor::index_scan(42));
}

/// 对齐 Go：recursive=true 下降一元子树；false 不改子节点；Join 只改非内表侧。
#[test]
fn update_executor_table_id_matches_recursive_go_paths() {
    let unary_types = [
        ExecType::Selection,
        ExecType::Aggregation,
        ExecType::StreamAgg,
        ExecType::TopN,
        ExecType::Limit,
        ExecType::ExchangeSender,
        ExecType::CteSink,
        ExecType::Projection,
        ExecType::Window,
        ExecType::Sort,
        ExecType::Expand,
        ExecType::Expand2,
    ];
    for tp in unary_types {
        let mut executor = Executor::unary(tp, Executor::table_scan(1));
        UpdateExecutorTableID(
            UpdateExecutorTableIDContext::default(),
            Some(&mut executor),
            true,
            &[88],
        )
        .unwrap();
        assert_eq!(executor.child().unwrap(), &Executor::table_scan(88));
    }

    let mut non_recursive = Executor::unary(ExecType::Limit, Executor::table_scan(2));
    // recursive=false：当前节点无扫描载荷，子节点保持原 ID。
    UpdateExecutorTableID(
        UpdateExecutorTableIDContext::default(),
        Some(&mut non_recursive),
        false,
        &[99],
    )
    .unwrap();
    assert_eq!(non_recursive.child().unwrap(), &Executor::table_scan(2));

    for inner_idx in 0..2 {
        let mut join = Executor::join(
            [Executor::table_scan(3), Executor::table_scan(4)],
            inner_idx,
        );
        UpdateExecutorTableID(
            UpdateExecutorTableIDContext::default(),
            Some(&mut join),
            true,
            &[66],
        )
        .unwrap();
        let children = join.join_children().unwrap();
        assert_eq!(
            children[inner_idx],
            &Executor::table_scan(3 + inner_idx as i64)
        );
        assert_eq!(children[1 - inner_idx], &Executor::table_scan(66));
    }
}

/// 覆盖 nil 执行器、无子节点终端算子与未知 tipb 协议错误。
#[test]
fn update_executor_table_id_covers_terminal_unknown_and_nil() {
    UpdateExecutorTableID(UpdateExecutorTableIDContext::default(), None, true, &[1]).unwrap();

    for tp in [ExecType::ExchangeReceiver, ExecType::CteSource] {
        let mut terminal = Executor::terminal(tp);
        UpdateExecutorTableID(
            UpdateExecutorTableIDContext::default(),
            Some(&mut terminal),
            true,
            &[1],
        )
        .unwrap();
    }

    let mut unknown = Executor::unknown(1234);
    let err = UpdateExecutorTableID(
        UpdateExecutorTableIDContext::default(),
        Some(&mut unknown),
        true,
        &[1],
    )
    .unwrap_err();
    assert_eq!(err.to_string(), "unknown new tipb protocol 1234");
}

/// 对齐 Go 的切片边界：普通扫描要求首个分区 ID，分区扫描允许空列表。
#[test]
fn update_executor_table_id_matches_go_partition_id_boundaries() {
    for mut scan in [Executor::table_scan(1), Executor::index_scan(1)] {
        let panic = catch_unwind(AssertUnwindSafe(|| {
            let _ = UpdateExecutorTableID(
                UpdateExecutorTableIDContext::default(),
                Some(&mut scan),
                false,
                &[],
            );
        }));
        assert!(panic.is_err());
    }

    let mut partition_scan = Executor::partition_table_scan(vec![1]);
    UpdateExecutorTableID(
        UpdateExecutorTableIDContext::default(),
        Some(&mut partition_scan),
        false,
        &[],
    )
    .unwrap();
    assert_eq!(partition_scan, Executor::partition_table_scan(vec![]));
}

/// 随机串长度与 Go `LETTER_BYTES` 字母表一致。
#[test]
fn random_string_has_requested_length_and_go_alphabet() {
    assert_eq!(
        LETTER_BYTES,
        "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789"
    );
    assert_eq!(GenerateRandomString(0), "");
    let value = GenerateRandomString(512);
    assert_eq!(value.len(), 512);
    assert!(
        value
            .bytes()
            .all(|byte| LETTER_BYTES.as_bytes().contains(&byte))
    );
}

/// 供 `GetFunctionName` 探测调用栈时识别的探针函数。
fn function_name_probe() -> String {
    GetFunctionName()
}

/// 验证 `GetFunctionName` 返回其直接调用方名称。
#[test]
fn function_name_reports_its_caller() {
    assert!(function_name_probe().contains("function_name_probe"));
}

/// 泄漏检查递归遍历目录，仅匹配给定前缀的文件名。
#[test]
fn leak_check_walks_recursively_and_matches_only_prefixes() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir(temp.path().join("nested")).unwrap();
    fs::create_dir(temp.path().join("nested/spill-directory")).unwrap();
    fs::write(temp.path().join("nested/other-file"), b"ok").unwrap();
    CheckNoLeakFiles(temp.path(), "spill-").unwrap();

    fs::write(temp.path().join("nested/spill-0001"), b"leak").unwrap();
    let err = CheckNoLeakFiles(temp.path(), "spill-").unwrap_err();
    assert!(err.path().ends_with("spill-0001"));
}

/// Go 的 `WalkDir` 对所有非目录项检查前缀，符号链接也必须按泄漏处理。
#[cfg(unix)]
#[test]
fn leak_check_matches_go_for_prefixed_symbolic_links() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("target"), b"ok").unwrap();
    symlink(temp.path().join("target"), temp.path().join("spill-link")).unwrap();

    let err = CheckNoLeakFiles(temp.path(), "spill-").unwrap_err();
    assert!(err.path().ends_with("spill-link"));
}
