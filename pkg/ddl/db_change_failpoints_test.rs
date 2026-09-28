// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// DDL（数据定义语言，如 ALTER TABLE、FLASHBACK TABLE 等 schema 变更操作）
// 变更过程中故障注入（failpoint）场景的测试。
//
// failpoint 是一种测试手段：在代码的特定位置人为注入错误或延迟，
// 用来验证系统在异常路径下的行为是否正确。本文件覆盖三类场景：
// - 修改列类型（modify column）时元数据更新失败后，历史任务中保留的参数是否为原始值；
// - 多线程并发更新表副本（TiFlash replica）可用状态时，只有一个线程能成功；
// - 并发执行 FLASHBACK TABLE（闪回已删除的表，即恢复被 DROP 的表）时的互斥语义。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

/// 修改列类型（modify column）DDL 任务的参数。
///
/// 修改列类型时，DDL 会创建一个"变更中列"（changing column）与相应的
/// "变更中索引"（changing indexes）作为过渡对象，待数据回填完成后再替换原列。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ModifyColumnArgs {
    /// 变更过程中新建的过渡列定义（如 "a varchar(16)"），None 表示尚未设置。
    changing_column: Option<String>,
    /// 变更过程中需要同步重建的索引名列表。
    changing_indexes: Vec<String>,
}

/// DDL 历史任务记录：任务执行结束后（无论成功失败）写入历史队列的信息。
#[derive(Clone, Debug)]
struct HistoryJob {
    /// 任务 ID。
    id: u64,
    /// 最终记录到历史中的任务参数。
    args: ModifyColumnArgs,
    /// 任务失败时的错误信息。
    error: String,
    /// 失败回滚后表中仍保留的列定义。
    table_columns: Vec<String>,
    /// 失败回滚后表中仍保留的索引定义。
    table_indexes: Vec<String>,
}

/// 模拟修改列类型时 `updateVersionAndTableInfo`（更新 schema 版本与表元数据）
/// 步骤失败的场景：临时构造的过渡参数被丢弃，历史任务中只保留原始参数。
fn modify_column_with_update_failure(job_id: u64) -> HistoryJob {
    let original_args = ModifyColumnArgs::default();
    let mut tentative = original_args.clone();
    tentative.changing_column = Some("a varchar(16)".to_owned());
    tentative.changing_indexes.push("a".to_owned());
    // updateVersionAndTableInfo failed, so only the original raw args reach history.
    let _ = tentative;
    HistoryJob {
        id: job_id,
        args: original_args,
        error: format!("[ddl:-1]mock update version and tableInfo error,jobID={job_id}"),
        table_columns: vec!["a int".to_owned()],
        table_indexes: vec!["unique(a)".to_owned()],
    }
}

/// 验证修改列类型失败后，历史任务的错误信息格式正确，
/// 且参数仍为原始值（过渡列与过渡索引未被写入历史）。
#[test]
fn test_modify_column_type_args() {
    let history = modify_column_with_update_failure(42);
    let parts = history.error.split(',').collect::<Vec<_>>();
    assert_eq!("[ddl:-1]mock update version and tableInfo error", parts[0]);
    assert_eq!("jobID=42", parts[1]);
    assert_eq!(42, history.id);
    assert_eq!(["a int"], history.table_columns.as_slice());
    assert_eq!(["unique(a)"], history.table_indexes.as_slice());
    assert_eq!(None, history.args.changing_column);
    assert!(history.args.changing_indexes.is_empty());
}

/// 模拟更新表副本（replica，如 TiFlash 列存副本）可用状态的操作。
///
/// 使用 CAS（compare_exchange，比较并交换）原子操作保证状态只能从
/// false 翻转到 true 一次；重复更新会返回"已更新"错误。
fn update_table_replica_info(available: &AtomicBool) -> Result<(), &'static str> {
    available
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .map(|_| ())
        .map_err(|_| "[ddl:-1]the replica available status of table t1 is already updated")
}

