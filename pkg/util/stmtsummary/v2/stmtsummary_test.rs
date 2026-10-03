// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 语句摘要窗口与持久化行为测试（对应 Go `stmtsummary_test.go`）。
//
// 覆盖 LRU 驱逐、刷新轮转、驱逐日志落盘、按用户分组，以及默认配置。

use std::fs;
use std::thread;
use std::time::{Duration, Instant};
use task_stmtsummary_v2::*;

#[test]
fn go_merge_38_internal_cleanup_keeps_mixed_record_and_capacity() {
    let summary = NewStmtSummary4Test(6);
    summary.SetEnableInternalQuery(true).unwrap();
    for digest in ["digest_0", "digest_1", "digest_2", "digest_3"] {
        summary.Add(&GenerateStmtExecInfo4Test(digest));
    }
    let mut pure = GenerateStmtExecInfo4Test("pure_internal_digest");
    pure.IsInternal = true;
    summary.Add(&pure);
    let mut mixed = GenerateStmtExecInfo4Test("mixed_digest");
    mixed.IsInternal = true;
    summary.Add(&mixed);
    summary.Add(&GenerateStmtExecInfo4Test("mixed_digest"));
    for digest in ["digest_0", "digest_1"] {
        summary.Add(&GenerateStmtExecInfo4Test(digest));
    }
    let digests = |summary: &StmtSummary| {
        summary
            .currentWindowSnapshot()
            .unwrap()
            .records
            .into_iter()
            .map(|record| record.Digest)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        digests(&summary),
        [
            "digest_1",
            "digest_0",
            "mixed_digest",
            "pure_internal_digest",
            "digest_3",
            "digest_2"
        ]
    );
    summary.SetEnableInternalQuery(false).unwrap();
    assert_eq!(
        digests(&summary),
        [
            "digest_1",
            "digest_0",
            "mixed_digest",
            "digest_3",
            "digest_2"
        ]
    );
    assert_eq!(summary.Len(), 5);
    for digest in ["new_0", "new_1", "new_2"] {
        summary.Add(&GenerateStmtExecInfo4Test(digest));
    }
    assert_eq!(summary.Len(), 6);
    assert_eq!(summary.EvictedCount(), 2);
    assert_eq!(
        summary
            .currentWindowSnapshot()
            .unwrap()
            .evicted
            .unwrap()
            .ExecCount,
        2
    );
    assert_eq!(
        digests(&summary),
        [
            "new_2",
            "new_1",
            "new_0",
            "digest_1",
            "digest_0",
            "mixed_digest"
        ]
    );
    summary.Close();
}

#[test]
fn go_merge_38_evicted_and_cleanup_are_safe_during_updates() {
    let summary = NewStmtSummary4Test(2);
    summary.SetEnableInternalQuery(true).unwrap();
    let reader = std::sync::Arc::clone(&summary);
    let inspector = thread::spawn(move || {
        for _ in 0..200 {
            let _ = reader.Evicted();
            reader.ClearInternal();
        }
    });
    for i in 0..200 {
        let mut info = GenerateStmtExecInfo4Test(&format!("digest_{i}"));
        info.IsInternal = i % 2 == 0;
        summary.Add(&info);
    }
    inspector.join().unwrap();
    assert!(summary.Len() <= 2);
    summary.Close();
}

#[test]
fn go_merge_38_internal_cleanup_waits_for_record_update() {
    let summary = NewStmtSummary4Test(1);
    let mut internal = GenerateStmtExecInfo4Test("internal_digest");
    internal.IsInternal = true;
    summary.Add(&internal);
    let record = summary.recordForTest("internal_digest").unwrap();
    let guard = record.lock();
    let (sender, receiver) = std::sync::mpsc::channel();
    let worker = std::thread::spawn({
        let summary = std::sync::Arc::clone(&summary);
        move || {
            summary.ClearInternal();
            sender.send(()).unwrap();
        }
    });
    assert!(receiver.recv_timeout(Duration::from_millis(100)).is_err());
    drop(guard);
    receiver.recv_timeout(Duration::from_secs(1)).unwrap();
    worker.join().unwrap();
    assert_eq!(summary.Len(), 0);
    summary.Close();
}

