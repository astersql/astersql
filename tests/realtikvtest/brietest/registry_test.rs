// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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
// 中文总览：函数 `init_registry_test` 负责 初始化 registry test。
// 中文总览：函数 `cleanup_registry_table` 负责 清理 registry 表。
// 中文总览：函数 `test_registry_basic_operations` 负责 注册表 基础场景 operations。
// 中文总览：函数 `test_registry_configuration_operations` 负责 注册表 configuration operations。
// 中文总览：函数 `test_registry_table_conflicts` 负责 注册表 表 conflicts。
// 中文总览：函数 `test_prevent_concurrent_restore_of_the_same_database` 负责 prevent concurrent 恢复 of the same database。
// 中文总览：类型 `Case` 负责 Case。
// 中文总览：函数 `test_get_registrations_by_max_id` 负责 读取 registrations by max id。

//! Go-equivalent tests for `registry_test.go`.
//!
//! Mapping:
//! - `TestRegistryBasicOperations` → [`test_registry_basic_operations`]
//! - `TestRegistryConfigurationOperations` → [`test_registry_configuration_operations`]
//! - `TestRegistryTableConflicts` → [`test_registry_table_conflicts`]
//! - `TestPreventConcurrentRestoreOfTheSameDatabase` → [`test_prevent_concurrent_restore_of_the_same_database`]
//! - `TestGetRegistrationsByMaxID` → [`test_get_registrations_by_max_id`]

use astersql_tests_realtikvtest_brietest::harness::{
    MemGlue, SetWithRealTiKV, TestCtx, WithRealTiKV, ast, config, create_store, gluetidb, metautil,
    model, registry, require, reset_engine, serial_guard, testfailpoint, testkit, utils,
};
use std::sync::{Arc, Mutex};
use std::thread;

// 该辅助函数负责 初始化 registry test。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
// 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。
// 注释强调的是为什么要独立封装，而不是简单翻译函数名本身。
// 当调用方继续传递这里返回的对象时，通常表示后续还要观察同一份上下文的中间状态。
// 因而本函数的价值不只是“返回一个值”，还包括固定测试时序和可观察性边界。
// 维护者若要扩展行为，优先在这里集中修改，避免不同 case 出现不一致的准备顺序。

fn init_registry_test(t: &TestCtx) -> (testkit::TestKit, registry::DomainHandle, MemGlue) {
    SetWithRealTiKV(true);
    if !WithRealTiKV() {
        panic!("skip: only run BR SQL integration test with tikv store");
    }
    let store = create_store(t);
    config::UpdateGlobal(|cfg| {
        cfg.Store = config::StoreTypeTiKV.to_string();
        cfg.Path = "127.0.0.1:2379".into();
    });
    let tk = testkit::NewTestKit(t, store.clone());
    let dom = registry::domain_from_store(&store);
    let g = gluetidb::New();
    (tk, dom, g)
}

// 该辅助函数负责 清理 registry 表。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
// 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。
// 注释强调的是为什么要独立封装，而不是简单翻译函数名本身。
// 当调用方继续传递这里返回的对象时，通常表示后续还要观察同一份上下文的中间状态。
// 因而本函数的价值不只是“返回一个值”，还包括固定测试时序和可观察性边界。
// 维护者若要扩展行为，优先在这里集中修改，避免不同 case 出现不一致的准备顺序。

fn cleanup_registry_table(tk: &testkit::TestKit) {
    tk.MustExec(&format!(
        "DELETE FROM {}.{}",
        registry::RestoreRegistryDBName,
        registry::RestoreRegistryTableName
    ));
}

/// `TestRegistryBasicOperations`.
// 该用例覆盖 注册表 基础场景 operations。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。
// Rust 版本继续复用与 Go 对照用例相近的 helper，方便双端定位行为差异。
// 对于异步任务、外部存储或全局配置，本 case 的成功标准通常是多层状态同时一致。
// 这比单纯检查返回值更接近真实产品语义，也是迁移测试最需要保留的部分。

