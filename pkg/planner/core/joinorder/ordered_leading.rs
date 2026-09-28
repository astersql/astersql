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

// 有序 Leading（Ordered Leading）连接顺序偏好推断。
//
// 当 `ORDER BY` 列可被某张叶表上的索引前缀覆盖时，本模块在连接组中选出
// 该承载表（carrier），并尝试把内部 Leading 偏好标注到当前连接组的锚点 Join 上，
// 促使后续连接重排优先采用 Merge Join 等保序算法。仅检查计划 Schema、索引
// 元数据与等值谓词，不访问存储、不扫描索引、不执行查询。

// 这里只检查计划 Schema、索引元数据和表达式，不会访问数据库、扫描索引或执行查询。
//
// use std::collections::HashSet;
//
// OrderedLeadingChoice 记录完整承载排序列的顶点，以及可桥接为单表 hint 的身份。
// pub struct OrderedLeadingChoice {
//     pub CarrierVertex: base::LogicalPlan,
//     pub LeadingTable: Option<hint::HintedTable>,
//     pub Vertices: Vec<base::LogicalPlan>,
// }
//
// FindOrderedLeadingChoice 只处理至少包含两个顶点的连接组。
// pub fn FindOrderedLeadingChoice(root: base::LogicalPlan, orderingCols: &[expression::Column]) -> Option<OrderedLeadingChoice> {
//     if root.is_nil() || orderingCols.is_empty() { return None; }
//     let group = extractJoinGroup(root);
//     if group.vertexes.len() <= 1 { return None; }
//     findOrderedLeadingChoice(&group, orderingCols)
// }
//
// TryAnnotateOrderedLeading 在不存在用户 hint、连接算法 hint 或旧内部 hint 时写入合成偏好。
// pub fn TryAnnotateOrderedLeading(root: base::LogicalPlan, choice: Option<&OrderedLeadingChoice>) -> bool {
//     let Some(choice) = choice else { return false; };
//     if root.is_nil() { return false; }
//     let group = extractJoinGroup(root);
//     let Some(anchor) = findLeadingHintAnchor(group.root) else { return false; };
//     if !group.leadingHints.is_empty() || anchor.PreferJoinOrder || anchor.InternalPreferJoinOrder
//         || anchor.PreferJoinType > 0 || anchor.HintInfo.is_some() || anchor.InternalHintInfo.is_some()
//         || choice.LeadingTable.is_none()
//     { return false; }
// 内部偏好与用户 LEADING 分开存储，避免后续错误地向用户发出 hint 警告。
//     anchor.InternalPreferJoinOrder = true;
//     anchor.InternalHintInfo = buildSingleTableLeadingHint(choice.LeadingTable.as_ref());
//     true
// }
//
// findLeadingHintAnchor 穿透连接组根部可能保留的 Selection，返回最上层 Join。
// pub fn findLeadingHintAnchor(mut root: base::LogicalPlan) -> Option<logicalop::LogicalJoin> {
//     loop {
//         if let Some(join) = root.as_logical_join() { return Some(join.clone()); }
//         if let Some(selection) = root.as_logical_selection() {
//             let child = selection.Children().first()?;
//             root = child.clone();
//             continue;
//         }
//         return None;
//     }
// }
//
// buildSingleTableLeadingHint 将一个 HintedTable 同时写入旧平面字段和新嵌套 LeadingList。
// pub fn buildSingleTableLeadingHint(table: Option<&hint::HintedTable>) -> Option<hint::PlanHints> {
//     let table = table?;
//     let qbName = if table.SelectOffset > 0 {
//         hint::generate_qb_name(hint::TYPE_SELECT, table.SelectOffset).unwrap_or_default()
//     } else { ast::CIStr::default() };
//     Some(hint::PlanHints {
//         LeadingJoinOrder: vec![table.clone()],
//         LeadingList: Some(ast::LeadingList { Items: vec![ast::HintTable {
//             DBName: table.DBName.clone(), TableName: table.TblName.clone(), QBName: qbName,
//         }.into()] }),
//         ..Default::default()
//     })
// }
//
// findOrderedLeadingChoice 要求整个排序向量属于同一个顶点，不能逐列拆到不同表。
// pub fn findOrderedLeadingChoice(group: &joinGroup, orderingCols: &[expression::Column]) -> Option<OrderedLeadingChoice> {
//     let (_, uniqueIDs) = normalizeOrderingColumns(orderingCols);
//     if uniqueIDs.len() != orderingCols.len() || uniqueIDs.is_empty() { return None; }
//     for vertex in &group.vertexes {
//         if schemaContainsAllOrderingColumns(vertex, &uniqueIDs) {
//             return Some(OrderedLeadingChoice {
//                 CarrierVertex: vertex.clone(),
//                 LeadingTable: planner_util::extract_table_alias(vertex, vertex.QueryBlockOffset()),
//                 Vertices: group.vertexes.clone(),
//             });
//         }
//     }
//     None
// }
//
// normalizeOrderingColumns 同时提取物理列 ID 和计划内 UniqueID，并拒绝无效列。
// pub fn normalizeOrderingColumns(orderingCols: &[expression::Column]) -> (Vec<i64>, HashSet<i64>) {
//     let mut ids = Vec::with_capacity(orderingCols.len());
//     let mut unique = HashSet::with_capacity(orderingCols.len());
//     for col in orderingCols {
//         if col.ID <= 0 || col.UniqueID <= 0 { return (Vec::new(), HashSet::new()); }
//         ids.push(col.ID);
//         unique.insert(col.UniqueID);
//     }
//     (ids, unique)
// }
//
// pub fn schemaContainsAllOrderingColumns(plan: &base::LogicalPlan, orderingUniqueIDs: &HashSet<i64>) -> bool {
//     if orderingUniqueIDs.is_empty() { return false; }
//     let Some(schema) = plan.Schema() else { return false; };
//     if schema.Columns.len() < orderingUniqueIDs.len() { return false; }
//     schema.Columns.iter().filter(|col| orderingUniqueIDs.contains(&col.UniqueID)).count() == orderingUniqueIDs.len()
// }
//
// DsSatisfiesOrdering 证明 DataSource 自身 Schema 包含排序列且存在匹配的公共可见索引。
// pub fn DsSatisfiesOrdering(ds: Option<&logicalop::DataSource>, orderingCols: &[expression::Column], parentFilters: &[expression::Expression]) -> bool {
//     let Some(ds) = ds else { return false; };
//     let (ids, unique) = normalizeOrderingColumns(orderingCols);
//     !ids.is_empty() && ids.len() == orderingCols.len()
//         && schemaContainsAllOrderingColumns(&ds.as_plan(), &unique)
//         && tableHasIndexMatchingOrdering(ds, &ids, &[], parentFilters)
// }
//
// tableHasIndexMatchingOrdering 汇总等值固定列后，逐个检查公共且可见的索引。
// pub fn tableHasIndexMatchingOrdering(ds: &logicalop::DataSource, orderingColIDs: &[i64], groupSelectionConds: &[expression::Expression], parentFilters: &[expression::Expression]) -> bool {
//     let equality = collectEqualityPredicateColumnIDs(&ds.as_plan(), groupSelectionConds, parentFilters);
//     ds.TableInfo.Indices.iter().any(|index|
//         index.State == model::STATE_PUBLIC && !index.Invisible
//             && indexMatchesOrdering(index, ds, orderingColIDs, &equality))
// }
//
// indexMatchesOrdering 允许在匹配第一个 ORDER BY 列前跳过被等值谓词固定的索引前缀。
// pub fn indexMatchesOrdering(index: &model::IndexInfo, ds: &logicalop::DataSource, orderingColIDs: &[i64], equalityColIDs: &HashSet<i64>) -> bool {
//     if orderingColIDs.is_empty() { return false; }
//     let mut orderPos = 0;
//     for idxCol in &index.Columns {
//         if idxCol.Offset >= ds.TableInfo.Columns.len() { return false; }
//         let colID = ds.TableInfo.Columns[idxCol.Offset].ID;
//         if colID == orderingColIDs[orderPos] {
//             orderPos += 1;
//             if orderPos == orderingColIDs.len() { return true; }
//         } else if orderPos == 0 && equalityColIDs.contains(&colID) {
//             continue;
//         } else {
// 一旦开始消费排序列，任何中间不匹配都会破坏所需的全局顺序。
//             return false;
//         }
//     }
//     orderPos == orderingColIDs.len()
// }
//
// collectEqualityPredicateColumnIDs 合并计划局部条件、组内 Selection 和祖先过滤器中的单表等值列。
// pub fn collectEqualityPredicateColumnIDs(plan: &base::LogicalPlan, groupSelectionConds: &[expression::Expression], parentFilters: &[expression::Expression]) -> HashSet<i64> {
//     let mut result = HashSet::new();
//     collectPlanLocalEqualityPredicateColumnIDs(plan, &mut result);
//     if let Some(schema) = plan.Schema() {
//         addEqualityColumnsFromLocalConds(&mut result, schema, groupSelectionConds);
//         addEqualityColumnsFromLocalConds(&mut result, schema, parentFilters);
//     }
//     result
// }
//
// collectPlanLocalEqualityPredicateColumnIDs 读取 Selection.Conditions 与 DataSource.AllConds 后继续递归子计划。
// pub fn collectPlanLocalEqualityPredicateColumnIDs(plan: &base::LogicalPlan, result: &mut HashSet<i64>) {
//     if let Some(selection) = plan.as_logical_selection() {
//         for cond in &selection.Conditions { extractEqualityColumns(cond, result); }
//     } else if let Some(ds) = plan.as_data_source() {
//         for cond in &ds.AllConds { extractEqualityColumns(cond, result); }
//     }
//     for child in plan.Children() { collectPlanLocalEqualityPredicateColumnIDs(child, result); }
// }
//
// pub fn addEqualityColumnsFromLocalConds(result: &mut HashSet<i64>, schema: &expression::Schema, conds: &[expression::Expression]) {
//     for cond in conds {
//         if condBelongsToSchema(cond, schema) { extractEqualityColumns(cond, result); }
//     }
// }
//
// condBelongsToSchema 要求条件至少引用一列，且所有列都属于当前顶点。
// pub fn condBelongsToSchema(cond: &expression::Expression, schema: &expression::Schema) -> bool {
//     let cols = expression::extract_columns(cond);
//     !cols.is_empty() && cols.iter().all(|col| schema.Contains(col))
// }
//
// extractEqualityColumns 只接受 AND 下的列=确定性常量，或只有一个值的 IN。
// pub fn extractEqualityColumns(expr: &expression::Expression, result: &mut HashSet<i64>) {
//     let Some(function) = expr.as_scalar_function() else { return; };
//     match function.FuncName.L.as_str() {
//         ast::LOGIC_AND => {
//             for arg in function.GetArgs() { extractEqualityColumns(arg, result); }
//         }
//         ast::EQ if function.GetArgs().len() == 2 => {
//             let args = function.GetArgs();
//             if let Some(col) = args[0].as_column() {
//                 if col.ID > 0 && isDeterministicConstExpr(&args[1]) { result.insert(col.ID); }
//             }
//             if let Some(col) = args[1].as_column() {
//                 if col.ID > 0 && isDeterministicConstExpr(&args[0]) { result.insert(col.ID); }
//             }
//         }
//         ast::IN if function.GetArgs().len() == 2 => {
//             let args = function.GetArgs();
//             if let Some(col) = args[0].as_column() {
// 多值 IN 会扫描多个点范围，不能保证后续索引列的全局顺序。
//                 if col.ID > 0 && isDeterministicConstExpr(&args[1]) { result.insert(col.ID); }
//             }
//         }
//         _ => {}
//     }
// }
//
// pub fn isDeterministicConstExpr(expr: &expression::Expression) -> bool {
//     expression::extract_columns(expr).is_empty() && !expression::is_mutable_effects_expr(expr)
// }
// */
use crate::util::{Expr, PlanKind, PlanNode};
use std::collections::BTreeSet;

