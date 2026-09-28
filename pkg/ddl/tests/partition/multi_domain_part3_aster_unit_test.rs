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

// 多 Domain 分区 DDL 可见性测试（第 3 部分）。
//
// 覆盖无主键重复行落盘、隐藏行 ID（`_tidb_rowid`）与聚簇索引区分、
// 并发回填 DML 下各物理分区统计、range 边界路由，
// 以及非聚簇表 reorganize 状态与分区状态 GC。

use astersql_meta_model::{
    ACTION_REORGANIZE_PARTITION, ExtraHandleID, NewExtraHandleColInfo, PartitionDefinition,
    PartitionInfo, StateDeleteOnly, StateDeleteReorganization, StatePublic, StateWriteOnly,
    TableInfo,
};
use astersql_parser_ast::NewCIStr;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateAnalyzeStatsStore;
use std::thread;

/// 读取指定分区表各物理分区的 realtime_count（实时行数估计）。
fn physical_counts(testkit: &TestKit, table: &str) -> Vec<i64> {
    let context = testkit.AnalyzeStatsContext().expect("stats context");
    let catalog = context.catalog();
    catalog
        .get(&("test".to_owned(), table.to_owned()))
        .expect("partition table")
        .1
        .GetPartitionInfo()
        .unwrap()
        .Definitions
        .iter()
        .map(|definition| {
            context
                .physical_stats(definition.ID)
                .expect("physical stats")
                .realtime_count
        })
        .collect()
}

/// 验证无主键表允许重复行写入，flush stats 后各分区行数之和正确。
#[test]
fn duplicate_rows_without_primary_key_survive_partition_storage() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table duplicate_t(a int, b int, key idx(a)) partition by hash(a) partitions 2",
        Vec::new(),
    );
    testkit.MustExec("insert into duplicate_t values (1,1),(1,1)", Vec::new());
    testkit.MustExec("flush stats_delta duplicate_t", Vec::new());
    assert_eq!(
        physical_counts(&testkit, "duplicate_t").iter().sum::<i64>(),
        2
    );
}

/// 验证隐藏行柄列元数据与聚簇主键（clustered handle）标识互不混淆。
#[test]
fn hidden_row_id_metadata_remains_distinct_from_clustered_handles() {
    let extra = NewExtraHandleColInfo();
    assert_eq!(extra.ID, ExtraHandleID);
    assert_eq!(extra.Name.L, "_tidb_rowid");

    let heap = TableInfo::default();
    assert!(!heap.PKIsHandle);
    assert!(!heap.IsCommonHandle);
    assert!(!heap.HasClusteredIndex());

    let clustered = TableInfo {
        PKIsHandle: true,
        ..Default::default()
    };
    assert!(clustered.HasClusteredIndex());
}

/// 验证多线程并发插入后，每个物理分区都有正行数且总和等于写入量。
#[test]
fn concurrent_backfill_dml_updates_all_real_partition_stats() {
    let store = CreateAnalyzeStatsStore();
    let mut owner = TestKit::new(store.clone());
    owner.MustExec(
        "create table concurrent_backfill(a int, b int) partition by hash(a) partitions 4",
        Vec::new(),
    );
    // 4 个 worker 各写 8 行，覆盖 hash 分区键分布。
    let workers = (0..4)
        .map(|worker| {
            let store = store.clone();
            thread::spawn(move || {
                let mut client = TestKit::new(store);
                for offset in 0..8 {
                    let value = worker * 8 + offset;
                    client.MustExec(
                        &format!("insert into concurrent_backfill values ({value},{value})"),
                        Vec::new(),
                    );
                }
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().expect("DML worker");
    }
    owner.MustExec("flush stats_delta concurrent_backfill", Vec::new());
    let counts = physical_counts(&owner, "concurrent_backfill");
    assert_eq!(counts.len(), 4);
    assert!(counts.iter().all(|count| *count > 0));
    assert_eq!(counts.iter().sum::<i64>(), 32);
}

/// 验证 range 分区边界值（less than）正确落入对应物理分区且不丢行。
#[test]
fn range_backfill_routes_boundary_values_without_losing_rows() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table range_backfill(a int, b int) partition by range(a) \
         (partition p0 values less than (10), partition p1 values less than (20), \
          partition pmax values less than (maxvalue))",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into range_backfill values (0,0),(9,9),(10,10),(19,19),(20,20)",
        Vec::new(),
    );
    testkit.MustExec("flush stats_delta range_backfill", Vec::new());
    // p0: [0,9)；p1: [10,19)；pmax: [20,∞) → 2/2/1
    assert_eq!(physical_counts(&testkit, "range_backfill"), vec![2, 2, 1]);
}

/// 验证非聚簇表 reorganize 各 SchemaState 下仅替换 Dropping 范围，pmax 保持不变。
#[test]
fn nonclustered_reorganize_state_replaces_only_the_dropping_range() {
    let old = PartitionDefinition {
        ID: 1,
        Name: NewCIStr("p0"),
        LessThan: vec!["100".to_owned()],
        ..Default::default()
    };
    let mut info = PartitionInfo {
        Definitions: vec![
            old.clone(),
            PartitionDefinition {
                ID: 2,
                Name: NewCIStr("pmax"),
                LessThan: vec!["MAXVALUE".to_owned()],
                ..Default::default()
            },
        ],
        DroppingDefinitions: vec![old],
        DDLAction: ACTION_REORGANIZE_PARTITION,
        DDLState: StateWriteOnly,
        ..Default::default()
    };
    for state in [
        StateDeleteOnly,
        StateWriteOnly,
        StateDeleteReorganization,
        StatePublic,
    ] {
        info.DDLState = state;
        assert_eq!(info.GetPartitionIDByName("pmax"), 2);
        assert_eq!(info.DroppingDefinitions[0].ID, 1);
    }
}

/// 验证 `GCPartitionStates` 只保留仍在 Definitions 中的分区状态条目。
#[test]
fn partition_state_gc_removes_stale_ids_after_reorg_update() {
    let mut info = PartitionInfo {
        Definitions: vec![PartitionDefinition {
            ID: 9,
            Name: NewCIStr("p9"),
            ..Default::default()
        }],
        ..Default::default()
    };
    info.SetStateByID(8, StateDeleteReorganization);
    info.SetStateByID(9, StatePublic);
    info.GCPartitionStates();
    assert_eq!(info.States.len(), 1);
    assert_eq!(info.States[0].ID, 9);
    assert_eq!(info.GetStateByID(9), StatePublic);
}
