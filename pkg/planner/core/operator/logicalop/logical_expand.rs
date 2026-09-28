// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// Expand（展开）逻辑算子：实现 ROLLUP/CUBE/GROUPING SETS 的多级分组展开。
//
// 将输入行按多个 Grouping Set 复制为多级投影；未激活的分组列填 NULL，
// 并生成 GID（Grouping ID）/GPos 标记当前分组层级。

use crate::*;
use std::any::Any;
use std::collections::{BTreeMap, BTreeSet, HashSet};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 分组 ID 编码模式：位掩码或按层级递增编号。
pub enum GroupingMode {
    #[default]
    ModeBitAnd,
    ModeNumericSet,
}

#[derive(Clone, Default)]
/// 单个分组集合：参与该层聚合的列 UniqueID 集合。
pub struct GroupingSet {
    pub ColumnIDs: BTreeSet<i64>,
}

impl GroupingSet {
    /// 返回本分组集合中全部列 UniqueID。
    pub fn all_col_ids(&self) -> BTreeSet<i64> {
        self.ColumnIDs.clone()
    }
}

#[derive(Clone, Default)]
/// 一组 GroupingSet，对应 ROLLUP/CUBE 展开后的各层级。
pub struct GroupingSets(pub Vec<GroupingSet>);

/// Expand 逻辑算子：按 LevelExprs 对子树输出做多级投影展开。
pub struct LogicalExpand {
    pub LogicalSchemaProducer: LogicalSchemaProducer,
    pub DistinctGroupByCol: Vec<Column>,
    pub DistinctGbyColNames: NameSlice,
    pub DistinctGbyExprs: Vec<Expression>,
    pub DistinctSize: usize,
    pub RollupGroupingSets: GroupingSets,
    pub RollupID2GIDS: BTreeMap<i64, BTreeSet<u64>>,
    pub RollupGroupingIDs: Vec<u64>,
    pub LevelExprs: Vec<Vec<Expression>>,
    pub ExtraGroupingColNames: Vec<String>,
    pub GroupingMode: GroupingMode,
    pub GID: Option<Column>,
    pub GIDName: Option<FieldName>,
    pub GPos: Option<Column>,
    pub GPosName: Option<FieldName>,
}

/// 默认空 Expand：无分组集、无额外 GID/GPos 列。
impl Default for LogicalExpand {
    fn default() -> Self {
        Self {
            LogicalSchemaProducer: LogicalSchemaProducer::default(),
            DistinctGroupByCol: Vec::new(),
            DistinctGbyColNames: NameSlice(Vec::new()),
            DistinctGbyExprs: Vec::new(),
            DistinctSize: 0,
            RollupGroupingSets: GroupingSets::default(),
            RollupID2GIDS: BTreeMap::new(),
            RollupGroupingIDs: Vec::new(),
            LevelExprs: Vec::new(),
            ExtraGroupingColNames: Vec::new(),
            GroupingMode: GroupingMode::default(),
            GID: None,
            GIDName: None,
            GPos: None,
            GPosName: None,
        }
    }
}

impl LogicalExpand {
    /// 初始化基类逻辑计划，算子名为 Expand。
    pub fn Init(mut self, ctx: base::ContextRef, offset: i32) -> Self {
        self.LogicalSchemaProducer.BaseLogicalPlan = NewBaseLogicalPlan(ctx, "Expand", offset);
        self
    }

