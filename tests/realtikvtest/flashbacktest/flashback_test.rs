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

// 中文总览：本文件承担 集群闪回、GC 边界与历史元数据 中的 场景回归集合。
// 中文总览：重点在于测试意图、环境搭建、关键动作和最终观察点。
// 中文总览：当前任务只补充注释，不改任何 SQL、断言、参数或桩实现。
// 中文总览：阅读时可先看公共搭建，再看核心动作，最后看状态观察和收尾清理。
// 中文总览：这类 RealTiKV 回归通常依赖时序、共享状态和外部副作用，因此顺序本身就是语义。
// 中文总览：Rust 版本继续保留与 Go 对照实现接近的行为边界，避免迁移后只剩表面通过。
// 中文总览：注释会优先解释为什么这样验证，而不是逐行翻译语法或重复函数名。
// 中文总览：下面的索引用于快速定位 helper、公共模块和场景 case 的职责分工。
// 中文总览：函数 `enable_inject_safe_ts` 负责 启用 inject safe 时间戳。
// 中文总览：函数 `flashback_sql` 负责 闪回 sql。
// 中文总览：函数 `test_flashback` 负责 闪回。
// 中文总览：类型 `ScopeGuard` 负责 ScopeGuard。
// 中文总览：函数 `drop` 负责 收尾删除。
// 中文总览：函数 `test_prepare_flashback_failed` 负责 准备闪回失败。
// 中文总览：函数 `test_flashback_add_drop_index` 负责 闪回 添加 收尾删除 索引。
// 中文总览：函数 `test_flashback_add_drop_modify_column` 负责 闪回 添加 收尾删除 modify 列。
// 中文总览：函数 `test_flashback_basic_rename_drop_create_table` 负责 闪回 基础场景 重命名 收尾删除 创建 表。
// 中文总览：函数 `test_flashback_create_drop_table_with_data` 负责 闪回 创建 收尾删除 表 携带 data。
// 中文总览：函数 `test_flashback_create_drop_schema` 负责 闪回 创建 收尾删除 schema。
// 中文总览：函数 `test_flashback_auto_id` 负责 闪回 自动 id。
// 中文总览：函数 `test_flashback_sequence` 负责 闪回 序列。

//! Go-equivalent tests for `flashback_test.go`.
//!
//! Mapping:
//! - `MockGC` → [`astersql_tests_realtikvtest_flashbacktest::harness::MockGC`]
//! - `TestFlashback` → [`test_flashback`]
//! - `TestPrepareFlashbackFailed` → [`test_prepare_flashback_failed`]
//! - `TestFlashbackAddDropIndex` → [`test_flashback_add_drop_index`]
//! - `TestFlashbackAddDropModifyColumn` → [`test_flashback_add_drop_modify_column`]
//! - `TestFlashbackBasicRenameDropCreateTable` → [`test_flashback_basic_rename_drop_create_table`]
//! - `TestFlashbackCreateDropTableWithData` → [`test_flashback_create_drop_table_with_data`]
//! - `TestFlashbackCreateDropSchema` → [`test_flashback_create_drop_schema`]
//! - `TestFlashbackAutoID` → [`test_flashback_auto_id`]
//! - `TestFlashbackSequence` → [`test_flashback_sequence`]
//! - `TestFlashbackPartitionTable` → [`test_flashback_partition_table`]
//! - `TestFlashbackTmpTable` → [`test_flashback_tmp_table`]
//! - `TestFlashbackInProcessErrorMsg` → [`test_flashback_in_process_error_msg`]

use astersql_tests_realtikvtest_flashbacktest::harness::{
    FailCtx, MockGC, SetWithRealTiKV, TestCtx, WithRealTiKV, assert, create_store, errno,
    failpoint, meta, model, oracle, require, reset_engine, serial_guard, testfailpoint, testkit,
};
use std::time::Duration;

// 该辅助函数负责 启用 inject safe 时间戳。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
// 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。
// 注释强调的是为什么要独立封装，而不是简单翻译函数名本身。
// 当调用方继续传递这里返回的对象时，通常表示后续还要观察同一份上下文的中间状态。

