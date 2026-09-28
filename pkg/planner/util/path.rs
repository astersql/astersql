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

// 访问路径（AccessPath）：优化器为表扫描选择的索引/表路径描述。
//
// 包含索引元信息、range、访问/过滤条件、行数估计、IndexMerge 子路径、
// 存储引擎类型，以及 IndexLookUp 下推来源等。
// 另提供相关列前缀长度映射 [`Col2Len`] 的提取与支配比较。

use std::collections::HashMap;

use expression::Expression as _;
use expression::exprctx::BuildContext as _;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(i32)]
/// IndexLookUp 算子下推到存储层的触发来源。
pub enum IndexLookUpPushDownByType {
    #[default]
    /// 未下推。
    IndexLookUpPushDownNone = 0,
    /// 由 hint 触发下推。
    IndexLookUpPushDownByHint = 1,
    /// 由系统变量触发下推。
    IndexLookUpPushDownBySysVar = 2,
}

/// 再导出枚举变体，便于调用方直接使用。
pub use IndexLookUpPushDownByType::{
    IndexLookUpPushDownByHint, IndexLookUpPushDownBySysVar, IndexLookUpPushDownNone,
};

/// 一条物理访问路径：表路径或索引路径（含 IndexMerge 部分路径）。
pub struct AccessPath {
    /// 索引元信息；表路径或 TiFlash 简单扫描可为 None。
    pub Index: Option<model::IndexInfo>,
    /// 完整索引列（含不可见/表达式列占位）。
    pub FullIdxCols: Vec<Option<expression::Column>>,
    /// 完整索引列前缀长度（-1 表示整列）。
    pub FullIdxColLens: Vec<isize>,
    /// 可用于构造 range 的索引列前缀。
    pub IdxCols: Vec<expression::Column>,
    /// 对应 IdxCols 的前缀长度。
    pub IdxColLens: Vec<isize>,
    /// 各索引列是否已被等值常量约束。
    pub ConstCols: Vec<bool>,
    /// 由访问条件推导的扫描 range 列表。
    pub Ranges: Vec<ranger::Range>,
    /// range 访问后的估计行数。
    pub CountAfterAccess: f64,
    /// 访问后行数下界估计。
    pub MinCountAfterAccess: f64,
    /// 访问后行数上界估计。
    pub MaxCountAfterAccess: f64,
    /// 索引过滤后的估计行数。
    pub CountAfterIndex: f64,
    /// 可下推到索引/表访问的条件。
    pub AccessConds: Vec<expression::ExprBox>,
    /// 等值访问条件个数。
    pub EqCondCount: usize,
    /// 等值或 IN 访问条件个数。
    pub EqOrInCondCount: usize,
    /// 索引侧残留过滤条件。
    pub IndexFilters: Vec<expression::ExprBox>,
    /// 回表后的表侧过滤条件。
    pub TableFilters: Vec<expression::ExprBox>,
    /// IndexMerge 的部分索引路径。
    pub PartialIndexPaths: Vec<AccessPath>,
    /// IndexMerge OR 的备选部分路径组合。
    pub PartialAlternativeIndexPaths: Vec<Vec<Vec<AccessPath>>>,
    /// 是否保留 IndexMerge OR 的源过滤。
    pub KeepIndexMergeORSourceFilter: bool,
    /// IndexMerge OR 源过滤表达式。
    pub IndexMergeORSourceFilter: Option<expression::ExprBox>,
    /// IndexMerge 是否为交集（AND）语义。
    pub IndexMergeIsIntersection: bool,
    /// 是否访问多值（MV）索引。
    pub IndexMergeAccessMVIndex: bool,
    /// 存储引擎类型（TiKV / TiFlash 等）。
    pub StoreType: kv::StoreType,
    /// 访问条件是否为 DNF（析取范式）。
    pub IsDNFCond: bool,
    /// DNF 条件下最少访问条件数。
    pub MinAccessCondsForDNFCond: usize,
    /// 是否整型主键表路径。
    pub IsIntHandlePath: bool,
    /// 是否公共句柄（聚簇索引）表路径。
    pub IsCommonHandlePath: bool,
    /// 是否被 hint 强制选用。
    pub Forced: bool,
    /// 强制保持索引序。
    pub ForceKeepOrder: bool,
    /// 强制不保持索引序。
    pub ForceNoKeepOrder: bool,
    /// 强制部分有序。
    pub ForcePartialOrder: bool,
    /// 是否单次扫描即可覆盖所需列（覆盖索引）。
    pub IsSingleScan: bool,
    /// 是否唯一键分片索引路径。
    pub IsUkShardIndexPath: bool,
    /// IndexLookUp 下推来源。
    pub IndexLookUpPushDownBy: IndexLookUpPushDownByType,
    /// 按列分组后的 range。
    pub GroupedRanges: Vec<Vec<ranger::Range>>,
    /// 分组所依据的列下标。
    pub GroupByColIdxs: Vec<usize>,
    /// 计划不可缓存的原因说明。
    pub NoncacheableReason: String,
}