#[test]
fn test_registry_basic_operations() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let (tk, dom, g) = init_registry_test(&t);
    cleanup_registry_table(&tk);
    testfailpoint::Enable(
        &t,
        "github.com/pingcap/tidb/br/pkg/registry/is-task-stale-ticker-duration",
        "return(1)",
    );
    let mut r = registry::NewRestoreRegistry((), &g, &dom).unwrap();
    let info = registry::RegistrationInfo {
        FilterStrings: vec!["db.table".into()],
        StartTS: 100,
        RestoredTS: 200,
        UpstreamClusterID: 1,
        WithSysTable: true,
        Cmd: "restore".into(),
    };
    let (restore_id, resolved) = r
        .ResumeOrCreateRegistration((), info.clone(), true)
        .unwrap();
    require::Greater(&t, restore_id, 0u64);
    require::Equal(&t, 200u64, resolved);

    tk.MustExec(&format!(
        "SHOW DATABASES LIKE '{}'",
        registry::RestoreRegistryDBName
    ));
    tk.MustExec(&format!("USE {}", registry::RestoreRegistryDBName));
    let rows = tk.MustQuery(&format!(
        "SELECT id, filter_strings, status, restored_ts FROM {} WHERE id = {}",
        registry::RestoreRegistryTableName,
        restore_id
    ));
    let row = &rows.Rows()[0];
    require::Equal(&t, restore_id.to_string(), row[0].clone());
    require::Equal(&t, "db.table".to_string(), row[1].clone());
    require::Equal(&t, "running".to_string(), row[2].clone());
    require::Equal(&t, "200".to_string(), row[3].clone());

    require::NoError(&t, r.PauseTask((), restore_id));
    let rows = tk.MustQuery(&format!(
        "SELECT status FROM {} WHERE id = {}",
        registry::RestoreRegistryTableName,
        restore_id
    ));
    require::Equal(&t, "paused".to_string(), rows.Rows()[0][0].clone());

    let info_diff = registry::RegistrationInfo {
        RestoredTS: 999,
        ..info.clone()
    };
    let (resumed, resolved2) = r.ResumeOrCreateRegistration((), info_diff, false).unwrap();
    require::Equal(&t, restore_id, resumed);
    require::Equal(&t, 200u64, resolved2);
    require::NotEqual(&t, 999u64, resolved2);
    let rows = tk.MustQuery(&format!(
        "SELECT status FROM {} WHERE id = {}",
        registry::RestoreRegistryTableName,
        restore_id
    ));
    require::Equal(&t, "running".to_string(), rows.Rows()[0][0].clone());

    require::NoError(&t, r.PauseTask((), restore_id));
    let info_same = registry::RegistrationInfo {
        RestoredTS: 200,
        ..info.clone()
    };
    let (resumed2, resolved3) = r.ResumeOrCreateRegistration((), info_same, true).unwrap();
    require::Equal(&t, restore_id, resumed2);
    require::Equal(&t, 200u64, resolved3);

    require::NoError(&t, r.PauseTask((), restore_id));
    let (resumed3, resolved4) = r
        .ResumeOrCreateRegistration((), info.clone(), false)
        .unwrap();
    require::Equal(&t, restore_id, resumed3);
    require::Equal(&t, 200u64, resolved4);

    let (resumed5, resolved5) = r
        .ResumeOrCreateRegistration((), info.clone(), true)
        .unwrap();
    require::Equal(&t, restore_id, resumed5);
    require::Equal(&t, 200u64, resolved5);

    let info_new = registry::RegistrationInfo {
        FilterStrings: vec!["different.table".into()],
        StartTS: 100,
        RestoredTS: 888,
        UpstreamClusterID: 1,
        WithSysTable: true,
        Cmd: "restore".into(),
    };
    let (new_id, resolved_new) = r.ResumeOrCreateRegistration((), info_new, false).unwrap();
    require::NotEqual(&t, restore_id, new_id);
    require::Equal(&t, 888u64, resolved_new);

    require::NoError(&t, r.Unregister((), restore_id));
    require::NoError(&t, r.Unregister((), new_id));
    let rows = tk.MustQuery(&format!(
        "SELECT COUNT(*) FROM {}.{} WHERE id IN ({}, {})",
        registry::RestoreRegistryDBName,
        registry::RestoreRegistryTableName,
        restore_id,
        new_id
    ));
    require::Equal(&t, "0".to_string(), rows.Rows()[0][0].clone());
    r.Close();
}

/// `TestRegistryConfigurationOperations`.
// 该用例覆盖 注册表 configuration operations。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。
// Rust 版本继续复用与 Go 对照用例相近的 helper，方便双端定位行为差异。
// 对于异步任务、外部存储或全局配置，本 case 的成功标准通常是多层状态同时一致。
// 这比单纯检查返回值更接近真实产品语义，也是迁移测试最需要保留的部分。

