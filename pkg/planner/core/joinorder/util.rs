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

// 变换内存中的计划、hint 与表达式。
//
// use std::collections::{HashMap, HashSet};
//
// JoinMethodHint 保存单个顶点的连接算法偏好及其来源 hint。
// #[derive(Clone)]
// pub struct JoinMethodHint {
//     pub PreferJoinMethod: u32,
//     pub HintInfo: Option<hint::PlanHints>,
// }
//
// impl JoinMethodHint {
//     pub fn new(method: u32, info: Option<hint::PlanHints>) -> Self {
//         Self { PreferJoinMethod: method, HintInfo: info }
//     }
// }
//
// CheckAndGenerateLeadingHint 要求一个连接组中的所有 LEADING 指针均指向同一 hint。
// pub fn CheckAndGenerateLeadingHint(hintInfo: &[hint::PlanHints]) -> (Option<hint::PlanHints>, bool) {
//     let Some(first) = hintInfo.first() else { return (None, false); };
//     let different = hintInfo.windows(2).any(|pair| !std::ptr::eq(&pair[0], &pair[1]));
//     if different { (None, true) } else { (Some(first.clone()), false) }
// }
//
// LeadingTreeFinder 对应 Go 泛型函数类型：按表 hint 找到节点并从可用列表移除。
// pub type LeadingTreeFinder<T> = fn(Vec<T>, &ast::HintTable) -> (Option<T>, Vec<T>, bool);
//
// LeadingTreeJoiner 对应 Go 泛型函数类型：校验并合并 LEADING 树的两个节点。
// pub type LeadingTreeJoiner<T> = fn(T, T) -> Result<(Option<T>, bool), errors::Error>;
//
// pub struct LeadingTreeResult<T> {
//     pub root: T,
//     pub remaining: Vec<T>,
//     pub applied: bool,
// }
//
// BuildLeadingTreeFromList 按嵌套 LeadingList 从左到右递归构树；任一步失败都回退原可用列表。
// pub fn BuildLeadingTreeFromList<T: Clone>(
//     leadingList: &ast::LeadingList,
//     availableGroups: Vec<T>,
//     findAndRemoveByHint: impl Fn(Vec<T>, &ast::HintTable) -> (Option<T>, Vec<T>, bool) + Copy,
//     checkAndJoin: impl Fn(T, T) -> Result<(Option<T>, bool), errors::Error> + Copy,
//     warn: impl Fn() + Copy,
// ) -> Result<Option<LeadingTreeResult<T>>, errors::Error> {
//     if leadingList.Items.is_empty() { return Ok(None); }
//     let original = availableGroups.clone();
//     let mut remaining = availableGroups;
//     let mut current: Option<T> = None;
//     for item in &leadingList.Items {
//         let next = if let Some(table) = item.as_hint_table() {
//             let (node, rest, ok) = findAndRemoveByHint(remaining, table);
//             if !ok { return Ok(None); }
//             remaining = rest;
//             node
//         } else if let Some(nested) = item.as_leading_list() {
//             let Some(result) = BuildLeadingTreeFromList(nested, remaining, findAndRemoveByHint, checkAndJoin, warn)? else {
//                 return Ok(None);
//             };
//             remaining = result.remaining;
//             Some(result.root)
//         } else {
//             warn();
//             return Ok(None);
//         };
//         let Some(next) = next else { return Ok(None); };
//         current = match current {
//             None => Some(next),
//             Some(left) => {
//                 let (joined, ok) = checkAndJoin(left, next)?;
//                 if !ok { return Ok(None); }
//                 joined
//             }
//         };
//     }
//     Ok(current.map(|root| LeadingTreeResult { root, remaining, applied: true }))
// }
//
// exprReplacer 保留 Go 回调的“新表达式 + 是否替换”返回形状。
// pub type exprReplacer = fn(expression::Expression) -> (expression::Expression, bool);
//
// rewriteExprTree 先序遍历表达式树，并以写时复制方式只克隆参数发生变化的 ScalarFunction。
// pub fn rewriteExprTree(
//     expr: expression::Expression,
//     replace: &impl Fn(&expression::Expression) -> (Option<expression::Expression>, bool),
// ) -> expression::Expression {
//     if expr.is_nil() { return expr; }
//     let (replacement, replaced) = replace(&expr);
//     if replaced {
//         let Some(newExpr) = replacement else { return expression::Expression::nil(); };
//         if newExpr != expr { return rewriteExprTree(newExpr, replace); }
//     }
//     let Some(function) = expr.as_scalar_function() else { return expr; };
//     let oldArgs = function.GetArgs();
//     let mut newArgs = oldArgs.clone();
//     let mut changed = false;
//     for (idx, arg) in oldArgs.iter().enumerate() {
//         let rewritten = rewriteExprTree(arg.clone(), replace);
//         changed |= rewritten != *arg;
//         newArgs[idx] = rewritten;
//     }
//     if !changed { return expr; }
//     let mut cloned = function.clone();
//     cloned.SetArgs(newArgs);
// 子参数改变后必须清除缓存哈希，否则 CanonicalHashCode 仍反映旧树。
//     cloned.CleanHashCode();
//     cloned.into()
// }
//
// SubstituteColsInEqEdges 替换等值边中的派生列；结果不是 ScalarFunction 时保留原边。
// pub fn SubstituteColsInEqEdges(edges: &[expression::ScalarFunction], colExprMap: &HashMap<i64, expression::Expression>) -> Vec<expression::ScalarFunction> {
//     edges.iter().map(|edge| {
//         SubstituteColsInExpr(edge.clone().into(), colExprMap).into_scalar_function().unwrap_or_else(|| edge.clone())
//     }).collect()
// }
//
// pub fn SubstituteColsInExprs(exprs: &[expression::Expression], colExprMap: &HashMap<i64, expression::Expression>) -> Vec<expression::Expression> {
//     exprs.iter().cloned().map(|expr| SubstituteColsInExpr(expr, colExprMap)).collect()
// }
//
// SubstituteColsInExpr 递归沿 colExprMap 链替换 Column；映射表达式在重排流程中按不可变对象复用。
// pub fn SubstituteColsInExpr(expr: expression::Expression, colExprMap: &HashMap<i64, expression::Expression>) -> expression::Expression {
//     if colExprMap.is_empty() { return expr; }
//     rewriteExprTree(expr, &|node| {
//         let Some(column) = node.as_column() else { return (Some(node.clone()), false); };
//         match colExprMap.get(&column.UniqueID) {
//             Some(definition) => (Some(definition.clone()), true),
//             None => (Some(node.clone()), false),
//         }
//     })
// }
//
// OuterJoinSideFiltersTouchMultipleLeaves 判断外侧条件是否同时依赖两个以上叶节点；是则保守禁用重排。
// pub fn OuterJoinSideFiltersTouchMultipleLeaves(
//     join: Option<&logicalop::LogicalJoin>,
//     outerGroup: &[base::LogicalPlan],
//     outerColExprMap: &HashMap<i64, expression::Expression>,
//     outerIsLeft: bool,
// ) -> bool {
//     let Some(join) = join else { return false; };
//     let mut other = join.OtherConditions.clone();
//     let mut side = if outerIsLeft { join.LeftConditions.clone() } else { join.RightConditions.clone() };
//     let mut eq = expression::scalar_funcs_to_exprs(&join.EqualConditions);
//     if !outerColExprMap.is_empty() {
//         other = SubstituteColsInExprs(&other, outerColExprMap);
//         side = SubstituteColsInExprs(&side, outerColExprMap);
//         eq = SubstituteColsInExprs(&eq, outerColExprMap);
//     }
//     let mut columns = HashMap::new();
//     expression::extract_columns_map_from_expressions(&mut columns, other.iter().chain(&side).chain(&eq));
//     let mut affected = 0;
//     for leaf in outerGroup {
//         if columns.values().any(|column| leaf.Schema().is_some_and(|schema| schema.Contains(column))) {
//             affected += 1;
//             if affected > 1 { return true; }
//         }
//     }
//     false
// }
//
// GetEqEdgeArgsAndCols 只接受恰有两个参数的等值边，并分别提取两侧引用列。
// pub fn GetEqEdgeArgsAndCols(edge: Option<&expression::ScalarFunction>) -> Option<(expression::Expression, expression::Expression, Vec<expression::Column>, Vec<expression::Column>)> {
//     let edge = edge?;
//     let args = edge.GetArgs();
//     if args.len() != 2 { return None; }
//     Some((args[0].clone(), args[1].clone(), expression::extract_columns(&args[0]), expression::extract_columns(&args[1])))
// }
//
// AlignJoinEdgeArgs 将边参数规范为 leftSchema、rightSchema 顺序，并报告是否交换。
// pub fn AlignJoinEdgeArgs(
//     lArg: expression::Expression,
//     rArg: expression::Expression,
//     leftSchema: &expression::Schema,
//     rightSchema: &expression::Schema,
// ) -> Option<(expression::Expression, expression::Expression, bool)> {
//     if expression::expr_from_schema(&lArg, leftSchema) && expression::expr_from_schema(&rArg, rightSchema) {
//         Some((lArg, rArg, false))
//     } else if expression::expr_from_schema(&lArg, rightSchema) && expression::expr_from_schema(&rArg, leftSchema) {
//         Some((rArg, lArg, true))
//     } else { None }
// }
//
// FindAndRemovePlanByAstHint 先按表名匹配，再按查询块别名匹配；别名歧义时不移除任何节点。
// pub fn FindAndRemovePlanByAstHint<T: Clone>(
//     ctx: &base::PlanContext,
//     plans: Vec<T>,
//     astTbl: &ast::HintTable,
//     getPlan: impl Fn(&T) -> base::LogicalPlan,
// ) -> (Option<T>, Vec<T>, bool) {
//     let queryBlockNames = ctx.GetSessionVars().PlannerSelectBlockAsName.Load().unwrap_or_default();
//     for (idx, group) in plans.iter().enumerate() {
//         let plan = getPlan(group);
//         if let Some(alias) = planner_util::extract_table_alias(&plan, plan.QueryBlockOffset()) {
//             let dbMatch = astTbl.DBName.L.is_empty() || astTbl.DBName.L == alias.DBName.L || astTbl.DBName.L == "*";
//             let tableMatch = astTbl.TableName.L == alias.TblName.L;
//             let qbMatch = if astTbl.QBName.L.is_empty() { true } else {
//                 let expected = extractSelectOffset(&astTbl.QBName.L);
//                 expected <= 0 || alias.SelectOffset == expected
//             };
//             if dbMatch && tableMatch && qbMatch {
//                 let mut rest = plans.clone();
//                 let matched = rest.remove(idx);
//                 return (Some(matched), rest, true);
//             }
//         }
//     }
//     let mut matchIdx = None;
//     for (idx, group) in plans.iter().enumerate() {
//         let offset = getPlan(group).QueryBlockOffset() as usize;
//         if offset > 1 && offset < queryBlockNames.len() {
//             let block = &queryBlockNames[offset];
//             let dbMatch = astTbl.DBName.L.is_empty() || astTbl.DBName.L == block.DBName.L;
//             if dbMatch && astTbl.TableName.L == block.TableName.L {
// 同一别名命中多个组时保持原列表，避免任意选择错误子树。
//                 if matchIdx.is_some() { return (None, plans, false); }
//                 matchIdx = Some(idx);
//             }
//         }
//     }
//     if let Some(idx) = matchIdx {
//         let mut rest = plans.clone();
//         let matched = rest.remove(idx);
//         return (Some(matched), rest, true);
//     }
//     (None, plans, false)
// }
//
// extractSelectOffset 从 sel_x 形式查询块名中解析正整数，失败返回 -1。
// pub fn extractSelectOffset(qbName: &str) -> i32 {
//     qbName.strip_prefix("sel_").and_then(|text| text.parse().ok()).unwrap_or(-1)
// }
//
// IsDerivedTableInLeadingHint 检查子查询别名是否显式出现在 LEADING 嵌套列表中。
// pub fn IsDerivedTableInLeadingHint(plan: &base::LogicalPlan, leadingHint: &hint::PlanHints) -> bool {
//     let Some(list) = leadingHint.LeadingList.as_ref() else { return false; };
//     let Some(names) = plan.SCtx().GetSessionVars().PlannerSelectBlockAsName.Load() else { return false; };
//     let offset = plan.QueryBlockOffset() as usize;
//     if offset <= 1 || offset >= names.len() { return false; }
//     let alias = &names[offset];
//     !alias.TableName.L.is_empty() && containsTableInLeadingList(list, &alias.DBName.L, &alias.TableName.L)
// }
//
// containsTableInLeadingList 递归匹配数据库名（允许空或 *）和表名。
// pub fn containsTableInLeadingList(list: &ast::LeadingList, dbName: &str, tableName: &str) -> bool {
//     for item in &list.Items {
//         if let Some(table) = item.as_hint_table() {
//             let dbMatch = table.DBName.L.is_empty() || table.DBName.L == dbName || table.DBName.L == "*";
//             if dbMatch && table.TableName.L == tableName { return true; }
//         } else if let Some(nested) = item.as_leading_list() {
//             if containsTableInLeadingList(nested, dbName, tableName) { return true; }
//         }
//     }
//     false
// }
//
// SetNewJoinWithHint 按左右孩子 ID 恢复连接算法偏好，最后让 LogicalJoin 计算首选类型。
// pub fn SetNewJoinWithHint(newJoin: Option<&mut logicalop::LogicalJoin>, vertexHints: &HashMap<i32, JoinMethodHint>) {
//     let Some(join) = newJoin else { return; };
//     let children = join.Children();
//     if let Some(info) = vertexHints.get(&children[0].ID()) {
//         join.LeftPreferJoinType = info.PreferJoinMethod;
//         join.HintInfo = info.HintInfo.clone();
//     }
//     if let Some(info) = vertexHints.get(&children[1].ID()) {
//         join.RightPreferJoinType = info.PreferJoinMethod;
//         join.HintInfo = info.HintInfo.clone();
//     }
//     join.SetPreferredJoinType();
// }
// */
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
