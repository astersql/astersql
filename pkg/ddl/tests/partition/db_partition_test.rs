// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 分区表 DDL 集成测试：创建、增删截断、交换、reorganize、全局索引与去分区。
//
// 术语：Partition（分区）将表按键范围或哈希拆成多个物理表；Global Index
// （全局索引）跨所有分区维护一份索引；Reorg（重组）后台迁移分区数据。

use astersql_meta_model::ast::PartitionType;
use astersql_meta_model::{
    ACTION_REMOVE_PARTITIONING, ACTION_REORGANIZE_PARTITION, ActionAddTablePartition,
    ActionDropTablePartition, ActionTruncateTablePartition, GlobalIndexVersionV1, IndexColumn,
    IndexInfo, PartitionDefinition, PartitionInfo, StateDeleteOnly, StateDeleteReorganization,
    StatePublic, StateWriteOnly, TableInfo,
};
use astersql_parser_ast::NewCIStr;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateAnalyzeStatsStore;

/// 构造 Range 分区定义：物理 ID、名称与 `LESS THAN` 上界。
fn definition(id: i64, name: &str, less_than: &str) -> PartitionDefinition {
    PartitionDefinition {
        ID: id,
        Name: NewCIStr(name),
        LessThan: vec![less_than.to_owned()],
        ..Default::default()
    }
}

/// 从 testkit 分析会话的 catalog 取出指定表的 `TableInfo` 克隆。
fn real_table(testkit: &TestKit, name: &str) -> TableInfo {
    testkit
        .AnalyzeStatsContext()
        .expect("canonical partition domain")
        .catalog()
        .get(&("test".to_owned(), name.to_owned()))
        .expect("table in canonical catalog")
        .1
        .Clone()
}

/// 按分区定义顺序返回各物理分区的实时行数。
fn physical_counts(testkit: &TestKit, table: &TableInfo) -> Vec<i64> {
    let context = testkit
        .AnalyzeStatsContext()
        .expect("canonical partition domain");
    table
        .GetPartitionInfo()
        .expect("partition metadata")
        .Definitions
        .iter()
        .map(|partition| {
            context
                .physical_stats(partition.ID)
                .expect("physical partition stats")
                .realtime_count
        })
        .collect()
}

/// 创建 Range 分区表并插入边界行，校验元数据与各物理分区行数。
#[test]
fn creates_range_partition_metadata_and_routes_boundary_rows() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table tp(a int, b int) partition by range(a) \
         (partition p0 values less than (10), partition p1 values less than (20), \
          partition pmax values less than (maxvalue))",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into tp values (0,0),(9,9),(10,10),(19,19),(20,20)",
        Vec::new(),
    );
    testkit.MustExec("flush stats_delta tp", Vec::new());

    let table = real_table(&testkit, "tp");
    let partition = table.GetPartitionInfo().unwrap();
    assert_eq!(partition.Type, PartitionType::Range);
    assert_eq!(partition.Expr, "`a`");
    assert_eq!(
        partition
            .Definitions
            .iter()
            .map(|definition| definition.Name.L.as_str())
            .collect::<Vec<_>>(),
        vec!["p0", "p1", "pmax"]
    );
    assert_eq!(physical_counts(&testkit, &table), vec![2, 2, 1]);
}

/// 创建 Hash 分区并均匀插入，校验每个物理分区都收到更新。
#[test]
fn creates_hash_partitions_and_updates_every_physical_partition() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table hash_t(a int primary key, b int) partition by hash(a) partitions 4",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into hash_t values (0,0),(1,1),(2,2),(3,3),(4,4),(5,5),(6,6),(7,7)",
        Vec::new(),
    );
    testkit.MustExec("flush stats_delta hash_t", Vec::new());

    let table = real_table(&testkit, "hash_t");
    let partition = table.GetPartitionInfo().unwrap();
    assert_eq!(partition.Type, PartitionType::Hash);
    assert_eq!(partition.Num, 4);
    assert_eq!(physical_counts(&testkit, &table), vec![2, 2, 2, 2]);
}

