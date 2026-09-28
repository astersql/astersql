// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// FTS 会话运行时单元测试。
//
// 覆盖部署模式门禁、TiFlash 下推元数据与 resolver 校验、prepared 永不命中
// plan cache，以及未提交 mem-buffer 脏数据拒绝全文查询等路径。

use std::sync::{Arc, Mutex};

use astersql_config_deploymode as deploymode;
use astersql_planner_core::{FullTextPushDown, PlanNode, StoreType};
use astersql_store_mockstore_mockstorage::{KVStore, NewMockStorage, mockStorage};

use crate::fts_runtime::FtsSessionRuntime;

/// 串行化部署模式全局切换，避免并行测试互相干扰。
static DEPLOY_MODE_LOCK: Mutex<()> = Mutex::new(());

fn lock_deploy_mode() -> std::sync::MutexGuard<'static, ()> {
    DEPLOY_MODE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// RAII：进入测试时设置部署模式，离开时恢复原值。
struct DeployModeGuard(deploymode::Mode);

impl DeployModeGuard {
    /// 切换到指定模式并保存旧值。
    fn set(mode: deploymode::Mode) -> Self {
        let original = deploymode::Get();
        deploymode::Set(mode).expect("set nextgen deploy mode");
        Self(original)
    }
}

impl Drop for DeployModeGuard {
    fn drop(&mut self) {
        deploymode::Set(self.0).expect("restore deploy mode");
    }
}

/// 构造可独占的内存 mock 存储。
fn storage() -> mockStorage {
    Arc::try_unwrap(NewMockStorage(KVStore::NewMemory(), None).expect("create mock storage"))
        .unwrap_or_else(|_| panic!("mock storage retained an unexpected owner"))
}

/// 基于内存存储创建 FTS 会话运行时。
fn runtime() -> FtsSessionRuntime<mockStorage> {
    FtsSessionRuntime::new(storage())
}

/// 在计划树中查找全文下推注解（存储类型、索引名、查询串）。
fn find_push_down(plan: &PlanNode) -> Option<(StoreType, String, String)> {
    if let Some(push_down) = FullTextPushDown(plan) {
        return Some((plan.store_type, push_down.index_name, push_down.query_text));
    }
    plan.children.iter().find_map(find_push_down)
}

/// Premium 模式下 CREATE/ALTER FULLTEXT 与 FTS_MATCH_WORD 均应失败。
#[test]
fn fts_requires_starter_mode_through_real_ddl_and_expression_gate() {
    let _lock = lock_deploy_mode();
    #[cfg(feature = "nextgen")]
    let _mode = DeployModeGuard::set(deploymode::Premium);
    #[cfg(not(feature = "nextgen"))]
    assert_eq!(deploymode::Get(), deploymode::Premium);
    let mut runtime = runtime();

    let error = runtime
        .execute("create table fts_blocked(id int primary key, title text, fulltext key ft_title(title))")
        .expect_err("premium CREATE FULLTEXT must fail");
    assert!(
        error
            .to_string()
            .contains("FULLTEXT index is only supported in starter deployment mode")
    );

    runtime
        .execute("create table fts_t(id int primary key, title text)")
        .expect("ordinary table remains supported");
    let error = runtime
        .execute("alter table fts_t add fulltext index ft_title(title)")
        .expect_err("premium ALTER FULLTEXT must fail");
    assert!(
        error
            .to_string()
            .contains("FULLTEXT index is only supported in starter deployment mode")
    );

    let error = runtime
        .execute("explain select * from fts_t where fts_match_word('hello', title)")
        .expect_err("premium FTS_MATCH_WORD must fail");
    assert!(
        error
            .to_string()
            .contains("FTS_MATCH_WORD() is only supported in starter deployment mode")
    );
}

