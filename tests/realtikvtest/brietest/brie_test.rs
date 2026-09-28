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

// 中文总览：本文件承担 BR 备份恢复、日志备份、注册表与调度器 中的 场景回归集合。
// 中文总览：重点在于测试意图、环境搭建、关键动作和最终观察点。
// 中文总览：当前任务只补充注释，不改任何 SQL、断言、参数或桩实现。
// 中文总览：阅读时可先看公共搭建，再看核心动作，最后看状态观察和收尾清理。
// 中文总览：这类 RealTiKV 回归通常依赖时序、共享状态和外部副作用，因此顺序本身就是语义。
// 中文总览：Rust 版本继续保留与 Go 对照实现接近的行为边界，避免迁移后只剩表面通过。
// 中文总览：注释会优先解释为什么这样验证，而不是逐行翻译语法或重复函数名。
// 中文总览：下面的索引用于快速定位 helper、公共模块和场景 case 的职责分工。
// 中文总览：函数 `insert_hello_world` 负责 insert hello world。
// 中文总览：函数 `test_show_backup_query` 负责 show 备份 query。
// 中文总览：函数 `test_show_backup_query_redact` 负责 show 备份 query redact。
// 中文总览：函数 `test_cancel` 负责 取消。

//! Go-equivalent tests for `brie_test.go`.
//!
//! Mapping:
//! - `TestShowBackupQuery` → [`test_show_backup_query`]
//! - `TestShowBackupQueryRedact` → [`test_show_backup_query_redact`]
//! - `TestCancel` → [`test_cancel`]
//! - `TestExistedTables` → [`test_existed_tables`]
//! - `TestExistedTablesOfIncremental` → [`test_existed_tables_of_incremental`]
//! - `TestExistedTablesOfIncremental_1` → [`test_existed_tables_of_incremental_1`]
//! - `TestExistedTablesOfIncremental_2` → [`test_existed_tables_of_incremental_2`]

use astersql_tests_realtikvtest_brietest::harness::{
    TestCtx, create_store, executor, failpoint, init_test_kit, logutil, make_temp_dir_for_backup,
    require, reset_engine, serial_guard, testkit, zapcore,
};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

fn run_with_operation_timeout<T: Send + 'static>(
    operation: &'static str,
    timeout: Duration,
    action: impl FnOnce() -> T + Send + 'static,
) -> T {
    let (tx, rx) = mpsc::sync_channel(1);
    let worker = thread::spawn(move || {
        let value = action();
        let _ = tx.send(value);
    });
    match rx.recv_timeout(timeout) {
        Ok(value) => {
            worker.join().expect("BR operation worker panicked");
            value
        }
        Err(mpsc::RecvTimeoutError::Timeout) => panic!("{operation} operation exceeded"),
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            worker.join().expect("BR operation worker panicked");
            unreachable!("BR operation worker exited without a result")
        }
    }
}

#[test]
fn operation_timeout_matches_go_guard() {
    let failure = std::panic::catch_unwind(|| {
        run_with_operation_timeout("Backup", Duration::from_millis(10), || {
            thread::sleep(Duration::from_millis(100));
        });
    })
    .expect_err("a stalled BR operation must trip the Go-equivalent timeout");
    let message = failure
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| failure.downcast_ref::<&str>().copied())
        .unwrap_or_default();
    assert!(message.contains("Backup operation exceeded"), "{message}");
}

// 该辅助函数负责 insert hello world。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。

fn insert_hello_world(tk: &testkit::TestKit, table: &str, n: usize) {
    let vals = (0..n)
        .map(|_| "('hello, world')")
        .collect::<Vec<_>>()
        .join(",");
    tk.MustExec(&format!("insert into {table}(v) values {vals};"));
}