fn enable_inject_safe_ts(t: &TestCtx, ts: u64) {
    let inject = oracle::GoTimeToTS(oracle::GetTimeFromTS(ts) + Duration::from_secs(100));
    require::NoError(
        t,
        failpoint::Enable(
            "github.com/pingcap/tidb/pkg/ddl/injectSafeTS",
            &format!("return({inject})"),
        ),
    );
}

// 该辅助函数负责 闪回 sql。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
// 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。
// 注释强调的是为什么要独立封装，而不是简单翻译函数名本身。
// 当调用方继续传递这里返回的对象时，通常表示后续还要观察同一份上下文的中间状态。

fn flashback_sql(ts: u64) -> String {
    format!(
        "flashback cluster to timestamp '{}'",
        oracle::format_fsp(oracle::GetTimeFromTS(ts))
    )
}

/// `TestFlashback`.
// 该用例覆盖 闪回。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 集群闪回、GC 边界与历史元数据 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。
// Rust 版本继续复用与 Go 对照用例相近的 helper，方便双端定位行为差异。

#[test]
fn test_flashback() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let store = create_store(&t);
    let tk = testkit::NewTestKit(&t, store);

    let (time_before_drop, _, safe_point_sql, reset_gc) = MockGC(&tk);
    let _guard = ScopeGuard {
        reset: Some(reset_gc),
    };

    tk.MustExec(&safe_point_sql.replace("%[1]s", &time_before_drop));
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t");
    tk.MustExec("create table t(a int, index i(a))");
    tk.MustExec("insert t values (1), (2), (3)");

    std::thread::sleep(Duration::from_millis(10));

    let ts = require::NoErrorVal(
        &t,
        tk.Session()
            .GetStore()
            .GetOracle()
            .GetTimestamp((), &oracle::Option {}),
    );
    enable_inject_safe_ts(&t, ts);

    tk.MustExec("insert t values (4), (5), (6)");
    tk.MustExec(&flashback_sql(ts));

    tk.MustExec("admin check table t");
    require::Equal(
        &t,
        "3".to_string(),
        tk.MustQuery("select max(a) from t").Rows()[0][0].clone(),
    );
    require::Equal(
        &t,
        "3".to_string(),
        tk.MustQuery("select max(a) from t use index(i)").Rows()[0][0].clone(),
    );

    require::NoError(
        &t,
        failpoint::Disable("github.com/pingcap/tidb/pkg/ddl/injectSafeTS"),
    );
}

/// Drop-style scope guard for MockGC reset closure.
// 该类型围绕 ScopeGuard 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。
// 维护者若调整字段，优先保证旧有观察点仍能表达同一条测试语义。
// 否则即使编译通过，也可能把回归从“数据不对”变成“数据看不见”。
// 这类类型的注释重点是说明它服务哪一层断言，而不是展开字段实现细节。
// 当一个类型只被测试使用时，更需要明确它承载的是哪种观测契约。

struct ScopeGuard {
    reset: Option<Box<dyn FnOnce() + Send>>,
}
impl Drop for ScopeGuard {
    // 该辅助函数负责 收尾删除。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
    // 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
    // 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
    // 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。
    // 这能降低跨 case 状态泄漏的风险，也让失败更容易回溯到真正的准备阶段。
    // 注释强调的是为什么要独立封装，而不是简单翻译函数名本身。
    // 当调用方继续传递这里返回的对象时，通常表示后续还要观察同一份上下文的中间状态。

    fn drop(&mut self) {
        if let Some(f) = self.reset.take() {
            f();
        }
    }
}

/// `TestPrepareFlashbackFailed`.
// 该用例覆盖 准备闪回失败。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 集群闪回、GC 边界与历史元数据 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。
// Rust 版本继续复用与 Go 对照用例相近的 helper，方便双端定位行为差异。

