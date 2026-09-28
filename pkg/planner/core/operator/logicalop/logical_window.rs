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

// 逻辑窗口函数（Window Function）算子。
//
// 对应 SQL 的 `OVER (PARTITION BY ... ORDER BY ... frame)`：在分区内按帧
// （Window Frame）计算聚合/排名等函数。仅引用分区列的谓词可下推；输出列在
// 子节点 schema 末尾追加窗口结果列。

use crate::*;
use std::any::Any;
use std::collections::{HashMap, HashSet};

/// 窗口帧类型：按行数、值范围或分组边界定义帧。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum FrameType {
    /// ROWS：按物理行偏移界定帧。
    Rows,
    /// RANGE：按 ORDER BY 值的范围界定帧（默认）。
    #[default]
    Range,
    /// GROUPS：按排序键相等分组的边界界定帧。
    Groups,
}

/// 帧边界相对当前行的方向。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum BoundType {
    /// 当前行之前（PRECEDING）。
    Preceding,
    /// 当前行（CURRENT ROW，默认）。
    #[default]
    CurrentRow,
    /// 当前行之后（FOLLOWING）。
    Following,
}

/// RANGE 帧比较时使用的数据类型分类。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub enum RangeCmpDataType {
    /// 整数比较。
    Int,
    /// 浮点比较。
    Real,
    /// Decimal 比较。
    Decimal,
    /// 日期时间比较。
    Time,
    /// 时长比较。
    Duration,
    /// 尚不支持的类型（默认）。
    #[default]
    Unsupported,
}

/// 窗口帧的一侧边界（START 或 END）。
#[derive(Clone)]
pub struct FrameBound {
    /// 边界方向（Preceding / CurrentRow / Following）。
    pub Type: BoundType,
    /// 是否为无界（UNBOUNDED）。
    pub UnBounded: bool,
    /// 有界时的偏移量。
    pub Num: u64,
    /// 计算边界值的表达式（RANGE 偏移等）。
    pub CalcFuncs: Vec<Expression>,
    /// 参与边界比较的列表达式。
    pub CompareCols: Vec<Expression>,
    /// 比较函数名列表（如 le/eq/ge）。
    pub CmpFuncs: Vec<String>,
    /// RANGE 比较的数据类型。
    pub CmpDataType: RangeCmpDataType,
    /// 是否为显式写出的 RANGE 边界。
    pub IsExplicitRange: bool,
}

impl Default for FrameBound {
    fn default() -> Self {
        Self {
            Type: BoundType::CurrentRow,
            UnBounded: false,
            Num: 0,
            CalcFuncs: Vec::new(),
            CompareCols: Vec::new(),
            CmpFuncs: Vec::new(),
            CmpDataType: RangeCmpDataType::Unsupported,
            IsExplicitRange: false,
        }
    }
}

impl FrameBound {
    /// 深拷贝本边界。
    pub fn Clone(&self) -> Self {
        self.clone()
    }
    /// 按数据类型更新比较函数名：Preceding→le，CurrentRow→eq，Following→ge。
    pub fn UpdateCmpFuncsAndCmpDataType(&mut self, data_type: RangeCmpDataType) {
        self.CmpDataType = data_type;
        self.CmpFuncs = match data_type {
            RangeCmpDataType::Unsupported => Vec::new(),
            _ => vec![
                match self.Type {
                    BoundType::Preceding => "le",
                    BoundType::CurrentRow => "eq",
                    BoundType::Following => "ge",
                }
                .to_owned(),
            ],
        };
    }
    /// 设置边界比较列。
    pub fn updateCompareCols(&mut self, columns: Vec<Expression>) {
        self.CompareCols = columns;
    }

    /// 计算边界的 64 位指纹，用于计划等价判断。
    pub fn Hash64(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.Type.hash(&mut hasher);
        self.UnBounded.hash(&mut hasher);
        self.Num.hash(&mut hasher);
        for expression in self.CalcFuncs.iter().chain(&self.CompareCols) {
            expression.CanonicalHashCode().hash(&mut hasher);
        }
        self.CmpFuncs.hash(&mut hasher);
        self.CmpDataType.hash(&mut hasher);
        self.IsExplicitRange.hash(&mut hasher);
        hasher.finish()
    }