/// `TestShowBackupQuery`.
// 该用例覆盖 show 备份 query。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_show_backup_query() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let tk = init_test_kit(&t);
    executor::ResetGlobalBRIEQueueForTest();
    let tmp = make_temp_dir_for_backup(&t);
    let sql_tmp = tmp.replace('\'', "''");
    logutil::OverrideLevelForTest(&t, zapcore::ErrorLevel);
    tk.MustExec("use test;");
    tk.MustExec("create table foo(pk int primary key auto_increment, v varchar(255));");
    insert_hello_world(&tk, "foo", 100);
    let backup_query = format!("BACKUP DATABASE * TO 'local://{sql_tmp}'");
    let _ = tk.MustQuery(&backup_query);
    let res = tk.MustQuery("show br job query 1;");
    res.CheckContain(&backup_query);

    tk.MustExec("drop table foo;");
    let restore_query = format!("RESTORE TABLE `test`.`foo` FROM 'local://{sql_tmp}'");
    tk.MustQuery(&restore_query);
    let res = tk.MustQuery("show br job query 2;");
    tk.MustExec("drop table foo;");
    res.CheckContain(&restore_query);
}

/// `TestShowBackupQueryRedact`.
// 该用例覆盖 show 备份 query redact。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_show_backup_query_redact() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let tk = init_test_kit(&t);
    executor::ResetGlobalBRIEQueueForTest();
    require::NoError(
        &t,
        failpoint::Enable(
            "github.com/pingcap/tidb/pkg/executor/block-on-brie",
            "return",
        ),
    );
    t.Cleanup(|| {
        let _ = failpoint::Disable("github.com/pingcap/tidb/pkg/executor/block-on-brie");
    });

    let done = Arc::new(Mutex::new(false));
    let done2 = done.clone();
    let store = tk.Session().GetStore();
    let t2 = t.clone();
    let h = thread::spawn(move || {
        let tk2 = testkit::NewTestKit(&t2, store);
        let err = tk2.QueryToErr(
            "backup database * to 's3://nonexist/real?endpoint=http://127.0.0.1&access-key=notleaked&secret-access-key=notleaked'",
        );
        assert!(err.is_err());
        *done2.lock().unwrap() = true;
    });

    require::Eventually(
        &t,
        || {
            let res = tk.MustQuery("show br job query 1;");
            let rs = res.Rows();
            if rs.is_empty() {
                return false;
            }
            let the_item = &rs[0][0];
            assert!(
                !the_item.contains("secret-access-key"),
                "The secret key not redacted: {the_item:?}"
            );
            res.CheckContain("BACKUP DATABASE * TO 's3://nonexist/real'");
            true
        },
        Duration::from_secs(5),
        Duration::from_millis(50),
    );
    tk.MustExec("cancel br job 1;");
    h.join().unwrap();
    assert!(*done.lock().unwrap());
}

/// `TestCancel`.
// 该用例覆盖 取消。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_cancel() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let tk = init_test_kit(&t);
    executor::ResetGlobalBRIEQueueForTest();
    tk.MustExec("use test;");
    require::NoError(
        &t,
        failpoint::Enable(
            "github.com/pingcap/tidb/pkg/executor/block-on-brie",
            "return",
        ),
    );
    t.Cleanup(|| {
        let _ = failpoint::Disable("github.com/pingcap/tidb/pkg/executor/block-on-brie");
    });

    let done = Arc::new(Mutex::new(false));
    let done2 = done.clone();
    let store = tk.Session().GetStore();
    let t2 = t.clone();
    let h = thread::spawn(move || {
        let tk2 = testkit::NewTestKit(&t2, store);
        let err = tk2.QueryToErr("backup database * to 'noop://'");
        assert!(err.is_err());
        *done2.lock().unwrap() = true;
    });

    require::Eventually(
        &t,
        || {
            let wb = tk.Session().GetSessionVars().StmtCtx.WarningCount();
            tk.MustExec("cancel br job 1;");
            let wa = tk.Session().GetSessionVars().StmtCtx.WarningCount();
            wb == wa
        },
        Duration::from_secs(5),
        Duration::from_millis(50),
    );

    let start = std::time::Instant::now();
    while !*done.lock().unwrap() {
        if start.elapsed() > Duration::from_secs(5) {
            require::FailNow(&t, "the backup job doesn't be canceled");
        }
        thread::sleep(Duration::from_millis(20));
    }
    h.join().unwrap();
}

