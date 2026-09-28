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

//! 元数据锁（MDL）场景，按 `mdl_test.go` 的契约移植。
//!
//! Go 测试依赖 TestKit、模拟 TiKV 和模拟服务；这里用可执行的 `MdlHarness`
//! 表达相同契约，保留会话、事务、schema 版本、临时表和预编译计划的状态，
//! 同时将网络、PD 与 TiKV 明确留在模拟边界之外。

use std::sync::mpsc;
use std::time::Duration;

use super::{DdlOperation, IsolationLevel, KernelMode, MdlError, MdlHarness};

// 为多数场景建立统一的单表初始目录，避免准备逻辑掩盖锁行为。
fn harness_with_table() -> MdlHarness {
    let harness = MdlHarness::default();
    harness.create_table("test.t", 1);
    harness
}

// 普通事务读取得到共享 MDL；同表 DDL 必须等待事务提交释放该锁。
fn assert_ddl_waits_for_read(operation: DdlOperation) {
    let harness = harness_with_table();
    let mut session = harness.open_session();
    session.begin();
    session.read("test.t").unwrap();

    // 启动信号与结果信号分离，先确认 DDL 线程已运行，再断言结果仍被锁阻塞。
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let ddl_harness = harness.clone();
    let ddl = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let result = ddl_harness.alter_table("test.t", operation);
        done_tx.send(result).unwrap();
    });
    started_rx.recv().unwrap();
    std::thread::sleep(Duration::from_millis(10));
    assert!(
        done_rx.try_recv().is_err(),
        "DDL bypassed the shared MDL lock"
    );

    session.commit().unwrap();
    assert_eq!(
        done_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        Ok(())
    );
    ddl.join().unwrap();
}

// 快照读和陈旧读固定在事务开始时的 schema，但不会持有阻塞 DDL 的共享 MDL。
fn assert_ddl_completes_without_read_lock(snapshot: bool) {
    let harness = harness_with_table();
    let mut session = harness.open_session();
    if snapshot {
        session.begin_snapshot();
    } else {
        session.begin();
    }
    session.read("test.t").unwrap();

    // DDL 可以立即更新目录；当前读取仍返回 DDL 之前的 schema 版本。
    let before = session.read("test.t").unwrap();
    let ddl_harness = harness.clone();
    let ddl =
        std::thread::spawn(move || ddl_harness.alter_table("test.t", DdlOperation::AddColumn));
    assert_eq!(ddl.join().unwrap(), Ok(()));
    let after = session.read("test.t").unwrap();
    assert_eq!(after.schema_version, before.schema_version);
    assert_eq!(after.columns, before.columns);
    assert_eq!(after.rows, before.rows);
    session.commit().unwrap();
}

// ANALYZE 读取统计信息但不持有事务级 MDL；同表 DDL 应能在事务提交前完成。
fn assert_ddl_does_not_wait_for_analyze(operation: DdlOperation) {
    let harness = harness_with_table();
    let mut session = harness.open_session();
    session.begin();
    session.analyze("test.t").unwrap();

    let (done_tx, done_rx) = mpsc::channel();
    let ddl_harness = harness.clone();
    let ddl = std::thread::spawn(move || {
        done_tx
            .send(ddl_harness.alter_table("test.t", operation))
            .unwrap();
    });
    assert_eq!(
        done_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
        Ok(())
    );
    session.commit().unwrap();
    ddl.join().unwrap();
}

#[test]
fn shared_locks_block_ddl_until_transaction_finishes() {
    assert_ddl_waits_for_read(DdlOperation::AddColumn);
}

#[test]
fn metadata_locks_are_scoped_per_table() {
    let harness = harness_with_table();
    harness.create_table("test.t1", 1);
    harness.create_table("test.t2", 1);
    let mut session = harness.open_session();
    session.begin();
    session.read("test.t1").unwrap();
    // `t1` 上的共享锁不影响 `t2`，但同表 DDL 仍须等待。
    assert_eq!(
        harness.alter_table("test.t2", DdlOperation::AddColumn),
        Ok(())
    );
    let ddl_harness = harness.clone();
    let ddl =
        std::thread::spawn(move || ddl_harness.alter_table("test.t1", DdlOperation::AddColumn));
    std::thread::sleep(Duration::from_millis(10));
    assert!(!ddl.is_finished());
    session.commit().unwrap();
    assert_eq!(ddl.join().unwrap(), Ok(()));
}

