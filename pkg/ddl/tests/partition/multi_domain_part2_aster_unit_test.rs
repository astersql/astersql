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

// 多 Domain 分区 DDL 可见性测试（第 2 部分）。
//
// 覆盖 Reorganize Partition（重组分区边界并回填）、截断分区时全局索引可见性集合、
// REMOVE/ALTER PARTITIONING 时覆盖索引列与 Global 标志的保留，
// 以及 Exchange Partition 身份与中间态元数据清理。

use astersql_meta_model::ast::PartitionType;
use astersql_meta_model::{
    ACTION_ALTER_TABLE_PARTITIONING, ACTION_REMOVE_PARTITIONING, ACTION_REORGANIZE_PARTITION,
    ActionNone, ActionTruncateTablePartition, ExchangePartitionInfo, GlobalIndexVersionV1,
    IndexColumn, IndexInfo, PartitionDefinition, PartitionInfo, StateDeleteOnly,
    StateDeleteReorganization, StateNone, StatePublic, StateWriteOnly, TableInfo, UpdateIndexInfo,
};
use astersql_parser_ast::NewCIStr;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateAnalyzeStatsStore;
use std::collections::HashMap;

/// 构造带 LessThan 上界的 range 分区定义。
fn partition(id: i64, name: &str, boundary: &str) -> PartitionDefinition {
    PartitionDefinition {
        ID: id,
        Name: NewCIStr(name),
        LessThan: vec![boundary.to_owned()],
        ..Default::default()
    }
}

/// 构造覆盖列 a、b 的唯一索引；`global` 控制是否为全局索引。
fn covering_index(global: bool) -> IndexInfo {
    IndexInfo {
        ID: 50,
        Name: NewCIStr("idx_ab"),
        Columns: vec![
            IndexColumn {
                Name: NewCIStr("a"),
                Offset: 0,
                Length: -1,
                ..Default::default()
            },
            IndexColumn {
                Name: NewCIStr("b"),
                Offset: 1,
                Length: -1,
                ..Default::default()
            },
        ],
        Unique: true,
        Global: global,
        GlobalIndexVersion: GlobalIndexVersionV1,
        State: StatePublic,
        ..Default::default()
    }
}

/// 验证 reorganize 过程中旧/新分区定义随 SchemaState 推进仍可按名解析。
#[test]
fn reorganize_partition_tracks_old_and_new_ranges_through_schema_states() {
    let old = partition(1, "p1", "200");
    let p0 = partition(2, "p0", "100");
    let p1 = partition(3, "p1", "200");
    let mut info = PartitionInfo {
        Definitions: vec![old.clone(), partition(4, "pmax", "MAXVALUE")],
        AddingDefinitions: vec![p0.clone(), p1.clone()],
        DroppingDefinitions: vec![old],
        DDLAction: ACTION_REORGANIZE_PARTITION,
        DDLState: StateWriteOnly,
        NewPartitionIDs: vec![2, 3],
        ..Default::default()
    };
    // WriteOnly 时按名仍指向旧物理 ID；进入 DeleteReorg 后用 Adding 替换 Definitions。
    assert_eq!(info.GetPartitionIDByName("p1"), 1);
    assert_eq!(info.AddingDefinitions.len(), 2);
    info.DDLState = StateDeleteReorganization;
    assert_eq!(info.DroppingDefinitions[0].ID, 1);
    info.Definitions.splice(0..1, [p0, p1]);
    assert_eq!(info.GetPartitionIDByName("P0"), 2);
    assert_eq!(info.GetPartitionIDByName("P1"), 3);
    assert_eq!(info.GetPartitionIDByName("missing"), -1);
}

/// 验证回填相关 DML 后，共享 Domain 对每个物理分区 ID 都有 pending stats。
#[test]
fn backfill_dml_is_visible_to_every_partition_physical_id() {
    let store = CreateAnalyzeStatsStore();
    let mut owner = TestKit::new(store.clone());
    owner.MustExec(
        "create table backfill_t(a int primary key, b int) partition by hash(a) partitions 2",
        Vec::new(),
    );
    owner.MustExec(
        "insert into backfill_t values (1,10),(2,20),(3,30),(4,40)",
        Vec::new(),
    );
    let observer = TestKit::new(store);
    let context = observer.AnalyzeStatsContext().expect("shared DDL domain");
    let catalog = context.catalog();
    let definitions = &catalog
        .get(&("test".to_owned(), "backfill_t".to_owned()))
        .unwrap()
        .1
        .GetPartitionInfo()
        .unwrap()
        .Definitions;
    let pending = context.pending_stats_delta_ids();
    assert_eq!(definitions.len(), 2);
    assert_eq!(pending.len(), 2);
    assert!(
        pending
            .iter()
            .all(|id| definitions.iter().any(|definition| definition.ID == *id))
    );
}

