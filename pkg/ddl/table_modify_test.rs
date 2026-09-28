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

// 表只读（read only）与 `LOCK TABLES` 互斥行为的单元测试。
//
// 可执行部分用 [`TableAccessController`] 模拟访问控制：
// read only 禁止写入与各类锁表；已有锁禁止切 read only；写锁互斥规则对齐 Go。
// 注释块保留依赖 testkit/failpoint 的集成测试迁移草稿。

// Copyright 2026 AsterSQL.
/*
// 表 read only 与 lock tables 的互斥行为，以及通过 failpoint 控制并发 DDL 队列的测试流程。

// tableModifyLease 对应 Go 常量：使用较短 schema lease 让 DDL 状态在测试中更快推进。
const tableModifyLease: time::Duration = 600 * time::Millisecond;

// test_lock_table_read_only 对应 Go 的 TestLockTableReadOnly。
// 它覆盖 read only 表禁止写入、禁止锁表，以及已有 lock tables 时禁止切 read only。
#[test]
fn test_lock_table_read_only() {
    let store = testkit::CreateMockStoreWithSchemaLease(testing::T, tableModifyLease);
    let tk1 = testkit::NewTestKit(testing::T, store);
    let tk2 = testkit::NewTestKit(testing::T, store);
    tk1.MustExec("use test");
    tk2.MustExec("use test");
    tk1.MustExec("drop table if exists t1,t2");
    defer(|| {
        // Go defer 在测试结束前把 read only 表恢复为 read write，再清理表。
        tk1.MustExec("alter table t1 read write");
        tk1.MustExec("alter table t2 read write");
        tk1.MustExec("drop table if exists t1,t2");
    });
    tk1.MustExec("create table t1 (a int key, b int)");
    tk1.MustExec("create table t2 (a int key)");

    tk1.MustExec("alter table t1 read only");
    tk1.MustQuery("select * from t1");
    tk2.MustQuery("select * from t1");

    // 同一 session 和另一 session 对 read only 表的 DML 都应返回 infoschema.ErrTableLocked。
    for (tk, sql) in [
        (&tk1, "insert into t1 set a=1, b=2"),
        (&tk1, "update t1 set a=1"),
        (&tk1, "delete from t1"),
        (&tk2, "insert into t1 set a=1, b=2"),
        (&tk2, "update t1 set a=1"),
        (&tk2, "delete from t1"),
    ] {
        require::True(
            testing::T,
            terror::ErrorEqual(tk.ExecToErr(sql), infoschema::ErrTableLocked),
        );
    }

    // 重复设置 read only 允许成功，但写入仍然被锁保护。
    tk2.MustExec("alter table t1 read only");
    require::True(
        testing::T,
        terror::ErrorEqual(tk2.ExecToErr("insert into t1 set a=1, b=2"), infoschema::ErrTableLocked),
    );

    // 已有 read/write/write local table lock 时，两个 session 都不能把表切为 read only。
    tk1.MustExec("alter table t1 read write");
    for lock_sql in ["lock tables t1 read", "lock tables t1 write", "lock tables t1 write local"] {
        tk1.MustExec(lock_sql);
        require::True(
            testing::T,
            terror::ErrorEqual(tk1.ExecToErr("alter table t1 read only"), infoschema::ErrTableLocked),
        );
        require::True(
            testing::T,
            terror::ErrorEqual(tk2.ExecToErr("alter table t1 read only"), infoschema::ErrTableLocked),
        );
    }
    tk1.MustExec("unlock tables");

    // read only 表反过来也禁止 lock tables，直到 admin cleanup table lock 清理元数据。
    tk1.MustExec("alter table t1 read only");
    for lock_sql in ["lock tables t1 read", "lock tables t1 write", "lock tables t1 write local"] {
        require::True(
            testing::T,
            terror::ErrorEqual(tk1.ExecToErr(lock_sql), infoschema::ErrTableLocked),
        );
        require::True(
            testing::T,
            terror::ErrorEqual(tk2.ExecToErr(lock_sql), infoschema::ErrTableLocked),
        );
    }
    tk1.MustExec("admin cleanup table lock t1");
    tk2.MustExec("insert into t1 set a=1, b=2");
}
*/

use crate::executor::{ExecutorError, TableAccessController, TableLockType};

/// 验证 read only 禁止写入与全部锁表模式，cleanup 后写权限恢复。
#[test]
fn read_only_blocks_writes_and_all_table_lock_modes() {
    let mut access = TableAccessController::default();
    // 首次设置 read only 返回 true（状态变更）；重复设置幂等返回 false。
    assert!(access.set_read_only(true).unwrap());
    assert!(!access.set_read_only(true).unwrap());
    assert_eq!(Err(ExecutorError::LockConflict), access.check_write());
    for lock_type in [
        TableLockType::Read,
        TableLockType::Write,
        TableLockType::WriteLocal,
    ] {
        assert_eq!(Err(ExecutorError::LockConflict), access.lock(1, lock_type));
    }
    // cleanup 清除只读/锁元数据后允许写入。
    access.cleanup();
    assert_eq!(Ok(()), access.check_write());
}

/// 验证已有锁禁止切 read only，以及写锁/本地写锁的互斥兼容性。
#[test]
fn existing_locks_block_read_only_and_lock_compatibility_matches_go() {
    let mut access = TableAccessController::default();
    // 多个共享读锁可并存，但整体禁止切 read only，也禁止再加写锁。
    access.lock(1, TableLockType::Read).unwrap();
    access.lock(2, TableLockType::Read).unwrap();
    assert_eq!(2, access.lock_count());
    assert_eq!(Err(ExecutorError::LockConflict), access.set_read_only(true));
    assert_eq!(
        Err(ExecutorError::LockConflict),
        access.lock(3, TableLockType::Write)
    );
    assert!(access.unlock(1));
    assert!(access.unlock(2));

    // Write 与 WriteLocal 均排他：第二会话同类型加锁应冲突。
    access.lock(1, TableLockType::Write).unwrap();
    assert_eq!(
        Err(ExecutorError::LockConflict),
        access.lock(2, TableLockType::Write)
    );
    assert!(access.unlock(1));
    access.lock(1, TableLockType::WriteLocal).unwrap();
    assert_eq!(
        Err(ExecutorError::LockConflict),
        access.lock(2, TableLockType::WriteLocal)
    );
}

