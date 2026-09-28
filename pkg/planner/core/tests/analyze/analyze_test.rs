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

// ANALYZE 统计信息收集的端到端集成测试。
//
// 覆盖生成列的 ANALYZE，以及分区表缺失分区统计时的自动 ANALYZE。
// ANALYZE 负责采样表数据生成优化器代价估算所需的统计信息。

// 本文件对应 pkg/planner/core/tests/analyze/analyze_test.go。两个 Go 用例都只依赖
// testkit（真实 session/DDL/DML/ANALYZE 执行）加上 domain.StatsHandle。两个测试都
// 直接对真实生产代码做端到端断言，不引入任何空桩或模拟层。

use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateMockStoreAndDomain;

/// Restores the global auto-analyze ratio through SQL when the test scope exits.
struct AutoAnalyzeRatioGuard<'a> {
    testkit: &'a mut TestKit,
    original: String,
}

impl Drop for AutoAnalyzeRatioGuard<'_> {
    fn drop(&mut self) {
        self.testkit.MustExec(
            &format!("set global tidb_auto_analyze_ratio = {}", self.original),
            Vec::new(),
        );
    }
}

/// 验证 JSON/vector 生成列场景可被 `ANALYZE TABLE ... ALL COLUMNS` 接受。
#[test]
fn test_analyze_virtual_columns() {
    // 对应 Go TestAnalyzeVirtualColumns：JSON 虚拟列和 vector distance 虚拟列
    // 必须能随 ALL COLUMNS 一起被 ANALYZE。保持 Go 的列定义与执行顺序，不以
    // 等价但较窄的表达式替代真实场景。
    let (store, _domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec(
        "create table t1 (
            id bigint NOT NULL,
            c1 varchar(50) NOT NULL,
            c2 int DEFAULT NULL,
            c3 json DEFAULT NULL,
            c4 varchar(255) GENERATED ALWAYS AS (json_unquote(json_extract(c3, '$.oppositePlaceId'))) VIRTUAL,
            c5 vector(3),
            c6 double GENERATED ALWAYS AS (vec_l2_distance(c5, '[0,0,0]')) VIRTUAL,
            PRIMARY KEY (id),
            UNIQUE KEY idx_unique (c1,c2))",
        Vec::new(),
    );
    tk.MustExec("analyze table t1 all columns", Vec::new());
}

