// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// Grouping-set, ROLLUP, deduplication and grouping-id algorithms from
// `grouping_sets.go`.
//
// GROUPING SETS / ROLLUP 相关的表达式侧算法：合并可前缀嵌套的分组布局、
// 为聚合参数挑选目标布局、判定是否需克隆列、计算 distinct grouping id、
// 以及根据布局调整 Schema 可空性与 GROUP BY 表达式去重/还原。

use crate::*;

use std::collections::{BTreeMap, BTreeSet, HashMap};

/// 一组 GROUPING SETS：外层为多个互斥/并列的分组布局。
#[derive(Clone, Default)]
pub struct GroupingSets(pub Vec<GroupingSet>);

/// 单个 grouping set：内部按前缀关系排列的多组分组表达式。
#[derive(Clone, Default)]
pub struct GroupingSet(pub Vec<GroupingExprs>);

/// 同一布局内的一组分组列表达式（通常为 Column）。
#[derive(Default)]
pub struct GroupingExprs(pub Vec<ExprBox>);

impl Clone for GroupingExprs {
    fn clone(&self) -> Self {
        Self(
            self.0
                .iter()
                .map(|expression| expression.CloneExpr())
                .collect(),
        )
    }
}

/// 分组列 UniqueID 集合（有序，便于集合运算与稳定序列化）。
pub type GroupingIds = BTreeSet<i64>;
/// 列 UniqueID → 使用该列的 grouping id 集合。
pub type IdToGids = BTreeMap<i64, BTreeSet<u64>>;

/// 将普通 GROUP BY 列列表提升为“每列一个独立 grouping set”的初始形态。
pub fn new_grouping_sets(group_columns: Vec<ExprBox>) -> GroupingSets {
    GroupingSets(
        group_columns
            .into_iter()
            .map(|column| GroupingSet(vec![GroupingExprs(vec![column])]))
            .collect(),
    )
}

impl GroupingSets {
    /// 将多个 grouping set 按前缀子集关系合并为尽量少的布局链。
    pub fn merge(&self) -> GroupingSets {
        let mut merged = GroupingSets::default();
        for grouping_set in &self.0 {
            for grouping_exprs in &grouping_set.0 {
                if merged.0.is_empty() {
                    merged.0.push(GroupingSet(vec![grouping_exprs.clone()]));
                } else {
                    merged = merged.merge_one(grouping_exprs.clone());
                }
            }
        }
        merged
    }

    /// 把一组分组表达式插入到已有布局中：能构成前缀则并入，否则新建 set。
    pub fn merge_one(mut self, target: GroupingExprs) -> GroupingSets {
        for grouping_set in &mut self.0 {
            for index in (0..grouping_set.0.len()).rev() {
                let current = &grouping_set.0[index];
                // target 是 current 的子集：插到链头或继续向上找插入点。
                if target.subset_of(current) {
                    if index == 0 {
                        grouping_set.0.insert(0, target);
                        return self;
                    }
                    continue;
                }
                // 走到链尾：若 current 是 target 子集则追加，否则跳出新建 set。
                if index == grouping_set.0.len() - 1 {
                    if current.subset_of(&target) {
                        grouping_set.0.push(target);
                        return self;
                    }
                    break;
                }
                grouping_set.0.insert(index + 1, target);
                return self;
            }
        }
        self.0.push(GroupingSet(vec![target]));
        self
    }

    /// 为普通聚合参数挑选一个不会把其引用列填 NULL 的布局下标；找不到返回 -1。
    pub fn target_one(&self, normal_aggregate_args: &[ExprBox]) -> isize {
        let mut normal_ids = BTreeSet::new();
        for argument in normal_aggregate_args {
            normal_ids.extend(
                crate::util_kernel::ExtractColumns(argument.as_ref())
                    .into_iter()
                    .map(|column| column.UniqueID),
            );
        }
        if normal_ids.is_empty() {
            return 0;
        }
        let all_ids = self.all_sets_col_ids();
        for (index, grouping_set) in self.0.iter().enumerate() {
            let ids = grouping_set.all_col_ids();
            // 当前布局相对全集会填 NULL 的列集合。
            let null_filled: BTreeSet<_> = all_ids.difference(&ids).copied().collect();
            if null_filled.is_disjoint(&normal_ids) {
                return index as isize;
            }
        }
        -1
    }

    /// 若任意两个 layout 的列集合相交，则需要克隆列以避免共享存储冲突。
    pub fn need_clone_column(&self) -> bool {
        let sets: Vec<_> = self.0.iter().map(GroupingSet::all_col_ids).collect();
        for (index, left) in sets.iter().enumerate() {
            if sets[index + 1..]
                .iter()
                .any(|right| !left.is_disjoint(right))
            {
                return true;
            }
        }
        false
    }