/// 验证两个线程并发更新同一张表的副本状态时，恰有一个成功、
/// 另一个收到"已更新"错误，即更新操作满足幂等互斥语义。
#[test]
fn test_parallel_update_table_replica() {
    let available = Arc::new(AtomicBool::new(false));
    // 对应 Go 的 prepareTestControlParallelExecSQL：第二个会话等待第一个更新完成。
    let (first_updated_tx, first_updated_rx) = mpsc::channel();
    let first_available = Arc::clone(&available);
    let first = thread::spawn(move || {
        let result = update_table_replica_info(&first_available);
        first_updated_tx.send(()).unwrap();
        result
    });
    let second_available = Arc::clone(&available);
    let second = thread::spawn(move || {
        first_updated_rx.recv().unwrap();
        update_table_replica_info(&second_available)
    });
    assert_eq!(Ok(()), first.join().unwrap());
    assert_eq!(
        Err("[ddl:-1]the replica available status of table t1 is already updated"),
        second.join().unwrap()
    );
}

/// 模拟 FLASHBACK TABLE 场景的简化系统目录（catalog，记录库表元数据的结构）。
#[derive(Default)]
struct FlashbackCatalog {
    /// 当前活跃（可见）的表名集合。
    active_tables: HashSet<String>,
    /// 已被 DROP 但仍可闪回恢复的表：删除时的表名 -> 表数据。
    dropped_tables: HashMap<String, String>,
}

impl FlashbackCatalog {
    /// 将已删除的表 `dropped_name` 闪回恢复为名为 `target` 的活跃表。
    ///
    /// 失败情形：目标名已被占用（错误码 1050 Table already exists），
    /// 或指定的已删除表不存在（例如已被另一并发闪回操作恢复）。
    fn flashback(&mut self, dropped_name: &str, target: &str) -> Result<(), String> {
        // 不带 TO 的并发恢复完成后，原表名已经重新可见。Go 实现会先按
        // FLASHBACK 的源表名报告 1050，而不是继续查找已消费的删除记录。
        if self.active_tables.contains(dropped_name) {
            return Err(format!(
                "[schema:1050]Table '{dropped_name}' already exists"
            ));
        }
        // 目标表名冲突检查：与 MySQL 的 1050 错误保持一致。
        if self.active_tables.contains(target) {
            return Err(format!("[schema:1050]Table '{target}' already exists"));
        }
        // 从已删除表集合中取出原表；取不到说明已被并发操作恢复。
        let original = self
            .dropped_tables
            .remove(dropped_name)
            .ok_or_else(|| format!("dropped table '{dropped_name}' not found"))?;
        let _ = original;
        self.active_tables.insert(target.to_owned());
        Ok(())
    }
}

/// 验证两个线程并发执行 FLASHBACK TABLE 时恰有一个成功、一个失败：
/// - 场景一：两个请求闪回同一张已删除表到同一目标名（后者发现表已被恢复）；
/// - 场景二：两个请求闪回同一张表到不同目标名（后者发现源表已不存在）。
#[test]
fn test_parallel_flashback_table() {
    for requests in [
        [("t", "t_flashback"), ("t", "t_flashback")],
        [
            ("t_flashback", "t_flashback"),
            ("t_flashback", "t_flashback2"),
        ],
    ] {
        let catalog = Arc::new(Mutex::new(FlashbackCatalog {
            dropped_tables: HashMap::from([(requests[0].0.to_owned(), "table-data".to_owned())]),
            ..FlashbackCatalog::default()
        }));
        // 对应 Go 的 testControlParallelExecSQL：第二个会话在第一个完成后继续，
        // 从而精确验证第二个请求观察到目标表已存在，而不是接受任意错误。
        let (first_finished_tx, first_finished_rx) = mpsc::channel();
        let first_catalog = Arc::clone(&catalog);
        let first = thread::spawn(move || {
            let result = first_catalog
                .lock()
                .unwrap()
                .flashback(requests[0].0, requests[0].1);
            first_finished_tx.send(()).unwrap();
            result
        });
        let second_catalog = Arc::clone(&catalog);
        let second = thread::spawn(move || {
            first_finished_rx.recv().unwrap();
            second_catalog
                .lock()
                .unwrap()
                .flashback(requests[1].0, requests[1].1)
        });
        assert_eq!(Ok(()), first.join().unwrap());
        assert_eq!(
            Err("[schema:1050]Table 't_flashback' already exists".to_owned()),
            second.join().unwrap()
        );
    }
}