/// 验证截断分区时，WriteOnly/DeleteOnly 下忽略集合在新旧物理 ID 间切换。
#[test]
fn truncate_partition_with_global_index_switches_visibility_sets() {
    let mut info = PartitionInfo {
        Definitions: vec![partition(10, "p0", "100"), partition(11, "p1", "MAXVALUE")],
        DroppingDefinitions: vec![partition(10, "p0", "100")],
        NewPartitionIDs: vec![20],
        DDLAction: ActionTruncateTablePartition,
        DDLState: StateWriteOnly,
        DDLChangedIndex: HashMap::from([(50, true)]),
        ..Default::default()
    };
    assert_eq!(info.IDsInDDLToIgnore(), vec![20]);
    assert_eq!(info.DDLChangedIndex.get(&50), Some(&true));
    info.DDLState = StateDeleteOnly;
    assert_eq!(info.IDsInDDLToIgnore(), vec![10]);
    info.DDLState = StateDeleteReorganization;
    assert_eq!(info.IDsInDDLToIgnore(), vec![10]);
    info.DroppingDefinitions.clear();
    assert!(info.IDsInDDLToIgnore().is_empty());
    info.DDLState = StatePublic;
    assert!(info.IDsInDDLToIgnore().is_empty());
}

/// 验证 REMOVE/REORGANIZE/ALTER PARTITIONING 时 Clone 保留覆盖索引列与 Global 标志。
#[test]
fn partitioning_variants_preserve_covering_index_columns_and_global_flag() {
    for action in [
        ACTION_REMOVE_PARTITIONING,
        ACTION_REORGANIZE_PARTITION,
        ACTION_ALTER_TABLE_PARTITIONING,
    ] {
        for global in [false, true] {
            let index = covering_index(global);
            let table = TableInfo {
                Indices: vec![index],
                Partition: Some(PartitionInfo {
                    DDLAction: action,
                    DDLUpdateIndexes: vec![UpdateIndexInfo {
                        IndexName: "idx_ab".to_owned(),
                        Global: global,
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            };
            let clone = table.Clone();
            let cloned_index = &clone.Indices[0];
            assert_eq!(cloned_index.Columns.len(), 2);
            assert_eq!(cloned_index.Columns[0].Name.L, "a");
            assert_eq!(cloned_index.Columns[1].Name.L, "b");
            assert_eq!(cloned_index.Global, global);
            assert_eq!(
                clone.Partition.as_ref().unwrap().DDLUpdateIndexes[0].Global,
                global
            );
        }
    }
}

/// 验证交换分区元数据在状态推进到 Public 后仍对齐源分区定义 ID。
#[test]
fn exchange_partition_state_keeps_source_and_destination_identity() {
    let mut table = TableInfo {
        ID: 100,
        Partition: Some(PartitionInfo {
            Definitions: vec![partition(101, "p0", "100")],
            DDLAction: ACTION_ALTER_TABLE_PARTITIONING,
            DDLState: StateWriteOnly,
            ..Default::default()
        }),
        ExchangePartitionInfo: Some(ExchangePartitionInfo {
            ExchangePartitionTableID: 200,
            ExchangePartitionDefID: 101,
            XXXExchangePartitionFlag: true,
        }),
        ..Default::default()
    };
    let exchange = table.ExchangePartitionInfo.as_ref().unwrap();
    assert_eq!(
        (
            exchange.ExchangePartitionTableID,
            exchange.ExchangePartitionDefID
        ),
        (200, 101)
    );
    table.Partition.as_mut().unwrap().DDLState = StatePublic;
    assert_eq!(
        table.Partition.as_ref().unwrap().GetPartitionIDByName("p0"),
        table
            .ExchangePartitionInfo
            .as_ref()
            .unwrap()
            .ExchangePartitionDefID
    );
}

/// 验证重组完成后 `ClearReorgIntermediateInfo` 清空全部中间态字段。
#[test]
fn completed_repartition_clears_all_intermediate_metadata() {
    let mut info = PartitionInfo {
        DDLAction: ACTION_REORGANIZE_PARTITION,
        DDLState: StateDeleteReorganization,
        DDLType: PartitionType::Range,
        NewTableID: 99,
        DDLExpr: "a".to_owned(),
        DDLColumns: vec![NewCIStr("a"), NewCIStr("b")],
        DDLChangedIndex: HashMap::from([(50, false)]),
        ..Default::default()
    };
    info.ClearReorgIntermediateInfo();
    assert_eq!(info.DDLAction, ActionNone);
    assert_eq!(info.DDLState, StateNone);
    assert_eq!(info.DDLType, PartitionType::None);
    assert_eq!(info.NewTableID, 0);
    assert!(info.DDLExpr.is_empty());
    assert!(info.DDLColumns.is_empty());
    assert!(info.DDLChangedIndex.is_empty());
}
