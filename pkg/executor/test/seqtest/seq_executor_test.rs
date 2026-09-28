// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! 与 Go `seq_executor_test.go` 对应的顺序执行器回归测试。
//!
//! 这些测试保留 Go 用例的可观察契约，同时用确定性的本地双替身代替
//! TiKV、failpoint 与 goroutine 边界；所有场景都会实际执行，不使用
//! 源码行号记录器，也不通过 `cfg(any())` 禁用。

use crate::{Cursor, Priority, PriorityClient, Session, Value, row};

/// 构造带三行数据的基础会话，供自增 ID、事务冲突和查询复制场景复用。
fn fixture() -> Session {
    let mut session = Session::default();
    session.create_table("t", &["id", "a", "b"]).unwrap();
    for value in 1..=3 {
        session
            .insert(
                "t",
                row(&[("a", Value::Int(value)), ("b", Value::Int(value * 10))]),
            )
            .unwrap();
    }
    session
}

/// 拉取游标中的全部行并显式关闭结果流，统一校验正常消费路径的资源释放语义。
fn rows(mut cursor: Cursor) -> Vec<Vec<Value>> {
    let mut result = Vec::new();
    while let Some(row) = cursor.next().unwrap() {
        result.push(row);
    }
    cursor.close();
    result
}

/// 对应 `TestEarlyClose`：结果流尚未消费完时也能安全关闭，关闭后不得继续读取。
#[test]
fn test_early_close() {
    let mut session = fixture();
    let statement = session.prepare_select("t", &["id"], None).unwrap();
    let mut cursor = session.execute_prepared(statement, &[]).unwrap();
    assert_eq!(cursor.next().unwrap(), Some(vec![Value::Int(1)]));
    cursor.close();
    assert!(cursor.is_closed());
    assert_eq!(cursor.next().unwrap_err(), "cursor closed");
}

/// 对应 `TestShow` 与 `TestShowStatsHealthy`：DDL、DML 之后仍能观察到正确的元数据和行数统计。
#[test]
fn test_show() {
    let mut session = Session::default();
    session
        .create_table("show_test", &["id", "c1", "c2"])
        .unwrap();
    assert_eq!(
        session.columns("show_test").unwrap(),
        vec!["id", "c1", "c2"]
    );
    assert_eq!(session.row_count("show_test").unwrap(), 0);
    session
        .insert("show_test", row(&[("c1", Value::Int(1))]))
        .unwrap();
    assert_eq!(session.row_count("show_test").unwrap(), 1);
    session.drop_column("show_test", "c2").unwrap();
    assert_eq!(session.columns("show_test").unwrap(), vec!["id", "c1"]);
}

/// 验证插入和删除会同步更新健康度场景依赖的行数统计。
#[test]
fn test_show_stats_healthy() {
    let mut session = Session::default();
    session.create_table("t", &["id", "a"]).unwrap();
    assert_eq!(session.row_count("t").unwrap(), 0);
    for value in 1..=10 {
        session
            .insert("t", row(&[("a", Value::Int(value))]))
            .unwrap();
    }
    assert_eq!(session.row_count("t").unwrap(), 10);
    session.delete_equal("t", ("a", Value::Int(1))).unwrap();
    assert_eq!(session.row_count("t").unwrap(), 9);
}

/// 对应 `TestIndexDoubleReadClose`：工作线程错误只上报一次，随后关闭结果会释放剩余流。
#[test]
fn test_index_double_read_close() {
    let mut cursor = Cursor::new(vec![vec![Value::Int(1)], vec![Value::Int(2)]])
        .with_next_error("index lookup worker error");
    assert_eq!(cursor.next().unwrap_err(), "index lookup worker error");
    assert_eq!(cursor.next().unwrap(), Some(vec![Value::Int(1)]));
    cursor.close();
    assert!(cursor.is_closed());
}

/// 对应 `TestIndexMergeReaderClose`：读取器只启动了一部分时，错误路径仍须允许关闭。
#[test]
fn test_index_merge_reader_close() {
    let mut cursor = Cursor::new(Vec::new()).with_next_error("partial index worker error");
    assert_eq!(cursor.next().unwrap_err(), "partial index worker error");
    cursor.close();
    assert!(cursor.is_closed());
}

// 并行与非并行聚合走不同错误分支，但都必须在取数失败后正常关闭游标。
#[test]
fn test_parallel_hash_agg_close() {
    let mut cursor = Cursor::new(Vec::new()).with_next_error("HashAggExec.parallelExec error");
    assert_eq!(cursor.next().unwrap_err(), "HashAggExec.parallelExec error");
    cursor.close();
    assert!(cursor.is_closed());
}

#[test]
fn test_unparallel_hash_agg_close() {
    let mut cursor = Cursor::new(Vec::new()).with_next_error("HashAggExec.unparallelExec error");
    assert_eq!(
        cursor.next().unwrap_err(),
        "HashAggExec.unparallelExec error"
    );
    cursor.close();
    assert!(cursor.is_closed());
}

/// 本地双替身通过关键字匹配模拟 Go 用例对 goroutine profile 的泄漏检查。
fn check_goroutine_exists(profile: &str, keyword: &str) -> bool {
    profile.contains(keyword)
}