/// 默认空路径：TiKV、无索引、零估计。
impl Default for AccessPath {
    fn default() -> Self {
        Self {
            Index: None,
            FullIdxCols: Vec::new(),
            FullIdxColLens: Vec::new(),
            IdxCols: Vec::new(),
            IdxColLens: Vec::new(),
            ConstCols: Vec::new(),
            Ranges: Vec::new(),
            CountAfterAccess: 0.0,
            MinCountAfterAccess: 0.0,
            MaxCountAfterAccess: 0.0,
            CountAfterIndex: 0.0,
            AccessConds: Vec::new(),
            EqCondCount: 0,
            EqOrInCondCount: 0,
            IndexFilters: Vec::new(),
            TableFilters: Vec::new(),
            PartialIndexPaths: Vec::new(),
            PartialAlternativeIndexPaths: Vec::new(),
            KeepIndexMergeORSourceFilter: false,
            IndexMergeORSourceFilter: None,
            IndexMergeIsIntersection: false,
            IndexMergeAccessMVIndex: false,
            StoreType: kv::StoreType::TiKV,
            IsDNFCond: false,
            MinAccessCondsForDNFCond: 0,
            IsIntHandlePath: false,
            IsCommonHandlePath: false,
            Forced: false,
            ForceKeepOrder: false,
            ForceNoKeepOrder: false,
            ForcePartialOrder: false,
            IsSingleScan: false,
            IsUkShardIndexPath: false,
            IndexLookUpPushDownBy: IndexLookUpPushDownNone,
            GroupedRanges: Vec::new(),
            GroupByColIdxs: Vec::new(),
            NoncacheableReason: String::new(),
        }
    }
}

/// 深拷贝索引元信息与表达式向量。
impl Clone for AccessPath {
    fn clone(&self) -> Self {
        Self {
            Index: self.Index.as_ref().map(model::IndexInfo::Clone),
            FullIdxCols: self.FullIdxCols.clone(),
            FullIdxColLens: self.FullIdxColLens.clone(),
            IdxCols: self.IdxCols.clone(),
            IdxColLens: self.IdxColLens.clone(),
            ConstCols: self.ConstCols.clone(),
            Ranges: self.Ranges.clone(),
            CountAfterAccess: self.CountAfterAccess,
            MinCountAfterAccess: self.MinCountAfterAccess,
            MaxCountAfterAccess: self.MaxCountAfterAccess,
            CountAfterIndex: self.CountAfterIndex,
            AccessConds: self.AccessConds.clone(),
            EqCondCount: self.EqCondCount,
            EqOrInCondCount: self.EqOrInCondCount,
            IndexFilters: self.IndexFilters.clone(),
            TableFilters: self.TableFilters.clone(),
            PartialIndexPaths: self.PartialIndexPaths.clone(),
            PartialAlternativeIndexPaths: self.PartialAlternativeIndexPaths.clone(),
            KeepIndexMergeORSourceFilter: self.KeepIndexMergeORSourceFilter,
            IndexMergeORSourceFilter: self.IndexMergeORSourceFilter.clone(),
            IndexMergeIsIntersection: self.IndexMergeIsIntersection,
            // Go's Clone struct literal omits this field, so the clone keeps
            // the zero value rather than inheriting the source path's state.
            IndexMergeAccessMVIndex: false,
            StoreType: self.StoreType,
            IsDNFCond: self.IsDNFCond,
            MinAccessCondsForDNFCond: self.MinAccessCondsForDNFCond,
            IsIntHandlePath: self.IsIntHandlePath,
            IsCommonHandlePath: self.IsCommonHandlePath,
            Forced: self.Forced,
            ForceKeepOrder: self.ForceKeepOrder,
            ForceNoKeepOrder: self.ForceNoKeepOrder,
            ForcePartialOrder: self.ForcePartialOrder,
            IsSingleScan: self.IsSingleScan,
            IsUkShardIndexPath: self.IsUkShardIndexPath,
            // Go's Clone struct literal also leaves this at its zero variant.
            IndexLookUpPushDownBy: IndexLookUpPushDownNone,
            GroupedRanges: self.GroupedRanges.clone(),
            GroupByColIdxs: self.GroupByColIdxs.clone(),
            NoncacheableReason: self.NoncacheableReason.clone(),
        }
    }
}

