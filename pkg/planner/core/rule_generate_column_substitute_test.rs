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

// 生成列表达式替换规则的单元测试。
//
// 覆盖：在 Selection 条件下用生产路径树遍历完成替换；映射表大小写不敏感且
// 热路径重复调用保持稳定。

use crate::rule_generate_column_substitute::{
    ExprColumnMap, GcSubstituter, SubstituteExpression, collectGenerateColumn,
};
use crate::task::{Expression, PlanKind, PlanNode};

/// 验证 GcSubstituter 能沿计划树收集虚拟列，并把匹配条件改写为列引用。
#[test]
fn generated_column_substitution_uses_production_tree_walk() {
    // 数据源上定义虚拟生成列 `a + 1` → col_7。
    let generated = Expression {
        name: "a + 1".to_owned(),
        column: Some(7),
        virtual_column: true,
        ..Default::default()
    };
    let mut root = PlanNode::new(PlanKind::Selection);
    root.conditions = vec![
        Expression {
            name: "A + 1".to_owned(),
            ..Default::default()
        },
        Expression {
            name: "b + 1".to_owned(),
            ..Default::default()
        },
    ];
    let mut source = PlanNode::new(PlanKind::Other("DataSource".to_owned()));
    source.expressions.push(generated);
    root.children.push(source);

    let (optimized, changed) = GcSubstituter.Optimize(root);
    assert!(!changed);
    // 大小写不同但语义相同的 `A + 1` 应被替换；`b + 1` 保持不变。
    assert_eq!(optimized.conditions[0].column, Some(7));
    assert_eq!(optimized.conditions[0].name, "col_7");
    assert_eq!(optimized.conditions[1].column, None);
}

/// 验证映射键统一小写，且 SubstituteExpression 热路径可重复调用。
#[test]
fn substitution_map_is_case_insensitive_and_stable_on_hot_path() {
    let mut plan = PlanNode::new(PlanKind::Other("DataSource".to_owned()));
    plan.expressions.push(Expression {
        name: "JSON_EXTRACT(doc, '$.a')".to_owned(),
        column: Some(11),
        virtual_column: true,
        ..Default::default()
    });
    let mut map = ExprColumnMap::new();
    collectGenerateColumn(&plan, &mut map);
    assert_eq!(map["json_extract(doc, '$.a')"], 11);

    // 重复替换同一表达式，结果应始终指向 col_11。
    for _ in 0..100 {
        let mut expression = Expression {
            name: "json_extract(doc, '$.a')".to_owned(),
            ..Default::default()
        };
        assert!(SubstituteExpression(&mut expression, &map));
        assert_eq!(expression.column, Some(11));
    }
}

/// 与 Go 的 LogicalOptRule 契约一致：规则名固定，Optimize 的布尔返回值不报告原地替换。
#[test]
fn optimizer_metadata_matches_go_contract() {
    let generated = Expression {
        name: "a + 1".to_owned(),
        column: Some(7),
        virtual_column: true,
        ..Default::default()
    };
    let mut root = PlanNode::new(PlanKind::Selection);
    root.conditions.push(Expression {
        name: "a + 1".to_owned(),
        ..Default::default()
    });
    let mut source = PlanNode::new(PlanKind::Other("DataSource".to_owned()));
    source.expressions.push(generated);
    root.children.push(source);

    let rule = GcSubstituter;
    let (optimized, plan_changed) = rule.Optimize(root);
    assert_eq!(rule.Name(), "generate_column_substitute");
    assert!(!plan_changed);
    assert_eq!(optimized.conditions[0].column, Some(7));
}

/// Go 仅从 DataSource 收集生成列，并把 CTE 当作独立优化边界。
#[test]
fn generated_columns_are_collected_only_from_data_sources_outside_ctes() {
    let generated = |name: &str, column| Expression {
        name: name.to_owned(),
        column: Some(column),
        virtual_column: true,
        ..Default::default()
    };

    let mut projection = PlanNode::new(PlanKind::Projection);
    projection.expressions.push(generated("projection", 1));
    let mut cte = PlanNode::new(PlanKind::Cte);
    let mut hidden_source = PlanNode::new(PlanKind::Other("DataSource".to_owned()));
    hidden_source.expressions.push(generated("cte", 2));
    cte.children.push(hidden_source);
    let mut source = PlanNode::new(PlanKind::Other("DataSource".to_owned()));
    source.expressions.push(generated("visible", 3));
    projection.children.extend([cte, source]);

    let mut map = ExprColumnMap::new();
    collectGenerateColumn(&projection, &mut map);
    assert_eq!(map.len(), 1);
    assert_eq!(map["visible"], 3);
}

/// Go 的 LogicalAggregation 同时替换聚合函数参数和 GROUP BY 表达式。
#[test]
fn aggregation_expressions_are_substituted() {
    let mut aggregation = PlanNode::new(PlanKind::HashAgg);
    aggregation.agg_funcs.push(Expression {
        name: "a + 1".to_owned(),
        ..Default::default()
    });
    let mut source = PlanNode::new(PlanKind::Other("DataSource".to_owned()));
    source.expressions.push(Expression {
        name: "a + 1".to_owned(),
        column: Some(4),
        virtual_column: true,
        ..Default::default()
    });
    aggregation.children.push(source);

    let (optimized, plan_changed) = GcSubstituter.Optimize(aggregation);
    assert!(!plan_changed);
    assert_eq!(optimized.agg_funcs[0].column, Some(4));
}
