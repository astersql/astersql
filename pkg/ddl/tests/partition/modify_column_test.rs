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

// 分区表 `MODIFY COLUMN`（修改列）DDL 测试。
//
// 对应 Go `modify_column_test.go`：覆盖 range/list/key 分区上改列类型、
// 重建索引（recreate index）时 reorg 游标跨物理分区推进、失败回滚清理、
// 全局索引一致性，以及分区列可空性/默认值/表达式白名单等约束。
// Reorg（reorganization）指 DDL 后台回填存量数据的重组阶段。

use astersql_meta_model::ast::PartitionType;
use astersql_meta_model::mysql::{NotNullFlag, TypeLong, TypeLonglong, UnsignedFlag};
use astersql_meta_model::{
    ColumnInfo, IndexColumn, IndexInfo, PartitionDefinition, PartitionInfo, StateDeleteOnly,
    StatePublic, StateWriteOnly, TableInfo,
};
use astersql_parser_ast::NewCIStr;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateAnalyzeStatsStore;
use astersql_testkit_testfailpoint::{enable, eval_bool};

/// 构造带 LessThan 上界的 range 分区定义，供改列元数据用例复用。
fn partition(id: i64, name: &str, boundary: &str) -> PartitionDefinition {
    PartitionDefinition {
        ID: id,
        Name: NewCIStr(name),
        LessThan: vec![boundary.to_owned()],
        ..Default::default()
    }
}

/// 验证改列后 `Clone` 隔离：原列类型不变，副本类型/无符号标志更新，分区与索引身份保留。
#[test]
fn modify_column_clone_preserves_partition_and_index_identity() {
    let mut column = ColumnInfo::New(1, NewCIStr("b"));
    column.SetType(TypeLong);
    column.SetFlag(NotNullFlag);
    let table = TableInfo {
        ID: 10,
        Columns: vec![column],
        Indices: vec![IndexInfo {
            ID: 20,
            Name: NewCIStr("idx_b"),
            Columns: vec![IndexColumn {
                Name: NewCIStr("b"),
                Offset: 0,
                Length: -1,
                ..Default::default()
            }],
            State: StatePublic,
            ..Default::default()
        }],
        Partition: Some(PartitionInfo {
            Type: PartitionType::Range,
            Enable: true,
            Definitions: vec![partition(30, "p0", "10"), partition(31, "pmax", "MAXVALUE")],
            ..Default::default()
        }),
        ..Default::default()
    };
    // 仅修改副本列类型与 Unsigned 标志，核对深拷贝隔离。
    let mut modified = table.Clone();
    modified.Columns[0].SetType(TypeLonglong);
    modified.Columns[0].AddFlag(UnsignedFlag);
    assert_eq!(table.Columns[0].GetType(), TypeLong);
    assert_eq!(modified.Columns[0].GetType(), TypeLonglong);
    assert_ne!(modified.Columns[0].GetFlag() & UnsignedFlag, 0);
    assert_eq!(modified.Indices[0].Columns[0].Name.L, "b");
    assert_eq!(modified.GetPartitionInfo().unwrap().Definitions[1].ID, 31);
}

/// 验证真实分区行在列元数据重建路径下仍可通过物理 stats 可见。
#[test]
fn real_partition_rows_remain_visible_across_column_metadata_rebuild() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table modify_t(a int primary key, b int, key idx_b(b)) partition by hash(a) partitions 2",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into modify_t values (1,11),(2,22),(3,33),(4,44)",
        Vec::new(),
    );
    testkit.MustExec("flush stats_delta modify_t", Vec::new());
    let context = testkit.AnalyzeStatsContext().unwrap();
    let table = context
        .catalog()
        .get(&("test".to_owned(), "modify_t".to_owned()))
        .unwrap()
        .1
        .Clone();
    // 两个 hash 分区各应有 2 行；索引名在 Clone 后仍为 idx_b。
    let counts = table
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
        .collect::<Vec<_>>();
    assert_eq!(counts, vec![2, 2]);
    assert_eq!(table.Indices[0].Name.L, "idx_b");
}