/// 路径判定、相关列条件拆分、点查与列前缀长度等辅助方法。
impl AccessPath {
    /// Go 风格 Clone 别名。
    pub fn Clone(&self) -> AccessPath {
        self.clone()
    }

    /// 是否表路径（整型/公共句柄，或带索引的 TiFlash 路径）。
    pub fn IsTablePath(&self) -> bool {
        self.IsIntHandlePath
            || self.IsCommonHandlePath
            || (self.Index.is_some() && self.StoreType == kv::StoreType::TiFlash)
    }

    /// 是否 TiKV 上的表路径。
    pub fn IsTiKVTablePath(&self) -> bool {
        (self.IsIntHandlePath || self.IsCommonHandlePath) && self.StoreType == kv::StoreType::TiKV
    }

    /// 是否无索引的 TiFlash 简单表扫描。
    pub fn IsTiFlashSimpleTablePath(&self) -> bool {
        self.StoreType == kv::StoreType::TiFlash && self.Index.is_none()
    }

    /// 从 TableFilters 中拆出与索引列相关的相关列（correlated）访问条件。
    /// 相关列来自外层查询，出现在子查询索引匹配中时通常使计划不可缓存。
    pub fn SplitCorColAccessCondFromFilters(
        &self,
        context: &dyn plan_base::PlanContext,
        eq_or_in_count: usize,
    ) -> (Vec<expression::ExprBox>, Vec<expression::ExprBox>) {
        let mut access = Vec::with_capacity(self.IdxCols.len().saturating_sub(eq_or_in_count));
        let mut used = vec![false; self.TableFilters.len()];

        // 从已有等值/IN 之后的索引列继续匹配相关列或常量等值。
        for index in eq_or_in_count..self.IdxCols.len() {
            let mut matched = false;
            for (filter_index, filter) in self.TableFilters.iter().enumerate() {
                if used[filter_index] {
                    continue;
                }
                let column_equals_constant = isColEqConstant(filter.as_ref(), &self.IdxCols[index]);
                // 下一列若是常量等值，说明相关列未能连续前缀匹配，放弃拆分。
                if index == eq_or_in_count && column_equals_constant {
                    return (Vec::new(), self.TableFilters.clone());
                }
                if !column_equals_constant && !isColEqCorCol(filter.as_ref(), &self.IdxCols[index])
                {
                    continue;
                }
                context
                    .GetExprCtx()
                    .SetSkipPlanCache("Correlated subquery is not cached currently");
                access.push(filter.clone());
                if self.IdxColLens[index] == -1 {
                    used[filter_index] = true;
                }
                matched = true;
                break;
            }
            if matched {
                continue;
            }

            // 等值未匹配时尝试范围相关列，匹配后提前返回剩余过滤。
            if let Some((_, range_filter)) =
                self.TableFilters
                    .iter()
                    .enumerate()
                    .find(|(filter_index, filter)| {
                        !used[*filter_index]
                            && isColRangeCorCol(filter.as_ref(), &self.IdxCols[index])
                    })
            {
                context
                    .GetExprCtx()
                    .SetSkipPlanCache("Correlated subquery is not cached currently");
                access.push(range_filter.clone());
                let remained = self
                    .TableFilters
                    .iter()
                    .enumerate()
                    .filter(|(filter_index, _)| !used[*filter_index])
                    .map(|(_, filter)| filter.clone())
                    .collect();
                return (access, remained);
            }
            break;
        }

        let remained = self
            .TableFilters
            .iter()
            .enumerate()
            .filter(|(index, _)| !used[*index])
            .map(|(_, filter)| filter.clone())
            .collect();
        (access, remained)
    }

