// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// Physical-plan regression tests ported from `physical_plan_test.go`.
//
// The Go suite creates a mock store for every case.  The Rust planner fixture
// provides the same table metadata and drives the real parse/build/optimize
// pipeline without starting a storage service.
//
// 物理执行计划回归测试（自 `physical_plan_test.go` 移植）。
//
// Go 侧为每用例创建 mock store；Rust 侧用规划器测试夹具提供相同表元数据，
// 走真实的 parse / build / optimize 流水线，无需启动存储服务。

use expression_dependency as expression;
use physicalop_dependency::{PhysicalExchangeSender, PhysicalSort};
use planner_util_dependency::ByItems;
use property_dependency::{MPPPartitionColumn, PhysicalProperty};

use crate::main_test::{exercise_statement_for_test, optimize_query_for_test};

/// 构造带 UniqueID 与列下标的表达式列，供物理算子测试使用。
fn column(unique_id: i64, index: isize) -> expression::Column {
    let mut column = expression::Column::default();
    column.UniqueID = unique_id;
    column.Index = index;
    column
}

/// 深度优先收集物理计划树中各算子的类型名字符串。
fn collect_plan_types(plan: &dyn base_dependency::PhysicalPlan, output: &mut Vec<String>) {
    output.push(plan.tp(&[]));
    for child in plan.children() {
        collect_plan_types(child, output);
    }
}

/// 校验扫描、连接与聚合语句能产出非空物理计划，且统计与内存占用合理。
#[test]
fn builds_real_physical_plans_for_scan_join_and_aggregation() {
    let cases = [
        "select a from t where a in (1, 10, 20)",
        "select t1.a from t t1 join t t2 on t1.a = t2.a",
        "select count(*), avg(a) from t group by b",
    ];

    for sql in cases {
        let plan = optimize_query_for_test(sql).unwrap_or_else(|error| panic!("{sql}: {error}"));
        let mut types = Vec::new();
        collect_plan_types(plan.as_ref(), &mut types);
        assert!(!types.is_empty(), "{sql}");
        assert!(plan.stats_count().is_finite(), "{sql}: {types:?}");
        assert!(plan.memory_usage() > 0, "{sql}: {types:?}");
    }
}

/// 子查询与 Join Hint（如 HASH_JOIN / INL_JOIN）走生产级语句演练路径。
#[test]
fn subquery_and_join_hints_use_the_production_pipeline() {
    let cases = [
        "select * from t where a in (select a from t2)",
        "select /*+ HASH_JOIN(t1, t2) */ t1.a from t t1 join t t2 on t1.a = t2.a",
        "select /*+ INL_JOIN(t2) */ t1.a from t t1 join t t2 on t1.a = t2.a",
        "select * from (select a from t order by a limit 2) x",
    ];

    for sql in cases {
        exercise_statement_for_test(sql).unwrap_or_else(|error| panic!("{sql}: {error}"));
    }
}

/// 物理计划内存占用会随动态字段（如 Sort.ByItems、MPP 分区列）增长。
#[test]
fn physical_plan_memory_tracks_dynamic_fields() {
    let plan = optimize_query_for_test("select a from t order by a")
        .expect("obtain a production planner context");
    let context = plan.s_ctx().clone();

    let mut sort = PhysicalSort::New(context);
    let sort_base = sort.MemoryUsage();
    sort.ByItems.push(ByItems {
        Expr: Box::new(column(1, 0)),
        Desc: false,
    });
    assert!(sort.MemoryUsage() > sort_base);

    let mut property = PhysicalProperty::default();
    let property_base = property.MemoryUsage();
    property.MPPPartitionCols.push(MPPPartitionColumn {
        Col: column(1, 0),
        CollateID: 0,
    });
    assert!(property.MemoryUsage() > property_base);
}

/// ExchangeSender 对克隆出的分区列做下标解析时互不影响，源列保持未绑定。
#[test]
fn exchange_sender_resolves_cloned_partition_columns_independently() {
    let plan = optimize_query_for_test("select a from t").expect("obtain planner context");
    let context = plan.s_ctx().clone();
    let schema1 =
        expression::NewSchema(vec![column(1, 0), column(2, 1), column(3, 2), column(4, 3)]);
    let schema2 = expression::NewSchema(vec![column(3, 0), column(4, 1)]);
    let partition = MPPPartitionColumn {
        Col: column(4, -1),
        CollateID: 0,
    };
    let mut first = PhysicalExchangeSender::New(context.clone());
    first.HashCols.push(partition.Clone());
    let mut second = PhysicalExchangeSender::New(context);
    second.HashCols.push(partition.Clone());

    first
        .ResolveIndicesItselfWithSchema(&schema1)
        .expect("resolve against four-column schema");
    second
        .ResolveIndicesItselfWithSchema(&schema2)
        .expect("resolve against two-column schema");

    assert_eq!(first.HashCols[0].Col.Index, 3);
    assert_eq!(second.HashCols[0].Col.Index, 1);
    assert_eq!(partition.Col.Index, -1, "source column remains unmodified");
}

/// Schema 中缺少分区列时，ExchangeSender 解析下标应失败而非静默绑定。
#[test]
fn exchange_sender_rejects_a_missing_partition_column() {
    let plan = optimize_query_for_test("select a from t").expect("obtain planner context");
    let mut sender = PhysicalExchangeSender::New(plan.s_ctx().clone());
    sender.HashCols.push(MPPPartitionColumn {
        Col: column(99, -1),
        CollateID: 0,
    });
    let schema = expression::NewSchema(vec![column(1, 0), column(2, 1)]);

    let error = sender
        .ResolveIndicesItselfWithSchema(&schema)
        .expect_err("unknown partition column must not silently bind");
    assert!(error.to_string().contains("Column"), "{error}");
}