#[test]
fn test_registry_configuration_operations() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let (tk, dom, g) = init_registry_test(&t);
    cleanup_registry_table(&tk);
    let mut r1 = registry::NewRestoreRegistry((), &g, &dom).unwrap();
    let mut r2 = registry::NewRestoreRegistry((), &g, &dom).unwrap();

    let mut put_and_drop = || {
        let info1 = registry::RegistrationInfo {
            FilterStrings: vec!["db1.table1".into()],
            StartTS: 100,
            RestoredTS: 200,
            UpstreamClusterID: 1,
            WithSysTable: true,
            Cmd: "restore".into(),
        };
        let info2 = registry::RegistrationInfo {
            FilterStrings: vec!["db2.table2".into()],
            StartTS: 100,
            RestoredTS: 200,
            UpstreamClusterID: 1,
            WithSysTable: true,
            Cmd: "restore".into(),
        };
        let (restore_id1, _) = r1.ResumeOrCreateRegistration((), info1, false).unwrap();
        let k = Arc::new(Mutex::new(1i32));
        let restore_id2 = thread::scope(|scope| {
            let k2 = k.clone();
            let r2 = &mut r2;
            let create_and_wait = scope.spawn(move || {
                let (id2, _) = r2.ResumeOrCreateRegistration((), info2, false).unwrap();
                r2.OperationAfterWaitIDs((), || {
                    *k2.lock().unwrap() = -1;
                    Ok(())
                })
                .unwrap();
                id2
            });
            let k3 = k.clone();
            let r1 = &mut r1;
            scope.spawn(move || {
                r1.GlobalOperationAfterSetResettingStatus((), restore_id1, || {
                    *k3.lock().unwrap() = 1;
                    Ok(())
                })
                .unwrap();
                r1.Unregister((), restore_id1).unwrap();
            });
            create_and_wait.join().unwrap()
        });
        require::Equal(&t, -1i32, *k.lock().unwrap());
        r2.Unregister((), restore_id2).unwrap();
    };
    put_and_drop();

    let mut drop_and_drop = || {
        let info1 = registry::RegistrationInfo {
            FilterStrings: vec!["db1.table1".into()],
            StartTS: 100,
            RestoredTS: 200,
            UpstreamClusterID: 1,
            WithSysTable: true,
            Cmd: "restore".into(),
        };
        let info2 = registry::RegistrationInfo {
            FilterStrings: vec!["db2.table2".into()],
            StartTS: 100,
            RestoredTS: 200,
            UpstreamClusterID: 1,
            WithSysTable: true,
            Cmd: "restore".into(),
        };
        let (id1, _) = r1.ResumeOrCreateRegistration((), info1, false).unwrap();
        let (id2, _) = r2.ResumeOrCreateRegistration((), info2, false).unwrap();
        let k = Arc::new(Mutex::new(-1i32));
        thread::scope(|scope| {
            let k1 = k.clone();
            let r1 = &mut r1;
            scope.spawn(move || {
                r1.GlobalOperationAfterSetResettingStatus((), id1, || {
                    *k1.lock().unwrap() = 1;
                    Ok(())
                })
                .unwrap();
                r1.Unregister((), id1).unwrap();
            });
            let k2 = k.clone();
            let r2 = &mut r2;
            scope.spawn(move || {
                r2.GlobalOperationAfterSetResettingStatus((), id2, || {
                    *k2.lock().unwrap() = 1;
                    Ok(())
                })
                .unwrap();
                r2.Unregister((), id2).unwrap();
            });
        });
        require::Equal(&t, 1i32, *k.lock().unwrap());
    };
    drop_and_drop();
    r1.Close();
    r2.Close();
}

#[test]
fn test_global_operation_waits_for_other_running_registration() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let (tk, dom, g) = init_registry_test(&t);
    cleanup_registry_table(&tk);
    let mut r1 = registry::NewRestoreRegistry((), &g, &dom).unwrap();
    let mut r2 = registry::NewRestoreRegistry((), &g, &dom).unwrap();
    let registration = |table: &str| registry::RegistrationInfo {
        FilterStrings: vec![table.into()],
        StartTS: 100,
        RestoredTS: 200,
        UpstreamClusterID: 1,
        WithSysTable: true,
        Cmd: "restore".into(),
    };
    let (id1, _) = r1
        .ResumeOrCreateRegistration((), registration("db1.table1"), false)
        .unwrap();
    let (id2, _) = r2
        .ResumeOrCreateRegistration((), registration("db2.table2"), false)
        .unwrap();
    let called = Arc::new(Mutex::new(false));
    let called_in_callback = called.clone();

    r1.GlobalOperationAfterSetResettingStatus((), id1, move || {
        *called_in_callback.lock().unwrap() = true;
        Ok(())
    })
    .unwrap();

    require::False(&t, *called.lock().unwrap());
    r1.Unregister((), id1).unwrap();
    r2.Unregister((), id2).unwrap();
}

