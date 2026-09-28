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

// 中文总览：本文件承担 BR 备份恢复、日志备份、注册表与调度器 中的 场景回归集合。
// 中文总览：重点在于测试意图、环境搭建、关键动作和最终观察点。
// 中文总览：当前任务只补充注释，不改任何 SQL、断言、参数或桩实现。
// 中文总览：阅读时可先看公共搭建，再看核心动作，最后看状态观察和收尾清理。
// 中文总览：这类 RealTiKV 回归通常依赖时序、共享状态和外部副作用，因此顺序本身就是语义。
// 中文总览：Rust 版本继续保留与 Go 对照实现接近的行为边界，避免迁移后只剩表面通过。
// 中文总览：注释会优先解释为什么这样验证，而不是逐行翻译语法或重复函数名。
// 中文总览：下面的索引用于快速定位 helper、公共模块和场景 case 的职责分工。
// 中文总览：函数 `test_for_coverage` 负责 for coverage。

//! Go-equivalent tests for `binlog_test.go`.
//!
//! Mapping:
//! - `TestForCoverage` → [`test_for_coverage`]

use astersql_tests_realtikvtest_brietest::harness::{
    TestCtx, create_store, mysql, require, reset_engine, serial_guard, testkit,
};

/// `TestForCoverage`.
// 该用例覆盖 for coverage。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。

#[test]
fn test_for_coverage() {
    let _serial = serial_guard();
    reset_engine();
    let t = TestCtx::new();
    let store = create_store(&t);
    let tk = testkit::NewTestKit(&t, store);
    tk.MustExec("use test");
    tk.MustExec("drop table if exists t");
    tk.MustExec("create table t (id int auto_increment, v int, index (id))");
    tk.MustExec("insert t values ()");
    tk.MustExec("insert t values ()");
    tk.MustExec("insert t values ()");

    tk.MustExec("set @@tidb_enable_fast_table_check=false");
    tk.MustExec("admin check table t");
    tk.MustExec("set @@tidb_enable_fast_table_check=true");
    tk.MustExec("admin check table t");

    tk.MustExec("begin");
    tk.MustExec("truncate table t");
    tk.MustExec("insert t values ()");
    tk.MustExec("delete from t where id = 2");
    tk.MustExec("update t set v = 5 where id = 2");
    tk.MustExec("insert t values ()");
    tk.MustExec("rollback");

    require::NoError(&t, tk.Session().SetCollation(mysql::DefaultCollationID));
    tk.MustExec("show processlist");
    require::NoError(&t, tk.Session().FieldList("t").map(|_| ()));
}