#[test]
fn test_prepare_flashback_failed() {
    let _serial = serial_guard();
    reset_engine();
    SetWithRealTiKV(true);
    if !WithRealTiKV() {
        return;
    }
    let t = TestCtx::new();
    let store = create_store(&t);
    let tk = testkit::NewTestKit(&t, store);

    let (time_before_drop, _, safe_point_sql, reset_gc) = MockGC(&tk);
    let _guard = ScopeGuard {
        reset: Some(reset_gc),
    };

    tk.MustExec(&safe_point_sql.replace("%[1]s", &time_before_drop));
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t");
    tk.MustExec("create table t(a int, index i(a))");
    tk.MustExec("insert t values (1), (2), (3)");

    std::thread::sleep(Duration::from_millis(10));

    let ts = require::NoErrorVal(
        &t,
        tk.Session()
            .GetStore()
            .GetOracle()
            .GetTimestamp((), &oracle::Option {}),
    );
    enable_inject_safe_ts(&t, ts);
    require::NoError(
        &t,
        failpoint::Enable(
            "github.com/pingcap/tidb/pkg/ddl/mockPrepareMeetsEpochNotMatch",
            "return(true)",
        ),
    );

    tk.MustExec("insert t values (4), (5), (6)");
    tk.MustExec(&flashback_sql(ts));

    tk.MustExec("admin check table t");
    require::Equal(
        &t,
        "3".to_string(),
        tk.MustQuery("select max(a) from t").Rows()[0][0].clone(),
    );
    require::Equal(
        &t,
        "3".to_string(),
        tk.MustQuery("select max(a) from t use index(i)").Rows()[0][0].clone(),
    );

    let job_meta = tk
        .MustQuery("select job_meta from mysql.tidb_ddl_history order by job_id desc limit 1")
        .Rows()[0][0]
        .clone();
    let mut job = model::Job::default();
    require::NoError(&t, job.Decode(job_meta.as_bytes()));
    require::Equal(&t, 0i64, job.ErrorCount);

    require::NoError(
        &t,
        failpoint::Disable("github.com/pingcap/tidb/pkg/ddl/injectSafeTS"),
    );
    require::NoError(
        &t,
        failpoint::Disable("github.com/pingcap/tidb/pkg/ddl/mockPrepareMeetsEpochNotMatch"),
    );
}

/// `TestFlashbackAddDropIndex`.
// 该用例覆盖 闪回 添加 收尾删除 索引。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 集群闪回、GC 边界与历史元数据 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。
// Rust 版本继续复用与 Go 对照用例相近的 helper，方便双端定位行为差异。

#[test]
fn test_flashback_add_drop_index() {
    let _serial = serial_guard();
    reset_engine();
    SetWithRealTiKV(true);
    if !WithRealTiKV() {
        return;
    }
    let t = TestCtx::new();
    let store = create_store(&t);
    let tk = testkit::NewTestKit(&t, store);

    let (time_before_drop, _, safe_point_sql, reset_gc) = MockGC(&tk);
    let _guard = ScopeGuard {
        reset: Some(reset_gc),
    };

    tk.MustExec(&safe_point_sql.replace("%[1]s", &time_before_drop));
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t");
    tk.MustExec("create table t(a int, index i(a))");
    tk.MustExec("insert t values (1), (2), (3)");
    let prev_gc = tk
        .MustQuery("select count(*) from mysql.gc_delete_range")
        .Rows()[0][0]
        .clone();

    std::thread::sleep(Duration::from_millis(10));
    let ts = require::NoErrorVal(
        &t,
        tk.Session()
            .GetStore()
            .GetOracle()
            .GetTimestamp((), &oracle::Option {}),
    );

    tk.MustExec("alter table t add index k(a)");
    require::Equal(
        &t,
        "3".to_string(),
        tk.MustQuery("select max(a) from t use index(k)").Rows()[0][0].clone(),
    );
    tk.MustExec("alter table t drop index i");
    tk.MustGetErrCode(
        "select max(a) from t use index(i)",
        errno::ErrKeyDoesNotExist,
    );
    require::Greater(
        &t,
        tk.MustQuery("select count(*) from mysql.gc_delete_range")
            .Rows()[0][0]
            .parse::<i64>()
            .unwrap(),
        prev_gc.parse::<i64>().unwrap(),
    );

    enable_inject_safe_ts(&t, ts);
    tk.MustExec("insert t values (4), (5), (6)");
    tk.MustExec(&flashback_sql(ts));

    tk.MustExec("admin check table t");
    require::Equal(
        &t,
        "3".to_string(),
        tk.MustQuery("select max(a) from t use index(i)").Rows()[0][0].clone(),
    );
    tk.MustGetErrCode(
        "select max(a) from t use index(k)",
        errno::ErrKeyDoesNotExist,
    );
    require::Equal(
        &t,
        prev_gc,
        tk.MustQuery("select count(*) from mysql.gc_delete_range")
            .Rows()[0][0]
            .clone(),
    );

    require::NoError(
        &t,
        failpoint::Disable("github.com/pingcap/tidb/pkg/ddl/injectSafeTS"),
    );
}