/// `TestRegistryTableConflicts`.
// 该用例覆盖 注册表 表 conflicts。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。
// Rust 版本继续复用与 Go 对照用例相近的 helper，方便双端定位行为差异。
// 对于异步任务、外部存储或全局配置，本 case 的成功标准通常是多层状态同时一致。
// 这比单纯检查返回值更接近真实产品语义，也是迁移测试最需要保留的部分。

#[test]
fn test_registry_table_conflicts() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let (tk, dom, g) = init_registry_test(&t);
    cleanup_registry_table(&tk);
    let mut r = registry::NewRestoreRegistry((), &g, &dom).unwrap();
    let info1 = registry::RegistrationInfo {
        FilterStrings: vec!["db1.table1".into()],
        StartTS: 100,
        RestoredTS: 200,
        UpstreamClusterID: 1,
        WithSysTable: true,
        Cmd: "restore".into(),
    };
    let info2 = registry::RegistrationInfo {
        FilterStrings: vec!["db2.table2".into()],
        StartTS: 100,
        RestoredTS: 200,
        UpstreamClusterID: 1,
        WithSysTable: true,
        Cmd: "restore".into(),
    };
    let (id1, _) = r.ResumeOrCreateRegistration((), info1, false).unwrap();
    let mut tracker = utils::NewPiTRIdTracker();
    tracker.AddDB(1);
    tracker.TrackTableId(1, 1);
    tracker.TrackTableName("db1", "table1");
    let err = r.CheckTablesWithRegisteredTasks((), id1 + 1, Some(&tracker), None, None);
    require::ErrorContains(&t, err, "cannot be restored concurrently by current task");

    let mut tracker = utils::NewPiTRIdTracker();
    tracker.AddDB(2);
    tracker.TrackTableId(2, 1);
    tracker.TrackTableName("db2", "table2");
    require::NoError(
        &t,
        r.CheckTablesWithRegisteredTasks((), id1 + 1, Some(&tracker), None, None),
    );
    let (id2, _) = r.ResumeOrCreateRegistration((), info2, false).unwrap();
    r.Unregister((), id1).unwrap();
    r.Unregister((), id2).unwrap();
    r.Close();
}

/// `TestPreventConcurrentRestoreOfTheSameDatabase`.
// 该用例覆盖 prevent concurrent 恢复 of the same database。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。
// Rust 版本继续复用与 Go 对照用例相近的 helper，方便双端定位行为差异。
// 对于异步任务、外部存储或全局配置，本 case 的成功标准通常是多层状态同时一致。
// 这比单纯检查返回值更接近真实产品语义，也是迁移测试最需要保留的部分。

#[test]
fn test_prevent_concurrent_restore_of_the_same_database() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let (tk, dom, g) = init_registry_test(&t);
    cleanup_registry_table(&tk);
    let mut r = registry::NewRestoreRegistry((), &g, &dom).unwrap();

    // 该类型围绕 Case 组织字段或状态视图。
    // 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
    // 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
    // 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。
    // 维护者若调整字段，优先保证旧有观察点仍能表达同一条测试语义。
    // 否则即使编译通过，也可能把回归从“数据不对”变成“数据看不见”。
    // 这类类型的注释重点是说明它服务哪一层断言，而不是展开字段实现细节。
    // 当一个类型只被测试使用时，更需要明确它承载的是哪种观测契约。

    struct Case {
        registered_filter: Vec<&'static str>,
        registered_cmd: &'static str,
        tracker_dbs: Vec<&'static str>,
        snapshot_dbs: Vec<&'static str>,
        expect_err: bool,
    }
    let cases = [
        Case {
            registered_filter: vec!["db1.table1"],
            registered_cmd: "Full Restore",
            tracker_dbs: vec![],
            snapshot_dbs: vec!["db1"],
            expect_err: false,
        },
        Case {
            registered_filter: vec!["db1.table1"],
            registered_cmd: "Full Restore",
            tracker_dbs: vec![],
            snapshot_dbs: vec!["db2"],
            expect_err: false,
        },
        Case {
            registered_filter: vec!["db1.table1"],
            registered_cmd: "Full Restore",
            tracker_dbs: vec!["db1"],
            snapshot_dbs: vec![],
            expect_err: true,
        },
        Case {
            registered_filter: vec!["db1.table1"],
            registered_cmd: "Full Restore",
            tracker_dbs: vec!["db2"],
            snapshot_dbs: vec![],
            expect_err: false,
        },
        Case {
            registered_filter: vec!["db1.table1"],
            registered_cmd: "Point Restore",
            tracker_dbs: vec!["db2"],
            snapshot_dbs: vec![],
            expect_err: false,
        },
        Case {
            registered_filter: vec!["db1.table1"],
            registered_cmd: "Point Restore",
            tracker_dbs: vec!["db1"],
            snapshot_dbs: vec![],
            expect_err: true,
        },
        Case {
            registered_filter: vec!["db1.table1"],
            registered_cmd: "Point Restore",
            tracker_dbs: vec![],
            snapshot_dbs: vec!["db1"],
            expect_err: true,
        },
        Case {
            registered_filter: vec!["db1.table1"],
            registered_cmd: "Point Restore",
            tracker_dbs: vec![],
            snapshot_dbs: vec!["db2"],
            expect_err: false,
        },
    ];

    for cs in cases {
        let info1 = registry::RegistrationInfo {
            FilterStrings: cs.registered_filter.iter().map(|s| s.to_string()).collect(),
            StartTS: 100,
            RestoredTS: 200,
            UpstreamClusterID: 1,
            WithSysTable: true,
            Cmd: cs.registered_cmd.into(),
        };
        let (id1, _) = r.ResumeOrCreateRegistration((), info1, false).unwrap();
        let tracker = if !cs.tracker_dbs.is_empty() {
            let mut tr = utils::NewPiTRIdTracker();
            for db in &cs.tracker_dbs {
                tr.TrackTableName(db, "table2");
            }
            Some(tr)
        } else {
            None
        };
        let dbs: Vec<metautil::Database> = cs
            .snapshot_dbs
            .iter()
            .map(|db| metautil::Database {
                Info: model::DBInfo {
                    Name: ast::NewCIStr(db),
                },
            })
            .collect();
        let dbs_ref = if dbs.is_empty() {
            None
        } else {
            Some(dbs.as_slice())
        };
        let err = r.CheckTablesWithRegisteredTasks((), id1 + 1, tracker.as_ref(), dbs_ref, None);
        if cs.expect_err {
            require::Error(&t, err);
        } else {
            require::NoError(&t, err);
        }
        r.Unregister((), id1).unwrap();
    }
    r.Close();
}