    /// 谓词下推屏障：Expand 会按层独立 NULL 填充，谓词不可穿越以免误删行。
    pub fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        // Expand 每层可独立将分组列填 NULL；
        // Expand can null-fill grouping columns independently at every level;
        // 谓词穿越可能删掉另一层仍需要的行。
        // pushing any predicate through it can remove a row that another level needs.
        let mut retained = if let Some(child) = self.Children_mut().first_mut() {
            PredicatePushDownPlan(child, Vec::new())?
        } else {
            Vec::new()
        };
        retained.extend(predicates);
        Ok(retained)
    }

    /// 列裁剪：保留父侧用列、Distinct 分组列以及 GID/GPos，再重生层级投影。
    pub fn PruneColumns(&mut self, parent_used_cols: &[Column]) -> Result<()> {
        let mut required_ids = parent_used_cols
            .iter()
            .chain(&self.DistinctGroupByCol)
            .map(|column| column.UniqueID)
            .collect::<HashSet<_>>();
        if let Some(gid) = &self.GID {
            required_ids.insert(gid.UniqueID);
        }
        if let Some(gpos) = &self.GPos {
            required_ids.insert(gpos.UniqueID);
        }
        self.Schema_mut()
            .Columns
            .retain(|column| required_ids.contains(&column.UniqueID));
        if let Some(child) = self.Children_mut().first_mut() {
            let child_required = child
                .Schema()
                .Columns
                .iter()
                .filter(|column| required_ids.contains(&column.UniqueID))
                .cloned()
                .collect::<Vec<_>>();
            child.PruneColumns(&child_required)?;
        }
        self.GenLevelProjections();
        Ok(())
    }

    /// 构建键信息：行复制破坏唯一性，清空 PK/UK；仅单层且子树 MaxOneRow 时保留。
    pub fn BuildKeyInfo(&mut self) {
        self.base_mut().BuildKeyInfo();
        // 行复制使子树唯一性保证全部失效。
        // Row replication destroys all child uniqueness guarantees.
        self.Schema_mut().PKOrUK.clear();
        self.Schema_mut().NullableUK.clear();
        self.SetMaxOneRow(
            self.RollupGroupingSets.0.len() <= 1
                && self
                    .Children()
                    .first()
                    .is_some_and(|child| child.MaxOneRow()),
        );
    }

    /// 从各层投影表达式中提取关联列（外层引用列）。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        self.LevelExprs
            .iter()
            .flatten()
            .flat_map(|expr| {
                expression::ExtractCorColumns(expr.as_ref())
                    .into_iter()
                    .map(CorrelatedColumn::Clone)
            })
            .collect()
    }

    /// 提取函数依赖（FD）并投影到输出列集合。
    pub fn ExtractFD(&mut self) -> fd::FDSet {
        let mut result = self.base_mut().ExtractFD().clone();
        let mut output = fd::intset::NewFastIntSet(Vec::new());
        for column in &self.Schema().Columns {
            output.Insert(column.UniqueID as i32);
        }
        result.ProjectCols(output);
        self.base_mut().SetFDs(result.clone());
        result
    }

    /// Expand 只复制子节点 schema，本身不声明额外的表达式用列。
    pub fn GetUsedCols(&self) -> Vec<Column> {
        Vec::new()
    }

    /// 位掩码模式：激活的分组列对应位置 1，生成 GID。
    pub fn GenerateGroupingIDModeBitAnd(&self, active_ids: &BTreeSet<i64>) -> u64 {
        self.DistinctGroupByCol
            .iter()
            .rev()
            .fold(0_u64, |value, column| {
                if active_ids.contains(&column.UniqueID) {
                    (value << 1) | 1
                } else {
                    value << 1
                }
            })
    }

    /// 数值递增模式：直接使用层级序号作为 GID。
    pub fn GenerateGroupingIDIncrementModeNumericSet(&self, level: usize) -> u64 {
        level as u64
    }

    /// 为每个 GroupingSet 计算 GID，并记录列→激活该列的 GID 映射。
    pub fn GenerateGroupingMarks(&mut self) {
        let active_sets = self
            .RollupGroupingSets
            .0
            .iter()
            .map(GroupingSet::all_col_ids)
            .collect::<Vec<_>>();
        let mut distinct_sets = Vec::<BTreeSet<i64>>::new();
        for active in &active_sets {
            if !distinct_sets.contains(active) {
                distinct_sets.push(active.clone());
            }
        }
        self.DistinctSize = distinct_sets.len();
        self.RollupGroupingIDs.clear();
        self.RollupID2GIDS.clear();
        for active in &active_sets {
            let gid = match self.GroupingMode {
                GroupingMode::ModeBitAnd => self.GenerateGroupingIDModeBitAnd(active),
                GroupingMode::ModeNumericSet => distinct_sets
                    .iter()
                    .position(|candidate| candidate == active)
                    .map(|level| self.GenerateGroupingIDIncrementModeNumericSet(level))
                    .expect("the active grouping set was collected above"),
            };
            self.RollupGroupingIDs.push(gid);
            for column in &self.DistinctGroupByCol {
                if active.contains(&column.UniqueID) {
                    self.RollupID2GIDS
                        .entry(column.UniqueID)
                        .or_default()
                        .insert(gid);
                }
            }
        }
    }

    /// 生成每层投影：GID/GPos 常量、未激活分组列填 NULL，其余透传。
    pub fn GenLevelProjections(&mut self) {
        self.GenerateGroupingMarks();
        self.LevelExprs.clear();
        let schema_columns = self.Schema().Columns.clone();
        let group_ids = self
            .DistinctGroupByCol
            .iter()
            .map(|column| column.UniqueID)
            .collect::<HashSet<_>>();
        for (level, set) in self.RollupGroupingSets.0.iter().enumerate() {
            let active = set.all_col_ids();
            let gid = self
                .RollupGroupingIDs
                .get(level)
                .copied()
                .unwrap_or(level as u64);
            let mut projection = Vec::with_capacity(schema_columns.len());
            // 按列角色选择常量 GID/GPos、NULL 填充或透传。
            for column in &schema_columns {
                let expr: Expression = if self
                    .GID
                    .as_ref()
                    .is_some_and(|candidate| candidate.UniqueID == column.UniqueID)
                {
                    Box::new(expression::NewUInt64Const(gid as usize))
                } else if self
                    .GPos
                    .as_ref()
                    .is_some_and(|candidate| candidate.UniqueID == column.UniqueID)
                {
                    Box::new(expression::NewUInt64Const(level))
                } else if group_ids.contains(&column.UniqueID) && !active.contains(&column.UniqueID)
                {
                    match &column.RetType {
                        Some(field_type) => {
                            Box::new(expression::NewNullWithFieldType(field_type.clone()))
                        }
                        None => Box::new(expression::NewNull()),
                    }
                } else {
                    Box::new(column.clone())
                };
                projection.push(expr);
            }
            self.LevelExprs.push(projection);
        }
    }

    /// 将 GROUPING 函数参数替换为对应的分组集列引用。
    pub fn ResolveGroupingFuncArgsInGroupBy(&self, args: &[Expression]) -> Vec<Expression> {
        args.iter()
            .map(|expr| self.TrySubstituteExprWithGroupingSetCol(expr))
            .collect()
    }

    /// 若表达式与 DistinctGbyExprs 语义等价，则替换为对应 DistinctGroupByCol。
    pub fn TrySubstituteExprWithGroupingSetCol(&self, expr: &Expression) -> Expression {
        let hash = expr.CanonicalHashCode();
        self.DistinctGbyExprs
            .iter()
            .position(|candidate| candidate.CanonicalHashCode() == hash)
            .and_then(|index| self.DistinctGroupByCol.get(index))
            .cloned()
            .map(|column| Box::new(column) as Expression)
            .unwrap_or_else(|| expr.clone())
    }
}

/// LogicalPlan trait 委托到 LogicalExpand 具体实现。
impl LogicalPlan for LogicalExpand {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn base(&self) -> &BaseLogicalPlan {
        &self.LogicalSchemaProducer.BaseLogicalPlan
    }
    fn base_mut(&mut self) -> &mut BaseLogicalPlan {
        &mut self.LogicalSchemaProducer.BaseLogicalPlan
    }
    fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        Self::PredicatePushDown(self, predicates)
    }
    fn PruneColumns(&mut self, columns: &[Column]) -> Result<()> {
        Self::PruneColumns(self, columns)
    }
    fn BuildKeyInfo(&mut self) {
        Self::BuildKeyInfo(self)
    }
}
