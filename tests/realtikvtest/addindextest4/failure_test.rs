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

// 中文总览：本文件承担 RealTiKV 加索引、分布式回填与全局排序 中的 场景回归集合。
// 中文总览：重点在于测试意图、环境搭建、关键动作和最终观察点。
// 中文总览：当前任务只补充注释，不改任何 SQL、断言、参数或桩实现。
// 中文总览：阅读时可先看公共搭建，再看核心动作，最后看状态观察和收尾清理。
// 中文总览：这类 RealTiKV 回归通常依赖时序、共享状态和外部副作用，因此顺序本身就是语义。
// 中文总览：Rust 版本继续保留与 Go 对照实现接近的行为边界，避免迁移后只剩表面通过。
// 中文总览：注释会优先解释为什么这样验证，而不是逐行翻译语法或重复函数名。
// 中文总览：下面的索引用于快速定位 helper、公共模块和场景 case 的职责分工。
// 中文总览：函数 `new_owner` 负责 创建 owner。
// 中文总览：函数 `assert_index_state` 负责 断言 index state。
// 中文总览：类型 `RecoveryStage` 负责 RecoveryStage。
// 中文总览：函数 `test_add_index_ingest_recover_partition` 负责 添加 索引 ingest 回填 恢复 分区。

//! Recovery coverage corresponding to `failure_test.go`.
//!
//! The Go test recursively launches three TiDB processes and terminates the
//! first two owners after their second partition-reorg checkpoint.  This
//! in-process harness uses three independent SQL sessions over one retained
//! store.  Panicking from the production failpoint models the abrupt process
//! boundary: both failed attempts unwind before index publication, while the
//! final owner repeats the durable ADD INDEX job and publishes it atomically.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use astersql_domain::{Domain, DomainConfig, KvInfoSchemaLoader};
use astersql_testkit::mockstore::{AnalyzeStatsStore, CreateMockStoreAndDomain};
use astersql_testkit::{NewTestKit, Rows, TestKit};
use astersql_testkit_testfailpoint as testfailpoint;
use astersql_tests_realtikvtest_addindextest4::serial_guard;

const AFTER_UPDATE_PARTITION_REORG_INFO: &str =
    "github.com/pingcap/tidb/pkg/ddl/afterUpdatePartitionReorgInfo";
const AFTER_FINISH_DDL_JOB: &str = "github.com/pingcap/tidb/pkg/ddl/afterFinishDDLJob";

// 该辅助函数负责 创建 owner。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。

fn new_owner(store: Arc<AnalyzeStatsStore>) -> TestKit {
    let mut tk = NewTestKit(store);
    tk.MustExec("create database if not exists addindexlit", Vec::new());
    tk.MustExec("use addindexlit", Vec::new());
    tk
}

// 该辅助函数负责 断言 index state。
// 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
// 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。
// 对迁移测试来说，单点封装还能保持与 Go 版本相近的步骤边界，便于并排核对。
// 一旦这里的默认行为改变，往往会同时影响多条用例，所以需要明确职责和约束。

fn assert_index_state(store: &AnalyzeStatsStore, present: bool) {
    let table = store
        .domain()
        .table_by_name("addindexlit", "t")
        .expect("load retained partition table");
    assert_eq!(
        table
            .Indices
            .iter()
            .any(|index| index.Name.L.eq_ignore_ascii_case("idx")),
        present,
        "unexpected idx publication state"
    );
}

// 该类型围绕 RecoveryStage 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。
// 这能让不同场景共享同一种读取方式，也让 Go/Rust 对照更直接。
// 维护者若调整字段，优先保证旧有观察点仍能表达同一条测试语义。

struct RecoveryStage {
    owner: &'static str,
    exit_after_partition: usize,
}

/// `TestAddIndexIngestRecoverPartition`.
///
/// Unlike the currently skipped Go test, this executes the full retained-store
/// scenario.  Every callback is reached through `ALTER TABLE ... ADD INDEX`;
/// test code never invokes a DDL callback directly.
// 该用例覆盖 添加 索引 ingest 回填 恢复 分区。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。
// 这样做不是样板代码偏好，而是为了让后续断言只反映当前场景的真实偏差。
// 用例主体会触发 RealTiKV 加索引、分布式回填与全局排序 里的核心动作，而不是只验证静态参数拼装结果。

