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

// 连接重排（join reorder）共用工具：表达式、计划节点与 Leading 树构造。
//
// 提供连接类型、简化表达式 AST、Join 算法 hint、计划节点骨架，以及
// LEADING hint 树构建、列替换、外连接侧过滤器多叶检测、等值边对齐与
// 新 Join 节点 hint 合并等辅助函数。对应 Go 侧 joinorder 包中的 util。

use std::collections::{BTreeMap, BTreeSet};

/// 简化连接类型枚举，覆盖内连接、外连接与半连接族。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JoinType {
    Inner,
    LeftOuter,
    RightOuter,
    FullOuter,
    Semi,
    AntiSemi,
}

/// 连接重排用的表达式树：列、常量、等值、合取与其它谓词。
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    /// 引用某叶表上的列（UniqueID + 叶节点 ID）。
    Column {
        unique_id: i64,
        leaf_id: usize,
    },
    /// 常量；`deterministic` 为假表示含非确定性副作用。
    Constant {
        deterministic: bool,
    },
    Eq(Box<Expr>, Box<Expr>),
    And(Vec<Expr>),
    /// 未进一步展开的谓词，仅携带涉及列集合与确定性标记。
    Other {
        columns: BTreeSet<i64>,
        deterministic: bool,
    },
}

impl Expr {
    /// 收集表达式中出现的全部列 UniqueID。
    pub fn column_ids(&self) -> BTreeSet<i64> {
        match self {
            Expr::Column { unique_id, .. } => BTreeSet::from([*unique_id]),
            Expr::Constant { .. } => BTreeSet::new(),
            Expr::Eq(left, right) => {
                let mut ids = left.column_ids();
                ids.extend(right.column_ids());
                ids
            }
            Expr::And(items) => items.iter().flat_map(Expr::column_ids).collect(),
            Expr::Other { columns, .. } => columns.clone(),
        }
    }

    /// 收集表达式引用到的叶节点 ID（用于判断条件是否跨多叶）。
    pub fn leaf_ids(&self) -> BTreeSet<usize> {
        match self {
            Expr::Column { leaf_id, .. } => BTreeSet::from([*leaf_id]),
            Expr::Constant { .. } => BTreeSet::new(),
            Expr::Eq(left, right) => {
                let mut ids = left.leaf_ids();
                ids.extend(right.leaf_ids());
                ids
            }
            Expr::And(items) => items.iter().flat_map(Expr::leaf_ids).collect(),
            Expr::Other { .. } => BTreeSet::new(),
        }
    }

    /// 表达式是否确定性（列视为确定；常量/其它看标记；复合要求子树皆确定）。
    pub fn deterministic(&self) -> bool {
        match self {
            Expr::Constant { deterministic } | Expr::Other { deterministic, .. } => *deterministic,
            Expr::Column { .. } => true,
            Expr::Eq(left, right) => left.deterministic() && right.deterministic(),
            Expr::And(items) => items.iter().all(Expr::deterministic),
        }
    }
}

/// 单个顶点上的连接算法偏好（Hash / Merge / Index / Broadcast）。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JoinMethodHint {
    pub prefer_hash: bool,
    pub prefer_merge: bool,
    pub prefer_index: bool,
    pub prefer_broadcast: bool,
}

/// 计划节点种类：表扫描叶，或带条件与 hint 的 Join。
#[derive(Clone, Debug, PartialEq)]
pub enum PlanKind {
    Table {
        database: String,
        table: String,
        /// 可用索引，每条为列 UniqueID 前缀序列。
        indexes: Vec<Vec<i64>>,
    },
    Join {
        join_type: JoinType,
        equal_conditions: Vec<Expr>,
        other_conditions: Vec<Expr>,
        hint: JoinMethodHint,
    },
}

/// 连接重排用的逻辑计划节点：标识、种类、孩子、列集与代价估计。
#[derive(Clone, Debug, PartialEq)]
pub struct PlanNode {
    pub id: usize,
    pub kind: PlanKind,
    pub children: Vec<PlanNode>,
    pub columns: BTreeSet<i64>,
    pub estimated_rows: f64,
    pub cumulative_cost: f64,
}

impl PlanNode {
    /// 构造表扫描叶节点；行数与代价至少为 1.0，避免后续除零。
    pub fn leaf(
        id: usize,
        database: impl Into<String>,
        table: impl Into<String>,
        columns: BTreeSet<i64>,
        rows: f64,
        indexes: Vec<Vec<i64>>,
    ) -> Self {
        Self {
            id,
            kind: PlanKind::Table {
                database: database.into(),
                table: table.into(),
                indexes,
            },
            children: Vec::new(),
            columns,
            estimated_rows: rows.max(1.0),
            cumulative_cost: rows.max(1.0),
        }
    }

    /// 返回子树中全部叶顶点 ID 集合。
    pub fn vertexes(&self) -> BTreeSet<usize> {
        if self.children.is_empty() {
            BTreeSet::from([self.id])
        } else {
            self.children.iter().flat_map(PlanNode::vertexes).collect()
        }
    }
}

