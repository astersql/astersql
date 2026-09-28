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

//! 跨查询块常量传播规则。
//!
//! 与 Go 实现一致，Join 从允许的一侧上拉 Selection 的列/常量比较谓词，
//! 在 Join 上方生成 Selection，供后续 predicate push down 继续传播到另一侧。
use crate::rule_init::{Expr, JoinType, LogicalRule, Plan, PlanKind};
use std::collections::BTreeMap;

/// 常量传播规则：收集等值常量并在表达式树中替换对应列引用。
pub struct ConstantPropagationSolver;
impl LogicalRule for ConstantPropagationSolver {
    fn name(&self) -> &'static str {
        "constant_propagation"
    }
    fn optimize(&self, plan: Plan) -> Result<(Plan, bool), String> {
        // Go intentionally reports false even when a Selection is inserted: this
        // rule does not request another interaction pass from the optimizer.
        Ok((propagate(plan), false))
    }
}

/// 按 Go 规则遍历计划树；Join 从允许的一侧上拉候选谓词，并在 Join 上方建 Selection。
fn propagate(mut plan: Plan) -> Plan {
    let candidates = join_candidates(&plan);
    plan.children = plan.children.into_iter().map(propagate).collect();

    if candidates.is_empty() {
        return plan;
    }

    Plan {
        kind: PlanKind::Selection,
        schema: plan.schema.clone(),
        children: vec![plan.clone()],
        predicates: candidates,
        keys: plan.keys.clone(),
        estimated_rows: plan.estimated_rows,
        used_stats: plan.used_stats.clone(),
    }
}

fn join_candidates(plan: &Plan) -> Vec<Expr> {
    let PlanKind::Join { join_type, .. } = plan.kind else {
        return Vec::new();
    };
    match join_type {
        JoinType::Inner => plan
            .children
            .iter()
            .take(2)
            .flat_map(pull_up_constant_predicates)
            .collect(),
        JoinType::LeftOuter => plan
            .children
            .first()
            .map(pull_up_constant_predicates)
            .unwrap_or_default(),
        JoinType::RightOuter => plan
            .children
            .get(1)
            .map(pull_up_constant_predicates)
            .unwrap_or_default(),
        JoinType::Semi | JoinType::AntiSemi => Vec::new(),
    }
}

/// 对应 Go 的 PullUpConstantPredicates：Selection 提供候选，恒等 Projection 改写列名。
fn pull_up_constant_predicates(plan: &Plan) -> Vec<Expr> {
    match &plan.kind {
        PlanKind::Selection => plan
            .predicates
            .iter()
            .filter(|predicate| valid_compare_constant_predicate(predicate))
            .cloned()
            .collect(),
        PlanKind::Projection { expressions }
            if expressions.len() == plan.schema.len() && plan.children.len() == 1 =>
        {
            let replacements = expressions
                .iter()
                .zip(&plan.schema)
                .filter_map(|(expression, output)| match expression {
                    Expr::Column { id, .. } => Some((*id, *output)),
                    _ => None,
                })
                .collect::<BTreeMap<_, _>>();
            pull_up_constant_predicates(&plan.children[0])
                .into_iter()
                .filter_map(|mut predicate| {
                    let column = predicate.columns().into_iter().next()?;
                    let output = replacements.get(&column)?;
                    replace_column(&mut predicate, column, *output);
                    Some(predicate)
                })
                .collect()
        }
        _ => Vec::new(),
    }
}

fn valid_compare_constant_predicate(expression: &Expr) -> bool {
    let Expr::Scalar { function, args, .. } = expression else {
        return false;
    };
    matches!(function.as_str(), "eq" | "lt" | "le" | "gt" | "ge")
        && args.len() == 2
        && matches!(
            (&args[0], &args[1]),
            (Expr::Column { .. }, Expr::Constant(_)) | (Expr::Constant(_), Expr::Column { .. })
        )
}

fn replace_column(expression: &mut Expr, source: i64, target: i64) {
    match expression {
        Expr::Column { id, .. } => {
            if *id == source {
                *id = target;
            }
        }
        Expr::Scalar { args, .. } => {
            for argument in args {
                replace_column(argument, source, target);
            }
        }
        Expr::Cast { expr, .. } => replace_column(expr, source, target),
        Expr::Constant(_) => {}
    }
}