#[test]
fn go_merge_38_new_summary_surfaces_logger_open_error() {
    let directory = tempfile::tempdir().unwrap();
    let error = NewStmtSummary(&Config {
        Filename: directory.path().display().to_string(),
        ..Default::default()
    })
    .err()
    .expect("opening a directory as the log must fail");
    assert!(!error.is_empty());
}

#[test]
fn go_merge_38_failed_setup_keeps_previous_instance_and_falls_back() {
    Close();
    let directory = tempfile::tempdir().unwrap();
    let valid = directory.path().join("statements.log");
    Setup(&Config {
        Filename: valid.display().to_string(),
        ..Default::default()
    })
    .unwrap();
    let previous = crate::stmtsummary::installedForTest().unwrap();
    let error = Setup(&Config {
        Filename: directory.path().display().to_string(),
        ..Default::default()
    })
    .unwrap_err();
    assert!(error.contains("falling back to v1"));
    let current = crate::stmtsummary::installedForTest().unwrap();
    assert!(std::sync::Arc::ptr_eq(&previous, &current));
    assert!(!previous.IsClosed());
    assert_eq!(
        Enabled(),
        task_stmtsummary::StmtSummaryByDigestMap
            .lock()
            .unwrap()
            .Enabled()
    );
    Add(&GenerateStmtExecInfo4Test("setup_fallback"));
    Close();
}

#[test]
fn go_merge_38_evicted_is_safe_during_rotation() {
    let summary = NewStmtSummary4Test(2);
    summary.SetRefreshInterval(1).unwrap();
    for digest in ["digest_1", "digest_2", "digest_3"] {
        summary.Add(&GenerateStmtExecInfo4Test(digest));
    }
    let reader = std::sync::Arc::clone(&summary);
    let worker = thread::spawn(move || {
        for _ in 0..100 {
            let _ = reader.Evicted();
        }
    });
    for i in 0..50 {
        summary.rotateForTest();
        for digest in [
            format!("new_{i}_1"),
            format!("new_{i}_2"),
            format!("new_{i}_3"),
        ] {
            summary.Add(&GenerateStmtExecInfo4Test(digest));
        }
    }
    worker.join().unwrap();
    summary.Close();
}

#[test]
fn go_merge_37_setup_failure_reports_fallback_and_keeps_v1_available() {
    Close();
    let dir = tempfile::tempdir().unwrap();
    let error = Setup(&Config {
        Filename: dir
            .path()
            .join("missing/statement.log")
            .display()
            .to_string(),
        ..Default::default()
    })
    .unwrap_err();
    assert!(error.contains("falling back to v1"), "{error}");
    assert_eq!(
        Enabled(),
        task_stmtsummary::StmtSummaryByDigestMap
            .lock()
            .unwrap()
            .Enabled()
    );
}

/// 在超时内轮询谓词，超时则断言失败（对应 Go 侧 Eventually 风格等待）。
fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    assert!(predicate(), "condition was not met within {timeout:?}");
}

/// 按行解析 statements.log；跳过尚未写完整的尾部半行。
fn read_records(path: &std::path::Path) -> Vec<serde_json::Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            // Async log writes can expose a trailing partial line; skip until the
            // next wait_until poll sees a complete JSON object (Go uses a locked
            // in-memory mock instead of reading the live file).
            serde_json::from_str(line).ok()
        })
        .collect()
}

/// 创建临时目录与基于文件存储的 `StmtSummary`，并设置最大语句条数。
fn file_summary(max_count: u32) -> (tempfile::TempDir, std::sync::Arc<StmtSummary>) {
    let dir = tempfile::tempdir().unwrap();
    let summary = NewStmtSummary(&Config {
        Filename: dir.path().join("statements.log").display().to_string(),
        ..Default::default()
    })
    .unwrap();
    summary.SetMaxStmtCount(max_count).unwrap();
    (dir, summary)
}