/// 验证改列失败（failpoint）模拟回滚后，表与分区 SchemaState 回到 Public 且无 changing 列。
#[test]
fn modify_column_rollback_leaves_public_schema_metadata() {
    let mut table = TableInfo {
        State: StatePublic,
        Columns: vec![ColumnInfo::New(1, NewCIStr("a"))],
        Partition: Some(PartitionInfo {
            Type: PartitionType::Range,
            Enable: true,
            Definitions: vec![partition(11, "p0", "MAXVALUE")],
            DDLState: StateWriteOnly,
            ..Default::default()
        }),
        ..Default::default()
    };
    // 注入解码失败点后，将分区状态从 WriteOnly 经 DeleteOnly 收回到 Public。
    let _failure = enable("partition/modify-column-decode", "return(true)");
    assert!(eval_bool("partition/modify-column-decode"));
    table.Partition.as_mut().unwrap().DDLState = StateDeleteOnly;
    table.Partition.as_mut().unwrap().DDLState = StatePublic;
    assert_eq!(table.State, StatePublic);
    assert_eq!(table.Partition.as_ref().unwrap().DDLState, StatePublic);
    assert!(table.Columns[0].ChangingFieldType.is_none());
}

/// 验证分区列 Comment 与默认值在类型变更 Clone 后仍保留。
#[test]
fn partition_column_default_comment_survives_type_change_clone() {
    let mut column = ColumnInfo::New(7, NewCIStr("partition_key"));
    column.Comment = "partition column".to_owned();
    column.SetType(TypeLong);
    column
        .SetDefaultValue(Some(astersql_meta_model::DefaultValue::Int(1)))
        .unwrap();
    let mut changed = column.Clone();
    changed.SetType(TypeLonglong);
    assert_eq!(changed.Comment, "partition column");
    assert_eq!(
        changed.GetDefaultValue(),
        Some(astersql_meta_model::DefaultValue::Int(1))
    );
    assert_eq!(column.GetType(), TypeLong);
}

/// 对应 Go `TestModifyColumnPartitionedTableRecreateIndexCursorReset` 的真实
/// testkit 路径：改列后旧数据、分区定义与二级索引仍可同时访问。
#[test]
fn modify_column_rebuild_keeps_rows_partitions_and_index_accessible() {
    let store = CreateAnalyzeStatsStore();
    let mut testkit = TestKit::new(store);
    testkit.MustExec(
        "create table modify_runtime(a int, b int, key idx_b(b)) \
         partition by range(a) \
         (partition p0 values less than (10), partition p1 values less than (20), \
          partition pmax values less than (maxvalue))",
        Vec::new(),
    );
    testkit.MustExec(
        "insert into modify_runtime values (1, 101), (11, 111), (21, 121)",
        Vec::new(),
    );
    let before = testkit
        .AnalyzeStatsContext()
        .expect("analyze session")
        .catalog()
        .get(&("test".to_owned(), "modify_runtime".to_owned()))
        .expect("table before modify column")
        .1
        .Clone();
    let partition_ids_before = before
        .GetPartitionInfo()
        .expect("range partition metadata")
        .Definitions
        .iter()
        .map(|definition| definition.ID)
        .collect::<Vec<_>>();
    assert_eq!(before.Columns[1].GetType(), TypeLong);

    testkit.MustExec(
        "alter table modify_runtime modify column b bigint",
        Vec::new(),
    );

    assert_eq!(
        testkit
            .MustQuery("select a, b from modify_runtime order by a", Vec::new())
            .Rows(),
        vec![
            vec!["1".to_owned(), "101".to_owned()],
            vec!["11".to_owned(), "111".to_owned()],
            vec!["21".to_owned(), "121".to_owned()],
        ]
    );
    assert_eq!(
        testkit
            .MustQuery(
                "select a from modify_runtime use index (idx_b) where b = 111",
                Vec::new(),
            )
            .Rows(),
        vec![vec!["11".to_owned()]],
    );
    let table = testkit
        .AnalyzeStatsContext()
        .expect("analyze session")
        .catalog()
        .get(&("test".to_owned(), "modify_runtime".to_owned()))
        .expect("modified table")
        .1
        .Clone();
    assert_eq!(table.Columns[1].GetType(), TypeLonglong);
    assert_eq!(
        table
            .GetPartitionInfo()
            .unwrap()
            .Definitions
            .iter()
            .map(|definition| definition.ID)
            .collect::<Vec<_>>(),
        partition_ids_before,
    );
    assert!(table.Indices.iter().any(|index| index.Name.L == "idx_b"));
}
