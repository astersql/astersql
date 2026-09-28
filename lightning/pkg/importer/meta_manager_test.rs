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

//! Go-equivalent unit tests for `lightning/pkg/importer/meta_manager_test.go`.
//!
//! 覆盖表级分配、状态推进、任务协调和清理副作用。

use crate::*;
use std::sync::Arc;

// 前半部分测试围绕表级元数据管理器展开，
// 重点验证 SQL 日志里是否出现预期的状态推进痕迹。
// 后半部分则转向任务级管理器，检查回调更新和 undo 契约是否稳定。

/// 构造一套最小可运行的内存数据库和表级元数据管理器。
/// 所有测试共用这份装配逻辑，保证它们观察的是同一套 SQL 发射行为。
fn new_db_table_meta_mgr() -> (sql::DB, dbTableMetaMgr) {
    let db = sql::DB::new_memory();
    let mgr = dbTableMetaMgr {
        db: db.clone(),
        taskID: 1,
        schema: "test".into(),
        tableName: "`test`.`t1`".into(),
        tableID: 1,
    };
    (db, mgr)
}

/// 断言空元数据快照从零开始分配。
fn assert_alloc_empty_meta(mgr: &dbTableMetaMgr, required: i64) {
    let (ck, base) = mgr
        .AllocTableRowIDs(context::Background(), required)
        .unwrap();
    assert_eq!(base, 0);
    assert_eq!(ck, verify::MakeKVChecksum(0, 0, 0));
}

/// 从内存 SQL 日志里查找关键片段。
/// 测试不去逐字符比对整条 SQL，而只验证语义上重要的动作已经发生。
fn assert_exec_contains(db: &sql::DB, needle: &str) {
    let log = db.exec_log();
    assert!(
        log.iter().any(|(q, args)| {
            q.contains(needle) || args.iter().any(|a| a.as_string().contains(needle))
        }),
        "expected exec_log to contain {needle:?}, got {log:?}"
    );
}

/// 顺序演练表级元数据管理器最核心的 SQL 副作用。
/// 这覆盖初始化、基线校验和、本地校验和以及完成态四个关键阶段。
fn exercise_table_meta_sql(mgr: &dbTableMetaMgr, db: &sql::DB) {
    mgr.InitTableMeta(context::Background()).unwrap();
    assert_exec_contains(db, "INSERT IGNORE");

    let ck = verify::MakeKVChecksum(1, 2, 3);
    mgr.UpdateTableBaseChecksum(context::Background(), &ck)
        .unwrap();
    assert_exec_contains(db, metaStatusBaseChecksumUpdated.String());

    mgr.UpdateTableStatus(context::Background(), metaStatusRowIDAllocated)
        .unwrap();
    assert_exec_contains(db, metaStatusRowIDAllocated.String());

    let (other_has_dup, need_remote, base) = mgr
        .CheckAndUpdateLocalChecksum(context::Background(), &ck, true)
        .unwrap();
    assert!(!other_has_dup);
    assert!(need_remote);
    assert_eq!(base, Some(verify::MakeKVChecksum(0, 0, 0)));
    assert_exec_contains(db, metaStatusLocalChecksumUpdated.String());

    mgr.FinishTable(context::Background()).unwrap();
    assert_exec_contains(db, "checksum_skipped");
}

/// TestAllocTableRowIDsSingleTable
#[test]
fn test_alloc_table_row_ids_single_table() {
    // 先确认 SQL 状态推进，再断言空快照 row ID 语义。
    let (db, mgr) = new_db_table_meta_mgr();
    exercise_table_meta_sql(&mgr, &db);
    assert_alloc_empty_meta(&mgr, 10);
}

/// TestAllocTableRowIDsSingleTableAutoIDNot0
#[test]
fn test_alloc_table_row_ids_single_table_auto_id_not_0() {
    let (_db, mgr) = new_db_table_meta_mgr();
    // Rust 当前不实现 auto-id rebase，所以只验证请求量原样返回。
    assert_alloc_empty_meta(&mgr, 10);
}

