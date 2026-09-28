// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// Memo Group 图的字符串化工具（对应 Go `ToString`）。
//
// 以前序遍历打印每个 Group 的 Schema / UniqueKey，以及等价表达式行；
// 子 Group 编号按首次出现顺序分配，便于与 golden 输出对齐。

use std::collections::{HashMap, HashSet};

use astersql_expression as expression;
use astersql_planner_memo::{GroupExpr, GroupRef};

/// 将 memo Group 图按前序遍历字符串化；子 Group 编号按首次出现顺序分配，
/// 行为与 Go 实现一致。
/// Stringifies a memo group graph in preorder, assigning child group numbers
/// in first-seen order exactly as the Go implementation does.
#[allow(non_snake_case)]
pub fn ToString(ctx: &dyn expression::exprctx::EvalContext, group: &GroupRef) -> Vec<String> {
    let root_id = group.borrow().ID();
    let mut id_map = HashMap::from([(root_id, 0)]);
    let mut visited = HashSet::new();
    let mut lines = Vec::new();
    toString(ctx, group, &mut id_map, &mut visited, &mut lines);
    lines
}

#[allow(non_snake_case)]
/// 递归遍历：先登记子 Group 编号，再输出本 Group，再深入未访问子 Group。
fn toString(
    ctx: &dyn expression::exprctx::EvalContext,
    group: &GroupRef,
    id_map: &mut HashMap<u64, usize>,
    visited: &mut HashSet<u64>,
    lines: &mut Vec<String>,
) {
    let group_id = group.borrow().ID();
    if !visited.insert(group_id) {
        return;
    }

    // 先为所有子 Group 分配稳定编号，再输出本 Group，保证先序编号与 Go 一致。
    let equivalents = group.borrow().Equivalents.clone();
    for expression in &equivalents {
        let children = expression.borrow().Children.clone();
        for child in children {
            let child_id = child.borrow().ID();
            if !id_map.contains_key(&child_id) {
                id_map.insert(child_id, id_map.len());
            }
        }
    }

    lines.extend(groupToString(ctx, group, id_map));
    for expression in equivalents {
        let children = expression.borrow().Children.clone();
        for child in children {
            toString(ctx, &child, id_map, visited, lines);
        }
    }
}

#[allow(non_snake_case)]
/// 渲染单个 Group：`Group#n Schema:[...]` 行及其等价表达式缩进行。
fn groupToString(
    ctx: &dyn expression::exprctx::EvalContext,
    group: &GroupRef,
    id_map: &HashMap<u64, usize>,
) -> Vec<String> {
    let group = group.borrow();
    let schema = group
        .Prop
        .Schema
        .as_deref()
        .expect("a memo group must have a logical schema");
    let columns = schema
        .Columns
        .iter()
        .map(|column| column.StringWithCtx(ctx, expression::errors::RedactLogDisable))
        .collect::<Vec<_>>();
    let mut group_line = format!(
        "Group#{} Schema:[{}]",
        id_map[&group.ID()],
        columns.join(",")
    );

    // UniqueKey 对应 schema 上的主键/唯一键列集合，调试输出中一并展示。
    if !schema.PKOrUK.is_empty() {
        let keys = schema
            .PKOrUK
            .iter()
            .map(|key| {
                key.iter()
                    .map(|column| column.StringWithCtx(ctx, expression::errors::RedactLogDisable))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .collect::<Vec<_>>();
        group_line.push_str(&format!(", UniqueKey:[{}]", keys.join(",")));
    }

    let mut result = Vec::with_capacity(group.Equivalents.len() + 1);
    result.push(group_line);
    result.extend(
        group
            .Equivalents
            .iter()
            .map(|expression| format!("    {}", groupExprToString(&expression.borrow(), id_map))),
    );
    result
}

#[allow(non_snake_case)]
/// 渲染一条 GroupExpr：`TP_ID`、子 Group 输入列表与 ExplainInfo。
fn groupExprToString(expression: &GroupExpr, id_map: &HashMap<u64, usize>) -> String {
    let node = expression.ExprNode.as_ref();
    // Go starts with ExprNode.ExplainID().String(), so the session option that
    // suppresses plan-ID suffixes must be observed here as well.
    let mut text = if node
        .SCtx()
        .is_some_and(|ctx| ctx.ignore_explain_id_suffix())
    {
        node.TP().to_owned()
    } else {
        format!("{}_{}", node.TP(), node.ID())
    };
    if expression.Children.is_empty() {
        text.push_str(&format!(" {}", node.ExplainInfo()));
    } else {
        text.push_str(&format!(" {}", getChildrenGroupID(expression, id_map)));
        let explain_info = node.ExplainInfo();
        if !explain_info.is_empty() {
            text.push_str(&format!(", {explain_info}"));
        }
    }
    text
}

#[allow(non_snake_case)]
/// 把子 Group 映射成 `input:[Group#a,Group#b]` 形式。
fn getChildrenGroupID(expression: &GroupExpr, id_map: &HashMap<u64, usize>) -> String {
    let children = expression
        .Children
        .iter()
        .map(|child| format!("Group#{}", id_map[&child.borrow().ID()]))
        .collect::<Vec<_>>()
        .join(",");
    format!("input:[{children}]")
}
