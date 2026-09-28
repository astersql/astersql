// Copyright 2026 AsterSQL.

// 子查询 Join / Apply 与聚合映射键的 Aster 单元测试。
//
// 覆盖 `AggregateMapperKey` 跨 Rust visitor 克隆保持输出映射，以及
// `buildApplyWithJoinType` 保留双孩子与合并 Schema。

use crate::ast;
use crate::expression_rewriter::AggregateMapperKey;
use crate::logical_plan_builder::{
    FlagBuildKeyInfo, FlagConstantPropagation, FlagDecorrelate, FlagEliminateOuterJoin,
    FlagPredicatePushDown,
};
use crate::task::{FieldType, JoinType, PlanKind, PlanNode, TypeCode};

/// 构造值为 `value` 的 COUNT 聚合表达式节点。
fn count_node(value: i64) -> ast::ExprNode {
    ast::ExprNode {
        node_text: Default::default(),
        Kind: ast::ExprKind::AggregateFunction {
            Name: ast::AggFuncCount.to_owned(),
            Args: vec![ast::NewValueExpr(value, "", "")],
            Distinct: false,
            Order: Vec::new(),
        },
        OriginTextPosition: 0,
        Flag: Default::default(),
    }
}

/// Rust visitor 会克隆 AST；聚合映射仍须找到克隆前已建立的输出列。
#[test]
fn aggregate_mapper_key_survives_rust_ast_cloning() {
    let aggregate = count_node(1);
    let cloned = aggregate.clone();
    assert!(!std::ptr::eq(&aggregate, &cloned));
    assert_eq!(
        AggregateMapperKey(&aggregate),
        AggregateMapperKey(&aggregate)
    );
    assert_eq!(AggregateMapperKey(&aggregate), AggregateMapperKey(&cloned));
    assert_ne!(
        AggregateMapperKey(&aggregate),
        AggregateMapperKey(&count_node(2))
    );
}

/// 结构键区分限定名、带点的引用名及 DISTINCT，同时按 Go 规则忽略列名大小写。
#[test]
fn aggregate_mapper_distinguishes_qualified_and_quoted_column_names() {
    let aggregate = |schema: &str, table: &str, name: &str, distinct: bool| {
        let mut node = count_node(1);
        if let ast::ExprKind::AggregateFunction { Args, Distinct, .. } = &mut node.Kind {
            *Distinct = distinct;
            *Args = vec![ast::ExprNode::Column(ast::ColumnName {
                Schema: ast::NewCIStr(schema),
                Table: ast::NewCIStr(table),
                Name: ast::NewCIStr(name),
            })];
        }
        node
    };
    let qualified = aggregate("db", "t", "a", false);
    assert_eq!(
        AggregateMapperKey(&qualified),
        AggregateMapperKey(&aggregate("DB", "T", "A", false))
    );
    assert_ne!(
        AggregateMapperKey(&qualified),
        AggregateMapperKey(&aggregate("db", "t", "a", true))
    );
    assert_ne!(
        AggregateMapperKey(&aggregate("", "t", "a.b", false)),
        AggregateMapperKey(&aggregate("", "t.a", "b", false))
    );
}

/// Apply 构建保留外/内孩子、合并 Schema、连接类型，且默认不启用缓存。
#[test]
fn typed_apply_preserves_both_children_and_join_schema() {
    let field_type = FieldType {
        code: TypeCode::Int,
        flen: 20,
        decimal: 0,
        unsigned: true,
    };
    let mut outer = PlanNode::new(PlanKind::Projection);
    outer.schema = vec![field_type.clone()];
    let mut inner = PlanNode::new(PlanKind::HashAgg);
    inner.schema = vec![field_type];

    let mut builder = crate::planbuilder::NewPlanBuilder(&[]);
    let apply = builder.buildApplyWithJoinType(outer, inner, JoinType::LeftOuter, true);
    assert!(matches!(apply.kind, PlanKind::Apply));
    assert_eq!(apply.children.len(), 2);
    assert_eq!(apply.schema.len(), 2);
    assert!(apply.schema[0].unsigned);
    assert!(!apply.schema[1].unsigned);
    assert_eq!(apply.join_type, JoinType::LeftOuter);
    assert!(!apply.flags.use_cache);
    assert_eq!(
        builder.optFlag
            & (FlagPredicatePushDown
                | FlagBuildKeyInfo
                | FlagDecorrelate
                | FlagConstantPropagation
                | FlagEliminateOuterJoin),
        FlagPredicatePushDown
            | FlagBuildKeyInfo
            | FlagDecorrelate
            | FlagConstantPropagation
            | FlagEliminateOuterJoin
    );
}