    /// 判断两个边界在语义上是否相等（含表达式规范哈希）。
    pub fn Equals(&self, other: &FrameBound) -> bool {
        self.Type == other.Type
            && self.UnBounded == other.UnBounded
            && self.Num == other.Num
            && self.CmpFuncs == other.CmpFuncs
            && self.CmpDataType == other.CmpDataType
            && self.IsExplicitRange == other.IsExplicitRange
            && self.CalcFuncs.len() == other.CalcFuncs.len()
            && self.CompareCols.len() == other.CompareCols.len()
            && self
                .CalcFuncs
                .iter()
                .zip(&other.CalcFuncs)
                .chain(self.CompareCols.iter().zip(&other.CompareCols))
                .all(|(left, right)| left.CanonicalHashCode() == right.CanonicalHashCode())
    }
}

/// 完整窗口帧：类型 + 起止边界。
#[derive(Clone, Default)]
pub struct WindowFrame {
    /// 帧类型（ROWS / RANGE / GROUPS）。
    pub Type: FrameType,
    /// 起始边界。
    pub Start: Option<FrameBound>,
    /// 结束边界。
    pub End: Option<FrameBound>,
}

impl WindowFrame {
    /// 深拷贝帧定义。
    pub fn Clone(&self) -> Self {
        self.clone()
    }
    /// 简易 Hash64：帧类型与边界 Num/Type 的多项式混合。
    pub fn Hash64(&self) -> u64 {
        let mut value = self.Type as u64;
        for bound in [&self.Start, &self.End].into_iter().flatten() {
            value = value
                .wrapping_mul(131)
                .wrapping_add(bound.Num)
                .wrapping_add(bound.Type as u64);
        }
        value
    }
    /// 帧是否相等（含两侧边界）。
    pub fn Equals(&self, other: &WindowFrame) -> bool {
        self.Type == other.Type
            && option_bound_equals(&self.Start, &other.Start)
            && option_bound_equals(&self.End, &other.End)
    }
}

/// 单个窗口函数描述：函数名与参数表达式。
#[derive(Clone, Default)]
pub struct WindowFuncDesc {
    /// 窗口函数名（如 `row_number`、`sum`）。
    pub Name: String,
    /// 函数参数列表。
    pub Args: Vec<Expression>,
}

/// 下推到执行层/PB 的窗口摘要信息。
#[derive(Clone, Debug, PartialEq)]
pub struct WindowPB {
    /// 窗口函数名列表。
    pub names: Vec<String>,
    /// 分区列 UniqueID。
    pub partition_columns: Vec<i64>,
    /// 排序列 UniqueID。
    pub order_columns: Vec<i64>,
    /// 帧类型（若有）。
    pub frame_type: Option<FrameType>,
}

/// 逻辑窗口算子：在子节点结果上计算一组窗口函数。
#[derive(Default)]
pub struct LogicalWindow {
    /// Schema 与基类逻辑计划。
    pub LogicalSchemaProducer: LogicalSchemaProducer,
    /// 要计算的窗口函数列表。
    pub WindowFuncDescs: Vec<WindowFuncDesc>,
    /// PARTITION BY 键。
    pub PartitionBy: Vec<SortItem>,
    /// ORDER BY 键。
    pub OrderBy: Vec<SortItem>,
    /// 可选窗口帧定义。
    pub Frame: Option<WindowFrame>,
}

impl LogicalWindow {
    /// 初始化算子名为 `"Window"`。
    pub fn Init(mut self, ctx: base::ContextRef, offset: i32) -> Self {
        self.LogicalSchemaProducer.BaseLogicalPlan = NewBaseLogicalPlan(ctx, "Window", offset);
        self
    }

    /// 克隆窗口算子并复制 schema / 输出列名。
    pub fn Clone(&self, ctx: base::ContextRef) -> Self {
        let mut cloned = Self {
            LogicalSchemaProducer: LogicalSchemaProducer::default(),
            WindowFuncDescs: self.WindowFuncDescs.clone(),
            PartitionBy: self.PartitionBy.clone(),
            OrderBy: self.OrderBy.clone(),
            Frame: self.Frame.clone(),
        }
        .Init(ctx, self.QueryBlockOffset());
        cloned.SetSchema(self.Schema().Clone());
        cloned.SetOutputNames(self.OutputNames().Shallow());
        cloned
    }