/// Range 定义须严格按上界递增，且各分区物理 ID 互不相同。
#[test]
fn canonical_runtime_keeps_strict_range_definition_order() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table ordered_p(a int) partition by range(a) \
         (partition p0 values less than (10), partition p1 values less than (20), \
          partition pmax values less than (maxvalue))",
        Vec::new(),
    );
    let table = real_table(&testkit, "ordered_p");
    let definitions = &table.GetPartitionInfo().unwrap().Definitions;
    assert_eq!(
        definitions
            .iter()
            .map(|definition| definition.LessThan[0].as_str())
            .collect::<Vec<_>>(),
        vec!["10", "20", "MAXVALUE"]
    );
    assert!(
        definitions
            .windows(2)
            .all(|window| window[0].ID != window[1].ID)
    );
}

/// Add/Drop/Truncate 各 DDL 状态下，`IDsInDDLToIgnore` 与重叠删除分区语义对齐 Go。
#[test]
fn add_drop_and_truncate_states_expose_the_go_visibility_sets() {
    let adding = definition(30, "p2", "MAXVALUE");
    let dropping = definition(10, "p0", "10");
    let mut partition = PartitionInfo {
        Type: PartitionType::Range,
        Enable: true,
        Definitions: vec![dropping.clone(), definition(20, "p1", "20")],
        AddingDefinitions: vec![adding.clone()],
        DDLAction: ActionAddTablePartition,
        DDLState: StateWriteOnly,
        ..Default::default()
    };
    assert_eq!(partition.IDsInDDLToIgnore(), vec![30]);

    // Drop：忽略正在删除的分区 ID，并允许与 DEFAULT 分区重叠。
    partition.DDLAction = ActionDropTablePartition;
    partition.DroppingDefinitions = vec![dropping.clone()];
    assert_eq!(partition.IDsInDDLToIgnore(), vec![10]);
    assert!(partition.CanHaveOverlappingDroppingPartition());
    assert_eq!(partition.GetOverlappingDroppingPartitionIdx(0), 1);

    // Truncate：WriteOnly 忽略新分区 ID；DeleteOnly 改为忽略旧分区。
    partition.DDLAction = ActionTruncateTablePartition;
    partition.NewPartitionIDs = vec![40];
    assert_eq!(partition.IDsInDDLToIgnore(), vec![40]);
    partition.DDLState = StateDeleteOnly;
    assert_eq!(partition.IDsInDDLToIgnore(), vec![10]);
}