/// 动态分区裁剪下仅手动 ANALYZE 部分分区后，自动 ANALYZE 应补齐缺失分区统计。
#[test]
fn test_auto_analyze_for_missing_partition() {
    // 对应 Go TestAutoAnalyzeForMissingPartition：dynamic partition prune 下只手动
    // analyze 了 p1，随后触发一次 domain 级自动 analyze，验证它会一并补齐 p0/p2 的分区统计
    // （Go 通过 h.HandleAutoAnalyze() + h.Update 观察；这里直接调用等价的生产入口
    // domain.try_handle_auto_analyze 并读回 domain 持久化统计）。
    let original_min_cnt = astersql_statistics::EffectiveAutoAnalyzeMinCnt();
    astersql_statistics::SetAutoAnalyzeMinCnt(0);
    struct MinCntGuard(i64);
    impl Drop for MinCntGuard {
        fn drop(&mut self) {
            astersql_statistics::SetAutoAnalyzeMinCnt(self.0);
        }
    }
    let _min_cnt_guard = MinCntGuard(original_min_cnt);

    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("use test", Vec::new());
    tk.MustExec("set @@tidb_skip_missing_partition_stats = 1", Vec::new());
    tk.MustExec("set @@tidb_partition_prune_mode = 'dynamic'", Vec::new());
    tk.MustExec(
        "create table t (a int, b int, c int, index idx_b(b)) partition by range (a) \
         (partition p0 values less than (100), partition p1 values less than (200), \
          partition p2 values less than (300))",
        Vec::new(),
    );
    tk.MustExec(
        "insert into t values (1,1,1), (2,2,2), (101,101,101), (102,102,102), \
         (201,201,201), (202,202,202)",
        Vec::new(),
    );
    tk.MustExec("flush stats_delta *.*", Vec::new());
    // 只手动 analyze p1；p0/p2 此刻仍然是 pseudo（从未被 analyze 过）。
    tk.MustExec("analyze table t partition p1", Vec::new());

    // Go 用例在手动分析后再次写入并刷新增量，再由 auto analyze 根据修改比例
    // 选择表；这两个步骤不能省略，否则不会覆盖同一触发路径。
    tk.MustExec(
        "insert into t values (1,1,1), (2,2,2), (101,101,101), (102,102,102), \
         (201,201,201), (202,202,202)",
        Vec::new(),
    );
    tk.MustExec("flush stats_delta *.*", Vec::new());

    let original_ratio = tk
        .MustQuery("select @@tidb_auto_analyze_ratio", Vec::new())
        .Rows()[0][0]
        .clone();
    tk.MustExec("set global tidb_auto_analyze_ratio = 0.01", Vec::new());
    // Go uses defer here; retain the same SQL restoration even if a later assertion unwinds.
    let _ratio_guard = AutoAnalyzeRatioGuard {
        testkit: &mut tk,
        original: original_ratio,
    };

    let (table_key, table_info) = domain
        .stats_context()
        .catalog()
        .get(&("test".to_owned(), "t".to_owned()))
        .cloned()
        .expect("partitioned table t must be registered in the stats catalog");
    let partition_info = table_info
        .GetPartitionInfo()
        .expect("range-partitioned table must expose partition metadata");
    let p0_id = partition_info
        .Definitions
        .iter()
        .find(|definition| definition.Name.L == "p0")
        .expect("p0 metadata")
        .ID;
    let p2_id = partition_info
        .Definitions
        .iter()
        .find(|definition| definition.Name.L == "p2")
        .expect("p2 metadata")
        .ID;
    let _ = table_key;

    let stats_context = domain.stats_context();
    // p0/p2 尚未被 analyze 过（只 analyze 了 p1），对应 Go 里两个分区此刻仍是
    // "从未 analyze" 状态：last_analyze_version 为 0。
    assert!(
        stats_context
            .persisted_physical_stats(p0_id)
            .is_none_or(|stats| stats.last_analyze_version == 0),
        "p0 must not be analyzed yet before the missing-partition auto-analyze runs"
    );
    assert!(
        stats_context
            .persisted_physical_stats(p2_id)
            .is_none_or(|stats| stats.last_analyze_version == 0),
        "p2 must not be analyzed yet before the missing-partition auto-analyze runs"
    );

    // 对应 Go: require.True(t, h.HandleAutoAnalyze())。一旦表内任一物理对象满足
    // 触发条件（这里是 p0/p2 从未 analyze 过），生产代码会把该表全部未加锁的物理
    // 对象（global + 每个分区）一起 analyze，从而补齐缺失的分区统计。
    assert!(
        domain
            .try_handle_auto_analyze()
            .expect("auto-analyze for missing partition stats must not fail")
    );
    domain
        .update_stats()
        .expect("stats handle update after auto-analyze must not fail");

    let p0_stats = stats_context
        .persisted_physical_stats(p0_id)
        .expect("p0 statistics must exist after auto-analyze");
    let p2_stats = stats_context
        .persisted_physical_stats(p2_id)
        .expect("p2 statistics must exist after auto-analyze");
    assert!(!p0_stats.pseudo, "p0 must no longer be pseudo");
    assert!(p0_stats.last_analyze_version > 0, "p0 must be analyzed");
    assert!(!p2_stats.pseudo, "p2 must no longer be pseudo");
    assert!(p2_stats.last_analyze_version > 0, "p2 must be analyzed");

    // 对应 Go: tk.MustQuery("select distinct state from mysql.analyze_jobs").Check(...)。
    let jobs = stats_context.analyze_jobs();
    let mut distinct_states = jobs
        .iter()
        .map(|job| job.state.as_str())
        .collect::<Vec<_>>();
    distinct_states.sort_unstable();
    distinct_states.dedup();
    assert_eq!(
        distinct_states,
        vec!["finished"],
        "all analyze jobs must be finished, got {jobs:?}"
    );
}