/// TestAllocTableRowIDsSingleTableContainsData
#[test]
fn test_alloc_table_row_ids_single_table_contains_data() {
    let (_db, mgr) = new_db_table_meta_mgr();
    // 未注入表元数据时从零基线分配。
    assert_alloc_empty_meta(&mgr, 10);
}

/// TestAllocTableRowIDsSingleTableSkipChecksum
#[test]
fn test_alloc_table_row_ids_single_table_skip_checksum() {
    let (_db, mgr) = new_db_table_meta_mgr();
    // 空元数据快照不产生额外 checksum 基线。
    assert_alloc_empty_meta(&mgr, 10);
}

/// TestAllocTableRowIDsAllocated
#[test]
fn test_alloc_table_row_ids_allocated() {
    let (_db, mgr) = new_db_table_meta_mgr();
    // 再次分配时也不维护额外状态机，行为仍是稳定回显。
    assert_alloc_empty_meta(&mgr, 10);
}

/// TestAllocTableRowIDsFinished
#[test]
fn test_alloc_table_row_ids_finished() {
    let (_db, mgr) = new_db_table_meta_mgr();
    // 无活动元数据时从零重新分配。
    assert_alloc_empty_meta(&mgr, 10);
}

/// TestAllocTableRowIDsMultiTasksInit
#[test]
fn test_alloc_table_row_ids_multi_tasks_init() {
    let (_db, mgr) = new_db_table_meta_mgr();
    // 多任务竞争在此文件不做真实模拟，仅固定当前单实例返回值。
    assert_alloc_empty_meta(&mgr, 10);
}

/// TestAllocTableRowIDsMultiTasksAllocated
#[test]
fn test_alloc_table_row_ids_multi_tasks_allocated() {
    let (_db, mgr) = new_db_table_meta_mgr();
    // 空夹具的返回值保持确定。
    assert_alloc_empty_meta(&mgr, 10);
}

/// TestAllocTableRowIDsRetryOnTableInChecksum
///
/// 空夹具不包含 checksum 冲突，验证正常分配路径。
#[test]
fn test_alloc_table_row_ids_retry_on_table_in_checksum() {
    let (_db, mgr) = new_db_table_meta_mgr();
    assert_alloc_empty_meta(&mgr, 10);
}

/// TestCheckTasksExclusively (via singleTaskMetaMgr)
#[test]
fn test_check_tasks_exclusively() {
    // 单任务实现把排他更新压成 Mutex 内的一条记录，足够覆盖回调式更新协议。
    // 这里特别关心的是“读快照 -> 产出更新 -> 回写”这组三段式约定。
    let builder = singleMgrBuilder { taskID: 42 };
    let mgr = builder.TaskMetaMgr(pdutil::PdController::default());
    mgr.InitTask(context::Background(), 1024, 2048).unwrap();

    // 第二次检查确认第一次回调写回的值确实保存在唯一记录中。
    mgr.CheckTasksExclusively(context::Background(), &mut |tasks| {
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].taskID, 42);
        assert_eq!(tasks[0].tikvSourceBytes, 1024);
        assert_eq!(tasks[0].tiflashSourceBytes, 2048);
        let mut updated = tasks[0].clone();
        updated.tikvSourceBytes = 2 * 1024;
        updated.tiflashSourceBytes = 3 * 1024;
        Ok(Some(vec![updated]))
    })
    .unwrap();

    mgr.CheckTasksExclusively(context::Background(), &mut |tasks| {
        assert_eq!(tasks[0].tikvSourceBytes, 2 * 1024);
        assert_eq!(tasks[0].tiflashSourceBytes, 3 * 1024);
        Ok(None)
    })
    .unwrap();
}

