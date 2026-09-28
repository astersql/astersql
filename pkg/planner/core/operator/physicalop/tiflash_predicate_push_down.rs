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

// 本文件描述 TiFlash 谓词下推、表达式分组及收益排序逻辑；不会执行查询。
// 主要函数和方法前保留对应 Go 语义，无法安全映射的指针、接口、错误与并发语义在相邻代码处说明。
// #![allow(dead_code, non_snake_case, non_upper_case_globals)]
//
//
// Go imports（仅作迁移参考，不虚构 crate）：
// 	"cmp"
// 	"math"
// 	"slices"
// 	"strings"
//// 	"github.com/pingcap/tidb/pkg/expression"
// 	"github.com/pingcap/tidb/pkg/meta/model"
// 	"github.com/pingcap/tidb/pkg/parser/ast"
// 	"github.com/pingcap/tidb/pkg/planner/cardinality"
// 	"github.com/pingcap/tidb/pkg/planner/core/base"
// 	"github.com/pingcap/tidb/pkg/util/logutil"
// 	"go.uber.org/zap"
//
// selectivity = (row count after filter) / (row count before filter), smaller is better
// income = (1 - selectivity) * restColumnCount * tableRowCount, greater is better
//
// const (
//     selectivityThreshold = 0.6
// TiFlash 谓词下推（predicate push down）与晚期物化（late materialization）逻辑。
//
// 谓词下推把过滤条件尽量贴近扫描节点执行，以减少上层算子处理的行数；
// 晚期物化则先用少量列过滤，再按需读取其余列。本模块按选择率（selectivity）
// 对谓词分组排序，并结合倒排列存索引（inverted columnar index）提示决定下推集合。