/// 窗口容量满后触发 LRU 驱逐，Clear 后计数归零。
#[test]
fn test_stmt_window() {
    let summary = NewStmtSummary4Test(5);
    for digest in [
        "digest1", "digest1", "digest2", "digest2", "digest3", "digest4", "digest5", "digest6",
        "digest7",
    ] {
        summary.Add(&GenerateStmtExecInfo4Test(digest));
    }
    assert_eq!(summary.Len(), 5);
    assert_eq!(summary.EvictedCount(), 2);
    let evicted = summary.Evicted().unwrap();
    assert_eq!(evicted.len(), 3);
    assert_eq!(evicted[2].GetInt64(), 2);
    serde_json::to_vec(&evicted[2].GetInt64()).unwrap();

    summary.Clear();
    assert_eq!(summary.Len(), 0);
    assert_eq!(summary.EvictedCount(), 0);
    assert!(summary.Evicted().is_none());
    summary.Close();
}

/// 刷新间隔到期后窗口轮转清空，新窗口重新累计。
#[test]
fn test_stmt_summary() {
    let (_dir, summary) = file_summary(3);
    summary.SetRefreshInterval(1).unwrap();
    for digest in ["digest1", "digest2", "digest3", "digest4", "digest5"] {
        summary.Add(&GenerateStmtExecInfo4Test(digest));
    }
    assert_eq!(summary.Len(), 3);
    assert_eq!(summary.EvictedCount(), 2);

    // 等待 rotateLoop 按 RefreshInterval 轮转并清空当前窗口。
    wait_until(Duration::from_secs(4), || {
        summary.Len() == 0 && summary.EvictedCount() == 0
    });
    summary.Add(&GenerateStmtExecInfo4Test("digest6"));
    summary.Add(&GenerateStmtExecInfo4Test("digest7"));
    assert_eq!(summary.Len(), 2);
    assert_eq!(summary.EvictedCount(), 0);
    summary.Clear();
    assert_eq!(summary.Len(), 0);
    summary.Close();
}

/// PersistEvicted 开启时，被 LRU 挤出的记录应异步写入日志并带 begin/end。
#[test]
fn test_stmt_summary_persist_evicted() {
    let (dir, summary) = file_summary(2);
    let path = dir.path().join("statements.log");
    summary.SetPersistEvicted(true).unwrap();
    for digest in ["digest1", "digest2", "digest3", "digest4"] {
        summary.Add(&GenerateStmtExecInfo4Test(digest));
    }
    wait_until(Duration::from_secs(2), || {
        read_records(&path)
            .iter()
            .filter(|record| record["evicted"] == true)
            .count()
            == 2
    });
    let evicted: Vec<_> = read_records(&path)
        .into_iter()
        .filter(|record| record["evicted"] == true)
        .collect();
    let mut digests: Vec<_> = evicted
        .iter()
        .map(|record| record["digest"].as_str().unwrap().to_owned())
        .collect();
    digests.sort();
    assert_eq!(digests, ["digest1", "digest2"]);
    assert!(evicted.iter().all(|record| {
        record["begin"].as_i64().unwrap() > 0
            && record["end"].as_i64().unwrap() >= record["begin"].as_i64().unwrap()
    }));

    // 关闭持久化后新增驱逐不应再追加 evicted 行。
    summary.SetPersistEvicted(false).unwrap();
    summary.Add(&GenerateStmtExecInfo4Test("digest5"));
    thread::sleep(Duration::from_millis(250));
    assert_eq!(
        read_records(&path)
            .iter()
            .filter(|record| record["evicted"] == true)
            .count(),
        2
    );
    summary.Close();
}

/// 已入队写日志的驱逐记录不应再并入窗口结束时的聚合驱逐行，exec_count 总和保持 4。
#[test]
fn test_stmt_summary_persist_evicted_does_not_persist_logged_records_as_aggregate() {
    let (dir, summary) = file_summary(2);
    let path = dir.path().join("statements.log");
    summary.SetPersistEvicted(true).unwrap();
    for digest in ["digest1", "digest2", "digest3", "digest4"] {
        summary.Add(&GenerateStmtExecInfo4Test(digest));
    }
    wait_until(Duration::from_secs(2), || {
        read_records(&path)
            .iter()
            .filter(|record| record["evicted"] == true)
            .count()
            == 2
    });
    summary.Close();

    let records = read_records(&path);
    let total_exec_count: i64 = records
        .iter()
        .map(|record| record["exec_count"].as_i64().unwrap())
        .sum();
    let mut evicted_digests: Vec<_> = records
        .iter()
        .filter(|record| record["evicted"] == true)
        .map(|record| record["digest"].as_str().unwrap().to_owned())
        .collect();
    evicted_digests.sort();
    assert_eq!(evicted_digests, ["digest1", "digest2"]);
    assert!(
        records
            .iter()
            .filter(|record| record["evicted"] == false)
            .all(|record| !record["digest"].as_str().unwrap_or_default().is_empty())
    );
    assert_eq!(total_exec_count, 4);
}