    /// Hash64：混合帧哈希与分区/排序列 UniqueID、升降序标记。
    pub fn Hash64(&self) -> u64 {
        let mut value = self.Frame.as_ref().map_or(0, WindowFrame::Hash64);
        for item in self.PartitionBy.iter().chain(&self.OrderBy) {
            value = value
                .wrapping_mul(131)
                .wrapping_add(item.Col.UniqueID as u64)
                .wrapping_add(item.Desc as u64);
        }
        value
    }

    /// 仅把只引用分区列的谓词下推；其余保留在窗口之上。
    pub fn PredicatePushDown(&mut self, predicates: Vec<Expression>) -> Result<Vec<Expression>> {
        let partition_ids = self
            .PartitionBy
            .iter()
            .map(|item| item.Col.UniqueID)
            .collect::<HashSet<_>>();
        // 谓词列必须全部落在分区键上，才不改变窗口语义
        let (pushable, mut retained): (Vec<_>, Vec<_>) =
            predicates.into_iter().partition(|predicate| {
                let columns = expression::ExtractColumns(predicate.as_ref());
                !columns.is_empty()
                    && columns
                        .iter()
                        .all(|column| partition_ids.contains(&column.UniqueID))
            });
        if let Some(child) = self.Children_mut().first_mut() {
            let residual = PredicatePushDownPlan(child, pushable)?;
            AttachSelectionToPlan(child, residual)?;
        }
        Ok(retained)
    }

    /// 列裁剪：剔除窗口结果列后，合并窗口内部用列，下推给子节点再拼回结果列。
    pub fn PruneColumns(&mut self, parent_used_cols: &[Column]) -> Result<()> {
        let window_columns = self.GetWindowResultColumns();
        let window_column_ids = window_columns
            .iter()
            .map(|column| column.UniqueID)
            .collect::<HashSet<_>>();
        let mut child_columns = parent_used_cols
            .iter()
            .filter(|column| !window_column_ids.contains(&column.UniqueID))
            .cloned()
            .collect::<Vec<_>>();
        child_columns.extend(self.extractUsedCols());
        child_columns.sort_by_key(|column| column.UniqueID);
        child_columns.dedup_by_key(|column| column.UniqueID);
        if let Some(child) = self.Children_mut().first_mut() {
            child.PruneColumns(&child_columns)?;
        }
        let mut columns = self
            .Children()
            .first()
            .map(|child| child.Schema().Columns.clone())
            .unwrap_or_default();
        columns.extend(window_columns);
        self.Schema_mut().Columns = columns;
        Ok(())
    }

    /// 按 UniqueID 映射替换函数参数、分区/排序列及帧边界中的列引用。
    pub fn ReplaceExprColumns(&mut self, replacements: &HashMap<i64, Column>) {
        for desc in &mut self.WindowFuncDescs {
            for arg in &mut desc.Args {
                *arg = replace_window_expr(arg, replacements);
            }
        }
        for item in self.PartitionBy.iter_mut().chain(&mut self.OrderBy) {
            if let Some(column) = replacements.get(&item.Col.UniqueID) {
                item.Col = column.clone();
            }
        }
        if let Some(frame) = &mut self.Frame {
            for bound in [&mut frame.Start, &mut frame.End].into_iter().flatten() {
                for expr in bound.CalcFuncs.iter_mut().chain(&mut bound.CompareCols) {
                    *expr = replace_window_expr(expr, replacements);
                }
            }
        }
    }

