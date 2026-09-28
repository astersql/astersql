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

// 分区表 Placement Policy（放置策略）元数据测试。
//
// Placement Policy 描述表或分区的副本应落在哪些 TiKV 标签/Region 上。
// `ClearPlacement` 应清空表级与各分区定义上的 `PlacementPolicyRef`，
// 且不影响事先 `Clone` 出的副本；`PolicyRefInfo` 的 clone 应为深拷贝名称。

use astersql_meta_model::{PartitionDefinition, PartitionInfo, PolicyRefInfo, TableInfo};
use astersql_parser_ast::NewCIStr;
use astersql_testkit::TestKit;
use astersql_testkit::mockstore::CreateAnalyzeStatsStore;

/// 构造一条放置策略引用（ID + 名称）。
fn policy(id: i64, name: &str) -> PolicyRefInfo {
    PolicyRefInfo {
        ID: id,
        Name: NewCIStr(name),
    }
}

struct TestCase {
    schema_pp: &'static str,
    create_table_extra: &'static str,
    alter_sql: &'static str,
    before_table_pp: &'static str,
    before_partitions_pp: &'static [&'static str],
    after_table_pp: &'static str,
    after_partitions_pp: &'static [&'static str],
    error: &'static str,
}

fn placement_names(testkit: &TestKit, database: &str) -> (String, Vec<String>) {
    let context = testkit.AnalyzeStatsContext().expect("shared domain");
    let catalog = context.catalog();
    let table = &catalog
        .get(&(database.to_ascii_lowercase(), "t".to_owned()))
        .expect("table t must exist")
        .1;
    let table_policy = table
        .PlacementPolicyRef
        .as_ref()
        .map_or_else(String::new, |policy| policy.Name.O.clone());
    let partition_policies = table
        .GetPartitionInfo()
        .map(|partition| {
            partition
                .Definitions
                .iter()
                .map(|definition| {
                    definition
                        .PlacementPolicyRef
                        .as_ref()
                        .map_or_else(String::new, |policy| policy.Name.O.clone())
                })
                .collect()
        })
        .unwrap_or_default();
    (table_policy, partition_policies)
}

