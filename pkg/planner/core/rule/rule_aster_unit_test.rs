// Copyright 2026 AsterSQL.

//! 逻辑优化规则的边界回归测试。
//!
//! 本模块用最小逻辑计划验证列裁剪、统计收集、键推导与连接改写等规则的
//! 保守性：规则既要完成合法转换，也必须在前置条件不足时保持原计划不变。

use std::collections::BTreeMap;

use super::collect_column_stats_usage::collect_column_stats_usage;
use super::rule_build_key_info::BuildKeySolver;
use super::rule_collect_plan_stats::CollectPredicateColumnsPoint;
use super::rule_column_pruning::ColumnPruner;
use super::rule_init::{Expr, FieldType, JoinType, LogicalRule, Plan, PlanKind};
use super::rule_join_key_type_cast::JoinKeyTypeCast;
use super::rule_max_min_eliminate::MaxMinEliminator;
use super::rule_order_aware_join_reorder::OrderAwareJoinReorder;
use super::rule_outer_join_to_semi_join::OuterJoinToSemiJoin;

/// 构造带固定有符号整数类型的列引用，便于测试只突出规则相关差异。
fn column(id: i64) -> Expr {
    Expr::Column {
        id,
        field_type: FieldType::SignedInt,
    }
}

/// 构造不含索引、分区与谓词的最小数据源计划。
fn source(schema: &[i64]) -> Plan {
    Plan {
        kind: PlanKind::DataSource {
            table_id: 1,
            indexes: BTreeMap::new(),
            partition: None,
            selected_partitions: None,
        },
        schema: schema.to_vec(),
        children: Vec::new(),
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 10.0,
        used_stats: BTreeMap::new(),
    }
}

/// 投影输出被裁剪后，表达式选择仍须使用裁剪前的原始输出位置。
#[test]
fn column_pruning_uses_original_projection_offsets() {
    let projection = Plan {
        kind: PlanKind::Projection {
            expressions: vec![column(10), column(20)],
        },
        schema: vec![100, 200],
        children: vec![source(&[10, 20])],
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 10.0,
        used_stats: BTreeMap::new(),
    };

    let plan = Plan {
        kind: PlanKind::Selection,
        schema: vec![200],
        children: vec![projection],
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 10.0,
        used_stats: BTreeMap::new(),
    };
    let (optimized, _) = ColumnPruner.optimize(plan).unwrap();

    assert_eq!(optimized.schema, vec![200]);
    assert_eq!(optimized.children[0].schema, vec![200]);
    assert_eq!(optimized.children[0].children[0].schema, vec![20]);
}

/// 连接等值条件中的列只要求元信息统计，不应被误标为完整直方图需求。
#[test]
fn join_predicate_columns_only_need_meta_stats() {
    let plan = Plan {
        kind: PlanKind::Join {
            join_type: JoinType::Inner,
            equal_conditions: vec![Expr::Scalar {
                function: "eq".into(),
                args: vec![column(10), column(20)],
                field_type: FieldType::Bool,
            }],
            other_conditions: Vec::new(),
        },
        schema: vec![10, 20],
        children: vec![source(&[10]), source(&[20])],
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 10.0,
        used_stats: BTreeMap::new(),
    };

    let usage = collect_column_stats_usage(&plan, false);
    assert_eq!(
        usage.predicate_columns,
        BTreeMap::from([(10, false), (20, false)])
    );
}

/// 多数据源计划收集谓词列时，统计需求必须归属各自的表，不能跨表混合。
#[test]
fn collected_stats_are_scoped_to_each_data_source() {
    let mut left = source(&[1]);
    left.kind = PlanKind::DataSource {
        table_id: 10,
        indexes: BTreeMap::new(),
        partition: None,
        selected_partitions: None,
    };
    left.predicates = vec![column(1)];
    let mut right = source(&[2]);
    right.kind = PlanKind::DataSource {
        table_id: 20,
        indexes: BTreeMap::new(),
        partition: None,
        selected_partitions: None,
    };
    right.predicates = vec![column(2)];
    let plan = Plan {
        kind: PlanKind::Join {
            join_type: JoinType::Inner,
            equal_conditions: Vec::new(),
            other_conditions: Vec::new(),
        },
        schema: vec![1, 2],
        children: vec![left, right],
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 1.0,
        used_stats: BTreeMap::new(),
    };

    let (optimized, _) = (CollectPredicateColumnsPoint {
        collect_index_pruning_columns: false,
    })
    .optimize(plan)
    .unwrap();
    assert_eq!(
        optimized.used_stats[&10].columns,
        std::collections::BTreeSet::from([1])
    );
    assert_eq!(
        optimized.used_stats[&20].columns,
        std::collections::BTreeSet::from([2])
    );
}

/// 投影 schema 已先行裁剪时，键推导应安全放弃无法映射的键，而不是越界访问。
#[test]
fn projection_with_pruned_schema_does_not_panic_while_building_keys() {
    let mut child = source(&[1]);
    child.keys = vec![vec![1]];
    let plan = Plan {
        kind: PlanKind::Projection {
            expressions: vec![column(1), column(2)],
        },
        schema: vec![10],
        children: vec![child],
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 1.0,
        used_stats: BTreeMap::new(),
    };
    let (optimized, changed) = BuildKeySolver.optimize(plan).unwrap();
    assert!(!changed);
    assert!(optimized.keys.is_empty());
}