/// Go 测试会在不执行 UNLOCK 的情况下，让同一会话依次把 READ 锁替换为
/// WRITE 和 WRITE LOCAL；换锁应成功，同时仍拒绝另一会话取得排他锁。
#[test]
fn same_connection_can_replace_its_table_lock_like_go() {
    let mut access = TableAccessController::default();

    access.lock(1, TableLockType::Read).unwrap();
    access.lock(1, TableLockType::Write).unwrap();
    assert_eq!(1, access.lock_count());
    assert_eq!(
        Err(ExecutorError::LockConflict),
        access.lock(2, TableLockType::Write)
    );

    access.lock(1, TableLockType::WriteLocal).unwrap();
    assert_eq!(1, access.lock_count());
    assert_eq!(
        Err(ExecutorError::LockConflict),
        access.lock(2, TableLockType::WriteLocal)
    );
}

/*
// test_concurrent_lock_tables 对应 Go 的 TestConcurrentLockTables：测试并发 lock/unlock tables。
#[test]
fn test_concurrent_lock_tables() {
    let store = testkit::CreateMockStoreWithSchemaLease(testing::T, tableModifyLease);
    let tk1 = testkit::NewTestKit(testing::T, store);
    let tk2 = testkit::NewTestKit(testing::T, store);
    tk1.MustExec("use test");
    tk2.MustExec("use test");
    tk1.MustExec("create table t1 (a int)");

    // Test concurrent lock tables read.
    test_parallel_exec_sql(
        testing::T,
        store,
        "lock tables t1 read",
        "lock tables t1 read",
        tk1.Session(),
        tk2.Session(),
        |t, err1, err2| {
            require::NoError(t, err1);
            require::NoError(t, err2);
        },
    );
    tk1.MustExec("unlock tables");
    tk2.MustExec("unlock tables");

    // Test concurrent lock tables write.
    test_parallel_exec_sql(
        testing::T,
        store,
        "lock tables t1 write",
        "lock tables t1 write",
        tk1.Session(),
        tk2.Session(),
        |t, err1, err2| {
            require::NoError(t, err1);
            require::True(t, terror::ErrorEqual(err2, infoschema::ErrTableLocked));
        },
    );
    tk1.MustExec("unlock tables");
    tk2.MustExec("unlock tables");

    // Test concurrent lock tables write local.
    test_parallel_exec_sql(
        testing::T,
        store,
        "lock tables t1 write local",
        "lock tables t1 write local",
        tk1.Session(),
        tk2.Session(),
        |t, err1, err2| {
            require::NoError(t, err1);
            require::True(t, terror::ErrorEqual(err2, infoschema::ErrTableLocked));
        },
    );
    tk1.MustExec("unlock tables");
    tk2.MustExec("unlock tables");
}

// test_parallel_exec_sql 对应 Go helper：让 sql1 先进入 DDLJobQueue，再放行 sql2，最后检查两个结果。
fn test_parallel_exec_sql<F>(
    t: testing::T,
    store: kv::Storage,
    sql1: &str,
    sql2: &str,
    se1: sessionapi::Session,
    se2: sessionapi::Session,
    f: F,
) where
    F: Fn(testing::T, errors::Error, errors::Error),
{
    let mut times = 0;
    let ctx = context::Background();

    // beforeRunOneJobStep failpoint 在第一个 DDL job 执行前轮询队列，确保两个 job 都已经排队。
    testfailpoint::EnableCall(
        t,
        "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep",
        |job: *mut model::Job| {
            if times != 0 {
                return;
            }
            let mut q_len = 0;
            loop {
                let sess = testkit::NewTestKit(t, store).Session();
                sessiontxn::NewTxn(ctx, sess).expect("Go require.NoError");
                let jobs = ddl::GetAllDDLJobs(ctx, sess).expect("Go require.NoError");
                q_len = jobs.len();
                if q_len == 2 {
                    break;
                }
                time::Sleep(5 * time::Millisecond);
            }
            times += 1;
        },
    );
    defer(|| {
        testfailpoint::Disable(t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep");
    });

    let mut wg = util::WaitGroupWrapper::new();
    let mut err1 = nil;
    let mut err2 = nil;
    let ch = channel::make::<()>();

    // Make sure the sql1 is put into the DDLJobQueue.
    go(|| {
        loop {
            let sess = testkit::NewTestKit(t, store).Session();
            sessiontxn::NewTxn(ctx, sess).expect("Go require.NoError");
            let jobs = ddl::GetAllDDLJobs(ctx, sess).expect("Go require.NoError");
            if jobs.len() == 1 {
                // Make sure sql2 is executed after the sql1.
                ch.close();
                break;
            }
            time::Sleep(5 * time::Millisecond);
        }
    });

    // 两条 SQL 分别在独立 session 执行；Go 通过 WaitGroupWrapper 收集异步错误。
    wg.Run(|| {
        (_, err1) = se1.Execute(context::Background(), sql1);
    });
    wg.Run(|| {
        ch.recv();
        (_, err2) = se2.Execute(context::Background(), sql2);
    });

    wg.Wait();
    f(t, err1, err2);
}
*/