/// `TestExistedTables`.
// 该用例覆盖 existed 表集合。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_existed_tables() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let tk = init_test_kit(&t);
    let tmp = make_temp_dir_for_backup(&t);
    let sql_tmp = tmp.replace('\'', "''");
    executor::ResetGlobalBRIEQueueForTest();
    tk.MustExec("use test;");
    for i in 0..5 {
        let table_name = format!("foo{i}");
        tk.MustExec(&format!(
            "create table {table_name}(pk int primary key auto_increment, v varchar(255));"
        ));
        insert_hello_world(&tk, &table_name, 100);
    }

    let backup_query = format!("BACKUP DATABASE * TO 'local://{sql_tmp}/full'");
    let backup_tk = tk.clone();
    run_with_operation_timeout("Backup", Duration::from_secs(20), move || {
        let _ = backup_tk.MustQuery(&backup_query);
    });

    let restore_query = format!("RESTORE DATABASE * FROM 'local://{sql_tmp}/full'");
    let restore_tk = tk.clone();
    let err = run_with_operation_timeout("Restore", Duration::from_secs(20), move || {
        let res = restore_tk.Exec(&restore_query).expect("exec ok");
        astersql_tests_realtikvtest_brietest::harness::session::ResultSetToStringSlice(
            (),
            &restore_tk.Session(),
            res,
        )
        .map(|_| ())
    });
    require::ErrorContains(&t, err, "table already exists");

    let backup_query = format!("BACKUP DATABASE test TO 'local://{sql_tmp}/db'");
    let backup_tk = tk.clone();
    run_with_operation_timeout("Backup", Duration::from_secs(20), move || {
        let _ = backup_tk.MustQuery(&backup_query);
    });
    let restore_query = format!("Restore DATABASE test FROM 'local://{sql_tmp}/db'");
    let restore_tk = tk.clone();
    let err = run_with_operation_timeout("Restore", Duration::from_secs(20), move || {
        let res = restore_tk.Exec(&restore_query).expect("exec ok");
        astersql_tests_realtikvtest_brietest::harness::session::ResultSetToStringSlice(
            (),
            &restore_tk.Session(),
            res,
        )
        .map(|_| ())
    });
    require::ErrorContains(&t, err, "table already exists");

    for i in 0..5 {
        tk.MustExec(&format!("drop table foo{i};"));
    }
}

// 该辅助函数负责 incremental flow。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。

fn incremental_flow(db_backup: &str, db_restore_full: &str, db_restore_incr: &str) {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let tk = init_test_kit(&t);
    let tmp = make_temp_dir_for_backup(&t);
    let sql_tmp = tmp.replace('\'', "''");
    executor::ResetGlobalBRIEQueueForTest();
    tk.MustExec("use test;");
    for i in 0..5 {
        let table_name = format!("foo{i}");
        tk.MustExec(&format!(
            "create table {table_name}(pk int primary key auto_increment, v varchar(255));"
        ));
        insert_hello_world(&tk, &table_name, 100);
    }
    let backup_query = format!("BACKUP DATABASE {db_backup} TO 'local://{sql_tmp}/full'");
    let res = tk.MustQuery(&backup_query);
    let backup_ts = res.Rows()[0][2].clone();
    for i in 0..5 {
        insert_hello_world(&tk, &format!("foo{i}"), 100);
    }
    let incr = format!(
        "BACKUP DATABASE {db_backup} TO 'local://{sql_tmp}/incremental' last_backup={backup_ts}"
    );
    let _ = tk.MustQuery(&incr);
    for i in 0..5 {
        tk.MustExec(&format!("drop table foo{i};"));
    }
    let _ = tk.MustQuery(&format!(
        "RESTORE DATABASE {db_restore_full} FROM 'local://{sql_tmp}/full'"
    ));
    let _ = tk.MustQuery(&format!(
        "RESTORE DATABASE {db_restore_incr} FROM 'local://{sql_tmp}/incremental'"
    ));
    for i in 0..5 {
        tk.MustExec(&format!("drop table foo{i};"));
    }
}

/// `TestExistedTablesOfIncremental`.
// 该用例覆盖 existed 表集合 of incremental。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_existed_tables_of_incremental() {
    incremental_flow("*", "*", "*");
}

/// `TestExistedTablesOfIncremental_1`.
// 该用例覆盖 existed 表集合 of incremental 1。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_existed_tables_of_incremental_1() {
    incremental_flow("*", "test", "test");
}

/// `TestExistedTablesOfIncremental_2`.
// 该用例覆盖 existed 表集合 of incremental 2。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_existed_tables_of_incremental_2() {
    incremental_flow("test", "*", "*");
}
