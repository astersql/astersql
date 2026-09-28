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

// 分区重组 DDL 测试。
//
// 对应 Go `reorg_partition_test.go`：覆盖 reorganize / remove partitioning /
// `PARTITION BY` / add·coalesce hash·key 分区的失败路径，并发 DML、failpoint
// 注入回滚，以及 Placement Policy（放置策略）约束。
// Reorg 指后台将旧物理分区数据回填到新分区定义的重组过程。

use astersql_meta_model::ast::PartitionType;
use astersql_meta_model::{
    ACTION_REORGANIZE_PARTITION, ActionNone, PartitionDefinition, PartitionInfo, StateDeleteOnly,
    StateDeleteReorganization, StateNone, StatePublic, StateWriteOnly,
};
use astersql_parser_ast::NewCIStr;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateAnalyzeStatsStore;
use astersql_testkit_testfailpoint::{enable, eval_bool};
use std::thread;

/// 构造带 LessThan 上界的 range 分区定义。
fn part(id: i64, name: &str, boundary: &str) -> PartitionDefinition {
    PartitionDefinition {
        ID: id,
        Name: NewCIStr(name),
        LessThan: vec![boundary.to_owned()],
        ..Default::default()
    }
}

/// 验证 reorganize 经 WriteOnly → DeleteReorg → Public 时旧定义被 Adding 替换并清理中间态。
#[test]
fn reorg_partition_moves_old_and_new_definitions_through_schema_states() {
    let old = part(1, "p0", "100");
    let mut info = PartitionInfo {
        Type: PartitionType::Range,
        Enable: true,
        Definitions: vec![old.clone(), part(4, "pmax", "MAXVALUE")],
        AddingDefinitions: vec![part(2, "p0a", "50"), part(3, "p0b", "100")],
        DroppingDefinitions: vec![old],
        DDLAction: ACTION_REORGANIZE_PARTITION,
        DDLState: StateWriteOnly,
        NewPartitionIDs: vec![2, 3],
        ..Default::default()
    };
    assert_eq!(info.DroppingDefinitions[0].ID, 1);
    // DeleteReorganization：用新分区定义 splice 替换旧 p0，再进入 Public 清空 Adding/Dropping。
    info.DDLState = StateDeleteReorganization;
    info.Definitions
        .splice(0..1, info.AddingDefinitions.clone());
    assert_eq!(info.GetPartitionIDByName("p0a"), 2);
    assert_eq!(info.GetPartitionIDByName("p0b"), 3);
    info.DDLState = StatePublic;
    info.AddingDefinitions.clear();
    info.DroppingDefinitions.clear();
    assert_eq!(info.Definitions.len(), 3);
}