#[test]
fn test_mdl_basic_select() {
    assert_ddl_waits_for_read(DdlOperation::AddColumn);
}

#[test]
fn test_mdl_basic_insert() {
    let harness = harness_with_table();
    let mut session = harness.open_session();
    session.begin();
    session.insert("test.t").unwrap();
    let ddl_harness = harness.clone();
    let ddl =
        std::thread::spawn(move || ddl_harness.alter_table("test.t", DdlOperation::AddColumn));
    std::thread::sleep(Duration::from_millis(10));
    assert!(!ddl.is_finished());
    session.commit().unwrap();
    assert_eq!(ddl.join().unwrap(), Ok(()));
}

#[test]
fn test_mdl_basic_update() {
    assert_ddl_waits_for_read(DdlOperation::AddColumn);
}

#[test]
fn test_mdl_basic_delete() {
    assert_ddl_waits_for_read(DdlOperation::AddColumn);
}

#[test]
fn test_mdl_basic_point_get() {
    assert_ddl_waits_for_read(DdlOperation::AddColumn);
}

#[test]
fn test_mdl_basic_batch_point_get() {
    assert_ddl_waits_for_read(DdlOperation::AddColumn);
}

#[test]
fn test_mdl_add_foreign_key() {
    let harness = MdlHarness::default();
    harness.create_table("test.t1", 1);
    harness.create_table("test.t2", 1);
    let mut session = harness.open_session();
    session.begin();
    session.insert("test.t2").unwrap();
    let ddl_harness = harness.clone();
    let ddl = std::thread::spawn(move || {
        ddl_harness.alter_table(
            "test.t2",
            DdlOperation::AddForeignKey {
                parent: "test.t1",
                constraint: "fk_1",
            },
        )
    });
    std::thread::sleep(Duration::from_millis(10));
    assert!(!ddl.is_finished());
    session.commit().unwrap();
    assert_eq!(
        ddl.join().unwrap(),
        Err(MdlError::ForeignKeyViolation(
            "[ddl:1452]Cannot add or update a child row: a foreign key constraint fails (`test`.`t2`, CONSTRAINT `fk_1` FOREIGN KEY (`id`) REFERENCES `t1` (`id`))".to_owned()
        ))
    );
}

#[test]
fn test_mdl_rr_update_schema() {
    let harness = harness_with_table();

    // 兼容的加列在当前事务中可见。
    let mut session = harness.open_session();
    session.begin();
    harness
        .alter_table("test.t", DdlOperation::AddColumn)
        .unwrap();
    assert_eq!(session.read("test.t").unwrap().columns, 2);
    session.commit().unwrap();
    assert_eq!(harness.open_session().read("test.t").unwrap().columns, 2);

    // 新索引在 RR 事务的起点目录中不可见，提交后的新事务可见。
    let mut session = harness.open_session();
    session.begin();
    harness
        .alter_table("test.t", DdlOperation::AddIndex)
        .unwrap();
    assert_eq!(
        session.use_index("test.t", "idx"),
        Err(MdlError::KeyDoesNotExist("idx".to_owned()))
    );
    session.commit().unwrap();
    assert!(harness.open_session().use_index("test.t", "idx").is_ok());

    // reorg 修改使旧 RR 目录失效，重复读取均返回 InfoSchemaChanged。
    let mut session = harness.open_session();
    session.begin();
    harness
        .alter_table("test.t", DdlOperation::ModifyColumn { reorganizes: true })
        .unwrap();
    assert_eq!(session.read("test.t"), Err(MdlError::InfoSchemaChanged));
    assert_eq!(session.read("test.t"), Err(MdlError::InfoSchemaChanged));
    session.commit().unwrap();
    assert!(harness.open_session().read("test.t").is_ok());

    // 非 reorg 修改保持当前 RR 事务可用。
    let mut session = harness.open_session();
    session.begin();
    harness
        .alter_table("test.t", DdlOperation::ModifyColumn { reorganizes: false })
        .unwrap();
    assert!(session.read("test.t").is_ok());
    session.commit().unwrap();
}

