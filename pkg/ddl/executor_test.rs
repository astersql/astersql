// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// DDL executor 的单元测试模块。
//
// DDL（Data Definition Language，数据定义语言）指 CREATE/DROP/ALTER 等修改
// 数据库 schema（模式，即库表结构元数据）的语句。本模块验证 `crate::executor`
// 中简化实现的核心行为：
// - `DdlJobQueue`：DDL job（DDL 任务，代表一条待执行的 DDL 语句）队列的
//   插入、全量读取、迭代与按 job ID 排序的语义；
// - job 的可回滚性判断（rollbackable，指 DDL 执行中途失败时能否安全回退）；
// - truncate（清空表并更换 table ID）场景下表锁在新旧 table ID 之间的迁移。
//

use std::collections::BTreeMap;

use crate::executor::{
    DdlAction, DdlJob, DdlJobQueue, JobState, ObjectState, SessionContext, TableLockType,
    handle_lock_on_finish, handle_lock_on_submit,
};

/// 测试辅助函数：按给定 ID、动作类型和状态构造一个最小化的 `DdlJob`。
///
/// 为简化测试，schema_id 固定为 1，table_id 复用 job ID，其余字段取默认空值。
fn job(id: i64, action: DdlAction, state: JobState, schema_state: ObjectState) -> DdlJob {
    DdlJob {
        id,
        schema_id: 1,
        table_id: id,
        action,
        state,
        schema_state,
        multi_schema_revertible: false,
        query: String::new(),
        error: None,
        warnings: BTreeMap::new(),
        schema_version: 0,
        involving_schema: Vec::new(),
        args: BTreeMap::new(),
    }
}

/// 对应 Go 的 TestGetDDLJobs：验证每写入一个 job 后，全量读取（`all`）与
/// 迭代接口（`iter_until`）都能看到全部已插入的 job，且重复 ID 的插入会报错。
#[test]
fn ddl_job_queue_reads_every_inserted_job() {
    let mut queue = DdlJobQueue::default();
    let mut iterated = Vec::new();
    // 逐个插入 10 个 job，每次插入后都检查两种读取方式的可见数量。
    for id in 0..10 {
        queue
            .add(job(
                id,
                DdlAction::CreateTable,
                JobState::None,
                ObjectState::None,
            ))
            .unwrap();
        assert_eq!(id as usize + 1, queue.all().len());
        iterated.clear();
        // iter_until 的回调返回 true 表示提前终止迭代；
        // 这里所有 job 都处于 None 状态，因此会完整遍历队列。
        queue.iter_until(|job| {
            iterated.push(job.id);
            job.state != JobState::None
        });
        assert_eq!(id as usize + 1, iterated.len());
    }
    assert_eq!((0..10).collect::<Vec<_>>(), iterated);
    // 重复插入已存在的 job ID（9）应当失败。
    assert!(
        queue
            .add(job(
                9,
                DdlAction::CreateTable,
                JobState::None,
                ObjectState::None,
            ))
            .is_err()
    );
}

/// 对应 Go 的 TestGetDDLJobsIsSort：以乱序（先 drop、再 create、后 add-index）
/// 插入不同动作类型的 job，验证 `all()` 仍按 job ID 升序返回。
///
/// TiDB 中普通 DDL 与 add-index（加索引，代价高、单独排队）历史上分属不同队列，
/// 但对外读取时必须给出全局按 ID 排序的统一视图。
#[test]
fn ddl_job_queue_is_sorted_across_action_types() {
    let mut queue = DdlJobQueue::default();
    for id in 10..15 {
        queue
            .add(job(
                id,
                DdlAction::DropTable,
                JobState::None,
                ObjectState::None,
            ))
            .unwrap();
    }
    for id in 0..5 {
        queue
            .add(job(
                id,
                DdlAction::CreateTable,
                JobState::None,
                ObjectState::None,
            ))
            .unwrap();
    }
    for id in 5..10 {
        queue
            .add(job(
                id,
                DdlAction::AddIndex,
                JobState::None,
                ObjectState::None,
            ))
            .unwrap();
    }
    assert_eq!(
        (0..15).collect::<Vec<_>>(),
        queue.all().iter().map(|job| job.id).collect::<Vec<_>>()
    );
}