// The default now of number of rows in a pack of TiFlash
//     tiflashDataPackSize  = 8192
//     columnCountThreshold = 3
// )
//
// expressionGroup is used to store
// 1. a group of expressions
// 2. the selectivity of the expressions
// pub struct expressionGroup {
//     exprs       []expression.Expression
//     selectivity float64
// }
//
// 对应 Go 同名函数或方法；保留原参数顺序、主要分支和返回值形状。
// pub fn compareExpressionGroups(x, y expressionGroup) int {
//     if diff := cmp.Compare(len(x.exprs), len(y.exprs)); diff != 0 {
//         return diff
//     }
//     for i := range x.exprs {
//         if diff := slices.Compare(y.exprs[i].HashCode(), x.exprs[i].HashCode()); diff != 0 {
//             return diff
//         }
//     }
//     return 0
// }
//
// transformColumnsToCode is used to transform the columns to a string of "0" and "1".
// @param: cols: the columns of a Expression
// @param: tableColumns: the total number of columns in the tablescan
// @example:
////			  the columns of tablescan are [a, b, c, d, e, f, g, h]
//			  the expression are ["a > 1", "c > 1", "e > 1 or g > 1"]
//	          so the columns of the expression are [a, c, e, g] and the tableColumns is 8
//	          the return value is "10101010"
//// @return: the string of "0" and "1"
// 对应 Go 同名函数或方法；保留原参数顺序、主要分支和返回值形状。
// pub fn transformColumnsToCode(cols []*expression.Column, tableColumns int) string {
//     if len(cols) == 0 {
//         return "0"
//     }
//
//     code := make([]byte, tableColumns)
//     for _, col := range cols {
//         if col.ID < int64(tableColumns) && col.ID >= 0 {
//             code[col.ID] = '1'
//         }
//     }
//     return string(code)
// }
//
// groupByColumnsSortBySelectivity is used to group the conditions by the column they use
// and sort the groups by the selectivity of the conditions in the group.
// @param: conds: the conditions to be grouped
// @return: the groups of conditions sorted by the selectivity of the conditions in the group
// @example: conds = [a > 1, b > 1, a > 2, c > 1, a > 3, b > 2], return = [[a > 3, a > 2, a > 1], [b > 2, b > 1], [c > 1]]
// @note: when the selectivity of one group is larger than the threshold, we will remove it from the returned result.
// @note: when the number of columns of one group is larger than the threshold, we will remove it from the returned result.
// 对应 Go 同名函数或方法；保留原参数顺序、主要分支和返回值形状。
// pub fn groupByColumnsSortBySelectivity(sctx base.PlanContext, conds []expression.Expression, ts *PhysicalTableScan) []expressionGroup {
// Create a map to store the groupMap of conditions keyed by the columns
//     groupMap := make(map[string][]expression.Expression)
//
// Iterate through the conditions and group them by columns
//     for _, cond := range conds {
// If excuting light cost condition first,
// the rows needed to be exucuted by the heavy cost condition will be reduced.
// So if the cond contains heavy cost functions, skip it to reduce the cost of calculating the selectivity.
//         if withHeavyCostFunctionForTiFlashPrefetch(cond) {
//             continue
//         }
//
// If the number of columns is larger than columnCountThreshold,
// the possibility of the selectivity of the condition is larger than the threshold is very small.
// Skip.
//         columns := expression.ExtractColumns(cond)
//         if len(columns) <= columnCountThreshold {
//             code := transformColumnsToCode(columns, len(ts.Table.Columns))
//             groupMap[code] = append(groupMap[code], cond)
//         }
//     }
//
// Estimate the selectivity of each group and check if it is larger than the selectivityThreshold
//     var exprGroups []expressionGroup
//     for _, group := range groupMap {
//         selectivity, err := cardinality.Selectivity(sctx, ts.TblColHists, group, nil)
//         if err != nil {
//             logutil.BgLogger().Warn("calculate selectivity failed, do not push down the conditions group", zap.Error(err))
//             continue
//         }
//         if selectivity <= selectivityThreshold {
//             exprGroups = append(exprGroups, expressionGroup{exprs: group, selectivity: selectivity})
//         }
//     }
//
// Keep the group order deterministic when selectivity and group size tie.
//     slices.SortStableFunc(exprGroups, func(x, y expressionGroup) int {
//         if diff := cmp.Compare(x.selectivity, y.selectivity); diff != 0 {
//             return diff
//         }
//         return compareExpressionGroups(x, y)
//     })
//
//     return exprGroups
// }
//
// withHeavyCostFunctionForTiFlashPrefetch is used to check if the condition contain heavy cost functions.
// @param: cond: filter condition
// @note: heavy cost functions are functions that may cause a lot of memory allocation or disk IO.
// 对应 Go 同名函数或方法；保留原参数顺序、主要分支和返回值形状。
// pub fn withHeavyCostFunctionForTiFlashPrefetch(cond expression.Expression) bool {
//     if sf, ok := cond.(*expression.ScalarFunction); ok {
//         switch sf.FuncName.L {
//         case ast.LogicAnd, ast.LogicOr:
//             return withHeavyCostFunctionForTiFlashPrefetch(sf.GetArgs()[0]) && withHeavyCostFunctionForTiFlashPrefetch(sf.GetArgs()[1])
//         case ast.UnaryNot:
//             return withHeavyCostFunctionForTiFlashPrefetch(sf.GetArgs()[0])
// JSON functions
//         case ast.JSONArray,
//             ast.JSONArrayAppend,
//             ast.JSONArrayInsert,
//             ast.JSONContains,
//             ast.JSONContainsPath,
//             ast.JSONDepth,
//             ast.JSONExtract,
//             ast.JSONInsert,
//             ast.JSONKeys,
//             ast.JSONLength,
//             ast.JSONMemberOf,
//             ast.JSONMerge,
//             ast.JSONMergePatch,
//             ast.JSONMergePreserve,
//             ast.JSONObject,
//             ast.JSONOverlaps,
//             ast.JSONPretty,
//             ast.JSONQuote,
//             ast.JSONRemove,
//             ast.JSONReplace,
//             ast.JSONSchemaValid,
//             ast.JSONSearch,
//             ast.JSONSet,
//             ast.JSONStorageFree,
//             ast.JSONStorageSize,
//             ast.JSONType,
//             ast.JSONUnquote,
//             ast.JSONValid:
//             return true
// some time functions
//         case ast.AddDate, ast.AddTime, ast.ConvertTz, ast.DateLiteral, ast.DateAdd, ast.DateFormat, ast.FromUnixTime, ast.GetFormat, ast.UTCTimestamp:
//             return true
//         case ast.DateSub, ast.DateDiff, ast.DayOfYear, ast.Extract, ast.FromDays, ast.TimestampLiteral, ast.TimestampAdd, ast.UnixTimestamp:
//             return true
//         case ast.LocalTimestamp, ast.MakeDate, ast.MakeTime, ast.MonthName, ast.PeriodAdd, ast.PeriodDiff, ast.Quarter, ast.SecToTime, ast.ToSeconds:
//             return true
//         case ast.StrToDate, ast.SubDate, ast.SubTime, ast.TimeLiteral, ast.TimeFormat, ast.TimeToSec, ast.TimeDiff, ast.TimestampDiff:
//             return true
// regexp functions
//         case ast.Regexp, ast.RegexpLike, ast.RegexpReplace, ast.RegexpSubstr, ast.RegexpInStr:
//             return true
// TODO: add more heavy cost functions
//         }
//     }
//     return false
// }
//
// predicatePushDownToTableScan is used to push down the some filter conditions to the tablescan.
// @param: sctx: the session context
// @param: conds: the filter conditions
// @param: ts: the PhysicalTableScan to be pushed down to
// 对应 Go 同名函数或方法；保留原参数顺序、主要分支和返回值形状。
// pub fn predicatePushDownToTableScan(sctx base.PlanContext, conds []expression.Expression, ts *PhysicalTableScan) {
//     if ts.hasFullTextIndexPushDown() {
//         return
//     }
// group the conditions by columns and sort them by selectivity
//     sortedConds := groupByColumnsSortBySelectivity(sctx, conds, ts)
//
//     selectedConds := make([]expression.Expression, 0, len(conds))
//     selectedIncome := 0.0
//     selectedColumnCount := 0
//     selectedSelectivity := 1.0
//     totalColumnCount := len(ts.Columns)
//     tableRowCount := ts.StatsInfo().RowCount
//
//     for _, exprGroup := range sortedConds {
//         mergedConds := append(selectedConds, exprGroup.exprs...)
//         selectivity, err := cardinality.Selectivity(sctx, ts.TblColHists, mergedConds, nil)
//         if err != nil {
//             logutil.BgLogger().Warn("calculate selectivity failed, do not push down the conditions group", zap.Error(err))
//             continue
//         }
//         colCnt := expression.ExtractColumnSet(mergedConds...).Len()
//         income := (1 - selectivity) * tableRowCount
// If selectedColumnCount does not change,
// or the increase of the number of filtered rows is greater than tiflashDataPackSize and the income increases, push down the conditions.
//         if colCnt == selectedColumnCount || (income > tiflashDataPackSize && income*(float64(totalColumnCount)-float64(colCnt)) > selectedIncome) {
//             selectedConds = mergedConds
//             selectedColumnCount = colCnt
//             selectedIncome = income * (float64(totalColumnCount) - float64(colCnt))
//             selectedSelectivity = selectivity
//         } else if income < tiflashDataPackSize {
// If the increase of the number of filtered rows is less than tiflashDataPackSize,
// break the loop to reduce the cost of calculating selectivity.
//             break
//         }
//     }
//
//     if len(selectedConds) == 0 {
//         return
//     }
// add the pushed down conditions to table scan
//     ts.LateMaterializationFilterCondition = selectedConds
//     ts.LateMaterializationSelectivity = selectedSelectivity
// }
//
// isPredicateSimpleCompare is used to check if the condition is a simple comparison predicate
// which only contains >, >=, =, !=, <=, <, in.
// 对应 Go 同名函数或方法；保留原参数顺序、主要分支和返回值形状。
// pub fn isPredicateSimpleCompare(cond expression.Expression) bool {
//     if sf, ok := cond.(*expression.ScalarFunction); ok {
//         switch sf.FuncName.L {
//         case ast.EQ, ast.GE, ast.LE, ast.LT, ast.GT, ast.In:
//             return true
//         case ast.LogicAnd, ast.LogicOr:
//             return isPredicateSimpleCompare(sf.GetArgs()[0]) && isPredicateSimpleCompare(sf.GetArgs()[1])
//         case ast.UnaryNot:
//             return isPredicateSimpleCompare(sf.GetArgs()[0])
//         }
//     }
//     return false
// }
//
// handleTiFlashPredicatePushDown is used to handle the TiFlash predicate push down.
// 1. Whether to use the inverted index.
// 2. Whether to push down the conditions to the table scan.
// 对应 Go 同名函数或方法；保留原参数顺序、主要分支和返回值形状。
// pub fn handleTiFlashPredicatePushDown(pctx base.PlanContext, ts *PhysicalTableScan, indexHints []*ast.IndexHint) {
// When the table is small, there is no need to push down the conditions.
//     if ts.TblColHists.RealtimeCount <= tiflashDataPackSize || ts.KeepOrder || len(ts.FilterCondition) == 0 {
//         return
//     }
//
// Currently, TiFlash does not support vector index with predicates.
// So for now, one DataSource only contains one TiFlash !keepOrder path,
// which means that the following code will only be executed once.
// TODO: If there are multiple TiFlash !keepOrder paths in one DataSource in the future,
// we need to move the following code to another place.
// Since caculating selectivity is expensive, we need to avoid duplicate execution.
//     for _, index := range ts.UsedColumnarIndexes {
//         if index.IndexInfo.VectorInfo != nil {
//             panic("TiFlash does not support vector index with pedicates")
//         }
//     }
//
// Consider use index hints
//     indexMap := make(map[string]int, len(ts.Table.Indices))
//     for _, hint := range indexHints {
//         if hint.HintScope != ast.HintForScan {
//             continue
//         }
//
//         switch hint.HintType {
//         case ast.HintUse:
//             for _, name := range hint.IndexNames {
//                 if indexMap[name.L] != math.MaxInt {
//                     indexMap[name.L]++
//                 }
//             }
//         case ast.HintIgnore:
//             for _, name := range hint.IndexNames {
//                 if indexMap[name.L] != math.MaxInt {
//                     indexMap[name.L]--
//                 }
//             }
//         case ast.HintForce:
//             for _, name := range hint.IndexNames {
//                 indexMap[name.L] = math.MaxInt
//             }
//         default:
//             continue
//         }
//     }
//
//     indexedColumnNameToIndexInfoMap := make(map[string]*model.IndexInfo, len(ts.Table.Indices))
//     for _, index := range ts.Table.Indices {
//         if index.State == model.StatePublic && index.InvertedInfo != nil {
//             if len(indexHints) > 0 && indexMap[index.Name.L] <= 0 {
//                 continue
//             }
// inverted index only support one column
//             indexedColumnNameToIndexInfoMap[index.Columns[0].Name.L] = index
//         }
//     }
//
//     selectedColumns := make(map[string]bool, len(indexedColumnNameToIndexInfoMap))
//     selectedConditions := make([]expression.Expression, 0, len(ts.FilterCondition))
//     columnNames := make([]string, 0, 4)
//     for _, cond := range ts.FilterCondition {
// 1. The predicate only contains >, >=, =, !=, <=, <, in.
//         if !isPredicateSimpleCompare(cond) {
//             continue
//         }
// 2. The columns of predicate all have inverted index.
//         allHaveInvertedIndex := true
//         columns := expression.ExtractColumns(cond)
//         columnNames = slices.Grow(columnNames, len(columns))
//         for _, col := range columns {
//             parts := strings.Split(col.OrigName, ".")
//             columnNames = append(columnNames, strings.ToLower(parts[len(parts)-1]))
//         }
//         for _, colName := range columnNames {
//             if _, ok := indexedColumnNameToIndexInfoMap[colName]; !ok {
//                 allHaveInvertedIndex = false
//                 break
//             }
//         }
//         if !allHaveInvertedIndex {
//             continue
//         }
// 3. The selectivity of the predicate is less than 60%.
//         selectivity, err := cardinality.Selectivity(pctx, ts.TblColHists, []expression.Expression{cond}, nil)
//         if err != nil {
//             logutil.BgLogger().Warn("calculate selectivity failed", zap.Error(err))
//             continue
//         }
//         if selectivity > selectivityThreshold {
//             continue
//         }
// all passed, add the columns to selected
//         for _, colName := range columnNames {
//             selectedColumns[colName] = true
//         }
//         selectedConditions = append(selectedConditions, cond)
//         columnNames = columnNames[:0] // reset columnNames slice to avoid unnecessary memory allocation
//     }
//
//     for colName := range selectedColumns {
//         if index, ok := indexedColumnNameToIndexInfoMap[colName]; ok {
//             ts.UsedColumnarIndexes = append(ts.UsedColumnarIndexes, buildInvertedIndexExtra(index))
//         }
//     }
//
//     for colName, index := range indexedColumnNameToIndexInfoMap {
//         if _, ok := selectedColumns[colName]; !ok && indexMap[index.Name.L] == math.MaxInt {
// if the index is not used, but the index hint is force use, add it to the used indexes
//             ts.UsedColumnarIndexes = append(ts.UsedColumnarIndexes, buildInvertedIndexExtra(index))
//         }
//     }
//
// if EnableLateMaterialization is set, try to push down some predicates to table scan
//     if len(ts.FilterCondition) > len(selectedConditions) && pctx.GetSessionVars().EnableLateMaterialization {
//         remaining := make([]expression.Expression, 0, len(ts.FilterCondition)-len(selectedConditions))
//         for _, cond := range ts.FilterCondition {
//             if !expression.Contains(pctx.GetExprCtx().GetEvalCtx(), selectedConditions, cond) {
//                 remaining = append(remaining, cond)
//             }
//         }
//         predicatePushDownToTableScan(pctx, remaining, ts)
//     }
//
// Update the row count of table scan.
//     if len(selectedConditions)+len(ts.LateMaterializationFilterCondition) != 0 {
//         selectedConditions = append(selectedConditions, ts.LateMaterializationFilterCondition...)
//         selectivity, err := cardinality.Selectivity(ts.SCtx(), ts.TblColHists, selectedConditions, nil)
//         if err != nil {
//             logutil.BgLogger().Warn("calculate selectivity failed", zap.Error(err))
//             selectivity = selectivityThreshold
//         }
//         ts.SetStats(ts.StatsInfo().Scale(ts.SCtx().GetSessionVars(), selectivity))
// just to make explain result stable
//         slices.SortFunc(ts.UsedColumnarIndexes, func(lhs, rhs *ColumnarIndexExtra) int {
//             return cmp.Compare(lhs.IndexInfo.ID, rhs.IndexInfo.ID)
//         })
//     }
// }
// */
use crate::physical_common_plans::PhysicalExpr;
use base::PhysicalPlan as _;
use base::Plan as _;
use expression::Expression as _;
use std::collections::{BTreeMap, BTreeSet};