    /// 统计继承子节点行数，并为每个窗口结果列写入 NDV=行数。
    pub fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        if !reload && let Some(stats) = self.StatsInfo() {
            return Ok((stats.clone(), false));
        }
        let child_stats = self
            .Children_mut()
            .first_mut()
            .ok_or_else(|| PlannerError("LogicalWindow requires one child".to_owned()))?
            .DeriveStats(reload)?
            .0;
        let mut stats = child_stats.clone();
        for column in self.GetWindowResultColumns() {
            stats.ColNDVs.insert(column.UniqueID, stats.RowCount);
        }
        self.SetStats(stats.clone());
        Ok((stats, true))
    }

    /// 过滤掉包含窗口结果列的列组，保留仅来自子节点的列组。
    pub fn ExtractColGroups(&self, groups: &[Vec<Column>]) -> Vec<Vec<Column>> {
        let result_ids = self
            .GetWindowResultColumns()
            .iter()
            .map(|column| column.UniqueID)
            .collect::<HashSet<_>>();
        groups
            .iter()
            .filter(|group| {
                group
                    .iter()
                    .all(|column| !result_ids.contains(&column.UniqueID))
            })
            .cloned()
            .collect()
    }

    /// 从窗口内部表达式提取相关列。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        self.extractUsedExprs()
            .into_iter()
            .flat_map(|expr| {
                expression::ExtractCorColumns(expr.as_ref())
                    .into_iter()
                    .map(CorrelatedColumn::Clone)
            })
            .collect()
    }

    /// 可能属性委托基类处理子节点 TiFlash 标记。
    pub fn PreparePossibleProperties(&mut self, child_tiflash: &[bool]) -> bool {
        self.base_mut().PreparePossibleProperties(child_tiflash)
    }
    /// 返回 PARTITION BY 项。
    pub fn GetPartitionBy(&self) -> &[SortItem] {
        &self.PartitionBy
    }
    /// 返回分区列向量。
    pub fn GetPartitionByCols(&self) -> Vec<Column> {
        self.PartitionBy
            .iter()
            .map(|item| item.Col.clone())
            .collect()
    }
    /// `GetPartitionByCols` 的别名。
    pub fn GetPartitionKeys(&self) -> Vec<Column> {
        self.GetPartitionByCols()
    }
    /// Schema 末尾与窗口函数一一对应的结果列。
    pub fn GetWindowResultColumns(&self) -> Vec<Column> {
        let count = self.WindowFuncDescs.len();
        self.Schema().Columns[self.Schema().Columns.len().saturating_sub(count)..].to_vec()
    }
    /// 透传统计中的分组 NDV。
    pub fn GetGroupNDVs(&self, stats: &StatsInfo) -> Vec<property::GroupNDV> {
        stats.GroupNDVs.clone()
    }
    /// 导出简化的 PB 摘要。
    pub fn ToPB(&self) -> WindowPB {
        WindowPB {
            names: self
                .WindowFuncDescs
                .iter()
                .map(|desc| desc.Name.clone())
                .collect(),
            partition_columns: self
                .PartitionBy
                .iter()
                .map(|item| item.Col.UniqueID)
                .collect(),
            order_columns: self.OrderBy.iter().map(|item| item.Col.UniqueID).collect(),
            frame_type: self.Frame.as_ref().map(|frame| frame.Type),
        }
    }
    /// 同步更新帧两侧边界的比较函数与数据类型。
    pub fn UpdateCmpFuncsAndCmpDataType(&mut self, data_type: RangeCmpDataType) {
        if let Some(frame) = &mut self.Frame {
            for bound in [&mut frame.Start, &mut frame.End].into_iter().flatten() {
                bound.UpdateCmpFuncsAndCmpDataType(data_type);
            }
        }
    }
    /// 检查 TiFlash 是否可执行该帧：显式 RANGE 时不得为 Unsupported 类型。
    pub fn checkComparisonForTiFlash(&self) -> bool {
        self.Frame.as_ref().is_none_or(|frame| {
            [&frame.Start, &frame.End]
                .into_iter()
                .flatten()
                .all(|bound| {
                    bound.CmpDataType != RangeCmpDataType::Unsupported || !bound.IsExplicitRange
                })
        })
    }
    /// 比较两窗口的帧定义是否相等。
    pub fn equalFrame(&self, other: &LogicalWindow) -> bool {
        match (&self.Frame, &other.Frame) {
            (None, None) => true,
            (Some(left), Some(right)) => left.Equals(right),
            _ => false,
        }
    }
    /// 比较 ORDER BY 项是否逐项相等。
    pub fn equalOrderBy(&self, other: &LogicalWindow) -> bool {
        self.OrderBy.len() == other.OrderBy.len()
            && self
                .OrderBy
                .iter()
                .zip(&other.OrderBy)
                .all(|(left, right)| left.EqualsSortItem(right))
    }
    /// 按 Go 实现的集合语义比较 PARTITION BY 列（忽略顺序与方向）。
    pub fn equalPartitionBy(&self, other: &LogicalWindow) -> bool {
        if self.PartitionBy.len() != other.PartitionBy.len() {
            return false;
        }
        let partition_ids = self
            .PartitionBy
            .iter()
            .map(|item| item.Col.UniqueID)
            .collect::<HashSet<_>>();
        other
            .PartitionBy
            .iter()
            .all(|item| partition_ids.contains(&item.Col.UniqueID))
    }
    /// 仅替换帧边界中的列引用。
    pub fn replaceFrameBoundColumns(&mut self, replacements: &HashMap<i64, Column>) {
        if let Some(frame) = &mut self.Frame {
            for bound in [&mut frame.Start, &mut frame.End].into_iter().flatten() {
                for expr in bound.CalcFuncs.iter_mut().chain(&mut bound.CompareCols) {
                    *expr = replace_window_expr(expr, replacements);
                }
            }
        }
    }
    /// 收集窗口函数参数与帧边界中的表达式引用。
    fn extractUsedExprs(&self) -> Vec<&Expression> {
        let mut result = self
            .WindowFuncDescs
            .iter()
            .flat_map(|desc| desc.Args.iter())
            .collect::<Vec<_>>();
        if let Some(frame) = &self.Frame {
            for bound in [&frame.Start, &frame.End].into_iter().flatten() {
                result.extend(bound.CalcFuncs.iter());
                result.extend(bound.CompareCols.iter());
            }
        }
        result
    }
    /// 汇总窗口内部用到的全部列（参数、帧、分区、排序），去重后返回。
    pub fn extractUsedCols(&self) -> Vec<Column> {
        let mut columns = self
            .extractUsedExprs()
            .into_iter()
            .flat_map(|expr| {
                expression::ExtractColumns(expr.as_ref())
                    .into_iter()
                    .cloned()
            })
            .collect::<Vec<_>>();
        columns.extend(self.GetPartitionByCols());
        columns.extend(self.OrderBy.iter().map(|item| item.Col.clone()));
        columns.sort_by_key(|column| column.UniqueID);
        columns.dedup_by_key(|column| column.UniqueID);
        columns
    }
}