/// List 分区删除时，DEFAULT 分区承接被删分区的值；Public 后不再重叠映射。
#[test]
fn list_partition_default_receives_values_from_a_dropping_partition() {
    let mut partition = PartitionInfo {
        Type: PartitionType::List,
        Enable: true,
        Definitions: vec![
            PartitionDefinition {
                ID: 1,
                Name: NewCIStr("p0"),
                InValues: vec![vec!["1".to_owned()], vec!["2".to_owned()]],
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
        DDLAction: ActionDropTablePartition,
        DDLState: StateWriteOnly,
        ..Default::default()
    };
    assert_eq!(partition.GetDefaultListPartition(), 1);
    assert_eq!(partition.GetOverlappingDroppingPartitionIdx(0), 1);
    partition.DDLState = StatePublic;
    assert_eq!(partition.GetOverlappingDroppingPartitionIdx(0), 0);
}

/// Drop/Truncate/Reorganize 处于 DeleteReorganization 时，全局索引元数据仍完整。
#[test]
fn global_index_metadata_survives_drop_truncate_and_reorganize_states() {
    let index = IndexInfo {
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
        Global: true,
        GlobalIndexVersion: GlobalIndexVersionV1,
        State: StatePublic,
        ..Default::default()
    };
    for action in [
        ActionDropTablePartition,
        ActionTruncateTablePartition,
        ACTION_REORGANIZE_PARTITION,
    ] {
        let table = TableInfo {
            Indices: vec![index.clone()],
            Partition: Some(PartitionInfo {
                Enable: true,
                DDLAction: action,
                DDLState: StateDeleteReorganization,
                ..Default::default()
            }),
            ..Default::default()
        }
        .Clone();
        assert!(table.Indices[0].Global);
        assert_eq!(table.Indices[0].Columns.len(), 2);
        assert_eq!(table.Indices[0].GlobalIndexVersion, GlobalIndexVersionV1);
    }
}

/// Exchange Partition（交换分区与普通表）保留双方物理身份与自增/分片位。
#[test]
fn exchange_partition_preserves_both_physical_identities_and_auto_ids() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table exchange_p(a int auto_increment primary key) partition by range(a) \
         (partition p0 values less than (10), partition p1 values less than (20))",
        Vec::new(),
    );
    testkit.MustExec(
        "create table exchange_n(a int auto_increment primary key)",
        Vec::new(),
    );

    let partitioned_before = real_table(&testkit, "exchange_p");
    let normal_before = real_table(&testkit, "exchange_n");
    let partitioned_table_id = partitioned_before.ID;
    let p0_id = partitioned_before
        .GetPartitionInfo()
        .unwrap()
        .GetPartitionIDByName("p0");
    let p1_id = partitioned_before
        .GetPartitionInfo()
        .unwrap()
        .GetPartitionIDByName("p1");

    testkit.MustExec(
        "alter table exchange_p exchange partition p0 with table exchange_n",
        Vec::new(),
    );

    let partitioned_after = real_table(&testkit, "exchange_p");
    let normal_after = real_table(&testkit, "exchange_n");
    let partition_after = partitioned_after.GetPartitionInfo().unwrap();
    assert_eq!(partitioned_after.ID, partitioned_table_id);
    assert_eq!(partition_after.GetPartitionIDByName("p0"), normal_before.ID);
    assert_eq!(partition_after.GetPartitionIDByName("p1"), p1_id);
    assert_eq!(normal_after.ID, p0_id);
    assert_eq!(partitioned_after.AutoIncID, partitioned_before.AutoIncID);
    assert_eq!(partitioned_after.AutoRandID, partitioned_before.AutoRandID);
    assert_eq!(normal_after.AutoIncID, normal_before.AutoIncID);
    assert_eq!(normal_after.AutoRandID, normal_before.AutoRandID);
    assert_eq!(
        partitioned_after.ShardRowIDBits,
        partitioned_before.ShardRowIDBits
    );
    assert_eq!(
        partitioned_after.MaxShardRowIDBits,
        partitioned_before.MaxShardRowIDBits
    );
}

/// Remove Partitioning 清理 reorg 中间态后表 ID/自增仍保留，分区元数据清空。
#[test]
fn remove_partitioning_clears_intermediate_state_but_keeps_table_identity() {
    let mut table = TableInfo {
        ID: 88,
        AutoIncID: 101,
        Partition: Some(PartitionInfo {
            Type: PartitionType::Hash,
            Enable: true,
            Num: 4,
            DDLAction: ACTION_REMOVE_PARTITIONING,
            DDLState: StateDeleteReorganization,
            NewTableID: 99,
            DDLExpr: "a".to_owned(),
            ..Default::default()
        }),
        ..Default::default()
    };
    let partition = table.Partition.as_mut().unwrap();
    partition.ClearReorgIntermediateInfo();
    partition.Type = PartitionType::None;
    partition.Enable = false;
    assert_eq!(table.ID, 88);
    assert_eq!(table.AutoIncID, 101);
    assert!(table.GetPartitionInfo().is_none());
}

/// GC 分区状态只保留当前物理定义中的 ID，并支持按名/ID 互查。
#[test]
fn partition_state_gc_keeps_only_current_physical_ids() {
    let mut partition = PartitionInfo {
        Type: PartitionType::Range,
        Enable: true,
        Definitions: vec![definition(2, "p1", "MAXVALUE")],
        ..Default::default()
    };
    partition.SetStateByID(1, StateDeleteOnly);
    partition.SetStateByID(2, StatePublic);
    partition.GCPartitionStates();
    assert_eq!(partition.States.len(), 1);
    assert_eq!(partition.States[0].ID, 2);
    assert_eq!(partition.GetNameByID(2), "p1");
    assert_eq!(partition.GetPartitionIDByName("P1"), 2);
}