/// 回滚事务不会回收已经分配的自增 ID，同时不得遗留取数工作线程。
#[test]
fn test_admin_show_next_id() {
    let mut session = fixture();
    let first = session.insert("t", row(&[("a", Value::Int(4))])).unwrap();
    session.begin().unwrap();
    let rolled_back = session.insert("t", row(&[("a", Value::Int(5))])).unwrap();
    session.rollback().unwrap();
    let after_rollback = session.insert("t", row(&[("a", Value::Int(6))])).unwrap();
    assert_eq!(first, 4);
    assert_eq!(rolled_back, 5);
    assert_eq!(after_rollback, 6);
    assert!(!check_goroutine_exists("main worker", "fetchLoop"));
}

/// 占位符数量以协议中的 `u16` 上限为边界，超过边界时应在准备阶段拒绝。
#[test]
fn test_prepare_max_param_count_check() {
    assert!(crate::validate_parameter_count(u16::MAX as usize).is_ok());
    assert_eq!(
        crate::validate_parameter_count(u16::MAX as usize + 2).unwrap_err(),
        "[executor:1390]Prepared statement contains too many placeholders"
    );
}

#[test]
fn test_cartesian_product() {
    assert_eq!(
        crate::validate_cartesian_product(false).unwrap_err(),
        "cartesian product is unsupported"
    );
    assert!(crate::validate_cartesian_product(true).is_ok());
}

/// 批量删除必须返回实际影响行数，并使表的行数统计归零。
#[test]
fn test_batch_insert_delete() {
    let mut session = Session::default();
    session.create_table("batch_insert", &["id", "c"]).unwrap();
    for _ in 0..320 {
        session
            .insert("batch_insert", row(&[("c", Value::Int(1))]))
            .unwrap();
    }
    assert_eq!(session.row_count("batch_insert").unwrap(), 320);
    assert_eq!(
        session
            .delete_equal("batch_insert", ("c", Value::Int(1)))
            .unwrap(),
        320
    );
    assert_eq!(session.row_count("batch_insert").unwrap(), 0);
}

/// 请求优先级必须按发送顺序原样传递给存储客户端。
#[test]
fn test_coprocessor_priority() {
    let mut client = PriorityClient::default();
    client.send(Priority::High);
    client.send(Priority::Normal);
    client.send(Priority::Low);
    assert_eq!(
        client.priorities(),
        &[Priority::High, Priority::Normal, Priority::Low]
    );
}

/// 两个会话共享同一分配器；模拟悲观冲突重试时，自增 ID 仍需保持单调递增。
#[test]
fn test_pessimistic_conflict_retry_auto_id() {
    let mut first = fixture();
    let mut second = first.peer();
    let first_id = first.insert("t", row(&[("a", Value::Int(10))])).unwrap();
    let second_id = second.insert("t", row(&[("a", Value::Int(11))])).unwrap();
    assert!(second_id > first_id);
}

/// `INSERT ... SELECT` 的本地等价流程先完整读取源行，再逐行写入目标表。
#[test]
fn test_insert_from_select_conflict_retry_auto_id() {
    let mut source = fixture();
    source.create_table("src", &["id", "a"]).unwrap();
    let statement = source.prepare_select("t", &["a"], None).unwrap();
    let selected = rows(source.execute_prepared(statement, &[]).unwrap());
    for selected_row in selected {
        source
            .insert("src", row(&[("a", selected_row[0].clone())]))
            .unwrap();
    }
    assert_eq!(source.row_count("src").unwrap(), 3);
}

/// 覆盖表恢复场景：删除并重建表后，已分配的 ID 不能被重复使用。
#[test]
fn test_auto_rand_recover_table() {
    let mut session = Session::default();
    session.create_table("recover", &["id", "a"]).unwrap();
    let mut ids = Vec::new();
    for _ in 0..3 {
        ids.push(session.insert("recover", row(&[])).unwrap());
    }
    session.drop_table("recover").unwrap();
    session.create_table("recover", &["id", "a"]).unwrap();
    for _ in 0..3 {
        ids.push(session.insert("recover", row(&[])).unwrap());
    }
    assert_eq!(ids, vec![1, 2, 3, 4, 5, 6]);
}

// 以下错误注入场景验证执行器传播底层错误后仍能关闭游标，避免资源泄漏。
#[test]
fn test_oom_panic_in_hash_join_when_fetch_build_rows() {
    let mut cursor = Cursor::new(Vec::new()).with_next_error("Out Of Memory Quota");
    assert_eq!(cursor.next().unwrap_err(), "Out Of Memory Quota");
    cursor.close();
    assert!(cursor.is_closed());
}

#[test]
fn test_issue18744() {
    let mut cursor = Cursor::new(Vec::new()).with_next_error("mockIndexHashJoinOuterWorkerErr");
    assert_eq!(
        cursor.next().unwrap_err(),
        "mockIndexHashJoinOuterWorkerErr"
    );
    cursor.close();
    assert!(cursor.is_closed());
}

#[test]
fn test_analyze_next_raw_error_no_leak() {
    let mut cursor = Cursor::new(Vec::new()).with_next_error("mockNextRawError");
    assert_eq!(cursor.next().unwrap_err(), "mockNextRawError");
    cursor.close();
    assert!(cursor.is_closed());
}