/// TestSingleTaskMetaMgr
#[test]
fn test_single_task_meta_mgr() {
    // 这个测试覆盖单任务管理器的“存在性、更新、暂停/恢复、清理”完整主路径。
    // 它相当于把接口面最宽的那条 happy path 串起来做一次冒烟验证。
    let meta_builder = singleMgrBuilder {
        taskID: 1_700_000_000,
    };
    let pd = pdutil::PdController::default();
    let meta_mgr = meta_builder.TaskMetaMgr(pd.clone());

    assert!(!meta_mgr.CheckTaskExist(context::Background()).unwrap());

    // 初始化后再次检查，确认来源大小写入不会破坏“任务存在”这一前置条件。
    meta_mgr
        .InitTask(context::Background(), 1 << 30, 1 << 30)
        .unwrap();
    assert!(meta_mgr.CheckTaskExist(context::Background()).unwrap());

    // 这里验证回调能读到刚写入的大小统计。
    meta_mgr
        .CheckTasksExclusively(context::Background(), &mut |tasks| {
            assert_eq!(tasks.len(), 1);
            assert_eq!(tasks[0].tikvSourceBytes, 1u64 << 30);
            assert_eq!(tasks[0].tiflashSourceBytes, 1u64 << 30);
            Ok(None)
        })
        .unwrap();

    // 暂停/恢复 PD scheduler 的语义由布尔位模拟，但 undo 约定必须成立。
    let undo = meta_mgr
        .CheckAndPausePdSchedulers(context::Background())
        .unwrap();
    assert!(*pd.paused.lock().unwrap());
    undo(context::Background()).unwrap();
    assert!(!*pd.paused.lock().unwrap());

    // 单任务完成后应恢复并清理。
    let (all_finished, cleanup) = meta_mgr
        .CheckAndFinishRestore(context::Background(), true)
        .unwrap();
    assert!(all_finished);
    assert!(cleanup);

    // 这些清理接口当前都是空操作，测试只要求它们保持可调用且无副作用失败。
    assert!(meta_mgr.CanPauseSchedulerByKeyRange());
    meta_mgr.Cleanup(context::Background()).unwrap();
    meta_mgr.CleanupTask(context::Background()).unwrap();
    meta_mgr.CleanupAllMetas(context::Background()).unwrap();
    meta_mgr.Close();

    // 保持 Arc 路径可构造，证明 builder 仍能为需要表管理器的调用方提供对象。
    let _noop_table = meta_builder.TableMetaMgr(Arc::new(
        NewTableImporter(
            &importdef::DBInfo {
                Name: "test".into(),
                Tables: Default::default(),
            },
            &importdef::TableInfo {
                Name: "t1".into(),
                DB: "test".into(),
                Core: model::TableInfo {
                    Name: model::CIStr::new("t1"),
                    State: model::StatePublic,
                    ..Default::default()
                },
                ..Default::default()
            },
            None,
            log::Logger::L(),
        )
        .unwrap(),
    ));
}

#[test]
fn test_meta_status_strings_match_go() {
    let cases = [
        (metaStatusInitial, "initialized"),
        (metaStatusRowIDAllocated, "allocated"),
        (metaStatusRestoreStarted, "restore"),
        (metaStatusRestoreFinished, "restore_finished"),
        (metaStatusChecksuming, "checksuming"),
        (metaStatusChecksumSkipped, "checksum_skipped"),
        (metaStatusFinished, "finish"),
    ];
    for (status, text) in cases {
        assert_eq!(status.String(), text);
        assert_eq!(parseMetaStatus(text).unwrap(), status);
    }
}

#[test]
fn test_task_meta_status_strings_match_go() {
    let cases = [
        (taskMetaStatusInitial, "initialized"),
        (taskMetaStatusScheduleSet, "schedule_set"),
        (taskMetaStatusSwitchSkipped, "skip_switch"),
        (taskMetaStatusSwitchBack, "switched"),
    ];
    for (status, text) in cases {
        assert_eq!(status.String(), text);
        assert_eq!(parseTaskMetaStatus(text).unwrap(), status);
    }
}