/// `TestGetRegistrationsByMaxID`.
// 该用例覆盖 读取 registrations by max id。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 BR 备份恢复、日志备份、注册表与调度器 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。
// Rust 版本继续复用与 Go 对照用例相近的 helper，方便双端定位行为差异。
// 对于异步任务、外部存储或全局配置，本 case 的成功标准通常是多层状态同时一致。
// 这比单纯检查返回值更接近真实产品语义，也是迁移测试最需要保留的部分。

#[test]
fn test_get_registrations_by_max_id() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let (tk, dom, g) = init_registry_test(&t);
    cleanup_registry_table(&tk);
    let mut r = registry::NewRestoreRegistry((), &g, &dom).unwrap();
    let info1 = registry::RegistrationInfo {
        FilterStrings: vec!["db1.table1".into()],
        StartTS: 100,
        RestoredTS: 200,
        UpstreamClusterID: 1,
        WithSysTable: true,
        Cmd: "restore task1".into(),
    };
    let info2 = registry::RegistrationInfo {
        FilterStrings: vec!["db2.table2".into()],
        StartTS: 300,
        RestoredTS: 400,
        UpstreamClusterID: 2,
        WithSysTable: false,
        Cmd: "restore task2".into(),
    };
    let (id1, _) = r.ResumeOrCreateRegistration((), info1, false).unwrap();
    let (id2, _) = r.ResumeOrCreateRegistration((), info2, false).unwrap();
    let regs = r.GetRegistrationsByMaxID((), id2 + 1).unwrap();
    require::GreaterOrEqual(&t, regs.len(), 2usize);
    let mut found1 = false;
    let mut found2 = false;
    for reg in &regs {
        if reg.Cmd == "restore task1" {
            found1 = true;
            require::Equal(&t, 100u64, reg.StartTS);
            require::Equal(&t, 200u64, reg.RestoredTS);
            require::Equal(
                &t,
                vec!["db1.table1".to_string()],
                reg.FilterStrings.clone(),
            );
        }
        if reg.Cmd == "restore task2" {
            found2 = true;
            require::Equal(&t, 300u64, reg.StartTS);
            require::Equal(&t, 400u64, reg.RestoredTS);
            require::Equal(
                &t,
                vec!["db2.table2".to_string()],
                reg.FilterStrings.clone(),
            );
        }
    }
    require::True(&t, found1);
    require::True(&t, found2);
    let regs = r.GetRegistrationsByMaxID((), id1).unwrap();
    require::Less(&t, regs.len(), 2usize);
    r.Unregister((), id1).unwrap();
    r.Unregister((), id2).unwrap();
    r.Close();
}
