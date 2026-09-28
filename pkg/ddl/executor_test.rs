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
// 文件前半部分是从 Go(TiDB) 机械迁移而来、暂以块注释保留的原始测试代码，
// 供后续人工迁移时逐段替换；真正生效的 Rust 测试位于块注释之后。

/*
// 和 errgroup 等 Go 依赖均以原调用形状保留，供后续人工迁移时逐段替换。

// TestGetDDLJobs 对应 Go 的 DDL job 队列读取测试：每写入一个 job，就分别通过全量读取
// 和迭代接口确认可见数量，并在最后核对 job 的 ID、SchemaID 和 Type。
#[test]
fn test_get_ddl_jobs() {
    let store = testkit::CreateMockStore(t);

    let sess = testkit::NewTestKit(t, store).Session();
    let (_, err) = sess.Execute(context::Background(), "begin");
    require::NoError(t, err);

    let (txn, err) = sess.Txn(true);
    require::NoError(t, err);

    let cnt = 10;
    let mut jobs: Vec<*mut model::Job> = vec![std::ptr::null_mut(); cnt];
    let ctx = context::Background();
    let mut curr_jobs2: Vec<*mut model::Job> = Vec::new();
    for i in 0..cnt {
        jobs[i] = Box::into_raw(Box::new(model::Job {
            ID: i as i64,
            SchemaID: 1,
            Type: model::ActionCreateTable,
            ..Default::default()
        }));
        let err = addDDLJobs(sess, jobs[i]);
        require::NoError(t, err);

        let (curr_jobs, err) = ddl::GetAllDDLJobs(ctx, sess);
        require::NoError(t, err);
        require::Len(t, curr_jobs, i + 1);

        curr_jobs2.clear();
        let err = ddl::IterAllDDLJobs(sess, txn, |jobs: Vec<*mut model::Job>| {
            for job in jobs {
                // Go 回调在遇到 Started job 时提前终止；这里保留停止迭代的布尔返回语义。
                if unsafe { (*job).Started() } {
                    return (true, nil);
                }
                curr_jobs2.push(job);
            }
            (false, nil)
        });
        require::NoError(t, err);
        require::Len(t, curr_jobs2, i + 1);
    }

    let (curr_jobs, err) = ddl::GetAllDDLJobs(ctx, sess);
    require::NoError(t, err);

    for (i, job) in jobs.iter().enumerate() {
        require::Equal(t, curr_jobs[i].ID, unsafe { (**job).ID });
        require::Equal(t, 1_i64, curr_jobs[i].SchemaID);
        require::Equal(t, model::ActionCreateTable, curr_jobs[i].Type);
    }
    require::Equal(t, curr_jobs2, curr_jobs);

    let (_, err) = sess.Execute(context::Background(), "rollback");
    require::NoError(t, err);
}

// TestGetDDLJobsIsSort 对应 Go 的排序校验：混合写入普通队列和 add-index 队列后，
// GetAllDDLJobs 仍需按 job ID 升序返回。
#[test]
fn test_get_ddl_jobs_is_sort() {
    let store = testkit::CreateMockStore(t);
    let ctx = context::Background();

    let sess = testkit::NewTestKit(t, store).Session();
    let (_, err) = sess.Execute(context::Background(), "begin");
    require::NoError(t, err);

    // Go 原测试先写入 drop-table 队列，再写入 create-table 队列，最后写 add-index 队列。
    en_queue_ddl_jobs(t, sess, model::ActionDropTable, 10, 15);
    en_queue_ddl_jobs(t, sess, model::ActionCreateTable, 0, 5);
    en_queue_ddl_jobs(t, sess, model::ActionAddIndex, 5, 10);

    let (curr_jobs, err) = ddl::GetAllDDLJobs(ctx, sess);
    require::NoError(t, err);
    require::Len(t, curr_jobs, 15);

    let is_sort = curr_jobs.windows(2).all(|pair| pair[0].ID <= pair[1].ID);
    require::True(t, is_sort);

    let (_, err) = sess.Execute(context::Background(), "rollback");
    require::NoError(t, err);
}

// TestIsJobRollbackable 对应 Go 的表驱动测试，覆盖 DropIndex、DropSchema、DropColumn
// 在不同 schema state 下是否允许回滚。
#[test]
fn test_is_job_rollbackable() {
    struct Case {
        tp: model::ActionType,
        state: model::SchemaState,
        result: bool,
    }

    let cases = vec![
        Case { tp: model::ActionDropIndex, state: model::StateNone, result: true },
        Case { tp: model::ActionDropIndex, state: model::StateDeleteOnly, result: false },
        Case { tp: model::ActionDropSchema, state: model::StateDeleteOnly, result: false },
        Case { tp: model::ActionDropColumn, state: model::StateDeleteOnly, result: false },
    ];
    let mut job = model::Job::default();
    for ca in cases {
        job.Type = ca.tp;
        job.SchemaState = ca.state;
        let re = job.IsRollbackable();
        require::Equal(t, ca.result, re);
    }
}

// enQueueDDLJobs 是 Go 测试的辅助函数：按 [start, end) 构造同类型 DDL job 并写入队列。
// 参数 sess 保持 sessionapi.Session 语义，不接线真实 session trait。
fn en_queue_ddl_jobs(
    t: testing::T,
    sess: sessionapi::Session,
    job_type: model::ActionType,
    start: i32,
    end: i32,
) {
    for i in start..end {
        let job = Box::into_raw(Box::new(model::Job {
            ID: i as i64,
            SchemaID: 1,
            Type: job_type,
            ..Default::default()
        }));
        let err = addDDLJobs(sess, job);
        require::NoError(t, err);
    }
}

// TestCreateViewConcurrently 对应 Go 的并发 create-or-replace view 测试。
// failpoint 计数器确保同一 create view job 不会被多个 worker 同时执行。
#[test]
fn test_create_view_concurrently() {
    let store = testkit::CreateMockStore(t);
    let tk = testkit::NewTestKit(t, store);
    tk.MustExec("use test");

    tk.MustExec("create table t (a int);");
    tk.MustExec("create view v as select * from t;");
    let mut counter_err: Option<errors::Error> = None;
    let mut counter = 0;
    testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/onDDLCreateView", |job: *mut model::Job| {
        counter += 1;
        if counter > 1 {
            counter_err = Some(fmt::Errorf("create view job should not run concurrently"));
            return;
        }
    });
    testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterDeliveryJob", |job: *mut model::JobW| {
        // Go 在 job 投递完成后递减计数；这里只保留针对 ActionCreateView 的资源收尾语义。
        if unsafe { (*job).Type == model::ActionCreateView } {
            counter -= 1;
        }
    });
    let mut eg = errgroup::Group::default();
    for _ in 0..5 {
        eg.Go(|| {
            let new_tk = testkit::NewTestKit(t, store);
            let (_, err) = new_tk.Exec("use test");
            if err != nil {
                return err;
            }
            let (_, err) = new_tk.Exec("create or replace view v as select * from t;");
            err
        });
    }
    let err = eg.Wait();
    require::NoError(t, err);
    require::NoError(t, counter_err);
}

// TestCreateDropCreateTable 对应 Go 的 drop table 与再次 create table 时序测试。
// 它通过 failpoint 延慢 owner 检查，并用 goroutine 抢在 drop 完成期间提交第二个 create。
#[test]
fn test_create_drop_create_table() {
    let store = testkit::CreateMockStore(t);
    let tk = testkit::NewTestKit(t, store);
    tk.MustExec("use test");
    let tk1 = testkit::NewTestKit(t, store);
    tk1.MustExec("use test");

    tk.MustExec("create table t (a int);");

    let mut wg = sync::WaitGroup::new();
    let mut create_err: Option<errors::Error> = None;
    let mut fp_err: Option<errors::Error> = None;
    let mut create_table = false;

    testfailpoint::EnableCall(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced", |job: *mut model::Job| {
        if unsafe { (*job).Type == model::ActionDropTable && (*job).SchemaState == model::StateNone } && !create_table {
            fp_err = failpoint::Enable(
                "github.com/pingcap/tidb/pkg/ddl/schemaver/mockOwnerCheckAllVersionSlow",
                fmt::Sprintf("return({})", unsafe { (*job).ID }),
            );
            wg.Add(1);
            // Go 在这里启动 goroutine；保留并发窗口，不实际调度线程。
            go(|| {
                let (_, err) = tk1.Exec("create table t (b int);");
                create_err = err;
                wg.Done();
            });
            create_table = true;
        }
    });
    tk.MustExec("drop table t;");
    testfailpoint::Disable(t, "github.com/pingcap/tidb/pkg/ddl/afterWaitSchemaSynced");

    wg.Wait();
    require::True(t, create_table);
    require::NoError(t, create_err);
    require::NoError(t, fp_err);
    require::NoError(t, failpoint::Disable("github.com/pingcap/tidb/pkg/ddl/schemaver/mockOwnerCheckAllVersionSlow"));

    let rs = tk.MustQuery("admin show ddl jobs 3;").Rows();
    let create1_job_id = rs[0][0].clone().into_string();
    let drop_job_id = rs[1][0].clone().into_string();
    let create0_job_id = rs[2][0].clone().into_string();
    let (job_record_set, err) = tk.Exec(
        "select job_meta from mysql.tidb_ddl_history where job_id in (?, ?, ?);",
        create1_job_id,
        drop_job_id,
        create0_job_id,
    );
    require::NoError(t, err);

    let mut finish_tss: Vec<u64> = Vec::new();
    let req = job_record_set.NewChunk(nil);
    let err = job_record_set.Next(context::Background(), req);
    require::Greater(t, req.NumRows(), 0);
    require::NoError(t, err);
    let iter = chunk::NewIterator4Chunk(req.CopyConstruct());
    let mut row = iter.Begin();
    while row != iter.End() {
        let job_meta = row.GetBytes(0);
        let mut job = model::Job::default();
        let err = job.Decode(job_meta);
        require::NoError(t, err);
        finish_tss.push(job.BinlogInfo.FinishedTS);
        row = iter.Next();
    }
    let (create1_ts, drop_ts, create0_ts) = (finish_tss[0], finish_tss[1], finish_tss[2]);
    require::Less(t, create0_ts, drop_ts, "first create should finish before drop");
    require::Less(t, drop_ts, create1_ts, "second create should finish after drop");
}

// TestHandleLockTable 对应 Go 的锁表收尾测试：truncate 成功提交后要把锁从旧 table ID
// 迁移到新 table ID；失败时则回滚新锁并保留旧锁。
#[test]
fn test_handle_lock_table() {
    let store = testkit::CreateMockStore(t);
    let tk = testkit::NewTestKit(t, store);
    let se = tk.Session().as_sessionctx_context();
    require::False(t, se.HasLockedTables());

    let check_table_locked = |tbl_id: i64, tp: ast::TableLockType| {
        let (locked, lock_type) = se.CheckTableLocked(tbl_id);
        require::True(t, locked);
        require::Equal(t, tp, lock_type);
    };
    let job = Box::into_raw(Box::new(model::Job {
        Version: model::GetJobVerInUse(),
        Type: model::ActionTruncateTable,
        TableID: 1,
        ..Default::default()
    }));
    let job_w = ddl::NewJobWrapperWithArgs(
        job,
        &model::TruncateTableArgs { NewTableID: 2 },
        false,
    );

    subtest("target table not locked", || {
        se.ReleaseAllTableLocks();
        ddl::HandleLockTablesOnSuccessSubmit(tk.Session(), job_w);
        require::False(t, se.HasLockedTables());
        ddl::HandleLockTablesOnFinish(se, job_w, errors::New("test error"));
        require::False(t, se.HasLockedTables());

        ddl::HandleLockTablesOnSuccessSubmit(tk.Session(), job_w);
        require::False(t, se.HasLockedTables());
        ddl::HandleLockTablesOnFinish(se, job_w, nil);
        require::False(t, se.HasLockedTables());
    });

    subtest("ddl success", || {
        se.ReleaseAllTableLocks();
        require::False(t, se.HasLockedTables());
        se.AddTableLock(vec![model::TableLockTpInfo { SchemaID: 1, TableID: 1, Tp: ast::TableLockRead }]);
        ddl::HandleLockTablesOnSuccessSubmit(tk.Session(), job_w);
        require::Len(t, se.GetAllTableLocks(), 2);
        check_table_locked(1, ast::TableLockRead);
        check_table_locked(2, ast::TableLockRead);

        ddl::HandleLockTablesOnFinish(se, job_w, nil);
        require::Len(t, se.GetAllTableLocks(), 1);
        check_table_locked(2, ast::TableLockRead);
    });

    subtest("ddl fail", || {
        se.ReleaseAllTableLocks();
        require::False(t, se.HasLockedTables());
        se.AddTableLock(vec![model::TableLockTpInfo { SchemaID: 1, TableID: 1, Tp: ast::TableLockRead }]);
        ddl::HandleLockTablesOnSuccessSubmit(tk.Session(), job_w);
        require::Len(t, se.GetAllTableLocks(), 2);
        check_table_locked(1, ast::TableLockRead);
        check_table_locked(2, ast::TableLockRead);

        // Go 传入错误时代表 DDL 失败，HandleLockTablesOnFinish 应释放新 table 的锁并恢复旧锁。
        ddl::HandleLockTablesOnFinish(se, job_w, errors::New("test error"));
        require::Len(t, se.GetAllTableLocks(), 1);
        check_table_locked(1, ast::TableLockRead);
    });
}
*/

// 以下 `use` 与测试函数组成当前真正参与编译和执行的 Rust 测试实现；
// 上面的块注释仅保留 Go 版本原始意图，便于逐段对照迁移。
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
