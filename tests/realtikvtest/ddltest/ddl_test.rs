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

// 中文总览：本文件承担 DDL 元信息、region scatter 与版本推进 中的 场景回归集合。
// 中文总览：重点在于测试意图、环境搭建、关键动作和最终观察点。
// 中文总览：当前任务只补充注释，不改任何 SQL、断言、参数或桩实现。
// 中文总览：阅读时可先看公共搭建，再看核心动作，最后看状态观察和收尾清理。
// 中文总览：这类 RealTiKV 回归通常依赖时序、共享状态和外部副作用，因此顺序本身就是语义。
// 中文总览：Rust 版本继续保留与 Go 对照实现接近的行为边界，避免迁移后只剩表面通过。
// 中文总览：注释会优先解释为什么这样验证，而不是逐行翻译语法或重复函数名。
// 中文总览：下面的索引用于快速定位 helper、公共模块和场景 case 的职责分工。
// 中文总览：类型 `ScatterCase` 负责 ScatterCase。
// 中文总览：函数 `prepare` 负责 prepare。
// 中文总览：函数 `table_leader_distribution` 负责 表 leader distribution。
// 中文总览：函数 `run_scatter_case` 负责 run scatter case。

//! Real SQL port of `ddl_test.go`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};
use astersql_testkit::{NewTestKit, TestKit};
use astersql_testkit_testfailpoint as testfailpoint;
use astersql_tests_realtikvtest_ddltest::serial_guard;

const PRE_SPLIT_AND_SCATTER: &str = "github.com/pingcap/tidb/pkg/ddl/preSplitAndScatter";
const PUT_KV_TO_ETCD_ERROR: &str = "github.com/pingcap/tidb/pkg/ddl/util/PutKVToEtcdError";

// 该类型围绕 ScatterCase 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。
// 维护者若调整字段，优先保证旧有观察点仍能表达同一条测试语义。
// 否则即使编译通过，也可能把回归从“数据不对”变成“数据看不见”。

struct ScatterCase {
    sqls: &'static [&'static str],
    total_region_count: usize,
}

struct AtomicU32Reset {
    target: &'static AtomicU32,
    previous: u32,
}

impl AtomicU32Reset {
    fn set(target: &'static AtomicU32, value: u32) -> Self {
        Self {
            target,
            previous: target.swap(value, Ordering::SeqCst),
        }
    }
}

impl Drop for AtomicU32Reset {
    fn drop(&mut self) {
        self.target.store(self.previous, Ordering::SeqCst);
    }
}

struct MetadataLockReset {
    store: Arc<AnalyzeStatsStore>,
}

impl MetadataLockReset {
    fn disable(store: Arc<AnalyzeStatsStore>, tk: &mut TestKit) -> Self {
        tk.MustExec("SET GLOBAL tidb_enable_metadata_lock = 0", Vec::new());
        Self { store }
    }
}

impl Drop for MetadataLockReset {
    fn drop(&mut self) {
        NewTestKit(self.store.clone())
            .MustExec("SET GLOBAL tidb_enable_metadata_lock = 1", Vec::new());
    }
}

const SCATTER_CASES: &[ScatterCase] = &[
    ScatterCase {
        sqls: &["CREATE TABLE t (a INT) SHARD_ROW_ID_BITS = 10 PRE_SPLIT_REGIONS=3"],
        total_region_count: 8,
    },
    ScatterCase {
        sqls: &[
            "CREATE TABLE t (a INT) SHARD_ROW_ID_BITS = 10 PRE_SPLIT_REGIONS=3",
            "TRUNCATE TABLE t",
        ],
        total_region_count: 8,
    },
    ScatterCase {
        sqls: &[
            "CREATE TABLE t (bal_dt DATE) SHARD_ROW_ID_BITS = 10 PRE_SPLIT_REGIONS=3 \
             PARTITION BY RANGE COLUMNS(bal_dt) (\
             PARTITION p202201 VALUES LESS THAN ('2022-02-01'), \
             PARTITION p202202 VALUES LESS THAN ('2022-02-02'))",
            "ALTER TABLE t TRUNCATE PARTITION p202201",
        ],
        total_region_count: 16,
    },
    ScatterCase {
        sqls: &[
            "CREATE TABLE t (bal_dt DATE) SHARD_ROW_ID_BITS = 10 PRE_SPLIT_REGIONS=3 \
             PARTITION BY RANGE COLUMNS(bal_dt) (\
             PARTITION p202201 VALUES LESS THAN ('2022-02-01'))",
            "ALTER TABLE t ADD PARTITION (\
             PARTITION p202202 VALUES LESS THAN ('2022-02-02'))",
        ],
        total_region_count: 16,
    },
];