#[test]
fn test_add_index_ingest_recover_partition() {
    let _serial = serial_guard();
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut bootstrap = new_owner(store.clone());
    bootstrap.MustExec("drop database if exists addindexlit", Vec::new());
    bootstrap.MustExec("create database addindexlit", Vec::new());
    bootstrap.MustExec("use addindexlit", Vec::new());
    bootstrap.MustExec("set global tidb_ddl_enable_fast_reorg = on", Vec::new());
    bootstrap.MustExec("set global tidb_enable_dist_task = off", Vec::new());
    bootstrap.MustExec(
        "create table t (a int primary key, b int) \
         partition by hash(a) partitions 8",
        Vec::new(),
    );
    bootstrap.MustExec("insert into t values (2, 3), (3, 3), (5, 5)", Vec::new());
    drop(bootstrap);

    let stages = [
        RecoveryStage {
            owner: "initial owner",
            exit_after_partition: 2,
        },
        RecoveryStage {
            owner: "replacement owner",
            exit_after_partition: 2,
        },
    ];
    let mut restarted_domains = Vec::new();

    for (stage_index, stage) in stages.into_iter().enumerate() {
        let partition_hits = Arc::new(AtomicUsize::new(0));
        let callback_hits = Arc::clone(&partition_hits);
        let exit_after_partition = stage.exit_after_partition;
        let _partition_hook =
            testfailpoint::enable_call(AFTER_UPDATE_PARTITION_REORG_INFO, move || {
                let current = callback_hits.fetch_add(1, Ordering::SeqCst) + 1;
                if current == exit_after_partition {
                    panic!("simulated abnormal TiDB exit after partition checkpoint {current}");
                }
            });

        let exited = catch_unwind(AssertUnwindSafe(|| {
            let mut owner = new_owner(store.clone());
            owner.MustQuery("select 1", Vec::new()).Check(Rows(&["1"]));
            if stage_index == 0 {
                owner.MustExec("alter table t add index idx(b)", Vec::new());
            }
        }));
        assert!(
            exited.is_err(),
            "{} must terminate at its reorg checkpoint",
            stage.owner
        );
        assert_eq!(
            partition_hits.load(Ordering::SeqCst),
            stage.exit_after_partition,
            "{} terminated at the wrong partition",
            stage.owner
        );
        drop(_partition_hook);

        // Publication is atomic: an owner dying during partition backfill
        // cannot leak a half-public index into retained metadata.
        assert_index_state(&store, false);

        // Recreate the Domain over the canonical KV handle, as the Go harness
        // recreates TiDB while retaining TiKV. The replacement observes the
        // committed cursor rather than any process-local callback counter.
        let restarted_domain = Domain::new_with_storage_handle(
            store.domain().storage_handle(),
            Arc::new(KvInfoSchemaLoader::new()),
            DomainConfig::default(),
        );
        let pending = restarted_domain
            .pending_add_index_jobs()
            .expect("reload pending ADD INDEX job from canonical KV");
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].next_partition,
            (stage_index + 1) * stage.exit_after_partition
        );
        restarted_domains.push(restarted_domain);
    }

    let partition_hits = Arc::new(AtomicUsize::new(0));
    let observed_partitions = Arc::clone(&partition_hits);
    let _partition_hook =
        testfailpoint::enable_call(AFTER_UPDATE_PARTITION_REORG_INFO, move || {
            observed_partitions.fetch_add(1, Ordering::SeqCst);
        });
    let finish_hits = Arc::new(AtomicUsize::new(0));
    let observed_finishes = Arc::clone(&finish_hits);
    let _finish_hook = testfailpoint::enable_call(AFTER_FINISH_DDL_JOB, move || {
        observed_finishes.fetch_add(1, Ordering::SeqCst);
    });

    let mut final_owner = new_owner(store.clone());
    assert_eq!(
        partition_hits.load(Ordering::SeqCst),
        4,
        "the final owner must resume from the fourth persisted partition"
    );
    assert_eq!(
        finish_hits.load(Ordering::SeqCst),
        1,
        "the recovered DDL job must finish exactly once"
    );
    assert_index_state(&store, true);
    final_owner.MustExec("admin check table t", Vec::new());
    final_owner
        .MustQuery("select * from t order by a", Vec::new())
        .Check(Rows(&["2 3", "3 3", "5 5"]));
}