    /// 是否为空（无 set，或所有 set 均为空）。
    pub fn is_empty(&self) -> bool {
        self.0.is_empty() || self.0.iter().all(GroupingSet::is_empty)
    }

    /// 收集全部 layout 中出现过的列 UniqueID。
    pub fn all_sets_col_ids(&self) -> GroupingIds {
        self.0.iter().flat_map(|set| set.all_col_ids()).collect()
    }

    /// 调试用字符串表示，形如 `[{...},{...}]`。
    pub fn string_with_ctx(&self) -> String {
        format!(
            "[{}]",
            self.0
                .iter()
                .map(GroupingSet::string_with_ctx)
                .collect::<Vec<_>>()
                .join(",")
        )
    }

    /// 编码为 tipb::GroupingSet 列表，供下推到 TiKV/TiFlash。
    pub fn to_pb(
        &self,
        ctx: &dyn EvalContext,
        client: &dyn kv::Client,
    ) -> Result<Vec<tipb::GroupingSet>, Error> {
        self.0
            .iter()
            .map(|grouping_set| grouping_set.to_pb(ctx, client))
            .collect()
    }

    /// 使用默认阈值 64 计算 distinct layout 规模及可选 grouping id 映射。
    pub fn distinct_size(&self) -> (usize, Option<Vec<u64>>, Option<IdToGids>) {
        self.distinct_size_with_threshold(64)
    }

    /// 去重 layout：规模不超过阈值时只返回 distinct 数；超过则分配 grouping id。
    ///
    /// 返回 `(distinct_count, Option<每 layout 的 gid>, Option<列→gid 集合>)`。
    pub fn distinct_size_with_threshold(
        &self,
        threshold: usize,
    ) -> (usize, Option<Vec<u64>>, Option<IdToGids>) {
        let mut distinct_offsets = Vec::new();
        let mut original_sets: Vec<GroupingIds> = Vec::with_capacity(self.0.len());
        for grouping_set in &self.0 {
            let current = grouping_set.all_col_ids();
            // 仅记录首次出现的 distinct layout 下标。
            if !distinct_offsets
                .iter()
                .any(|offset| original_sets[*offset] == current)
            {
                distinct_offsets.push(original_sets.len());
            }
            original_sets.push(current);
        }
        if distinct_offsets.len() <= threshold {
            return (distinct_offsets.len(), None, None);
        }

        // 超过阈值：为每个原始 layout 赋 gid，并构建列到 gid 集合的反查表。
        let mut gids = vec![0; original_sets.len()];
        for (gid, offset) in distinct_offsets.iter().copied().enumerate() {
            for (index, original) in original_sets.iter().enumerate() {
                if *original == original_sets[offset] {
                    gids[index] = gid as u64;
                }
            }
        }
        let mut id_to_gids = BTreeMap::new();
        for column_id in self.all_sets_col_ids() {
            let mut used_by = BTreeSet::new();
            for (index, original) in original_sets.iter().enumerate() {
                if original.contains(&column_id) {
                    used_by.insert(gids[index]);
                }
            }
            id_to_gids.insert(column_id, used_by);
        }
        (distinct_offsets.len(), Some(gids), Some(id_to_gids))
    }
}

impl GroupingSet {
    /// 是否为空（无表达式组，或各组均为空）。
    pub fn is_empty(&self) -> bool {
        self.0.is_empty() || self.0.iter().all(GroupingExprs::is_empty)
    }

    /// 本 layout 内全部列 UniqueID 的并集。
    pub fn all_col_ids(&self) -> GroupingIds {
        self.0.iter().flat_map(GroupingExprs::id_set).collect()
    }

    /// 提取所有可向下转型为 Column 的分组表达式引用。
    pub fn extract_cols(&self) -> Vec<&Column> {
        self.0
            .iter()
            .flat_map(|expressions| expressions.0.iter())
            .map(|expression| {
                expression
                    .as_any()
                    .downcast_ref::<Column>()
                    .expect("grouping expression must be a column")
            })
            .collect()
    }

    /// 深拷贝本 grouping set。
    pub fn clone_set(&self) -> GroupingSet {
        self.clone()
    }

    /// 调试用字符串，形如 `{<a,b>,<c>}`。
    pub fn string_with_ctx(&self) -> String {
        format!(
            "{{{}}}",
            self.0
                .iter()
                .map(GroupingExprs::string_with_ctx)
                .collect::<Vec<_>>()
                .join(",")
        )
    }

    /// 估算本结构占用的近似内存字节数。
    pub fn memory_usage(&self) -> i64 {
        size::SizeOfSlice
            + self.0.capacity() as i64 * size::SizeOfPointer
            + self.0.iter().map(GroupingExprs::memory_usage).sum::<i64>()
    }