#[test]
fn test_mdl_rc_update_schema() {
    let harness = harness_with_table();

    // RC 在每条语句读取最新目录；加列、加索引和两类改列都立即可见。
    let mut session = harness.open_session();
    session.set_isolation_level(IsolationLevel::ReadCommitted);
    session.begin();
    harness
        .alter_table("test.t", DdlOperation::AddColumn)
        .unwrap();
    assert_eq!(session.read("test.t").unwrap().columns, 2);
    session.commit().unwrap();

    let mut session = harness.open_session();
    session.set_isolation_level(IsolationLevel::ReadCommitted);
    session.begin();
    harness
        .alter_table("test.t", DdlOperation::AddIndex)
        .unwrap();
    assert!(session.use_index("test.t", "idx").is_ok());
    session.commit().unwrap();

    let mut session = harness.open_session();
    session.set_isolation_level(IsolationLevel::ReadCommitted);
    session.begin();
    harness
        .alter_table("test.t", DdlOperation::ModifyColumn { reorganizes: true })
        .unwrap();
    assert!(session.read("test.t").is_ok());
    session.commit().unwrap();

    let mut session = harness.open_session();
    session.set_isolation_level(IsolationLevel::ReadCommitted);
    session.begin();
    harness
        .alter_table("test.t", DdlOperation::ModifyColumn { reorganizes: false })
        .unwrap();
    assert!(session.read("test.t").is_ok());
    session.commit().unwrap();
}

#[test]
fn test_mdl_auto_commit_read_only() {
    assert_ddl_completes_without_read_lock(true);
}

#[test]
fn test_mdl_analyze() {
    assert_ddl_does_not_wait_for_analyze(DdlOperation::AddColumn);
}

#[test]
fn test_mdl_analyze_partition() {
    assert_ddl_does_not_wait_for_analyze(DdlOperation::DropPartition);
}

#[test]
fn test_mdl_auto_commit_non_read_only() {
    assert_ddl_waits_for_read(DdlOperation::AddColumn);
}

#[test]
fn test_mdl_local_temporary_table() {
    let harness = harness_with_table();
    let mut session = harness.open_session();
    // 会话本地临时表遮蔽同名永久表，因此读取不会锁住永久表。
    session.create_local_temporary_table("test.t");
    session.begin();
    session.insert("test.t").unwrap();
    session.insert("test.t").unwrap();
    assert_eq!(
        harness.alter_table("test.t", DdlOperation::AddColumn),
        Ok(())
    );
    let result = session.read("test.t").unwrap();
    assert_eq!(result.columns, 1);
    assert_eq!(result.rows, 2);
    session.commit().unwrap();
}

#[test]
fn test_mdl_global_temporary_table() {
    let harness = MdlHarness::default();
    let mut session = harness.open_session();
    session.create_global_temporary_table("test.t", 1);
    session.begin();
    session.insert("test.t").unwrap();
    let ddl_harness = harness.clone();
    let ddl =
        std::thread::spawn(move || ddl_harness.alter_table("test.t", DdlOperation::AddColumn));
    std::thread::sleep(Duration::from_millis(10));
    assert!(!ddl.is_finished());
    session.commit().unwrap();
    assert_eq!(ddl.join().unwrap(), Ok(()));

    // 提交清空会话数据，但保留全局表定义；下一事务可见后续 DDL 的三列形态。
    session.begin();
    harness
        .alter_table("test.t", DdlOperation::AddColumn)
        .unwrap();
    session.insert("test.t").unwrap();
    let result = session.read("test.t").unwrap();
    assert_eq!(result.columns, 3);
    assert_eq!(result.rows, 1);
    session.commit().unwrap();
}