/// 无分组的标量 MAX/MIN 消除不应以数据源已有索引为前提。
#[test]
fn scalar_max_min_does_not_require_an_index() {
    let aggregate = Plan {
        kind: PlanKind::Aggregation {
            aggregates: vec![super::rule_init::AggregateExpr {
                kind: super::rule_init::AggKind::Max,
                args: vec![column(1)],
                distinct: false,
            }],
            group_by: Vec::new(),
        },
        schema: vec![1],
        children: vec![source(&[1])],
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 1.0,
        used_stats: BTreeMap::new(),
    };

    let (optimized, changed) = MaxMinEliminator.optimize(aggregate).unwrap();
    assert!(!changed, "Go's MaxMinEliminator never sets planChanged");
    assert!(matches!(
        optimized.children[0].kind,
        PlanKind::Limit { count: 1 }
    ));
}

/// 没有顺序要求时，顺序感知连接重排不得仅因基数差异交换左右输入。
#[test]
fn join_reorder_is_noop_without_an_order_requirement() {
    let left = Plan {
        estimated_rows: 100.0,
        ..source(&[1])
    };
    let right = Plan {
        estimated_rows: 1.0,
        ..source(&[2])
    };
    let plan = Plan {
        kind: PlanKind::Join {
            join_type: JoinType::Inner,
            equal_conditions: Vec::new(),
            other_conditions: Vec::new(),
        },
        schema: vec![1, 2],
        children: vec![left, right],
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 1.0,
        used_stats: BTreeMap::new(),
    };

    let (optimized, changed) = OrderAwareJoinReorder.optimize(plan.clone()).unwrap();
    assert!(!changed);
    assert_eq!(optimized.children[0].schema, plan.children[0].schema);
}

/// 缺少连接条件时，外连接转半连接规则必须保持原计划，避免改变笛卡尔语义。
#[test]
fn outer_join_to_semi_join_requires_a_join_condition() {
    let inner = source(&[2]);
    let join = Plan {
        kind: PlanKind::Join {
            join_type: JoinType::LeftOuter,
            equal_conditions: Vec::new(),
            other_conditions: Vec::new(),
        },
        schema: vec![1, 2],
        children: vec![source(&[1]), inner],
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 10.0,
        used_stats: BTreeMap::new(),
    };
    let plan = Plan {
        kind: PlanKind::Selection,
        schema: vec![1],
        children: vec![join],
        predicates: vec![Expr::Scalar {
            function: "is_null".into(),
            args: vec![column(2)],
            field_type: FieldType::Bool,
        }],
        keys: Vec::new(),
        estimated_rows: 1.0,
        used_stats: BTreeMap::new(),
    };

    let (optimized, changed) = OuterJoinToSemiJoin.optimize(plan.clone()).unwrap();
    assert!(!changed);
    assert_eq!(optimized, plan);
}

/// 只有内表列参与有效等值连接时，针对该列的空值筛选才可转换为反半连接。
#[test]
fn outer_join_is_converted_only_for_a_null_rejected_inner_key() {
    let join = Plan {
        kind: PlanKind::Join {
            join_type: JoinType::LeftOuter,
            equal_conditions: vec![Expr::Scalar {
                function: "eq".into(),
                args: vec![column(1), column(2)],
                field_type: FieldType::Bool,
            }],
            other_conditions: Vec::new(),
        },
        schema: vec![1, 2],
        children: vec![source(&[1]), source(&[2])],
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 10.0,
        used_stats: BTreeMap::new(),
    };
    let plan = Plan {
        kind: PlanKind::Selection,
        schema: vec![1],
        children: vec![join],
        predicates: vec![Expr::Scalar {
            function: "is_null".into(),
            args: vec![column(2)],
            field_type: FieldType::Bool,
        }],
        keys: Vec::new(),
        estimated_rows: 1.0,
        used_stats: BTreeMap::new(),
    };
    let (optimized, changed) = OuterJoinToSemiJoin.optimize(plan).unwrap();
    assert!(changed);
    assert!(matches!(
        optimized.children[0].kind,
        PlanKind::Join {
            join_type: JoinType::AntiSemi,
            ..
        }
    ));
}

/// 空值安全等号具有独立语义，连接键类型转换不得按普通等号路径重写它。
#[test]
fn null_safe_join_equality_is_not_cast_rewritten() {
    let plan = Plan {
        kind: PlanKind::Join {
            join_type: JoinType::Inner,
            equal_conditions: vec![Expr::Scalar {
                function: "null_eq".into(),
                args: vec![
                    column(1),
                    Expr::Column {
                        id: 2,
                        field_type: FieldType::UnsignedInt,
                    },
                ],
                field_type: FieldType::Bool,
            }],
            other_conditions: Vec::new(),
        },
        schema: vec![1, 2],
        children: vec![source(&[1]), source(&[2])],
        predicates: Vec::new(),
        keys: Vec::new(),
        estimated_rows: 1.0,
        used_stats: BTreeMap::new(),
    };
    let (optimized, changed) = JoinKeyTypeCast.optimize(plan.clone()).unwrap();
    assert!(!changed);
    assert_eq!(optimized, plan);
}

/// 同步统计仅加载部分请求项时必须报告降级原因，防止不完整统计进入计划缓存。
#[test]
fn request_load_stats_reports_partial_load_as_fallback() {
    struct PartialLoader;
    impl super::rule_collect_plan_stats::StatsLoader for PartialLoader {
        fn load(
            &self,
            items: &[(i64, i64)],
            _wait_ms: u64,
        ) -> Result<std::collections::BTreeSet<(i64, i64)>, String> {
            Ok(items.first().copied().into_iter().collect())
        }
    }

    let error =
        super::rule_collect_plan_stats::request_load_stats(&PartialLoader, &[(1, 1), (1, 2)], 10)
            .unwrap_err();
    assert_eq!(
        error,
        super::rule_collect_plan_stats::SKIP_PLAN_CACHE_REASON_SYNC_LOAD_FALLBACK
    );
}