/// `TestFlashbackAddDropModifyColumn`.
// 该用例覆盖 闪回 添加 收尾删除 modify 列。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 集群闪回、GC 边界与历史元数据 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。
// Rust 版本继续复用与 Go 对照用例相近的 helper，方便双端定位行为差异。

#[test]
fn test_flashback_add_drop_modify_column() {
    let _serial = serial_guard();
    reset_engine();
    SetWithRealTiKV(true);
    if !WithRealTiKV() {
        return;
    }
    let t = TestCtx::new();
    let store = create_store(&t);
    let tk = testkit::NewTestKit(&t, store);

    let (time_before_drop, _, safe_point_sql, reset_gc) = MockGC(&tk);
    let _guard = ScopeGuard {
        reset: Some(reset_gc),
    };

    tk.MustExec(&safe_point_sql.replace("%[1]s", &time_before_drop));
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t");
    tk.MustExec("create table t(a int, b int, index i(a))");
    tk.MustExec("insert t values (1, 1), (2, 2), (3, 3)");

    std::thread::sleep(Duration::from_millis(10));
    let ts = require::NoErrorVal(
        &t,
        tk.Session()
            .GetStore()
            .GetOracle()
            .GetTimestamp((), &oracle::Option {}),
    );

    tk.MustExec("alter table t add column c int");
    tk.MustExec("alter table t drop column b");
    tk.MustExec("alter table t modify column a tinyint");
    require::Equal(
        &t,
        "CREATE TABLE `t` (\n  `a` tinyint(4) DEFAULT NULL,\n  `c` int(11) DEFAULT NULL,\n  KEY `i` (`a`)\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"
            .to_string(),
        tk.MustQuery("show create table t").Rows()[0][1].clone(),
    );

    enable_inject_safe_ts(&t, ts);
    tk.MustExec("insert t values (4, 4), (5, 5), (6, 6)");
    tk.MustExec(&flashback_sql(ts));

    tk.MustExec("admin check table t");
    require::Equal(
        &t,
        "CREATE TABLE `t` (\n  `a` int(11) DEFAULT NULL,\n  `b` int(11) DEFAULT NULL,\n  KEY `i` (`a`)\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4 COLLATE=utf8mb4_bin"
            .to_string(),
        tk.MustQuery("show create table t").Rows()[0][1].clone(),
    );
    require::Equal(
        &t,
        "3".to_string(),
        tk.MustQuery("select max(b) from t").Rows()[0][0].clone(),
    );

    require::NoError(
        &t,
        failpoint::Disable("github.com/pingcap/tidb/pkg/ddl/injectSafeTS"),
    );
}

/// `TestFlashbackBasicRenameDropCreateTable`.
// 该用例覆盖 闪回 基础场景 重命名 收尾删除 创建 表。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 集群闪回、GC 边界与历史元数据 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。
// Rust 版本继续复用与 Go 对照用例相近的 helper，方便双端定位行为差异。