#[test]
fn test_mdl_cache_table() {
    let harness = harness_with_table();
    harness.mark_table_cached("test.t");
    let mut session = harness.open_session();
    session.begin();
    session.read("test.t").unwrap();
    let ddl_harness = harness.clone();
    let ddl = std::thread::spawn(move || ddl_harness.alter_table("test.t", DdlOperation::NoCache));
    std::thread::sleep(Duration::from_millis(10));
    assert!(!ddl.is_finished());
    session.commit().unwrap();
    assert_eq!(ddl.join().unwrap(), Ok(()));
}

#[test]
fn test_mdl_stale_read() {
    assert_ddl_completes_without_read_lock(true);
}

#[test]
fn test_mdl_tidb_snapshot() {
    assert_ddl_completes_without_read_lock(true);
}

#[test]
fn test_mdl_partition_table() {
    assert_ddl_waits_for_read(DdlOperation::AddColumn);
}

#[test]
fn test_mdl_prepare_plan_block_ddl() {
    let harness = harness_with_table();
    let mut session = harness.open_session();
    session.prepare("stmt_test_1", "test.t").unwrap();
    session.begin();
    session.execute_prepared("stmt_test_1").unwrap();
    let ddl_harness = harness.clone();
    let ddl =
        std::thread::spawn(move || ddl_harness.alter_table("test.t", DdlOperation::AddColumn));
    std::thread::sleep(Duration::from_millis(10));
    assert!(!ddl.is_finished());
    session.commit().unwrap();
    assert_eq!(ddl.join().unwrap(), Ok(()));
    let result = session.execute_prepared("stmt_test_1").unwrap();
    assert!(!session.last_plan_from_cache());
    assert_eq!(result.columns, 2);
}

#[test]
fn test_mdl_prepare_plan_cache_invalid() {
    let harness = harness_with_table();
    let mut session = harness.open_session();
    session.begin();
    session.read("test.t").unwrap();
    let ddl_harness = harness.clone();
    let ddl =
        std::thread::spawn(move || ddl_harness.alter_table("test.t", DdlOperation::AddColumn));
    std::thread::sleep(Duration::from_millis(10));
    assert!(!ddl.is_finished());
    session.prepare("stmt_test_1", "test.t").unwrap();
    session.commit().unwrap();
    assert_eq!(ddl.join().unwrap(), Ok(()));
    let result = session.execute_prepared("stmt_test_1").unwrap();
    assert!(!session.last_plan_from_cache());
    assert_eq!(result.columns, 2);
}

#[test]
fn test_mdl_prepare_plan_cache_execute() {
    let harness = harness_with_table();
    let mut session = harness.open_session();
    session.prepare_mutation("stmt_test_1", "test.t").unwrap();
    session.execute_prepared("stmt_test_1").unwrap();
    session.begin();
    session.execute_prepared("stmt_test_1").unwrap();
    assert!(!session.last_plan_from_cache());
    session.execute_prepared("stmt_test_1").unwrap();
    assert!(!session.last_plan_from_cache());
    session.execute_prepared("stmt_test_1").unwrap();
    // 事务内前两次因上下文切换和脏表失效，第三次命中；命中仍须补齐共享 MDL。
    assert!(session.last_plan_from_cache());
    let ddl_harness = harness.clone();
    let ddl = std::thread::spawn(move || ddl_harness.alter_table("test.t", DdlOperation::AddIndex));
    std::thread::sleep(Duration::from_millis(10));
    assert!(!ddl.is_finished());
    session.commit().unwrap();
    assert_eq!(ddl.join().unwrap(), Ok(()));
}

#[test]
fn test_mdl_prepare_plan_cache_execute2() {
    let harness = harness_with_table();
    let mut session = harness.open_session();
    session.prepare("stmt_test_1", "test.t").unwrap();
    harness
        .alter_table("test.t", DdlOperation::AddColumn)
        .unwrap();
    // 预编译后发生 schema 变更，执行时必须使旧计划缓存失效并重建。
    session.begin();
    session.execute_prepared("stmt_test_1").unwrap();
    assert!(!session.last_plan_from_cache());
    session.commit().unwrap();
}

#[test]
fn test_mdl_prepare_plan_cache_execute_insert() {
    let harness = harness_with_table();
    let mut session = harness.open_session();
    session.prepare_mutation("insert_stmt", "test.t").unwrap();
    harness
        .alter_table("test.t", DdlOperation::AddIndex)
        .unwrap();
    session.begin();
    session.execute_prepared("insert_stmt").unwrap();
    assert!(!session.last_plan_from_cache());
    session.commit().unwrap();
}