// 该辅助函数负责 prepare。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。

fn prepare() -> (Arc<AnalyzeStatsStore>, TestKit) {
    astersql_config::update_global(|conf| conf.path = "127.0.0.1:2379".to_owned());
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = NewTestKit(store.clone());
    tk.MustExec("USE test", Vec::new());
    (store, tk)
}

/// Go `getTableLeaderDistribute`: group the fifth SHOW REGIONS column by
/// leader store ID and return each store's leader count.
// 该辅助函数负责 表 leader distribution。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。

fn table_leader_distribution(tk: &TestKit, table: &str) -> (usize, HashMap<u64, usize>) {
    let rows = tk
        .MustQuery(&format!("SHOW TABLE {table} REGIONS"), Vec::new())
        .Rows();
    let mut distribution = HashMap::new();
    for row in &rows {
        assert!(
            row.len() >= 5,
            "SHOW TABLE REGIONS must expose LEADER_STORE_ID as column 5: {row:?}"
        );
        let store_id = row[4]
            .parse::<u64>()
            .unwrap_or_else(|error| panic!("invalid LEADER_STORE_ID in {row:?}: {error}"));
        *distribution.entry(store_id).or_insert(0) += 1;
    }
    (rows.len(), distribution)
}

#[test]
fn go_init_sets_pd_path_before_ddl_tests() {
    let _serial = serial_guard();
    let previous = astersql_config::get_global_config();
    astersql_config::store_global_config(astersql_config::new_config());

    let (_store, _tk) = prepare();
    let observed = astersql_config::get_global_config().path.clone();
    astersql_config::store_global_config(previous.as_ref().clone());

    assert_eq!(observed, "127.0.0.1:2379");
}

#[test]
fn disabled_split_flag_keeps_a_new_table_in_one_region() {
    let _serial = serial_guard();
    let _split_region = AtomicU32Reset::set(&astersql_ddl::EnableSplitTableRegion, 0);
    let (_store, mut tk) = prepare();
    tk.MustExec(
        "CREATE TABLE disabled_split (a INT) SHARD_ROW_ID_BITS = 10 PRE_SPLIT_REGIONS = 3",
        Vec::new(),
    );
    let (region_count, _) = table_leader_distribution(&tk, "disabled_split");
    tk.MustExec("DROP TABLE disabled_split", Vec::new());

    assert_eq!(
        region_count, 1,
        "Go only applies PRE_SPLIT_REGIONS when ddl.EnableSplitTableRegion is enabled"
    );
}

#[test]
fn metadata_lock_is_restored_when_ddl_test_unwinds() {
    let _serial = serial_guard();
    let (store, mut tk) = prepare();

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _metadata_lock = MetadataLockReset::disable(store.clone(), &mut tk);
        tk.MustQuery("SELECT @@global.tidb_enable_metadata_lock", Vec::new())
            .Check(vec![vec!["0".to_owned()]]);
        panic!("simulate a failing DDL assertion");
    }));

    let panic = result.expect_err("the simulated DDL assertion must unwind");
    assert_eq!(
        panic.downcast_ref::<&str>().copied(),
        Some("simulate a failing DDL assertion"),
        "the test must reach the simulated assertion after disabling MDL"
    );
    let observer = NewTestKit(store);
    observer
        .MustQuery("SELECT @@global.tidb_enable_metadata_lock", Vec::new())
        .Check(vec![vec!["1".to_owned()]]);
}

// 该辅助函数负责 run scatter case。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。
// 如果该函数涉及全局状态、failpoint 或外部存储，它还承担把副作用限制在局部的责任。