/// GroupByUser 开启后同 digest 不同用户分键；关闭后重新合并。
#[test]
fn test_stmt_summary_group_by_user() {
    let summary = NewStmtSummary4Test(100);
    let mut alice = GenerateStmtExecInfo4Test("digest1");
    alice.User = "alice".into();
    let mut bob = GenerateStmtExecInfo4Test("digest1");
    bob.User = "bob".into();
    summary.Add(&alice);
    summary.Add(&bob);
    assert_eq!(summary.Len(), 1);

    // 切换分组策略会清空当前窗口。
    summary.SetGroupByUser(true).unwrap();
    assert_eq!(summary.Len(), 0);
    summary.Add(&alice);
    summary.Add(&bob);
    summary.Add(&alice);
    assert_eq!(summary.Len(), 2);

    let mut merged = NewStmtRecord(&alice);
    merged.Add(&alice);
    merged.Add(&bob);
    assert_eq!(merged.ExecCount, 2);
    assert_eq!(merged.AuthUsers.len(), 2);

    summary.SetGroupByUser(false).unwrap();
    summary.Add(&alice);
    summary.Add(&bob);
    assert_eq!(summary.Len(), 1);
    summary.Close();
}

/// 窗口轮转后 EvictedCount 归零，新窗口独立累计驱逐次数。
#[test]
fn test_window_evicted_count_reset_on_rotate() {
    let (_dir, summary) = file_summary(2);
    summary.SetRefreshInterval(1).unwrap();
    for digest in ["digest1", "digest2", "digest3", "digest4"] {
        summary.Add(&GenerateStmtExecInfo4Test(digest));
    }
    assert_eq!(summary.Len(), 2);
    assert_eq!(summary.EvictedCount(), 2);

    wait_until(Duration::from_secs(4), || {
        summary.Len() == 0 && summary.EvictedCount() == 0
    });
    for digest in ["digest5", "digest6", "digest7"] {
        summary.Add(&GenerateStmtExecInfo4Test(digest));
    }
    assert_eq!(summary.Len(), 2);
    assert_eq!(summary.EvictedCount(), 1);
    summary.Close();
}

/// 多次轮转后 Close 应把各窗口记录刷到文件，exec_count 总和为 9。
#[test]
fn test_stmt_summary_flush() {
    let (dir, summary) = file_summary(1000);
    let path = dir.path().join("statements.log");
    summary.SetRefreshInterval(1).unwrap();
    for _ in 0..2 {
        for digest in ["digest1", "digest2", "digest3"] {
            summary.Add(&GenerateStmtExecInfo4Test(digest));
        }
        wait_until(Duration::from_secs(4), || summary.Len() == 0);
    }
    for digest in ["digest1", "digest2", "digest3"] {
        summary.Add(&GenerateStmtExecInfo4Test(digest));
    }
    summary.Close();
    let records = read_records(&path);
    assert_eq!(records.len(), 9);
    assert_eq!(
        records
            .iter()
            .map(|record| record["exec_count"].as_i64().unwrap())
            .sum::<i64>(),
        9
    );
    assert!(
        records.iter().all(|record| {
            record["NormalizedSQL"] == "normalized_sql"
                && !record["SumLatency"].is_null()
                && record["AuthUsers"].is_array()
        }),
        "persisted record omitted statement summary fields: {:?}",
        records.first()
    );
}

/// 默认刷新间隔应为 30 分钟（1800 秒）。
#[test]
fn test_default_config() {
    let dir = tempfile::tempdir().unwrap();
    let summary = NewStmtSummary(&Config {
        Filename: dir.path().join("test.log").display().to_string(),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(summary.RefreshInterval(), 1800);
    summary.Close();
}