/// 对应 Go 的 TestIsJobRollbackable：表驱动地验证破坏性 DDL（drop index/schema/column）
/// 的可回滚性取决于 schema state（模式变更状态机中的阶段）。
///
/// 作业生命周期可以仍是 Running；真正决定 drop 类作业能否回滚的是独立的
/// schema state。进入 DeleteOnly 后数据删除已经开始，无法再安全回滚。
#[test]
fn destructive_job_rollbackability_matches_schema_state_transition() {
    let cases = [
        (DdlAction::DropIndex, ObjectState::None, true),
        (DdlAction::DropIndex, ObjectState::DeleteOnly, false),
        (DdlAction::DropSchema, ObjectState::DeleteOnly, false),
        (DdlAction::DropColumn, ObjectState::DeleteOnly, false),
        (DdlAction::DropSchema, ObjectState::Public, true),
        (DdlAction::DropColumn, ObjectState::Public, true),
        (DdlAction::DropIndex, ObjectState::Public, true),
    ];
    for (action, schema_state, expected) in cases {
        assert_eq!(
            expected,
            job(1, action, JobState::Running, schema_state).is_rollbackable()
        );
    }
}

#[test]
fn rollbackability_covers_go_action_specific_schema_states() {
    let cases = [
        (DdlAction::DropPrimaryKey, ObjectState::WriteOnly, false),
        (DdlAction::ModifyColumn, ObjectState::Public, false),
        (DdlAction::AddPartition, ObjectState::ReplicaOnly, true),
        (DdlAction::AddPartition, ObjectState::Public, false),
        (DdlAction::DropTable, ObjectState::None, false),
        (DdlAction::DropTable, ObjectState::Public, true),
        (DdlAction::TruncatePartition, ObjectState::WriteOnly, true),
        (DdlAction::TruncateTable, ObjectState::Public, false),
        (
            DdlAction::FlashbackCluster,
            ObjectState::WriteReorganization,
            false,
        ),
        (DdlAction::ReorganizePartition, ObjectState::Public, false),
        (DdlAction::CreateTable, ObjectState::Public, true),
    ];
    for (action, schema_state, expected) in cases {
        assert_eq!(
            expected,
            job(1, action, JobState::Synced, schema_state).is_rollbackable()
        );
    }

    let mut multi = job(
        1,
        DdlAction::MultiSchemaChange,
        JobState::Running,
        ObjectState::None,
    );
    assert!(!multi.is_rollbackable());
    multi.multi_schema_revertible = true;
    assert!(multi.is_rollbackable());
}

/// 对应 Go 的 TestHandleLockTable：验证 truncate table 场景下的表锁交接。
///
/// truncate 会为表分配新的 table ID（这里旧 ID 为 1、新 ID 为 2）。若会话持有
/// 旧表的锁，提交 job 时需先把锁复制到新 ID；DDL 成功则释放旧锁只留新锁，
/// 失败则回滚新锁、保留旧锁。
#[test]
fn truncate_lock_handoff_commits_or_rolls_back() {
    let mut session = SessionContext::default();
    // `locked_tables` 以 table ID 为键，模拟会话层当前持有的表锁集合。
    // 场景一：目标表本来就没有锁，提交与收尾都不应产生任何锁。
    handle_lock_on_submit(&mut session, 1, 2);
    assert!(session.locked_tables.is_empty());
    handle_lock_on_finish(&mut session, 1, 2, true);
    assert!(session.locked_tables.is_empty());

    // 场景二：DDL 成功。提交后新旧 ID 同时持锁，收尾时释放旧锁只保留新锁。
    session.locked_tables.insert(1, TableLockType::Read);
    handle_lock_on_submit(&mut session, 1, 2);
    assert_eq!(Some(&TableLockType::Read), session.locked_tables.get(&1));
    assert_eq!(Some(&TableLockType::Read), session.locked_tables.get(&2));
    handle_lock_on_finish(&mut session, 1, 2, true);
    assert!(!session.locked_tables.contains_key(&1));
    assert_eq!(Some(&TableLockType::Read), session.locked_tables.get(&2));

    // 场景三：DDL 失败（success 传 false）。应释放新 ID 的锁并保留旧锁。
    session.locked_tables.clear();
    session.locked_tables.insert(1, TableLockType::Write);
    handle_lock_on_submit(&mut session, 1, 2);
    handle_lock_on_finish(&mut session, 1, 2, false);
    assert_eq!(Some(&TableLockType::Write), session.locked_tables.get(&1));
    assert!(!session.locked_tables.contains_key(&2));
}