struct CardinalityContextAdapter<'a>(&'a dyn base::PlanContext);

impl cardinality::CardinalityContext for CardinalityContextAdapter<'_> {
    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        self.0.GetSessionVars()
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        self.0.GetExprCtx()
    }

    fn GetRangerCtx(&self) -> &rangerctx::RangerContext<'_> {
        self.0.GetRangerCtx()
    }
}

/// 选择率阈值：高于此值的谓词分组不再参与晚期物化候选。
pub const SELECTIVITY_THRESHOLD: f64 = 0.6;
/// TiFlash 默认数据块（pack）行数，用于收益是否值得下推的下限参照。
pub const TIFLASH_DATA_PACK_SIZE: f64 = 8192.0;
/// 单组谓词涉及列数上限；超出则不进入分组候选。
pub const COLUMN_COUNT_THRESHOLD: usize = 3;

/// Go handleTiFlashPredicatePushDown 的真实物理扫描适配：当扫描超过一个 pack、
/// 不要求顺序且有过滤时，按列分组和估算收益挑选延迟物化条件。
pub fn handle_physical_tiflash_late_materialization(scan: &mut crate::PhysicalTableScan) {
    if scan.StoreType != kv::StoreType::TiFlash
        || scan.KeepOrder
        || scan.FilterCondition.is_empty()
        || !scan.s_ctx().GetSessionVars().EnableLateMaterialization
    {
        return;
    }

    let histograms = scan
        .TblColHists
        .as_deref()
        .and_then(|histograms| histograms.downcast_ref::<statistics::HistColl>())
        .or_else(|| {
            scan.stats_info()
                .HistColl
                .as_deref()
                .and_then(|histograms| histograms.downcast_ref::<statistics::HistColl>())
        });
    let Some(histograms) = histograms else {
        return;
    };
    if histograms.RealtimeCount as f64 <= TIFLASH_DATA_PACK_SIZE {
        return;
    }

    struct ExpressionGroup {
        expressions: Vec<expression::ExprBox>,
        selectivity: f64,
    }
    let context = CardinalityContextAdapter(scan.s_ctx().as_ref());
    let estimate = |expressions: &[expression::ExprBox]| {
        cardinality::Selectivity(&context, histograms, expressions, &[]).ok()
    };

    let mut grouped = BTreeMap::<Vec<i64>, Vec<expression::ExprBox>>::new();
    for condition in &scan.FilterCondition {
        if with_heavy_cost_function_expression(condition.as_ref()) {
            continue;
        }
        let mut columns =
            expression::ExtractColumnsMapFromExpressions(|_| true, std::slice::from_ref(condition))
                .into_keys()
                .collect::<Vec<_>>();
        columns.sort_unstable();
        if columns.len() <= COLUMN_COUNT_THRESHOLD {
            grouped
                .entry(columns)
                .or_default()
                .push(condition.CloneExpr());
        }
    }

    let mut groups = grouped
        .into_values()
        .filter_map(|expressions| {
            let selectivity = estimate(&expressions)?;
            (selectivity <= SELECTIVITY_THRESHOLD).then_some(ExpressionGroup {
                expressions,
                selectivity,
            })
        })
        .collect::<Vec<_>>();
    groups.sort_by(|left, right| {
        left.selectivity
            .partial_cmp(&right.selectivity)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.expressions.len().cmp(&right.expressions.len()))
            .then_with(|| {
                left.expressions
                    .iter()
                    .zip(&right.expressions)
                    .map(|(left, right)| right.HashCode().cmp(&left.HashCode()))
                    .find(|ordering| *ordering != std::cmp::Ordering::Equal)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    let mut selected = Vec::<expression::ExprBox>::new();
    let mut selected_income = 0.0;
    let mut selected_columns = 0;
    let mut selected_selectivity = 1.0;
    let row_count = scan.stats_info().RowCount;
    for group in groups {
        let mut merged = selected
            .iter()
            .map(|condition| condition.CloneExpr())
            .collect::<Vec<_>>();
        merged.extend(
            group
                .expressions
                .iter()
                .map(|condition| condition.CloneExpr()),
        );
        let Some(selectivity) = estimate(&merged) else {
            continue;
        };
        let column_count = expression::ExtractColumnsMapFromExpressions(|_| true, &merged).len();
        let income = (1.0 - selectivity) * row_count;
        let adjusted_income = income * scan.Columns.len().saturating_sub(column_count) as f64;
        if column_count == selected_columns
            || (income > TIFLASH_DATA_PACK_SIZE && adjusted_income > selected_income)
        {
            selected = merged;
            selected_columns = column_count;
            selected_income = adjusted_income;
            selected_selectivity = selectivity;
        } else if income < TIFLASH_DATA_PACK_SIZE {
            break;
        }
    }

    if !selected.is_empty() {
        scan.LateMaterializationFilterCondition = selected;
        scan.LateMaterializationSelectivity = selected_selectivity;
        let mut stats = scan.stats_info().clone();
        stats.RowCount *= selected_selectivity;
        scan.set_stats(stats);
    }
}

fn with_heavy_cost_function_expression(condition: &dyn expression::Expression) -> bool {
    let Some(function) = condition.as_scalar_function() else {
        return false;
    };
    let args = function.GetArgs();
    match function.FuncName.L.as_str() {
        "and" | "or" if args.len() == 2 => {
            with_heavy_cost_function_expression(args[0].as_ref())
                && with_heavy_cost_function_expression(args[1].as_ref())
        }
        "not" if args.len() == 1 => with_heavy_cost_function_expression(args[0].as_ref()),
        "json_array"
        | "json_array_append"
        | "json_array_insert"
        | "json_contains"
        | "json_contains_path"
        | "json_depth"
        | "json_extract"
        | "json_insert"
        | "json_keys"
        | "json_length"
        | "json_member_of"
        | "json_merge"
        | "json_merge_patch"
        | "json_merge_preserve"
        | "json_object"
        | "json_overlaps"
        | "json_pretty"
        | "json_quote"
        | "json_remove"
        | "json_replace"
        | "json_schema_valid"
        | "json_search"
        | "json_set"
        | "json_storage_free"
        | "json_storage_size"
        | "json_type"
        | "json_unquote"
        | "json_valid"
        | "add_date"
        | "add_time"
        | "convert_tz"
        | "date_literal"
        | "date_add"
        | "date_format"
        | "from_unixtime"
        | "get_format"
        | "utc_timestamp"
        | "date_sub"
        | "date_diff"
        | "day_of_year"
        | "extract"
        | "from_days"
        | "timestamp_literal"
        | "timestamp_add"
        | "unix_timestamp"
        | "local_timestamp"
        | "make_date"
        | "make_time"
        | "month_name"
        | "period_add"
        | "period_diff"
        | "quarter"
        | "sec_to_time"
        | "to_seconds"
        | "str_to_date"
        | "sub_date"
        | "sub_time"
        | "time_literal"
        | "time_format"
        | "time_to_sec"
        | "time_diff"
        | "timestamp_diff"
        | "regexp"
        | "regexp_like"
        | "regexp_replace"
        | "regexp_substr"
        | "regexp_instr" => true,
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// 索引提示类型：Use / Ignore / Force。
pub enum HintType {
    Use,
    Ignore,
    Force,
}
#[derive(Clone, Debug, PartialEq, Eq)]
/// 作用于扫描范围的索引提示：作用域、类型与索引名列表。
pub struct IndexHint {
    pub scan_scope: bool,
    pub hint_type: HintType,
    pub index_names: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
/// 列存索引种类：倒排（Inverted）或向量（Vector）。
pub enum ColumnarIndexKind {
    Inverted,
    Vector,
}
#[derive(Clone, Debug, PartialEq, Eq)]
/// 列存索引元数据：编号、名称、覆盖列与是否公开可用。
pub struct ColumnarIndex {
    pub id: i64,
    pub name: String,
    pub column_ids: Vec<i64>,
    pub kind: ColumnarIndexKind,
    pub public: bool,
}
#[derive(Clone, Debug, PartialEq)]
/// TiFlash 表扫描节点的规划侧状态。
///
/// 含过滤条件、行数估计、可用/已用列存索引及晚期物化相关字段。
pub struct TiFlashTableScan {
    pub column_ids: Vec<i64>,
    pub filters: Vec<PhysicalExpr>,
    pub realtime_count: f64,
    pub row_count: f64,
    pub keep_order: bool,
    pub indexes: Vec<ColumnarIndex>,
    pub used_indexes: Vec<ColumnarIndex>,
    pub late_materialization: bool,
    pub late_filter_conditions: Vec<PhysicalExpr>,
    pub late_selectivity: f64,
    pub expression_selectivity: BTreeMap<String, f64>,
    pub full_text_push_down: bool,
}

#[derive(Clone, Debug, PartialEq)]
/// 按列集合分组的谓词及其综合选择率。
struct ExpressionGroup {
    expressions: Vec<PhysicalExpr>,
    selectivity: f64,
}

/// 将列 ID 集合编码为与表列宽等长的 `0/1` 位串，用作分组键。
pub fn transform_columns_to_code(columns: &BTreeSet<i64>, table_columns: usize) -> String {
    if columns.is_empty() {
        return "0".into();
    }
    let mut code = vec![b'0'; table_columns];
    for column in columns {
        if *column >= 0 && (*column as usize) < table_columns {
            code[*column as usize] = b'1';
        }
    }
    String::from_utf8(code).expect("binary column code")
}

/// 用扫描侧缓存的表达式选择率乘积估计过滤后剩余比例。
fn estimate_selectivity(scan: &TiFlashTableScan, expressions: &[PhysicalExpr]) -> f64 {
    expressions
        .iter()
        .map(|expression| {
            scan.expression_selectivity
                .get(&format!("{expression:?}"))
                .copied()
                .unwrap_or(0.8)
        })
        .product::<f64>()
        .clamp(0.0, 1.0)
}

/// 按列集合分组谓词，过滤高代价/高选择率组后按选择率升序排序。
fn group_by_columns_sort_by_selectivity(
    conditions: &[PhysicalExpr],
    scan: &TiFlashTableScan,
) -> Vec<ExpressionGroup> {
    let mut groups = BTreeMap::<String, Vec<PhysicalExpr>>::new();
    for condition in conditions {
        if with_heavy_cost_function(condition) {
            continue;
        }
        let columns = condition.columns();
        if columns.len() <= COLUMN_COUNT_THRESHOLD {
            groups
                .entry(transform_columns_to_code(&columns, scan.column_ids.len()))
                .or_default()
                .push(condition.clone());
        }
    }
    let mut result: Vec<_> = groups
        .into_values()
        .filter_map(|expressions| {
            let selectivity = estimate_selectivity(scan, &expressions);
            (selectivity <= SELECTIVITY_THRESHOLD).then_some(ExpressionGroup {
                expressions,
                selectivity,
            })
        })
        .collect();
    result.sort_by(|left, right| {
        left.selectivity
            .partial_cmp(&right.selectivity)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.expressions.len().cmp(&right.expressions.len()))
            .then_with(|| {
                format!("{:?}", right.expressions).cmp(&format!("{:?}", left.expressions))
            })
    });
    result
}

/// 判断谓词是否含 JSON/日期/正则等高代价函数，此类谓词不参与分组下推。
pub fn with_heavy_cost_function(condition: &PhysicalExpr) -> bool {
    let PhysicalExpr::Scalar { function, args } = condition else {
        return false;
    };
    match function.as_str() {
        "and" | "or" if args.len() == 2 => {
            with_heavy_cost_function(&args[0]) && with_heavy_cost_function(&args[1])
        }
        "not" if args.len() == 1 => with_heavy_cost_function(&args[0]),
        "json_array"
        | "json_array_append"
        | "json_array_insert"
        | "json_contains"
        | "json_contains_path"
        | "json_depth"
        | "json_extract"
        | "json_insert"
        | "json_keys"
        | "json_length"
        | "json_member_of"
        | "json_merge"
        | "json_merge_patch"
        | "json_merge_preserve"
        | "json_object"
        | "json_overlaps"
        | "json_pretty"
        | "json_quote"
        | "json_remove"
        | "json_replace"
        | "json_schema_valid"
        | "json_search"
        | "json_set"
        | "json_storage_free"
        | "json_storage_size"
        | "json_type"
        | "json_unquote"
        | "json_valid"
        | "add_date"
        | "add_time"
        | "convert_tz"
        | "date_literal"
        | "date_add"
        | "date_format"
        | "from_unixtime"
        | "get_format"
        | "utc_timestamp"
        | "date_sub"
        | "date_diff"
        | "day_of_year"
        | "extract"
        | "from_days"
        | "timestamp_literal"
        | "timestamp_add"
        | "unix_timestamp"
        | "local_timestamp"
        | "make_date"
        | "make_time"
        | "month_name"
        | "period_add"
        | "period_diff"
        | "quarter"
        | "sec_to_time"
        | "to_seconds"
        | "str_to_date"
        | "sub_date"
        | "sub_time"
        | "time_literal"
        | "time_format"
        | "time_to_sec"
        | "time_diff"
        | "timestamp_diff"
        | "regexp"
        | "regexp_like"
        | "regexp_replace"
        | "regexp_substr"
        | "regexp_instr" => true,
        _ => false,
    }
}

/// 按收益贪心挑选晚期物化过滤条件，写回扫描节点的 late_filter 字段。
fn predicate_push_down_to_table_scan(conditions: &[PhysicalExpr], scan: &mut TiFlashTableScan) {
    if scan.full_text_push_down {
        return;
    }
    let groups = group_by_columns_sort_by_selectivity(conditions, scan);
    let mut selected = Vec::new();
    let mut selected_income = 0.0;
    let mut selected_columns = 0;
    let mut selected_selectivity = 1.0;
    for group in groups {
        let mut merged = selected.clone();
        merged.extend(group.expressions);
        let selectivity = estimate_selectivity(scan, &merged);
        let column_count = merged
            .iter()
            .flat_map(PhysicalExpr::columns)
            .collect::<BTreeSet<_>>()
            .len();
        let income = (1.0 - selectivity) * scan.row_count;
        let adjusted = income * (scan.column_ids.len().saturating_sub(column_count)) as f64;
        if column_count == selected_columns
            || (income > TIFLASH_DATA_PACK_SIZE && adjusted > selected_income)
        {
            selected = merged;
            selected_columns = column_count;
            selected_income = adjusted;
            selected_selectivity = selectivity;
        } else if income < TIFLASH_DATA_PACK_SIZE {
            break;
        }
    }
    if !selected.is_empty() {
        scan.late_filter_conditions = selected;
        scan.late_selectivity = selected_selectivity;
    }
}

/// 判断谓词是否为可下推到倒排索引的简单比较（及 and/or/not 组合）。
pub fn is_predicate_simple_compare(condition: &PhysicalExpr) -> bool {
    let PhysicalExpr::Scalar { function, args } = condition else {
        return false;
    };
    match function.as_str() {
        "eq" | "ge" | "le" | "lt" | "gt" | "in" => true,
        "and" | "or" if args.len() == 2 => args.iter().all(is_predicate_simple_compare),
        "not" if args.len() == 1 => is_predicate_simple_compare(&args[0]),
        _ => false,
    }
}

/// TiFlash 谓词下推入口：结合索引提示选倒排索引，并可选触发晚期物化。
///
/// 向量索引与谓词并存时返回错误；成功后按选择率缩放估算行数。
pub fn handle_tiflash_predicate_push_down(
    scan: &mut TiFlashTableScan,
    hints: &[IndexHint],
) -> Result<(), String> {
    if scan.realtime_count <= TIFLASH_DATA_PACK_SIZE || scan.keep_order || scan.filters.is_empty() {
        return Ok(());
    }
    if scan
        .used_indexes
        .iter()
        .any(|index| index.kind == ColumnarIndexKind::Vector)
    {
        return Err("TiFlash does not support vector index with pedicates".into());
    }
    let mut hint_score = BTreeMap::<String, i32>::new();
    for hint in hints.iter().filter(|hint| hint.scan_scope) {
        for name in &hint.index_names {
            let entry = hint_score.entry(name.to_ascii_lowercase()).or_default();
            match hint.hint_type {
                HintType::Use if *entry != i32::MAX => *entry += 1,
                HintType::Ignore if *entry != i32::MAX => *entry -= 1,
                HintType::Force => *entry = i32::MAX,
                _ => {}
            }
        }
    }
    // Go's column-name map keeps the last eligible index for a column.
    let candidates_by_column: BTreeMap<_, _> = scan
        .indexes
        .iter()
        .filter(|index| {
            index.public
                && index.kind == ColumnarIndexKind::Inverted
                && index.column_ids.len() == 1
                && (hints.is_empty()
                    || hint_score
                        .get(&index.name.to_ascii_lowercase())
                        .copied()
                        .unwrap_or(0)
                        > 0)
        })
        .map(|index| (index.column_ids[0], index.clone()))
        .collect();
    let candidates: Vec<_> = candidates_by_column.into_values().collect();
    let indexed_columns: BTreeSet<_> = candidates
        .iter()
        .flat_map(|index| index.column_ids.iter().copied())
        .collect();
    let mut selected_conditions = Vec::new();
    let mut selected_columns = BTreeSet::new();
    for condition in &scan.filters {
        if !is_predicate_simple_compare(condition) {
            continue;
        }
        let columns = condition.columns();
        if !columns.is_subset(&indexed_columns)
            || estimate_selectivity(scan, std::slice::from_ref(condition)) > SELECTIVITY_THRESHOLD
        {
            continue;
        }
        selected_columns.extend(columns);
        selected_conditions.push(condition.clone());
    }
    for index in candidates {
        if index
            .column_ids
            .iter()
            .any(|column| selected_columns.contains(column))
            || hint_score.get(&index.name.to_ascii_lowercase()) == Some(&i32::MAX)
        {
            scan.used_indexes.push(index);
        }
    }
    if scan.late_materialization && scan.filters.len() > selected_conditions.len() {
        let remaining: Vec<_> = scan
            .filters
            .iter()
            .filter(|condition| !selected_conditions.contains(condition))
            .cloned()
            .collect();
        predicate_push_down_to_table_scan(&remaining, scan);
    }
    if !selected_conditions.is_empty() || !scan.late_filter_conditions.is_empty() {
        selected_conditions.extend(scan.late_filter_conditions.clone());
        let selectivity = estimate_selectivity(scan, &selected_conditions);
        scan.row_count *= if selectivity.is_finite() {
            selectivity
        } else {
            SELECTIVITY_THRESHOLD
        };
        scan.used_indexes.sort_by_key(|index| index.id);
    }
    Ok(())
}