#[test]
fn test_flashback_basic_rename_drop_create_table() {
    let _serial = serial_guard();
    reset_engine();
    SetWithRealTiKV(true);
    if !WithRealTiKV() {
        return;
    }
    let t = TestCtx::new();
    let store = create_store(&t);
    let tk = testkit::NewTestKit(&t, store);

    let (time_before_drop, _, safe_point_sql, reset_gc) = MockGC(&tk);
    let _guard = ScopeGuard {
        reset: Some(reset_gc),
    };

    tk.MustExec(&safe_point_sql.replace("%[1]s", &time_before_drop));
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t, t1, t2, t3");
    tk.MustExec("create table t(a int, index i(a))");
    tk.MustExec("insert t values (1), (2), (3)");
    tk.MustExec("create table t1(a int, index i(a))");
    tk.MustExec("insert t1 values (4), (5), (6)");
    let prev_gc = tk
        .MustQuery("select count(*) from mysql.gc_delete_range")
        .Rows()[0][0]
        .clone();

    std::thread::sleep(Duration::from_millis(10));
    let ts = require::NoErrorVal(
        &t,
        tk.Session()
            .GetStore()
            .GetOracle()
            .GetTimestamp((), &oracle::Option {}),
    );

    tk.MustExec("rename table t to t3");
    tk.MustExec("drop table t1");
    tk.MustExec("create table t2(a int, index i(a))");
    tk.MustExec("insert t2 values (7), (8), (9)");

    require::Equal(
        &t,
        "3".to_string(),
        tk.MustQuery("select max(a) from t3").Rows()[0][0].clone(),
    );
    require::Equal(
        &t,
        "9".to_string(),
        tk.MustQuery("select max(a) from t2").Rows()[0][0].clone(),
    );
    require::Greater(
        &t,
        tk.MustQuery("select count(*) from mysql.gc_delete_range")
            .Rows()[0][0]
            .parse::<i64>()
            .unwrap(),
        prev_gc.parse::<i64>().unwrap(),
    );

    enable_inject_safe_ts(&t, ts);
    tk.MustExec(&flashback_sql(ts));

    tk.MustExec("admin check table t");
    require::Equal(
        &t,
        "3".to_string(),
        tk.MustQuery("select max(a) from t").Rows()[0][0].clone(),
    );
    tk.MustExec("admin check table t1");
    require::Equal(
        &t,
        "6".to_string(),
        tk.MustQuery("select max(a) from t1").Rows()[0][0].clone(),
    );
    require::Equal(
        &t,
        prev_gc,
        tk.MustQuery("select count(*) from mysql.gc_delete_range")
            .Rows()[0][0]
            .clone(),
    );

    require::NoError(
        &t,
        failpoint::Disable("github.com/pingcap/tidb/pkg/ddl/injectSafeTS"),
    );
}

/// `TestFlashbackCreateDropTableWithData`.
// 该用例覆盖 闪回 创建 收尾删除 表 携带 data。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 集群闪回、GC 边界与历史元数据 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。
// Rust 版本继续复用与 Go 对照用例相近的 helper，方便双端定位行为差异。

#[test]
fn test_flashback_create_drop_table_with_data() {
    let _serial = serial_guard();
    reset_engine();
    SetWithRealTiKV(true);
    if !WithRealTiKV() {
        return;
    }
    let t = TestCtx::new();
    let store = create_store(&t);
    let tk = testkit::NewTestKit(&t, store);

    let (time_before_drop, _, safe_point_sql, reset_gc) = MockGC(&tk);
    let _guard = ScopeGuard {
        reset: Some(reset_gc),
    };

    tk.MustExec(&safe_point_sql.replace("%[1]s", &time_before_drop));
    tk.MustExec("use test");
    tk.MustExec("create table t(a int)");

    std::thread::sleep(Duration::from_millis(10));
    let ts = require::NoErrorVal(
        &t,
        tk.Session()
            .GetStore()
            .GetOracle()
            .GetTimestamp((), &oracle::Option {}),
    );

    tk.MustExec("insert into t values (1)");
    tk.MustExec("drop table t");
    tk.MustExec("create table t(b int)");
    tk.MustExec("insert into t(b) values (1)");

    enable_inject_safe_ts(&t, ts);
    tk.MustExec(&flashback_sql(ts));

    tk.MustExec("admin check table t");
    require::Equal(
        &t,
        "0".to_string(),
        tk.MustQuery("select count(a) from t").Rows()[0][0].clone(),
    );

    require::NoError(
        &t,
        failpoint::Disable("github.com/pingcap/tidb/pkg/ddl/injectSafeTS"),
    );
}

/// `TestFlashbackCreateDropSchema`.
// 该用例覆盖 闪回 创建 收尾删除 schema。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 集群闪回、GC 边界与历史元数据 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。
// Rust 版本继续复用与 Go 对照用例相近的 helper，方便双端定位行为差异。