/// LEADING hint 的嵌套树：叶子为表名，内部节点为强制左右结合。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LeadingTree {
    Table(String),
    Join(Box<LeadingTree>, Box<LeadingTree>),
}

/// 按 Leading 树从左到右消费可用组：查表移除后与右侧递归结果做 join。
pub fn build_leading_tree_from_list<T: Clone>(
    available: &[T],
    hint: &LeadingTree,
    finder: &impl Fn(&[T], &str) -> Option<(T, Vec<T>)>,
    joiner: &impl Fn(T, T) -> Result<T, String>,
) -> Result<(T, Vec<T>), String> {
    match hint {
        LeadingTree::Table(name) => {
            finder(available, name).ok_or_else(|| format!("leading table {name} not found"))
        }
        LeadingTree::Join(left, right) => {
            let (left, remaining) = build_leading_tree_from_list(available, left, finder, joiner)?;
            let (right, remaining) =
                build_leading_tree_from_list(&remaining, right, finder, joiner)?;
            Ok((joiner(left, right)?, remaining))
        }
    }
}

/// 按 UniqueID 映射递归替换表达式中的列引用。
pub fn substitute_columns(expr: &Expr, replacements: &BTreeMap<i64, Expr>) -> Expr {
    fn rewrite(
        expr: &Expr,
        replacements: &BTreeMap<i64, Expr>,
        visiting: &mut BTreeSet<i64>,
    ) -> Expr {
        match expr {
            Expr::Column { unique_id, .. } => {
                let Some(replacement) = replacements.get(unique_id) else {
                    return expr.clone();
                };
                // Go's copy-on-write rewriter follows a replacement recursively.  A
                // malformed cyclic map must not recurse forever, however; retaining
                // the current column is the safe fixed point for that case.
                if !visiting.insert(*unique_id) {
                    return expr.clone();
                }
                let rewritten = rewrite(replacement, replacements, visiting);
                visiting.remove(unique_id);
                rewritten
            }
            Expr::Eq(left, right) => Expr::Eq(
                Box::new(rewrite(left, replacements, visiting)),
                Box::new(rewrite(right, replacements, visiting)),
            ),
            Expr::And(items) => Expr::And(
                items
                    .iter()
                    .map(|item| rewrite(item, replacements, visiting))
                    .collect(),
            ),
            _ => expr.clone(),
        }
    }

    rewrite(expr, replacements, &mut BTreeSet::new())
}

/// 外连接外侧过滤器若引用超过一个叶，则保守视为不可安全重排。
pub fn outer_join_side_filters_touch_multiple_leaves(filters: &[Expr]) -> bool {
    filters
        .iter()
        .flat_map(Expr::leaf_ids)
        .collect::<BTreeSet<_>>()
        .len()
        > 1
}

/// 拆出等值边两侧参数及其列集合；非 `Eq` 返回 `None`。
pub fn get_eq_edge_args_and_columns(
    edge: &Expr,
) -> Option<(&Expr, &Expr, BTreeSet<i64>, BTreeSet<i64>)> {
    match edge {
        Expr::Eq(left, right) => Some((left, right, left.column_ids(), right.column_ids())),
        _ => None,
    }
}

/// 将等值边参数规范为 left_schema / right_schema 顺序；必要时交换两侧。
pub fn align_join_edge_args(
    edge: &Expr,
    left_schema: &BTreeSet<i64>,
    right_schema: &BTreeSet<i64>,
) -> Option<Expr> {
    let (left, right, left_cols, right_cols) = get_eq_edge_args_and_columns(edge)?;
    if left_cols.is_subset(left_schema) && right_cols.is_subset(right_schema) {
        Some(edge.clone())
    } else if left_cols.is_subset(right_schema) && right_cols.is_subset(left_schema) {
        Some(Expr::Eq(Box::new(right.clone()), Box::new(left.clone())))
    } else {
        None
    }
}

/// 按子树顶点 ID 合并各顶点的 Join 算法 hint 到新 Join 节点。
pub fn set_new_join_with_hint(join: &mut PlanNode, vertex_hints: &BTreeMap<usize, JoinMethodHint>) {
    // Go restores hints from the two direct children only.  Looking through all
    // descendant leaves would leak a hint from an already-built subtree onto a
    // new parent join.
    let child_ids: Vec<usize> = join.children.iter().take(2).map(|child| child.id).collect();
    if let PlanKind::Join { hint, .. } = &mut join.kind {
        for child_id in child_ids {
            if let Some(source) = vertex_hints.get(&child_id) {
                hint.prefer_hash |= source.prefer_hash;
                hint.prefer_merge |= source.prefer_merge;
                hint.prefer_index |= source.prefer_index;
                hint.prefer_broadcast |= source.prefer_broadcast;
            }
        }
    }
}