fn run_scatter_case(
    store: Arc<AnalyzeStatsStore>,
    tk: &mut TestKit,
    scatter_scope: &str,
    case: &ScatterCase,
    global: bool,
) {
    if global {
        tk.MustExec(
            &format!("SET @@global.tidb_scatter_region = '{scatter_scope}'"),
            Vec::new(),
        );
        *tk = NewTestKit(store);
        tk.MustExec("USE test", Vec::new());
    } else {
        tk.MustExec(
            &format!("SET @@session.tidb_scatter_region = '{scatter_scope}'"),
            Vec::new(),
        );
    }
    tk.MustQuery("SELECT @@session.tidb_scatter_region", Vec::new())
        .Check(vec![vec![scatter_scope.to_owned()]]);
    tk.MustExec("DROP TABLE IF EXISTS t", Vec::new());

    let callback_scopes = Arc::new(Mutex::new(Vec::new()));
    let callback_scopes_for_hook = Arc::clone(&callback_scopes);
    let hook = testfailpoint::enable_value_call(PRE_SPLIT_AND_SCATTER, move |scope| {
        callback_scopes_for_hook
            .lock()
            .expect("scatter callback lock poisoned")
            .push(scope.to_owned());
    });
    for sql in case.sqls {
        tk.MustExec(sql, Vec::new());
    }
    drop(hook);

    let callback_scopes = callback_scopes
        .lock()
        .expect("scatter callback lock poisoned");
    assert_eq!(callback_scopes.len(), case.sqls.len());
    assert!(
        callback_scopes
            .iter()
            .all(|observed| observed == scatter_scope),
        "preSplitAndScatter received wrong scope: {callback_scopes:?}"
    );
    let (region_count, leaders) = table_leader_distribution(tk, "t");
    assert_eq!(
        region_count, case.total_region_count,
        "PRE_SPLIT_REGIONS=3 must create 8 regions per physical table"
    );
    assert!(
        leaders.len() > 1,
        "scatter must distribute leaders across stores: {leaders:?}"
    );
    for count in leaders.values() {
        assert!(
            *count < case.total_region_count,
            "one store owns every table leader: {leaders:?}"
        );
    }
}

/// Go `TestTiDBScatterRegion`: all four DDL variants are exercised with both
/// `table`/`global` scatter modes, first as a session setting and then through
/// a global setting inherited by a new session.
// 该用例覆盖 TiDB scatter region。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 DDL 元信息、region scatter 与版本推进 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。

#[test]
fn test_tidb_scatter_region() {
    let _serial = serial_guard();
    let _split_region = AtomicU32Reset::set(&astersql_ddl::EnableSplitTableRegion, 1);
    let (store, mut tk) = prepare();

    for scatter_scope in ["table", "global"] {
        for case in SCATTER_CASES {
            run_scatter_case(store.clone(), &mut tk, scatter_scope, case, false);
            run_scatter_case(store.clone(), &mut tk, scatter_scope, case, true);
        }
    }

    tk.MustExec("DROP TABLE IF EXISTS t", Vec::new());
    tk.MustExec("SET @@global.tidb_scatter_region = ''", Vec::new());
}

/// Go `TestUpdateSelfVersionFail`: with metadata locking disabled, the DDL
/// version publication retries three injected etcd failures and both CREATE
/// and DROP still publish the expected metadata.
// 该用例覆盖 update 自身版本 fail。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 DDL 元信息、region scatter 与版本推进 里的核心动作，而不是只验证静态参数拼装结果。
// 因而这里既看最终 SQL 或状态结果，也看中间任务、元数据或副作用是否收敛。

#[test]
fn test_update_self_version_fail() {
    let _serial = serial_guard();
    let (store, mut tk) = prepare();

    let _metadata_lock = MetadataLockReset::disable(store.clone(), &mut tk);
    let retry_events = Arc::new(Mutex::new(Vec::new()));
    let retry_events_for_hook = Arc::clone(&retry_events);
    let retry_hook = testfailpoint::enable_value_call(PUT_KV_TO_ETCD_ERROR, move |event| {
        retry_events_for_hook
            .lock()
            .expect("etcd retry callback lock poisoned")
            .push(event.to_owned());
    });
    let failpoint = testfailpoint::enable(PUT_KV_TO_ETCD_ERROR, "3*return(true)");

    tk.MustExec("CREATE TABLE t (a INT)", Vec::new());
    assert!(
        store.domain().table_by_name("test", "t").is_ok(),
        "CREATE TABLE must publish metadata after the retryable failures"
    );
    tk.MustExec("DROP TABLE t", Vec::new());
    assert!(
        store.domain().table_by_name("test", "t").is_err(),
        "DROP TABLE must remove metadata after CREATE recovered"
    );
    assert_eq!(
        *retry_events
            .lock()
            .expect("etcd retry callback lock poisoned"),
        ["retry", "retry", "retry"],
        "DDL must retry exactly the three injected etcd failures"
    );

    drop(failpoint);
    assert!(
        !testfailpoint::is_active(PUT_KV_TO_ETCD_ERROR),
        "dropping the failpoint guard must unregister it"
    );
    drop(retry_hook);
}