#[test]
fn test_flashback_create_drop_schema() {
    let _serial = serial_guard();
    reset_engine();
    SetWithRealTiKV(true);
    if !WithRealTiKV() {
        return;
    }
    let t = TestCtx::new();
    let store = create_store(&t);
    let tk = testkit::NewTestKit(&t, store);

    let (time_before_drop, _, safe_point_sql, reset_gc) = MockGC(&tk);
    let _guard = ScopeGuard {
        reset: Some(reset_gc),
    };

    tk.MustExec(&safe_point_sql.replace("%[1]s", &time_before_drop));
    tk.MustExec("use test");
    tk.MustExec("create table t(a int, index k(a))");
    tk.MustExec("insert into t values (1),(2)");

    std::thread::sleep(Duration::from_millis(10));
    let ts = require::NoErrorVal(
        &t,
        tk.Session()
            .GetStore()
            .GetOracle()
            .GetTimestamp((), &oracle::Option {}),
    );

    tk.MustExec("drop schema test");
    tk.MustExec("create schema test1");
    tk.MustExec("create schema test2");
    tk.MustExec("use test1");
    tk.MustGetErrCode("use test", errno::ErrBadDB);
    tk.MustExec("use test2");
    tk.MustExec("drop schema test2");

    enable_inject_safe_ts(&t, ts);
    tk.MustExec(&flashback_sql(ts));
    tk.MustExec("admin check table test.t");
    let res = tk.MustQuery("select max(a) from test.t").Rows();
    require::Equal(&t, "2".to_string(), res[0][0].clone());
    tk.MustGetErrCode("use test1", errno::ErrBadDB);
    tk.MustGetErrCode("use test2", errno::ErrBadDB);

    require::NoError(
        &t,
        failpoint::Disable("github.com/pingcap/tidb/pkg/ddl/injectSafeTS"),
    );
}

/// `TestFlashbackAutoID`.
// 该用例覆盖 闪回 自动 id。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 集群闪回、GC 边界与历史元数据 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。
// Rust 版本继续复用与 Go 对照用例相近的 helper，方便双端定位行为差异。

#[test]
fn test_flashback_auto_id() {
    let _serial = serial_guard();
    reset_engine();
    SetWithRealTiKV(true);
    if !WithRealTiKV() {
        return;
    }
    let t = TestCtx::new();
    let store = create_store(&t);
    let tk = testkit::NewTestKit(&t, store);

    let (time_before_drop, _, safe_point_sql, reset_gc) = MockGC(&tk);
    let _guard = ScopeGuard {
        reset: Some(reset_gc),
    };

    tk.MustExec(&safe_point_sql.replace("%[1]s", &time_before_drop));
    tk.MustExec("use test");
    tk.MustExec("create table t(a int auto_increment, primary key(a)) auto_id_cache 100");
    tk.MustExec("insert into t values (),()");

    std::thread::sleep(Duration::from_millis(10));
    let ts = require::NoErrorVal(
        &t,
        tk.Session()
            .GetStore()
            .GetOracle()
            .GetTimestamp((), &oracle::Option {}),
    );

    tk.MustExec("insert into t values (),()");
    let res = tk.MustQuery("select max(a) from test.t").Rows();
    require::Equal(&t, "4".to_string(), res[0][0].clone());
    tk.MustExec("drop table t");

    enable_inject_safe_ts(&t, ts);
    tk.MustExec(&flashback_sql(ts));

    tk.MustExec("admin check table t");
    let res = tk.MustQuery("select max(a) from t").Rows();
    require::Equal(&t, "2".to_string(), res[0][0].clone());
    tk.MustExec("insert into t values ()");
    let res = tk.MustQuery("select max(a) from t").Rows();
    require::Equal(&t, "101".to_string(), res[0][0].clone());

    require::NoError(
        &t,
        failpoint::Disable("github.com/pingcap/tidb/pkg/ddl/injectSafeTS"),
    );
}

/// `TestFlashbackSequence`.
// 该用例覆盖 闪回 序列。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 集群闪回、GC 边界与历史元数据 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。
// Rust 版本继续复用与 Go 对照用例相近的 helper，方便双端定位行为差异。