/// 比较两个可选 FrameBound 是否相等（表达式用 HashCode）。
fn option_bound_equals(left: &Option<FrameBound>, right: &Option<FrameBound>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.Type == right.Type
                && left.UnBounded == right.UnBounded
                && left.Num == right.Num
                && left.CmpDataType == right.CmpDataType
                && left.IsExplicitRange == right.IsExplicitRange
                && left
                    .CalcFuncs
                    .iter()
                    .map(|expr| expr.HashCode())
                    .eq(right.CalcFuncs.iter().map(|expr| expr.HashCode()))
                && left
                    .CompareCols
                    .iter()
                    .map(|expr| expr.HashCode())
                    .eq(right.CompareCols.iter().map(|expr| expr.HashCode()))
        }
        _ => false,
    }
}

/// 递归替换表达式树中的列引用；标量函数会清理缓存的 HashCode。
fn replace_window_expr(expr: &Expression, replacements: &HashMap<i64, Column>) -> Expression {
    if let Some(column) = expr.as_column() {
        return replacements
            .get(&column.UniqueID)
            .cloned()
            .map(|column| Box::new(column) as Expression)
            .unwrap_or_else(|| expr.clone());
    }
    if let Some(function) = expr.as_scalar_function() {
        let mut result = function.clone_scalar();
        for arg in result.GetArgsMut() {
            *arg = replace_window_expr(arg, replacements);
        }
        result.CleanHashCode();
        return Box::new(result);
    }
    expr.clone()
}

impl LogicalPlan for LogicalWindow {
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
    fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        Self::DeriveStats(self, reload)
    }
}