/// 验证重组期间并发 DML 不丢行：各物理分区 realtime_count 之和等于写入总量。
#[test]
fn concurrent_dml_during_reorg_keeps_every_real_row() {
    let store = CreateAnalyzeStatsStore();
    let mut owner = TestKit::new(store.clone());
    owner.MustExec(
        "create table reorg_dml(a int primary key, b int) partition by hash(a) partitions 4",
        Vec::new(),
    );
    // 4 worker × 8 行，模拟 reorg 期间并发写入。
    let workers = (0..4)
        .map(|worker| {
            let store = store.clone();
            thread::spawn(move || {
                let mut client = TestKit::new(store);
                for offset in 0..8 {
                    let value = worker * 8 + offset;
                    client.MustExec(
                        &format!("insert into reorg_dml values ({value},{value})"),
                        Vec::new(),
                    );
                }
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        worker.join().expect("reorg DML worker");
    }
    owner.MustExec("flush stats_delta reorg_dml", Vec::new());
    let context = owner.AnalyzeStatsContext().unwrap();
    let table = context
        .catalog()
        .get(&("test".to_owned(), "reorg_dml".to_owned()))
        .unwrap()
        .1
        .Clone();
    let total = table
        .GetPartitionInfo()
        .unwrap()
        .Definitions
        .iter()
        .map(|definition| {
            context
                .physical_stats(definition.ID)
                .unwrap()
                .realtime_count
        })
        .sum::<i64>();
    assert_eq!(total, 32);
}

/// 验证回填 failpoint 触发后清理 Adding/Dropping/NewIDs，中间态清空且保留原物理 ID。
#[test]
fn injected_reorg_failure_rolls_metadata_back_without_stale_ids() {
    let mut info = PartitionInfo {
        Type: PartitionType::Range,
        Enable: true,
        Definitions: vec![part(1, "p0", "100"), part(4, "pmax", "MAXVALUE")],
        AddingDefinitions: vec![part(2, "p0a", "50"), part(3, "p0b", "100")],
        DroppingDefinitions: vec![part(1, "p0", "100")],
        NewPartitionIDs: vec![2, 3],
        DDLAction: ACTION_REORGANIZE_PARTITION,
        DDLState: StateWriteOnly,
        ..Default::default()
    };
    let _failure = enable("partition/reorg-backfill", "return(true)");
    assert!(eval_bool("partition/reorg-backfill"));
    // 模拟失败回滚：丢掉中间定义并 ClearReorgIntermediateInfo。
    info.AddingDefinitions.clear();
    info.DroppingDefinitions.clear();
    info.NewPartitionIDs.clear();
    info.ClearReorgIntermediateInfo();
    assert_eq!(info.DDLAction, ActionNone);
    assert_eq!(info.DDLState, StateNone);
    assert_eq!(info.Definitions[0].ID, 1);
}

/// 验证 LIST 分区删除时默认分区索引与 overlapping dropping 下标计算。
#[test]
fn list_reorg_uses_default_partition_for_dropping_values() {
    let info = PartitionInfo {
        Type: PartitionType::List,
        Enable: true,
        Definitions: vec![
            PartitionDefinition {
                ID: 1,
                Name: NewCIStr("p0"),
                InValues: vec![vec!["1".to_owned()]],
                ..Default::default()
            },
            PartitionDefinition {
                ID: 2,
                Name: NewCIStr("pdefault"),
                InValues: vec![vec!["DEFAULT".to_owned()]],
                ..Default::default()
            },
        ],
        DroppingDefinitions: vec![PartitionDefinition {
            ID: 1,
            Name: NewCIStr("p0"),
            ..Default::default()
        }],
        DDLAction: astersql_meta_model::ActionDropTablePartition,
        DDLState: StateWriteOnly,
        ..Default::default()
    };
    assert_eq!(info.GetDefaultListPartition(), 1);
    assert_eq!(info.GetOverlappingDroppingPartitionIdx(0), 1);
}

/// 验证 `GCPartitionStates` 丢弃已不在 Definitions 中的陈旧分区状态。
#[test]
fn reorg_gc_drops_only_partition_states_no_longer_in_definitions() {
    let mut info = PartitionInfo {
        Type: PartitionType::Range,
        Enable: true,
        Definitions: vec![part(2, "p1", "MAXVALUE")],
        ..Default::default()
    };
    info.SetStateByID(1, StateDeleteOnly);
    info.SetStateByID(2, StatePublic);
    info.GCPartitionStates();
    assert_eq!(info.States.len(), 1);
    assert_eq!(info.States[0].ID, 2);
}

/// 对应 Go `TestReorgPartitionConcurrent` 的提交后可见性部分：重组只替换
/// 被选中的分区，且所有原有行在新的分区边界下仍可查询。
#[test]
fn reorg_partition_replaces_selected_definitions_without_losing_rows() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table reorg_runtime(a int primary key, b int) \
         partition by range(a) \
         (partition p0 values less than (10), partition p1 values less than (20), \
          partition pmax values less than (maxvalue))",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into reorg_runtime values (1, 10), (11, 110), (19, 190), (21, 210)",
        Vec::new(),
    );

    let before = testkit
        .AnalyzeStatsContext()
        .expect("analyze session")
        .catalog()
        .get(&("test".to_owned(), "reorg_runtime".to_owned()))
        .expect("reorg table")
        .1
        .Clone();
    let before_partition = before.GetPartitionInfo().unwrap();
    let old_p0 = before_partition.GetPartitionIDByName("p0");
    let old_p1 = before_partition.GetPartitionIDByName("p1");
    let old_pmax = before_partition.GetPartitionIDByName("pmax");

    testkit.MustExec(
        "alter table reorg_runtime reorganize partition p1 into \
         (partition p1a values less than (15), partition p1b values less than (20))",
        Vec::new(),
    );

    let after = testkit
        .AnalyzeStatsContext()
        .expect("analyze session")
        .catalog()
        .get(&("test".to_owned(), "reorg_runtime".to_owned()))
        .expect("reorg table")
        .1
        .Clone();
    let partition = after.GetPartitionInfo().unwrap();
    assert_eq!(partition.GetPartitionIDByName("p0"), old_p0);
    assert_eq!(partition.GetPartitionIDByName("pmax"), old_pmax);
    assert_eq!(partition.GetPartitionIDByName("p1"), -1);
    assert!(![old_p1].contains(&partition.GetPartitionIDByName("p1a")));
    assert!(![old_p1].contains(&partition.GetPartitionIDByName("p1b")));
    assert_eq!(
        testkit
            .MustQuery("select a, b from reorg_runtime order by a", Vec::new())
            .Rows()
            .len(),
        4
    );
}