#[test]
fn test_flashback_sequence() {
    let _serial = serial_guard();
    reset_engine();
    SetWithRealTiKV(true);
    if !WithRealTiKV() {
        return;
    }
    let t = TestCtx::new();
    let store = create_store(&t);
    let tk = testkit::NewTestKit(&t, store);

    let (time_before_drop, _, safe_point_sql, reset_gc) = MockGC(&tk);
    let _guard = ScopeGuard {
        reset: Some(reset_gc),
    };

    tk.MustExec(&safe_point_sql.replace("%[1]s", &time_before_drop));
    tk.MustExec("use test");
    tk.MustExec("create sequence seq cache 100");
    let res = tk.MustQuery("select nextval(seq)").Rows();
    require::Equal(&t, "1".to_string(), res[0][0].clone());

    std::thread::sleep(Duration::from_millis(10));
    let ts = require::NoErrorVal(
        &t,
        tk.Session()
            .GetStore()
            .GetOracle()
            .GetTimestamp((), &oracle::Option {}),
    );

    let res = tk.MustQuery("select nextval(seq)").Rows();
    require::Equal(&t, "2".to_string(), res[0][0].clone());
    tk.MustExec("drop sequence seq");

    enable_inject_safe_ts(&t, ts);
    tk.MustExec(&flashback_sql(ts));

    let res = tk.MustQuery("select nextval(seq)").Rows();
    require::Equal(&t, "101".to_string(), res[0][0].clone());

    require::NoError(
        &t,
        failpoint::Disable("github.com/pingcap/tidb/pkg/ddl/injectSafeTS"),
    );
    tk.MustExec("drop sequence seq");
}

/// `TestFlashbackPartitionTable`.
// 该用例覆盖 闪回 分区 表。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 集群闪回、GC 边界与历史元数据 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。
// Rust 版本继续复用与 Go 对照用例相近的 helper，方便双端定位行为差异。

#[test]
fn test_flashback_partition_table() {
    let _serial = serial_guard();
    reset_engine();
    SetWithRealTiKV(true);
    if !WithRealTiKV() {
        return;
    }
    let t = TestCtx::new();
    let store = create_store(&t);
    let tk = testkit::NewTestKit(&t, store);

    let (time_before_drop, _, safe_point_sql, reset_gc) = MockGC(&tk);
    let _guard = ScopeGuard {
        reset: Some(reset_gc),
    };

    tk.MustExec(&safe_point_sql.replace("%[1]s", &time_before_drop));
    tk.MustExec("use test");
    tk.MustExec(
        "create table t(a int) partition by range(`a`) \
         (partition `a_1` values less than (25), \
         partition `a_2` values less than (75), \
         partition `a_3` values less than (200))",
    );

    for i in 0..100 {
        tk.MustExec(&format!("insert into t values ({i})"));
    }

    std::thread::sleep(Duration::from_millis(10));
    let ts = require::NoErrorVal(
        &t,
        tk.Session()
            .GetStore()
            .GetOracle()
            .GetTimestamp((), &oracle::Option {}),
    );

    tk.MustExec("alter table t drop partition `a_3`");
    tk.MustExec("alter table t add partition (partition `a_3` values less than (300))");
    let res = tk.MustQuery("select max(a) from t").Rows();
    require::Equal(&t, "74".to_string(), res[0][0].clone());
    tk.MustExec("drop table t");

    enable_inject_safe_ts(&t, ts);
    tk.MustExec(&flashback_sql(ts));

    tk.MustExec("admin check table t");
    let res = tk
        .MustQuery("select max(a), min(a), count(*) from t")
        .Rows();
    require::Equal(&t, "99".to_string(), res[0][0].clone());
    require::Equal(&t, "0".to_string(), res[0][1].clone());
    require::Equal(&t, "100".to_string(), res[0][2].clone());
    tk.MustExec("insert into t values (100), (-1)");
    let res = tk
        .MustQuery("select max(a), min(a), count(*) from t")
        .Rows();
    require::Equal(&t, "100".to_string(), res[0][0].clone());
    require::Equal(&t, "-1".to_string(), res[0][1].clone());
    require::Equal(&t, "102".to_string(), res[0][2].clone());

    require::NoError(
        &t,
        failpoint::Disable("github.com/pingcap/tidb/pkg/ddl/injectSafeTS"),
    );
}

/// `TestFlashbackTmpTable`.
// 该用例覆盖 闪回 tmp 表。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 集群闪回、GC 边界与历史元数据 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。
// Rust 版本继续复用与 Go 对照用例相近的 helper，方便双端定位行为差异。