/// 有序 Leading 选择结果：承载排序列的叶表、规范化后的排序列集合，以及是否反向扫描。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrderedLeadingChoice {
    /// 承载排序列的叶节点（表扫描）计划 ID。
    pub leaf_id: usize,
    /// 去重后仍保持原顺序的排序列 UniqueID 列表。
    pub ordering_columns: Vec<i64>,
    /// 是否需要反向索引扫描以满足降序；当前实现固定为 `false`。
    pub reverse: bool,
}

/// 在连接计划中寻找能用索引前缀覆盖 `ordering_columns` 的叶表。
///
/// 会先对排序列去重，再根据 Join 等值条件扩展等价类，最后逐叶检查索引是否匹配。
pub fn find_ordered_leading_choice(
    root: &PlanNode,
    ordering_columns: &[i64],
) -> Option<OrderedLeadingChoice> {
    // Go 版本要求连接组至少有两个顶点，并拒绝无效或重复的排序列。
    if root.vertexes().len() <= 1 {
        return None;
    }
    let mut normalized = Vec::with_capacity(ordering_columns.len());
    let mut seen = BTreeSet::new();
    for column in ordering_columns {
        if *column <= 0 || !seen.insert(*column) {
            return None;
        }
        normalized.push(*column);
    }
    if normalized.is_empty() {
        return None;
    }
    // 每个排序列先自成等价类，再由等值谓词合并可互换的列 ID。
    let mut equivalence = normalized
        .iter()
        .copied()
        .map(|column| BTreeSet::from([column]))
        .collect::<Vec<_>>();
    collect_equalities(root, &mut equivalence);
    let mut leaves = Vec::new();
    collect_leaves(root, &mut leaves);
    let mut fixed_prefix_columns = BTreeSet::new();
    collect_fixed_columns(root, &mut fixed_prefix_columns);
    for leaf in leaves {
        let PlanKind::Table { indexes, .. } = &leaf.kind else {
            continue;
        };
        for index in indexes {
            if index_matches_ordering(index, &equivalence, &fixed_prefix_columns) {
                return Some(OrderedLeadingChoice {
                    leaf_id: leaf.id,
                    ordering_columns: normalized,
                    reverse: false,
                });
            }
        }
    }
    None
}