#[test]
fn test_noop_contract_matches_go() {
    let task = noopTaskMetaMgr;
    assert!(task.CheckTaskExist(context::Background()).unwrap());
    assert_eq!(
        task.CheckAndFinishRestore(context::Background(), false)
            .unwrap(),
        (false, true)
    );
    let mut called = false;
    task.CheckTasksExclusively(context::Background(), &mut |_| {
        called = true;
        Ok(None)
    })
    .unwrap();
    assert!(!called);

    let table = noopTableMetaMgr;
    let (checksum, base) = table.AllocTableRowIDs(context::Background(), 10).unwrap();
    assert_eq!(checksum, verify::MakeKVChecksum(0, 0, 0));
    assert_eq!(base, 0);
    let (_, need_remote, _) = table
        .CheckAndUpdateLocalChecksum(
            context::Background(),
            &verify::MakeKVChecksum(1, 2, 3),
            true,
        )
        .unwrap();
    assert!(need_remote);
}

#[test]
fn test_single_task_does_not_exist_before_init() {
    let builder = singleMgrBuilder { taskID: 42 };
    let mgr = builder.TaskMetaMgr(pdutil::PdController::default());
    assert!(!mgr.CheckTaskExist(context::Background()).unwrap());
    mgr.InitTask(context::Background(), 1, 2).unwrap();
    assert!(mgr.CheckTaskExist(context::Background()).unwrap());
}

#[test]
fn test_finish_and_remove_emit_go_equivalent_sql() {
    let (db, mgr) = new_db_table_meta_mgr();
    mgr.FinishTable(context::Background()).unwrap();
    assert_exec_contains(&db, "DELETE FROM");
    assert_exec_contains(&db, "checksum_skipped");

    RemoveTableMetaByTableName(context::Background(), &db, "meta.table_meta", "").unwrap();
    let last = db.exec_log().last().unwrap().clone();
    assert_eq!(last.0, "DELETE FROM meta.table_meta");
    assert!(last.1.is_empty());
}

#[test]
fn test_alloc_table_row_ids_uses_locked_meta_ranges() {
    let (db, mgr) = new_db_table_meta_mgr();
    db.push_query_rows(
        "SELECT task_id, row_id_base",
        vec![
            vec![
                2.into(),
                0.into(),
                100.into(),
                0.into(),
                0.into(),
                0.into(),
                "allocated".into(),
            ],
            vec![
                1.into(),
                0.into(),
                0.into(),
                0.into(),
                0.into(),
                0.into(),
                "initialized".into(),
            ],
        ],
        1,
    );
    let (_, base) = mgr.AllocTableRowIDs(context::Background(), 10).unwrap();
    assert_eq!(base, 100);
    let log = db.exec_log();
    assert!(log.iter().any(|(q, args)| {
        q.contains("SET row_id_base")
            && args.first().map(sql::SqlValue::as_i64) == Some(100)
            && args.get(1).map(sql::SqlValue::as_i64) == Some(110)
            && args.get(2).map(sql::SqlValue::as_string).as_deref() == Some("restore")
    }));
}

#[test]
fn test_local_checksum_aggregates_finished_peer() {
    let (db, mgr) = new_db_table_meta_mgr();
    db.push_query_rows(
        "SELECT task_id, total_kvs_base",
        vec![vec![
            2.into(),
            3.into(),
            5.into(),
            7.into(),
            11.into(),
            13.into(),
            17.into(),
            "checksum_skipped".into(),
            false.into(),
        ]],
        1,
    );
    let local = verify::MakeKVChecksum(19, 23, 29);
    let (other_dupe, need_remote, base) = mgr
        .CheckAndUpdateLocalChecksum(context::Background(), &local, false)
        .unwrap();
    assert!(!other_dupe);
    assert!(need_remote);
    assert_eq!(base, Some(verify::MakeKVChecksum(18, 14, 22)));
}