#[test]
fn test_flashback_tmp_table() {
    let _serial = serial_guard();
    reset_engine();
    SetWithRealTiKV(true);
    if !WithRealTiKV() {
        return;
    }
    let t = TestCtx::new();
    let store = create_store(&t);
    let tk = testkit::NewTestKit(&t, store);

    let (time_before_drop, _, safe_point_sql, reset_gc) = MockGC(&tk);
    let _guard = ScopeGuard {
        reset: Some(reset_gc),
    };

    tk.MustExec(&safe_point_sql.replace("%[1]s", &time_before_drop));
    tk.MustExec("use test");
    tk.MustExec("create temporary table t(a int)");

    std::thread::sleep(Duration::from_millis(10));
    let ts = require::NoErrorVal(
        &t,
        tk.Session()
            .GetStore()
            .GetOracle()
            .GetTimestamp((), &oracle::Option {}),
    );

    tk.MustExec("insert into t values (1), (2), (3)");

    enable_inject_safe_ts(&t, ts);
    tk.MustExec(&flashback_sql(ts));

    let res = tk.MustQuery("select max(a) from t").Rows();
    require::Equal(&t, "3".to_string(), res[0][0].clone());

    std::thread::sleep(Duration::from_millis(10));
    let ts = require::NoErrorVal(
        &t,
        tk.Session()
            .GetStore()
            .GetOracle()
            .GetTimestamp((), &oracle::Option {}),
    );

    tk.MustExec("drop table t");

    enable_inject_safe_ts(&t, ts);
    tk.MustExec(&flashback_sql(ts));

    tk.MustGetErrCode("select * from t", errno::ErrNoSuchTable);

    require::NoError(
        &t,
        failpoint::Disable("github.com/pingcap/tidb/pkg/ddl/injectSafeTS"),
    );
}

/// `TestFlashbackInProcessErrorMsg`.
// 该用例覆盖 闪回 in process 错误 msg。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 集群闪回、GC 边界与历史元数据 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。
// 如果准备顺序被改乱，很多失败会变成偶发现象，反而掩盖真正的回归来源。
// 注释在这里强调时序约束，是为了提醒维护者不要把 setup、执行和校验随意拆散。
// Rust 版本继续复用与 Go 对照用例相近的 helper，方便双端定位行为差异。

#[test]
fn test_flashback_in_process_error_msg() {
    let _serial = serial_guard();
    reset_engine();
    SetWithRealTiKV(true);
    if !WithRealTiKV() {
        return;
    }
    let t = TestCtx::new();
    let store = create_store(&t);
    let tk = testkit::NewTestKit(&t, store.clone());

    let (time_before_drop, _, safe_point_sql, reset_gc) = MockGC(&tk);
    let _guard = ScopeGuard {
        reset: Some(reset_gc),
    };

    tk.MustExec(&safe_point_sql.replace("%[1]s", &time_before_drop));
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t");
    tk.MustExec("create table t(a int)");

    std::thread::sleep(Duration::from_millis(10));
    let ts = require::NoErrorVal(
        &t,
        tk.Session()
            .GetStore()
            .GetOracle()
            .GetTimestamp((), &oracle::Option {}),
    );

    tk.MustExec("alter table t add index k(a)");
    tk.MustExec("insert into t values (1), (2), (3)");

    enable_inject_safe_ts(&t, ts);

    let store_h = tk.Session().GetStore();
    testfailpoint::EnableCall(
        &t,
        "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep",
        move |ctx| {
            if let FailCtx::Job(job) = ctx {
                if job.Type == model::ActionFlashbackCluster
                    && job.SchemaState == model::StateWriteReorganization
                {
                    let txn = store_h.Begin().expect("begin");
                    let err = meta::NewMutator(&txn).ListDatabases().err().expect("err");
                    assert::Contains(&err, "is in flashback progress, FlashbackStartTS is ");
                    let slices: Vec<&str> = err
                        .split("is in flashback progress, FlashbackStartTS is ")
                        .collect();
                    assert::Equal(2, slices.len());
                    assert::NotEqual(slices[1], "0");
                    txn.Rollback();
                }
            }
        },
    );

    let _ = tk.Exec(&flashback_sql(ts));
    testfailpoint::Disable(&t, "github.com/pingcap/tidb/pkg/ddl/beforeRunOneJobStep");

    require::NoError(
        &t,
        failpoint::Disable("github.com/pingcap/tidb/pkg/ddl/injectSafeTS"),
    );
}