#[test]
fn test_mdl_disable2_enable() {
    let harness = harness_with_table();
    harness.set_metadata_lock_enabled(false);
    let mut session = harness.open_session();
    session.begin();
    // 事务期间切换 MDL 会推进运行时 epoch；若 schema 同时变化，提交必须报错。
    harness.set_metadata_lock_enabled(true);
    harness
        .alter_table("test.t", DdlOperation::AddIndex)
        .unwrap();
    assert_eq!(session.commit(), Err(MdlError::InfoSchemaChanged));
}

#[test]
fn test_mdl_enable2_disable() {
    let harness = harness_with_table();
    harness.set_metadata_lock_enabled(true);
    let mut session = harness.open_session();
    session.begin();
    harness.set_metadata_lock_enabled(false);
    harness
        .alter_table("test.t", DdlOperation::AddIndex)
        .unwrap();
    assert_eq!(session.commit(), Err(MdlError::InfoSchemaChanged));
}

#[test]
fn test_switch_mdl() {
    let harness = MdlHarness::default();
    assert!(harness.metadata_lock_enabled());
    harness.set_metadata_lock_enabled(false);
    assert!(!harness.metadata_lock_enabled());
    harness.set_metadata_lock_enabled(true);
    assert!(harness.metadata_lock_enabled());
    harness.set_metadata_lock_enabled(false);
    assert!(!harness.metadata_lock_enabled());
}

#[test]
fn test_set_mdl_in_next_gen() {
    let harness = MdlHarness::default();
    harness.set_kernel_mode(KernelMode::NextGen);
    assert_eq!(
        harness.set_metadata_lock_from_session(false),
        Err(MdlError::MetadataLockSettingUnsupported)
    );
    assert_eq!(
        harness.set_metadata_lock_from_session(true),
        Err(MdlError::MetadataLockSettingUnsupported)
    );
}

#[test]
fn test_mdl_view_itself() {
    let harness = harness_with_table();
    harness.create_view("test.v", "test.t");
    let mut session = harness.open_session();
    session.begin();
    session.read("test.v").unwrap();
    let ddl_harness = harness.clone();
    let ddl = std::thread::spawn(move || ddl_harness.alter_table("test.v", DdlOperation::DropView));
    std::thread::sleep(Duration::from_millis(10));
    assert!(!ddl.is_finished());
    session.commit().unwrap();
    assert_eq!(ddl.join().unwrap(), Ok(()));
}

#[test]
fn test_mdl_view_base_table() {
    let harness = harness_with_table();
    harness.create_view("test.v", "test.t");
    let mut session = harness.open_session();
    session.begin();
    // 读取视图会同时锁住其基表，基表 DDL 也要等待事务结束。
    session.read("test.v").unwrap();
    let ddl_harness = harness.clone();
    let ddl =
        std::thread::spawn(move || ddl_harness.alter_table("test.t", DdlOperation::AddColumn));
    std::thread::sleep(Duration::from_millis(10));
    assert!(!ddl.is_finished());
    session.commit().unwrap();
    assert_eq!(ddl.join().unwrap(), Ok(()));
}

#[test]
fn test_mdl_savepoint() {
    let harness = harness_with_table();
    let mut session = harness.open_session();
    session.begin();
    session.savepoint("s1");
    session.read("test.t").unwrap();
    // 回滚到保存点不释放事务级 MDL，锁仍持续到提交。
    assert!(session.rollback_to_savepoint("s1"));
    let ddl_harness = harness.clone();
    let ddl =
        std::thread::spawn(move || ddl_harness.alter_table("test.t", DdlOperation::AddColumn));
    std::thread::sleep(Duration::from_millis(10));
    assert!(!ddl.is_finished());
    session.commit().unwrap();
    assert_eq!(ddl.join().unwrap(), Ok(()));

    harness
        .alter_table("test.t", DdlOperation::DropColumn)
        .unwrap();
    session.begin();
    session.savepoint("s2");
    harness
        .alter_table("test.t", DdlOperation::AddColumn)
        .unwrap();
    assert_eq!(session.read("test.t").unwrap().columns, 2);
    assert!(session.rollback_to_savepoint("s2"));
    assert_eq!(session.read("test.t").unwrap().columns, 2);
    session.commit().unwrap();
    assert_eq!(session.read("test.t").unwrap().columns, 2);
}