    /// 全部 range 是否为点查（可走点查优化）。
    pub fn OnlyPointRange(&self, type_context: expression::types::Context) -> bool {
        if self.IsIntHandlePath {
            return self
                .Ranges
                .iter()
                .all(|range| range.IsPointNullable(type_context.clone()));
        }
        let Some(index) = &self.Index else {
            return false;
        };
        self.Ranges.iter().all(|range| {
            range.IsPointNonNullable(type_context.clone())
                && range.HighVal.len() == index.Columns.len()
        })
    }

    /// 从访问条件提取列 → 前缀长度映射。
    pub fn GetCol2LenFromAccessConds(&self, context: &dyn plan_base::PlanContext) -> Col2Len {
        if self.IsTablePath() {
            ExtractCol2Len(
                context.GetExprCtx().GetEvalCtx(),
                &self.AccessConds,
                None,
                None,
            )
        } else {
            ExtractCol2Len(
                context.GetExprCtx().GetEvalCtx(),
                &self.AccessConds,
                Some(&self.IdxCols),
                Some(&self.IdxColLens),
            )
        }
    }

    /// range 是否覆盖全表（考虑无符号整型句柄）。
    pub fn IsFullScanRange(&self, table: &model::TableInfo) -> bool {
        let unsigned_integer_handle = self.IsIntHandlePath
            && table.PKIsHandle
            && table
                .GetPkColInfo()
                .is_some_and(|column| mysql::r#type::HasUnsignedFlag(column.GetFlag()));
        ranger::HasFullRange(&self.Ranges, unsigned_integer_handle)
    }

    /// 路径是否不确定（MV 索引或带条件表达式的索引）。
    pub fn IsUndetermined(&self) -> bool {
        if self.IsTablePath() {
            return false;
        }
        self.Index
            .as_ref()
            .is_some_and(|index| index.MVIndex || !index.ConditionExprString.is_empty())
    }

    /// IndexJoin 是否不可用（与 IsUndetermined 相同判定）。
    pub fn IsIndexJoinUnapplicable(&self) -> bool {
        self.IsUndetermined()
    }
}

/// 表达式是否为「列 = 常量」。
fn isColEqConstant(expr: &dyn expression::Expression, column: &expression::Column) -> bool {
    isColEqExpr(expr, column, |argument| {
        argument.as_any().is::<expression::Constant>()
    })
}

/// 表达式是否为「列 = 相关列」。
fn isColEqCorCol(expr: &dyn expression::Expression, column: &expression::Column) -> bool {
    isColEqExpr(expr, column, |argument| {
        argument.as_any().is::<expression::CorrelatedColumn>()
    })
}

/// 表达式是否为「列 与 相关列」的范围比较（< <= > >=）。
fn isColRangeCorCol(expr: &dyn expression::Expression, column: &expression::Column) -> bool {
    let Some(function) = expr.as_any().downcast_ref::<expression::ScalarFunction>() else {
        return false;
    };
    if !matches!(
        function.FuncName.L.as_str(),
        parser_ast::LT | parser_ast::GT | parser_ast::LE | parser_ast::GE
    ) {
        return false;
    }
    columnComparedWith(function, column, |argument| {
        argument.as_any().is::<expression::CorrelatedColumn>()
    })
}

/// 通用：检测 EQ 且一侧为给定列、另一侧满足 check。
fn isColEqExpr(
    expr: &dyn expression::Expression,
    column: &expression::Column,
    check: impl Fn(&dyn expression::Expression) -> bool,
) -> bool {
    let Some(function) = expr.as_any().downcast_ref::<expression::ScalarFunction>() else {
        return false;
    };
    function.FuncName.L == parser_ast::EQ && columnComparedWith(function, column, check)
}

/// 检查二元比较两侧是否为「目标列」与「满足 check 的另一操作数」，并校验字符串 collation。
fn columnComparedWith(
    function: &expression::ScalarFunction,
    column: &expression::Column,
    check: impl Fn(&dyn expression::Expression) -> bool,
) -> bool {
    let arguments = function.GetArgs();
    if arguments.len() != 2 {
        return false;
    }
    let (_, collation) = function.CharsetAndCollation();
    // 列可在比较符左侧或右侧。
    for (column_side, other_side) in [(0, 1), (1, 0)] {
        let Some(candidate) = arguments[column_side]
            .as_any()
            .downcast_ref::<expression::Column>()
        else {
            continue;
        };
        if candidate.GetStaticType().EvalType() == expression::types::ETString
            && !collate::CompatibleCollate(&collation, candidate.GetStaticType().GetCollate())
        {
            continue;
        }
        if check(arguments[other_side].as_ref()) && column.EqualColumn(candidate) {
            return true;
        }
    }
    false
}

/// 列 UniqueID → 索引前缀长度（-1 表示整列）。
pub type Col2Len = HashMap<i64, isize>;

/// 从表达式列表提取 Col2Len；表路径时长度固定为 -1。
pub fn ExtractCol2Len(
    context: &dyn expression::exprctx::EvalContext,
    expressions: &[expression::ExprBox],
    index_columns: Option<&[expression::Column]>,
    index_column_lengths: Option<&[isize]>,
) -> Col2Len {
    let mut result = HashMap::with_capacity(index_columns.map_or(0, <[expression::Column]>::len));
    for expression in expressions {
        extractCol2LenFromExpr(
            context,
            expression.as_ref(),
            index_columns,
            index_column_lengths,
            &mut result,
        );
    }
    result
}

/// 递归从列或标量函数参数填充 Col2Len。
fn extractCol2LenFromExpr(
    context: &dyn expression::exprctx::EvalContext,
    expr: &dyn expression::Expression,
    index_columns: Option<&[expression::Column]>,
    index_column_lengths: Option<&[isize]>,
    result: &mut Col2Len,
) {
    if let Some(column) = expr.as_any().downcast_ref::<expression::Column>() {
        match (index_columns, index_column_lengths) {
            (Some(columns), Some(lengths)) => {
                if let Some(index) = columns
                    .iter()
                    .position(|candidate| column.EqualByExprAndID(context, candidate))
                {
                    result.insert(column.UniqueID, lengths[index]);
                }
            }
            (None, None) => {
                result.insert(column.UniqueID, -1);
            }
            _ => {}
        }
        return;
    }
    if let Some(function) = expr.as_any().downcast_ref::<expression::ScalarFunction>() {
        for argument in function.GetArgs() {
            extractCol2LenFromExpr(
                context,
                argument.as_ref(),
                index_columns,
                index_column_lengths,
                result,
            );
        }
    }
}

/// 比较前缀长度：-1（整列）大于任何有限前缀。
fn compareLength(left: isize, right: isize) -> i32 {
    if left == right {
        0
    } else if left == -1 {
        1
    } else if right == -1 {
        -1
    } else if left > right {
        1
    } else {
        -1
    }
}

/// left 是否在每个列上都不弱于 right（用于路径覆盖判定）。
fn dominate(left: &Col2Len, right: &Col2Len) -> bool {
    right.len() <= left.len()
        && right.iter().all(|(column, right_length)| {
            left.get(column)
                .is_some_and(|left_length| compareLength(*right_length, *left_length) != 1)
        })
}

/// 比较两个 Col2Len：返回偏序结果，以及是否构成支配关系。
pub fn CompareCol2Len(left: &Col2Len, right: &Col2Len) -> (i32, bool) {
    if left.len() > right.len() {
        return (1, dominate(left, right));
    }
    if left.len() < right.len() {
        return (-1, dominate(right, left));
    }
    for (column, right_length) in right {
        let Some(left_length) = left.get(column) else {
            return (0, false);
        };
        if left_length != right_length {
            return (if left_length > right_length { 1 } else { -1 }, false);
        }
    }
    (0, true)
}