    /// 编码为单个 tipb::GroupingSet（各组表达式转 PB 列表）。
    pub fn to_pb(
        &self,
        ctx: &dyn EvalContext,
        client: &dyn kv::Client,
    ) -> Result<tipb::GroupingSet, Error> {
        let mut encoded = tipb::GroupingSet::new();
        let mut grouping_expressions = Vec::with_capacity(self.0.len());
        for expressions in &self.0 {
            let mut grouping_expression = tipb::GroupingExpr::new();
            grouping_expression.set_grouping_expr(
                crate::expr_to_pb_kernel::ExpressionsToPBList(ctx, &expressions.0, client)?.into(),
            );
            grouping_expressions.push(grouping_expression);
        }
        encoded.set_grouping_exprs(grouping_expressions.into());
        Ok(encoded)
    }
}

impl GroupingExprs {
    /// 是否不含任何分组表达式。
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// 判断本列集合是否为 `other` 的子集（按 UniqueID）。
    pub fn subset_of(&self, other: &GroupingExprs) -> bool {
        self.id_set().is_subset(&other.id_set())
    }

    /// 收集本组全部列 UniqueID；非 Column 表达式会 panic（与 Go 断言一致）。
    pub fn id_set(&self) -> GroupingIds {
        self.0
            .iter()
            .map(|expression| {
                expression
                    .as_any()
                    .downcast_ref::<Column>()
                    .expect("grouping expression must be a column")
                    .UniqueID
            })
            .collect()
    }

    /// 调试用字符串，形如 `<col1,col2>`。
    pub fn string_with_ctx(&self) -> String {
        format!(
            "<{}>",
            self.0
                .iter()
                .map(|expression| expression.StringWithCtx(None, errors::RedactLogDisable))
                .collect::<Vec<_>>()
                .join(",")
        )
    }

    /// 估算本组表达式占用的近似内存。
    pub fn memory_usage(&self) -> i64 {
        size::SizeOfSlice
            + self.0.capacity() as i64 * size::SizeOfInterface
            + self
                .0
                .iter()
                .map(|expression| expression.MemoryUsage())
                .sum::<i64>()
    }
}

/// 将 ROLLUP(a,b,...) 展开为 `(), (a), (a,b), ...` 的 GROUPING SETS。
pub fn rollup_grouping_sets(rollup_expressions: &[ExprBox]) -> GroupingSets {
    GroupingSets(
        (0..=rollup_expressions.len())
            .map(|length| {
                GroupingSet(vec![GroupingExprs(
                    rollup_expressions[..length]
                        .iter()
                        .map(|expression| expression.CloneExpr())
                        .collect(),
                )])
            })
            .collect(),
    )
}

/// 若某分组列在至少一个 layout 中缺失，则清除 Schema 上的 NotNull 标志。
pub fn adjust_nullability_from_grouping_sets(grouping_sets: &GroupingSets, schema: &mut Schema) {
    let grouping_ids = grouping_sets.all_sets_col_ids();
    let per_set_ids: Vec<_> = grouping_sets
        .0
        .iter()
        .map(GroupingSet::all_col_ids)
        .collect();
    for column in &mut schema.Columns {
        if grouping_ids.contains(&column.UniqueID)
            && per_set_ids
                .iter()
                .any(|ids| !ids.contains(&column.UniqueID))
        {
            if let Some(ret_type) = column.RetType.as_mut() {
                ret_type.SetFlag(ret_type.GetFlag() & !mysql::NotNullFlag);
            }
        }
    }
}

/// 按 CanonicalHashCode 对 GROUP BY 表达式去重，返回去重列表与原下标映射。
pub fn deduplicate_gby_expression(expressions: &[ExprBox]) -> (Vec<ExprBox>, Vec<usize>) {
    let mut positions: HashMap<Vec<u8>, usize> = HashMap::with_capacity(expressions.len());
    let mut distinct = Vec::new();
    let mut original_positions = Vec::with_capacity(expressions.len());
    for expression in expressions {
        let key = expression.CanonicalHashCode();
        let position = match positions.get(&key) {
            Some(position) => *position,
            None => {
                let position = distinct.len();
                distinct.push(expression.CloneExpr());
                positions.insert(key, position);
                position
            }
        };
        original_positions.push(position);
    }
    (distinct, original_positions)
}

/// 按去重阶段记录的下标，从去重后的 Column 列表还原原始 GROUP BY 表达式序列。
pub fn restore_gby_expression(expressions: &[Column], indexes: &[usize]) -> Vec<ExprBox> {
    indexes
        .iter()
        .map(|index| Box::new(expressions[*index].CloneColumn()) as ExprBox)
        .collect()
}