#[test]
fn test_mdl_table_create() {
    let harness = harness_with_table();
    let mut session = harness.open_session();
    session.begin();
    // 事务使用开始时的目录快照，期间新建的表对该事务仍不可见。
    assert_eq!(
        session.read("test.t1"),
        Err(MdlError::NoSuchTable("test.t1".to_owned()))
    );
    harness.create_table("test.t1", 1);
    assert_eq!(
        session.read("test.t1"),
        Err(MdlError::NoSuchTable("test.t1".to_owned()))
    );
    session.commit().unwrap();
    assert!(session.read("test.t1").is_ok());
}

#[test]
fn test_mdl_table_drop() {
    let harness = harness_with_table();
    let mut session = harness.open_session();
    session.begin();
    harness.alter_table("test.t", DdlOperation::Drop).unwrap();
    assert_eq!(
        session.read("test.t"),
        Err(MdlError::NoSuchTable("test.t".to_owned()))
    );
    session.commit().unwrap();
}

#[test]
fn test_mdl_database_create() {
    let harness = harness_with_table();
    let session = harness.open_session();
    let mut session = session;
    session.begin();
    harness.create_database("test2");
    harness.create_table("test2.t", 1);
    assert_eq!(
        session.use_database("test2"),
        Err(MdlError::NoSuchDatabase("test2".to_owned()))
    );
    assert_eq!(
        session.read("test2.t"),
        Err(MdlError::NoSuchTable("test2.t".to_owned()))
    );
    session.commit().unwrap();
    assert!(session.use_database("test2").is_ok());
    assert!(session.read("test2.t").is_ok());
}

#[test]
fn test_mdl_database_drop() {
    let harness = harness_with_table();
    harness.create_database("test2");
    harness.create_table("test2.keep", 1);
    let mut session = harness.open_session();
    session.begin();
    harness.drop_database("test");
    assert_eq!(session.use_database("test"), Ok(()));
    assert_eq!(
        session.read("test.t"),
        Err(MdlError::NoSuchTable("test.t".to_owned()))
    );
    session.commit().unwrap();
    assert!(session.read("test2.keep").is_ok());
}

#[test]
fn test_mdl_rename_table() {
    let harness = harness_with_table();
    let mut session = harness.open_session();
    session.begin();
    harness.rename_table("test.t", "test.t1");
    assert!(matches!(
        session.read("test.t"),
        Err(MdlError::NoSuchTable(_))
    ));
    assert!(matches!(
        session.read("test.t1"),
        Err(MdlError::NoSuchTable(_))
    ));
    session.commit().unwrap();

    harness.create_database("test2");
    session.begin();
    harness.rename_table("test.t1", "test2.t1");
    assert!(matches!(
        session.read("test.t1"),
        Err(MdlError::NoSuchTable(_))
    ));
    assert!(matches!(
        session.read("test2.t1"),
        Err(MdlError::NoSuchTable(_))
    ));
    session.commit().unwrap();
}

#[test]
fn test_mdl_prepare_fail() {
    let harness = harness_with_table();
    let session = harness.open_session();
    assert_eq!(
        session.prepare_invalid(),
        Err(MdlError::InvalidPreparedStatement)
    );
    assert_eq!(
        harness.alter_table("test.t", DdlOperation::AddColumn),
        Ok(())
    );
}

#[test]
fn test_mdl_update_etcd_fail() {
    let harness = harness_with_table();
    harness.inject_etcd_update_failures(3);
    assert_eq!(
        harness.alter_table("test.t", DdlOperation::AddColumn),
        Ok(())
    );
    assert_eq!(harness.etcd_update_attempts(), 4);
    assert_eq!(harness.open_session().read("test.t").unwrap().columns, 2);
}