/// Go `TestPartitionByWithPlacement` 的完整表驱动矩阵：覆盖 schema/table policy
/// 继承、显式分区 policy、PARTITION BY/REMOVE PARTITIONING，以及禁止把 placement
/// 与另一 schema change 合并的错误路径。共享 store 的独立会话还验证 DDL 后可写性。
#[test]
fn partition_by_with_placement_matches_go_matrix() {
    const NONE: &[&str] = &[];
    const P0_P3_P0: &[&str] = &["", "pp3", ""];
    const P0_P1_P2: &[&str] = &["", "pp1", "pp2"];
    const P1_P2_P0: &[&str] = &["pp1", "pp2", ""];
    let cases = [
        TestCase {
            schema_pp: "",
            create_table_extra: "",
            alter_sql: "alter table t partition by range (a) (partition p0 values less than (88), partition p1 values less than (222), partition p2 values less than (288), partition pMax values less than (maxvalue))",
            before_table_pp: "",
            before_partitions_pp: NONE,
            after_table_pp: "",
            after_partitions_pp: NONE,
            error: "",
        },
        TestCase {
            schema_pp: " placement policy pp1",
            create_table_extra: "",
            alter_sql: "alter table t partition by range (a) (partition p0 values less than (88), partition p1 values less than (222), partition p2 values less than (288), partition pMax values less than (maxvalue))",
            before_table_pp: "pp1",
            before_partitions_pp: NONE,
            after_table_pp: "pp1",
            after_partitions_pp: NONE,
            error: "",
        },
        TestCase {
            schema_pp: " placement policy pp1",
            create_table_extra: " placement policy pp2",
            alter_sql: "alter table t partition by range (a) (partition p0 values less than (88), partition p1 values less than (222), partition p2 values less than (288), partition pMax values less than (maxvalue))",
            before_table_pp: "pp2",
            before_partitions_pp: NONE,
            after_table_pp: "pp2",
            after_partitions_pp: NONE,
            error: "",
        },
        TestCase {
            schema_pp: "",
            create_table_extra: " placement policy pp1",
            alter_sql: "alter table t partition by range (a) (partition p0 values less than (88), partition p1 values less than (222), partition p2 values less than (288), partition pMax values less than (maxvalue))",
            before_table_pp: "pp1",
            before_partitions_pp: NONE,
            after_table_pp: "pp1",
            after_partitions_pp: NONE,
            error: "",
        },
        TestCase {
            schema_pp: " placement policy pp1",
            create_table_extra: " placement policy pp2 partition by hash(a) partitions 3",
            alter_sql: "alter table t partition by range (a) (partition p0 values less than (88), partition p1 values less than (222), partition p2 values less than (288), partition pMax values less than (maxvalue))",
            before_table_pp: "pp2",
            before_partitions_pp: NONE,
            after_table_pp: "pp2",
            after_partitions_pp: NONE,
            error: "",
        },
        TestCase {
            schema_pp: " placement policy pp1",
            create_table_extra: " partition by hash(a) (partition p0, partition p1 placement policy pp3, partition p2)",
            alter_sql: "alter table t partition by range (a) (partition p0 values less than (88), partition p1 values less than (222), partition p2 values less than (288) placement policy pp3, partition pMax values less than (maxvalue))",
            before_table_pp: "pp1",
            before_partitions_pp: P0_P3_P0,
            after_table_pp: "pp1",
            after_partitions_pp: &["", "", "pp3", ""],
            error: "",
        },
        TestCase {
            schema_pp: " placement policy pp1",
            create_table_extra: " placement policy pp2 partition by hash(a) (partition p0, partition p1 placement policy pp3, partition p2)",
            alter_sql: "alter table t partition by range (a) (partition p0 values less than (88), partition p1 values less than (222), partition p2 values less than (288), partition pMax values less than (maxvalue))",
            before_table_pp: "pp2",
            before_partitions_pp: P0_P3_P0,
            after_table_pp: "pp2",
            after_partitions_pp: NONE,
            error: "",
        },
        TestCase {
            schema_pp: " placement policy pp2",
            create_table_extra: " placement policy pp3 partition by hash(a) (partition p0, partition p1 placement policy pp1, partition p2 placement policy pp2)",
            alter_sql: "alter table t placement policy pp1 partition by range(a) (partition p0 values less than (100) placement policy pp1, partition p1 values less than (200) placement policy pp2, partition p2 values less than (maxvalue))",
            before_table_pp: "pp3",
            before_partitions_pp: P0_P1_P2,
            after_table_pp: "",
            after_partitions_pp: NONE,
            error: "[ddl:8200]Unsupported multi schema change for alter table placement",
        },
        TestCase {
            schema_pp: " placement policy pp2",
            create_table_extra: " placement policy pp3 partition by hash(a) (partition p0, partition p1 placement policy pp1, partition p2 placement policy pp2)",
            alter_sql: "alter table t partition by range(a) (partition p0 values less than (100) placement policy pp1, partition p1 values less than (200) placement policy pp2, partition p2 values less than (maxvalue))",
            before_table_pp: "pp3",
            before_partitions_pp: P0_P1_P2,
            after_table_pp: "pp3",
            after_partitions_pp: P1_P2_P0,
            error: "",
        },
        TestCase {
            schema_pp: " placement policy pp2",
            create_table_extra: " placement policy pp3 partition by hash(a) (partition p0, partition p1 placement policy pp1, partition p2 placement policy pp2)",
            alter_sql: "alter table t placement policy pp1",
            before_table_pp: "pp3",
            before_partitions_pp: P0_P1_P2,
            after_table_pp: "pp1",
            after_partitions_pp: P0_P1_P2,
            error: "",
        },
        TestCase {
            schema_pp: " placement policy pp2",
            create_table_extra: " placement policy pp3 partition by hash(a) (partition p0, partition p1 placement policy pp1, partition p2 placement policy pp2)",
            alter_sql: "alter table t placement policy pp1 remove partitioning",
            before_table_pp: "pp3",
            before_partitions_pp: P0_P1_P2,
            after_table_pp: "",
            after_partitions_pp: NONE,
            error: "[ddl:8200]Unsupported multi schema change for alter table placement",
        },
        TestCase {
            schema_pp: " placement policy pp2",
            create_table_extra: " placement policy pp3 partition by hash(a) (partition p0, partition p1 placement policy pp1, partition p2 placement policy pp2)",
            alter_sql: "alter table t remove partitioning",
            before_table_pp: "pp3",
            before_partitions_pp: P0_P1_P2,
            after_table_pp: "pp3",
            after_partitions_pp: NONE,
            error: "",
        },
    ];

    let store = CreateAnalyzeStatsStore();
    let mut owner = TestKit::new(store.clone());
    owner.MustExec("create placement policy pp1 followers=1", Vec::new());
    owner.MustExec("create placement policy pp2 followers=2", Vec::new());
    owner.MustExec("create placement policy pp3 followers=3", Vec::new());
    let database = "PartitionWithPlacement";
    for (index, case) in cases.iter().enumerate() {
        owner.MustExec(&format!("drop schema if exists {database}"), Vec::new());
        owner.MustExec(
            &format!("create schema {database}{}", case.schema_pp),
            Vec::new(),
        );
        owner.MustExec(&format!("use {database}"), Vec::new());
        owner.MustExec(
            &format!(
                "create table t (a int not null auto_increment primary key, b varchar(255)){}",
                case.create_table_extra
            ),
            Vec::new(),
        );
        owner.MustExec("insert into t (b) values ('a'),('b'),('c')", Vec::new());

        let mut before_writer = TestKit::new(store.clone());
        before_writer.MustExec(&format!("use {database}"), Vec::new());
        before_writer.MustExec("insert into t (b) values ('before')", Vec::new());
        let (table_policy, partition_policies) = placement_names(&before_writer, database);
        assert_eq!(
            table_policy, case.before_table_pp,
            "case {index} before table"
        );
        if !case.before_partitions_pp.is_empty() {
            assert_eq!(
                partition_policies, case.before_partitions_pp,
                "case {index} before partitions"
            );
        }

        if !case.error.is_empty() {
            owner.MustContainErrMsg(case.alter_sql, case.error);
            continue;
        }
        owner.MustExec(case.alter_sql, Vec::new());
        let mut after_writer = TestKit::new(store.clone());
        after_writer.MustExec(&format!("use {database}"), Vec::new());
        after_writer.MustExec("insert into t (b) values ('after')", Vec::new());
        let (table_policy, partition_policies) = placement_names(&after_writer, database);
        assert_eq!(
            table_policy, case.after_table_pp,
            "case {index} after table"
        );
        if case.after_partitions_pp.is_empty() {
            assert!(
                partition_policies.iter().all(String::is_empty),
                "case {index} after partitions: {partition_policies:?}"
            );
        } else {
            assert_eq!(
                partition_policies, case.after_partitions_pp,
                "case {index} after partitions"
            );
        }
    }
}