/// Starter + TiFlash：合法谓词下推到 TiFlash；非法形态由 resolver 拒绝。
#[test]
#[cfg(feature = "nextgen")]
fn tiflash_fts_match_word_uses_canonical_metadata_and_resolver_pipeline() {
    let _lock = lock_deploy_mode();
    let _mode = DeployModeGuard::set(deploymode::Starter);
    let mut runtime = runtime();
    runtime
        .execute("create table fts_t(id int primary key, title text, body text)")
        .expect("create canonical table");
    runtime
        .execute("alter table fts_t add fulltext index ft_title(title)")
        .expect("append canonical FULLTEXT metadata through ALTER TABLE");
    runtime
        .execute("alter table fts_t set tiflash replica 1")
        .expect("install available TiFlash replica metadata");

    let result = runtime
        .execute("explain select * from fts_t where fts_match_word('hello', title)")
        .expect("plan FTS predicate from parser AST");
    let (store, index, query) = find_push_down(result.plan().expect("EXPLAIN returns a plan"))
        .expect("resolver annotates the physical scan");
    assert_eq!(store, StoreType::TiFlash);
    assert_eq!(index, "ft_title");
    assert_eq!(query, "hello");

    // 非法 SELECT/WHERE/ORDER BY 形态：应返回带关键字的错误串。
    for (sql, expected) in [
        (
            "explain select fts_match_word('hello', title) from fts_t",
            "in SELECT requires a matching",
        ),
        (
            "explain select fts_match_word('hello', title) * 2 from fts_t where fts_match_word('hello', title)",
            "in SELECT must not be wrapped",
        ),
        (
            "explain select fts_match_word('world', title) from fts_t where fts_match_word('hello', title)",
            "in SELECT must match",
        ),
        (
            "explain select * from fts_t where fts_match_word('hello', title) and fts_match_word('world', title)",
            "must be used alone",
        ),
        (
            "explain select * from fts_t where fts_match_word('hello', body)",
            "matching fulltext index",
        ),
        (
            "explain select * from fts_t order by fts_match_word('hello', title)",
            "ORDER BY without a LIMIT",
        ),
    ] {
        let error = runtime
            .execute(sql)
            .expect_err("invalid FTS form must fail");
        assert!(error.to_string().contains(expected), "{sql}: {error}");
    }
}

/// PREPARE/EXECUTE 始终重建计划且不命中 cache；非常量 match 参数在 prepare 失败。
#[test]
#[cfg(feature = "nextgen")]
fn tiflash_fts_prepared_execution_rebuilds_and_never_hits_plan_cache() {
    let _lock = lock_deploy_mode();
    let _mode = DeployModeGuard::set(deploymode::Starter);
    let mut runtime = runtime();
    runtime
        .execute("create table fts_t(id int primary key, title text, body text, fulltext key ft_title(title))")
        .unwrap();
    runtime
        .execute("alter table fts_t set tiflash replica 1")
        .unwrap();

    runtime
        .prepare(
            "stmt",
            "select * from fts_t where fts_match_word('hello', title)",
        )
        .expect("prepare constant FTS query");
    // 连续 EXECUTE：应始终重建并保留下推注解，且 from_plan_cache 为 false。
    for _ in 0..2 {
        let result = runtime.execute_prepared("stmt").expect("rebuild FTS plan");
        assert!(!result.from_plan_cache());
        assert!(find_push_down(result.plan().unwrap()).is_some());
    }
    runtime.deallocate("stmt").expect("deallocate statement");

    // match 文本为占位符时，prepare 阶段非常量，应直接报错。
    let error = runtime
        .prepare(
            "stmt_param",
            "select * from fts_t where fts_match_word(?, title)",
        )
        .expect_err("FTS query parameter is not constant at prepare time");
    assert!(
        error
            .to_string()
            .contains("match against a non-constant string")
    );
}

/// 事务内未提交写导致 dirty mem-buffer，FTS 查询被拒绝；回滚后恢复。
#[test]
#[cfg(feature = "nextgen")]
fn tiflash_fts_rejects_real_uncommitted_table_mem_buffer_content() {
    let _lock = lock_deploy_mode();
    let _mode = DeployModeGuard::set(deploymode::Starter);
    let mut runtime = runtime();
    runtime
        .execute("create table fts_t(id int primary key, title text, body text, fulltext key ft_title(title))")
        .unwrap();
    runtime
        .execute("alter table fts_t set tiflash replica 1")
        .unwrap();
    runtime
        .execute("select * from fts_t where fts_match_word('hello', title)")
        .expect("clean transaction can plan FTS");

    // 开启显式事务并写入行键，制造 mem-buffer 脏状态。
    runtime.execute("begin").unwrap();
    runtime
        .execute("insert into fts_t values (1, 'hello', 'dirty')")
        .expect("write encoded row to the live KV transaction");
    let error = runtime
        .execute("select * from fts_t where fts_match_word('hello', title)")
        .expect_err("dirty mem-buffer must reject FTS");
    assert!(
        error
            .to_string()
            .contains("FTS_MATCH_WORD() cannot be used in a transaction with uncommitted changes")
    );
    runtime.execute("rollback").unwrap();

    // 回滚清空脏写后，FTS 规划应恢复可用。
    let clean = runtime
        .execute("select * from fts_t where fts_match_word('hello', title)")
        .expect("rollback clears dirty state");
    assert!(find_push_down(clean.plan().unwrap()).is_some());
}