/// 若选择的叶表仍在当前连接组内且不存在冲突 hint，则标注锚点 Join。
pub fn try_annotate_ordered_leading(root: &mut PlanNode, choice: &OrderedLeadingChoice) -> bool {
    if !root.vertexes().contains(&choice.leaf_id) {
        return false;
    }
    let PlanKind::Join { hint, .. } = &mut root.kind else {
        return false;
    };
    if hint.prefer_hash || hint.prefer_merge || hint.prefer_index || hint.prefer_broadcast {
        return false;
    }
    hint.prefer_merge = true;
    true
}

/// 收集连接树中的全部叶节点（无孩子的表扫描）。
fn collect_leaves<'a>(node: &'a PlanNode, output: &mut Vec<&'a PlanNode>) {
    if node.children.is_empty() {
        output.push(node);
    } else {
        for child in &node.children {
            collect_leaves(child, output);
        }
    }
}

/// 遍历 Join 的等值与其它条件，向等价类中并入可互换列。
fn collect_equalities(node: &PlanNode, equivalence: &mut [BTreeSet<i64>]) {
    if let PlanKind::Join {
        equal_conditions,
        other_conditions,
        ..
    } = &node.kind
    {
        for condition in equal_conditions.iter().chain(other_conditions) {
            collect_equality(condition, equivalence);
        }
    }
    for child in &node.children {
        collect_equalities(child, equivalence);
    }
}