/// 验证 `ClearPlacement` 清空表与分区上的策略引用，且不改动已 Clone 的原副本。
#[test]
fn partition_placement_is_cleared_without_mutating_the_original_clone() {
    let mut table = TableInfo {
        PlacementPolicyRef: Some(policy(1, "pp1")),
        Partition: Some(PartitionInfo {
            Definitions: vec![
                PartitionDefinition {
                    ID: 11,
                    Name: NewCIStr("p0"),
                    PlacementPolicyRef: Some(policy(2, "pp2")),
                    ..Default::default()
                },
                PartitionDefinition {
                    ID: 12,
                    Name: NewCIStr("p1"),
                    PlacementPolicyRef: None,
                    ..Default::default()
                },
            ],
            ..Default::default()
        }),
        ..Default::default()
    };
    // 先 Clone 再 Clear，确认深拷贝隔离。
    let original = table.Clone();
    table.ClearPlacement();
    assert!(table.PlacementPolicyRef.is_none());
    assert!(
        table
            .Partition
            .as_ref()
            .unwrap()
            .Definitions
            .iter()
            .all(|definition| definition.PlacementPolicyRef.is_none())
    );
    assert_eq!(original.PlacementPolicyRef.unwrap().Name.O, "pp1");
    assert_eq!(
        original.Partition.unwrap().Definitions[0]
            .PlacementPolicyRef
            .as_ref()
            .unwrap()
            .Name
            .O,
        "pp2"
    );
}

/// 验证 `PolicyRefInfo` clone 后改名不影响原对象，ID 保持稳定。
#[test]
fn placement_policy_reference_clone_keeps_stable_id_and_owned_name() {
    let original = policy(42, "primary");
    let mut clone = original.clone();
    clone.Name = NewCIStr("replica");
    assert_eq!(original.ID, 42);
    assert_eq!(original.Name.O, "primary");
    assert_eq!(clone.ID, 42);
    assert_eq!(clone.Name.O, "replica");
}
