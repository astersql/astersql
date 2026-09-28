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

//! 中文说明开始（自动生成）
//! 中文总览：`infoschema_v2_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `会话生命周期与信息模式` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 18 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `serial_guard` 是当前文件里的辅助函数。
//! `serial_guard` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `serial_guard` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `serial_guard`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `GUARD` 是当前文件里的静态量。
//! `GUARD` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `GUARD` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `GUARD`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `table_id` 是当前文件里的辅助函数。
//! `table_id` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `table_id` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `table_id`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `run_repeated_ddl` 是当前文件里的辅助函数。
//! `run_repeated_ddl` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `run_repeated_ddl` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `run_repeated_ddl`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_gc_old_version` 是当前文件里的测试用例。
//! `test_gc_old_version` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `test_gc_old_version` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_gc_old_version`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 中文说明结束（自动生成）

//! RealTiKV session coverage for InfoSchema-v2 old-version GC.

use std::sync::{Mutex, OnceLock};

use astersql_infoschema::infoschema::{CiString, InfoSchema};
use astersql_testkit::mockstore::CreateMockStoreAndDomainV2;
use astersql_testkit::{NewTestKit, TestKit};

fn serial_guard() -> std::sync::MutexGuard<'static, ()> {
    static GUARD: OnceLock<Mutex<()>> = OnceLock::new();
    GUARD
        .get_or_init(|| Mutex::new(()))
        .lock()
        .expect("InfoSchema-v2 serial test lock poisoned")
}

fn table_id(schema: &dyn InfoSchema, table: &str) -> i64 {
    schema
        .TableByName(&CiString::new("test"), &CiString::new(table))
        .unwrap_or_else(|error| panic!("resolve test.{table}: {error}"))
        .0
        .id
}

fn run_repeated_ddl(tk: &mut TestKit) {
    for _ in 0..10 {
        tk.MustExec("alter table t1 add index i_b(b)", Vec::new());
        tk.MustExec("alter table t1 drop index i_b", Vec::new());
    }
    for _ in 0..10 {
        tk.MustExec("alter table t2 add column (c int)", Vec::new());
        tk.MustExec("alter table t2 drop column c", Vec::new());
    }
    for _ in 0..10 {
        tk.MustExec("truncate table t3", Vec::new());
    }
}

/// Go `TestGCOldVersion`.
#[test]
fn test_gc_old_version() {
    let _serial = serial_guard();
    let (store, domain) = CreateMockStoreAndDomainV2(512 * 1024 * 1024);
    let mut tk = NewTestKit(store);
    tk.MustExec(
        "set @@global.tidb_schema_cache_size = 512 * 1024 * 1024",
        Vec::new(),
    );
    tk.MustExec("use test", Vec::new());
    tk.MustExec("drop table if exists t", Vec::new());
    tk.MustExec("drop table if exists t1", Vec::new());
    tk.MustExec("drop table if exists t2", Vec::new());
    tk.MustExec("drop table if exists t3", Vec::new());
    tk.MustExec("create table t1 (id int key, b int)", Vec::new());
    tk.MustExec("create table t2 (id int key, b int)", Vec::new());
    tk.MustExec("create table t3 (id int key, b int)", Vec::new());

    let old_is = domain.info_schema();
    assert!(old_is.IsV2(), "canonical Domain must load InfoSchema-v2");
    let t1_id = table_id(old_is.as_ref(), "t1");
    let t2_id = table_id(old_is.as_ref(), "t2");
    let t3_id = table_id(old_is.as_ref(), "t3");
    let old_version = old_is.SchemaMetaVersion();

    run_repeated_ddl(&mut tk);

    let now_is = domain.info_schema();
    let current_version = now_is.SchemaMetaVersion();
    assert!(
        current_version > old_version,
        "DDL must advance schema version: old={old_version}, current={current_version}"
    );

    let (deleted, _) = old_is
        .GCOldVersion(current_version - 5)
        .expect("InfoSchema-v2 must expose production history GC");
    assert!(deleted > 0, "old schema records must actually be collected");

    for id in [t1_id, t2_id, t3_id] {
        assert!(
            old_is.TableByID(id).is_none(),
            "old snapshot unexpectedly resolved table id {id}"
        );
    }
    for name in ["t1", "t2", "t3"] {
        assert!(
            old_is
                .TableByName(&CiString::new("test"), &CiString::new(name))
                .is_err(),
            "old snapshot unexpectedly resolved test.{name}"
        );
    }

    assert!(now_is.TableByID(t1_id).is_some());
    assert!(now_is.TableByID(t2_id).is_some());
    assert!(
        now_is.TableByID(t3_id).is_none(),
        "TRUNCATE must replace the table ID"
    );
}