/// Collect columns fixed to one deterministic value by a local equality
/// predicate.  Such columns may be skipped before the first ORDER BY column in
/// a composite index, matching the Go index-prefix rule.
fn collect_fixed_columns(node: &PlanNode, fixed: &mut BTreeSet<i64>) {
    if let PlanKind::Join {
        equal_conditions,
        other_conditions,
        ..
    } = &node.kind
    {
        for condition in equal_conditions.iter().chain(other_conditions) {
            collect_fixed_columns_from_expr(condition, fixed);
        }
    }
    for child in &node.children {
        collect_fixed_columns(child, fixed);
    }
}

fn collect_fixed_columns_from_expr(expr: &Expr, fixed: &mut BTreeSet<i64>) {
    match expr {
        Expr::Eq(left, right) => {
            if left.column_ids().len() == 1
                && right.column_ids().is_empty()
                && right.deterministic()
            {
                fixed.extend(left.column_ids());
            } else if right.column_ids().len() == 1
                && left.column_ids().is_empty()
                && left.deterministic()
            {
                fixed.extend(right.column_ids());
            }
        }
        Expr::And(items) => {
            for item in items {
                collect_fixed_columns_from_expr(item, fixed);
            }
        }
        _ => {}
    }
}

/// 解析确定性等值（及 AND 合取），把两侧列并入同一等价类。
fn collect_equality(expr: &Expr, equivalence: &mut [BTreeSet<i64>]) {
    match expr {
        Expr::Eq(left, right) if left.deterministic() && right.deterministic() => {
            let left = left.column_ids();
            let right = right.column_ids();
            if left.len() == 1 && right.len() == 1 {
                let left = *left.first().unwrap();
                let right = *right.first().unwrap();
                for class in equivalence.iter_mut() {
                    if class.contains(&left) || class.contains(&right) {
                        class.insert(left);
                        class.insert(right);
                    }
                }
            }
        }
        Expr::And(items) => {
            for item in items {
                collect_equality(item, equivalence);
            }
        }
        _ => {}
    }
}

/// 索引前缀长度不少于排序列数，且每一前缀列落入对应排序列的等价类。
fn index_matches_ordering(
    index: &[i64],
    equivalence: &[BTreeSet<i64>],
    fixed_prefix_columns: &BTreeSet<i64>,
) -> bool {
    if index.len() < equivalence.len() {
        return false;
    }
    let mut order_pos = 0;
    for column in index {
        if equivalence
            .get(order_pos)
            .is_some_and(|equivalent| equivalent.contains(column))
        {
            order_pos += 1;
            if order_pos == equivalence.len() {
                return true;
            }
        } else if order_pos == 0 && fixed_prefix_columns.contains(column) {
            continue;
        } else {
            return false;
        }
    }
    order_pos == equivalence.len()
}