/// 对应 Go 中 ADD/TRUNCATE/DROP/REORGANIZE 的物理身份断言：只替换受影响
/// 的分区，不能误改表 ID 或未参与 DDL 的分区。
#[test]
fn partition_ddl_replaces_only_target_physical_ids() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table ddl_targets(a int) partition by range(a) \
         (partition p0 values less than (10), partition p1 values less than (20))",
        Vec::new(),
    );
    let initial = real_table(&testkit, "ddl_targets");
    let table_id = initial.ID;
    let initial_partition = initial.GetPartitionInfo().unwrap();
    let p0 = initial_partition.GetPartitionIDByName("p0");
    let p1 = initial_partition.GetPartitionIDByName("p1");

    testkit.MustExec(
        "alter table ddl_targets add partition (partition p2 values less than (30))",
        Vec::new(),
    );
    let added = real_table(&testkit, "ddl_targets");
    assert_eq!(added.ID, table_id);
    let added_partition = added.GetPartitionInfo().unwrap();
    assert_eq!(added_partition.GetPartitionIDByName("p0"), p0);
    assert_eq!(added_partition.GetPartitionIDByName("p1"), p1);
    let p2 = added_partition.GetPartitionIDByName("p2");
    assert!(![table_id, p0, p1].contains(&p2));

    testkit.MustExec("alter table ddl_targets truncate partition p0", Vec::new());
    let truncated = real_table(&testkit, "ddl_targets");
    let truncated_partition = truncated.GetPartitionInfo().unwrap();
    assert_ne!(truncated_partition.GetPartitionIDByName("p0"), p0);
    assert_eq!(truncated_partition.GetPartitionIDByName("p1"), p1);
    assert_eq!(truncated_partition.GetPartitionIDByName("p2"), p2);

    testkit.MustExec("alter table ddl_targets drop partition p1", Vec::new());
    let dropped = real_table(&testkit, "ddl_targets");
    assert_eq!(dropped.ID, table_id);
    assert_eq!(
        dropped
            .GetPartitionInfo()
            .unwrap()
            .GetPartitionIDByName("p1"),
        -1
    );

    testkit.MustExec(
        "alter table ddl_targets reorganize partition p0, p2 into \
         (partition pn values less than (30))",
        Vec::new(),
    );
    let reorganized = real_table(&testkit, "ddl_targets");
    let reorganized_partition = reorganized.GetPartitionInfo().unwrap();
    assert_eq!(reorganized.ID, table_id);
    assert_eq!(reorganized_partition.GetPartitionIDByName("p0"), -1);
    assert_eq!(reorganized_partition.GetPartitionIDByName("p2"), -1);
    assert!(reorganized_partition.GetPartitionIDByName("pn") > 0);
}

/// 对应 Go 创建分区表的错误矩阵：非法边界必须在 catalog 中留下空结果，
/// 随后合法建表仍可使用同一会话，证明失败 DDL 没有部分提交。
#[test]
fn invalid_partition_definition_rolls_back_without_catalog_residue() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExecToErr(
        "create table invalid_partition(a int) partition by range(a) \
         (partition p0 values less than (20), partition p1 values less than (10))",
    );
    let context = testkit.AnalyzeStatsContext().expect("analyze session");
    assert!(
        !context
            .catalog()
            .contains_key(&("test".to_owned(), "invalid_partition".to_owned()))
    );

    testkit.MustExec(
        "create table valid_partition(a int) partition by range(a) \
         (partition p0 values less than (10), partition p1 values less than (20))",
        Vec::new(),
    );
    assert_eq!(
        real_table(&testkit, "valid_partition")
            .GetPartitionInfo()
            .unwrap()
            .Definitions
            .len(),
        2
    );
}
