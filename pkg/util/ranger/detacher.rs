// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 条件拆分（detach）与索引/分区 Range 构造，对齐 detacher.go。
//
// 从 WHERE 谓词中抽出可用于索引扫描的 access 条件，并构造多列 Range；
// 残留条件作为 filter。覆盖 CNF/DNF、EQ/IN、前缀索引与分片索引 GC 列补全。

// 条件拆分及索引、分区 range 构造，对齐 detacher.go。

#![allow(dead_code)]
#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![allow(unused_variables)]

use crate::checker_impl::conditionChecker;
use crate::points_impl::{builder, point};
use crate::ranger_impl::{
    convertStringFTToBinaryCollate, hasPrefix, newFieldType, points2EqOrInCond, points2Ranges,
};
use crate::{
    AppendRanges2PointRanges, FullRange, Ranges, UnionRanges, ast, chunk, collate, errors, mysql,
    types,
};

type GoResult<T> = Result<T, errors::Error>;

// detachColumnCNFConditions detaches the condition for calculating range from the other conditions.
// Please make sure that the top level is CNF form.
// detachColumnCNFConditions 对应 Go 中从 CNF 顶层条件里拆出单列 access 条件的函数。
// 返回值第一组用于构造 range，第二组仍需作为 filter 保留；DNF 子树会递归拆分后再重建。
fn detachColumnCNFConditions(
    sctx: &dyn expression::BuildContext,
    conditions: Vec<expression::ExprBox>,
    checker: &mut conditionChecker,
) -> (Vec<expression::ExprBox>, Vec<expression::ExprBox>) {
    let mut accessConditions = Vec::new();
    let mut filterConditions = Vec::new();

    for cond in conditions {
        // Go 里先识别 CNF 顶层里的 OR 子表达式；OR 内部必须以 DNF 方式整体处理，
        // 否则只抽出一部分分支会改变 SQL 语义。
        if let Some(sf) = cond.as_scalar_function() {
            if sf.FuncName.L == ast::LogicOr {
                let dnfItems = expression::FlattenDNFConditions(sf);
                let (columnDNFItems, hasResidual) =
                    detachColumnDNFConditions(sctx, dnfItems, checker);
                // If this CNF has expression that cannot be resolved as access condition, then the total DNF expression
                // should be also appended into filter condition.
                if hasResidual {
                    filterConditions.push(cond.clone());
                }
                if columnDNFItems.is_empty() {
                    continue;
                }
                if let Some(rebuildDNF) = expression::ComposeDNFCondition(sctx, &columnDNFItems) {
                    accessConditions.push(rebuildDNF);
                }
                continue;
            }
        }

        // 普通 CNF item 直接交给 conditionChecker；shouldReserve 表示即使可建 range，
        // 也因为前缀索引、collation 等原因仍需保留为 filter 做精确判断。
        let (isAccessCond, shouldReserve) = checker.check(cond.as_ref());
        if !isAccessCond {
            filterConditions.push(cond);
            continue;
        }
        accessConditions.push(cond.clone());
        if shouldReserve {
            filterConditions.push(cond);
        }
    }

    (accessConditions, filterConditions)
}

// detachColumnDNFConditions detaches the condition for calculating range from the other conditions.
// Please make sure that the top level is DNF form.
// detachColumnDNFConditions 对应 Go 中处理 DNF 顶层条件的函数。
// 每个 OR 分支都必须至少能拆出 access 条件，否则整个 DNF 不能用于 range，只能作为 filter。
fn detachColumnDNFConditions(
    sctx: &dyn expression::BuildContext,
    conditions: Vec<expression::ExprBox>,
    checker: &mut conditionChecker,
) -> (Vec<expression::ExprBox>, bool) {
    let mut hasResidualConditions = false;
    let mut accessConditions = Vec::new();

    for cond in conditions {
        if let Some(sf) = cond.as_scalar_function() {
            if sf.FuncName.L == ast::LogicAnd {
                let cnfItems = expression::FlattenCNFConditions(sf);
                let (columnCNFItems, others) = detachColumnCNFConditions(sctx, cnfItems, checker);
                if !others.is_empty() {
                    hasResidualConditions = true;
                }
                // If one part of DNF has no access condition. Then this DNF cannot get range.
                if columnCNFItems.is_empty() {
                    return (Vec::new(), true);
                }
                if let Some(rebuildCNF) = expression::ComposeCNFCondition(sctx, &columnCNFItems) {
                    accessConditions.push(rebuildCNF);
                }
                continue;
            }
        }

        let (isAccessCond, shouldReserve) = checker.check(cond.as_ref());
        if !isAccessCond {
            return (Vec::new(), true);
        }
        accessConditions.push(cond);
        if shouldReserve {
            hasResidualConditions = true;
        }
    }

    (accessConditions, hasResidualConditions)
}

// getPotentialEqOrInColOffset checks if the expression is a eq/le/ge/lt/gt function that one side is constant and another is column or an
// in function which is `column in (constant list)`.
// If so, it will return the offset of this column in the slice, otherwise return -1 for not found.
// Since combining `x >= 2` and `x <= 2` can lead to an eq condition `x = 2`, we take le/ge/lt/gt into consideration.
// Notice: in points2EqOrInCond, when we convert points to acess condition reversely,
// we will build isnull func again from a single point range with null value,
// when we find the ref col is with not null flag, it will output zero constant
// which breaks the function check outside. That's why we abandon the nulleq range detecting,
// treat it as non-eq-in condition for later range build.
// getPotentialEqOrInColOffset 判断表达式是否能作为某个索引列的 EQ/IN/边界条件。
// 返回 Go 语义里的列下标；-1 表示不能归入等值或 IN 提取路径。
fn getPotentialEqOrInColOffset(
    sctx: &rangerctx::RangerContext,
    expr: expression::ExprBox,
    cols: Vec<expression::Column>,
) -> isize {
    let evalCtx = sctx.ExprCtx.GetEvalCtx();
    let Some(f) = expr.as_scalar_function() else {
        return -1;
    };
    let (_, collation) = expr.CharsetAndCollation();

    match f.FuncName.L.as_str() {
        ast::LogicOr => {
            let dnfItems = expression::FlattenDNFConditions(f);
            let mut offset: isize = -1;
            for dnfItem in dnfItems {
                let curOffset = getPotentialEqOrInColOffset(sctx, dnfItem, cols.clone());
                if curOffset == -1 {
                    return -1;
                }
                if offset != -1 && curOffset != offset {
                    return -1;
                }
                offset = curOffset;
            }
            offset
        }
        ast::EQ | ast::NullEQ | ast::LE | ast::GE | ast::LT | ast::GT => {
            // Go 代码允许 column 在左右任意一侧，idxConst 记录常量参数所在位置。
            let args = f.GetArgs();
            let mut idxConst = 1usize;
            let mut c = args[0].as_column();
            if c.is_none() {
                idxConst = 0;
                c = args[1].as_column();
                if c.is_none() {
                    return -1;
                }
            }
            let c = c.unwrap();

            // TODO: This collation-compatibility check rejects columns with binary-casted literals
            // (e.g. f = CAST('a' AS BINARY) on a non-binary string column), which causes the EQ/IN
            // extraction path to demote such predicates to filters. As a consequence, suffix index
            // columns on a composite index (e.g. index on (f,g) with f = CAST('a' AS BINARY) AND g = 1)
            // cannot be used as index equalities and are treated as filters as well. Relax this check
            // for the CAST(... AS BINARY) / binary-collation case so binary-cast equality predicates
            // can be treated as index-equalities, and add a regression test for the composite-index
            // scenario above. Fix in a later release.
            // 字符串列必须通过 collate 兼容性检查；否则 range 近似可能导致错误提取。
            if c.GetType(evalCtx).EvalType() == types::ETString
                && !collate::CompatibleCollate(c.GetType(evalCtx).GetCollate(), &collation)
            {
                return -1;
            }
            // 非整数列上的严格大小比较不能走后续等值/IN 合并路径。
            if (f.FuncName.L == ast::LT || f.FuncName.L == ast::GT)
                && c.GetType(evalCtx).EvalType() != types::ETInt
            {
                return -1;
            }

            let Some(constVal) = args[idxConst].as_constant() else {
                return -1;
            };

            let valResult = constVal.Eval(evalCtx, chunk::Row::default());
            if sctx.ExprCtx.ConnectionID() == 0 {
                intest::Assert(sctx.RegardNULLAsPoint, &[]);
            }
            let Ok(val) = valResult else {
                return -1;
            };
            if (!sctx.RegardNULLAsPoint && val.IsNull())
                || (f.FuncName.L == ast::NullEQ && val.IsNull())
            {
                // treat col<=>null as range scan instead of point get to avoid incorrect results
                // when nullable unique index has multiple matches for filter x is null
                return -1;
            }
            for (i, col) in cols.iter().enumerate() {
                // When cols are a generated expression col, compare them in terms of virtual expr.
                if col.EqualByExprAndID(evalCtx, c) {
                    return i as isize;
                }
            }
            -1
        }
        ast::In => {
            let args = f.GetArgs();
            let Some(c) = args[0].as_column() else {
                return -1;
            };
            if c.GetType(evalCtx).EvalType() == types::ETString
                && !collate::CompatibleCollate(c.GetType(evalCtx).GetCollate(), &collation)
            {
                return -1;
            }
            // IN 的每个候选值都必须是常量；包含表达式或列引用时不进入该快速路径。
            for arg in args.iter().skip(1) {
                if arg.as_constant().is_none() {
                    return -1;
                }
            }
            for (i, col) in cols.iter().enumerate() {
                if col.EqualColumn(c) {
                    return i as isize;
                }
            }
            -1
        }
        ast::IsNull => {
            let args = f.GetArgs();
            let Some(c) = args[0].as_column() else {
                return -1;
            };
            for (i, col) in cols.iter().enumerate() {
                if col.EqualColumn(c) {
                    return i as isize;
                }
            }
            -1
        }
        _ => -1,
    }
}

// cnfItemRangeResult 保存某个 CNF 子项构造出的 range 结果及其选择性指标。
// sameLenPointRanges 对应 Go 注释：所有 range 都是点范围，且列数一致。
#[derive(Clone)]
struct cnfItemRangeResult {
    rangeResult: Option<DetachRangeResult>,
    offset: usize,
    // sameLenPointRanges means that each range is point range and all of them have the same column numbers(i.e., maxColNum = minColNum).
    sameLenPointRanges: bool,
    maxColNum: usize,
    minColNum: usize,
}

// getCNFItemRangeResult 统计一个 DetachRangeResult 的点范围形态，供后续挑选最佳 CNF 子项。
fn getCNFItemRangeResult(
    sctx: &rangerctx::RangerContext,
    rangeResult: DetachRangeResult,
    offset: usize,
) -> cnfItemRangeResult {
    let mut sameLenPointRanges = true;
    let mut maxColNum = 0usize;
    let mut minColNum = 0usize;
    for (i, ran) in rangeResult.Ranges.iter().enumerate() {
        if !ran.IsPoint(sctx) {
            sameLenPointRanges = false;
        }
        if i == 0 {
            maxColNum = ran.LowVal.len();
            minColNum = ran.LowVal.len();
        } else {
            maxColNum = maxColNum.max(ran.LowVal.len());
            minColNum = minColNum.min(ran.LowVal.len());
        }
    }
    if minColNum != maxColNum {
        sameLenPointRanges = false;
    }
    cnfItemRangeResult {
        rangeResult: Some(rangeResult),
        offset,
        sameLenPointRanges,
        maxColNum,
        minColNum,
    }
}

// compareCNFItemRangeResult 比较两个 CNF 子项结果谁更值得采用。
// Go 语义优先选择列数更多的点范围；非点范围则看 min/max 列数。
fn compareCNFItemRangeResult(
    curResult: &cnfItemRangeResult,
    bestResult: &cnfItemRangeResult,
) -> bool {
    if curResult.sameLenPointRanges && bestResult.sameLenPointRanges {
        return curResult.minColNum > bestResult.minColNum;
    }
    if !curResult.sameLenPointRanges && !bestResult.sameLenPointRanges {
        if curResult.minColNum == bestResult.minColNum {
            return curResult.maxColNum > bestResult.maxColNum;
        }
        return curResult.minColNum > bestResult.minColNum;
    }
    // Point ranges is better than non-point ranges since we can append subsequent column ranges to point ranges.
    curResult.sameLenPointRanges
}

// mergeTwoCNFRanges merges two ranges results rangeResult and otherRangeResult.
// The main objective of this function is to apply intersection between these two
// ranges when possible. The overall logic is:
// - if rangeResult is empty then result is otherRangeResult
// - Skip intersection logic if it is turned off (Fix54337)
// - If either range is a subset then set result to the subset.
// - Result = intersection of the two ranges
// - Try heuristic to pick which range is better if intersections fails or if feature is off.
// mergeTwoCNFRanges 合并两个 CNF range 结果；Fix54337 打开时优先做 subset/intersection，
// 否则退回启发式选择。intersection 出错以 nil 表示，Go 代码也会转入启发式分支。
fn mergeTwoCNFRanges(
    sctx: &rangerctx::RangerContext,
    cond: expression::ExprBox,
    rangeResult: Option<cnfItemRangeResult>,
    otherRangeResult: Option<cnfItemRangeResult>,
) -> Option<cnfItemRangeResult> {
    let Some(mut mergedResult) = rangeResult else {
        return otherRangeResult;
    };

    let mut tryHeuristic = false;
    if let Some(mut otherRangeResult) = otherRangeResult {
        if mergedResult.rangeResult.is_some() && otherRangeResult.rangeResult.is_some() {
            if fixcontrol::GetBoolWithDefault(
                &sctx.OptimizerFixControl,
                fixcontrol::Fix54337,
                false,
            ) {
                let merged = mergedResult.rangeResult.as_ref().unwrap();
                let other = otherRangeResult.rangeResult.as_ref().unwrap();
                let mergedResultIsSubset = merged
                    .Ranges
                    .Subset(sctx.TypeCtx.clone(), other.Ranges.clone());
                // if mergedResult is a subset then do nothing
                if !mergedResultIsSubset {
                    let otherRangeResultIsSubset = other
                        .Ranges
                        .Subset(sctx.TypeCtx.clone(), merged.Ranges.clone());
                    // if otherRangeResult is subset (more selective) then make it mergedResult.
                    if otherRangeResultIsSubset {
                        mergedResult = otherRangeResult.clone();
                    } else {
                        // Try intersecting result of different conjuncts.
                        let intersection = other
                            .Ranges
                            .IntersectRanges(sctx.TypeCtx.clone(), merged.Ranges.clone());
                        // Skip intersection if an error occurred during the intersection computation.
                        if intersection.is_none() {
                            tryHeuristic = true;
                        } else if let Some(mergedRange) = mergedResult.rangeResult.as_mut() {
                            mergedRange.Ranges = intersection.unwrap();
                            mergedRange.AccessConds = AppendConditionsIfNotExist(
                                sctx.ExprCtx.GetEvalCtx(),
                                mergedRange.AccessConds.clone(),
                                vec![cond],
                            );
                        }
                    }
                }
            } else {
                tryHeuristic = true;
            }
        }

        // 启发式分支只在 intersection 未启用或失败时使用，保持 Go 的选择顺序。
        if tryHeuristic && compareCNFItemRangeResult(&otherRangeResult, &mergedResult) {
            mergedResult = otherRangeResult;
        }
    }
    Some(mergedResult)
}

// extractBestCNFItemRanges builds ranges for each CNF item from the input CNF expressions and returns the best CNF
// item ranges.
// e.g, for input CNF expressions ((a,b) in ((1,1),(2,2))) and a > 1 and ((a,b,c) in (1,1,1),(2,2,2))
// ((a,b,c) in (1,1,1),(2,2,2)) would be extracted.
// extractBestCNFItemRanges 对每个 CNF item 单独构造 range，并选出能覆盖更多列、选择性更好的结果。
// 这里刻意不合并连续 range，保留 Go 为后续追加列条件而维护点范围的策略。
fn extractBestCNFItemRanges(
    sctx: &rangerctx::RangerContext,
    conds: Vec<expression::ExprBox>,
    cols: Vec<expression::Column>,
    lengths: Vec<i32>,
    newTpSlice: Vec<types::FieldType>,
    rangeMaxSize: i64,
    convertToSortKey: bool,
) -> GoResult<(Option<cnfItemRangeResult>, Vec<Option<valueInfo>>)> {
    if conds.len() < 2 {
        return Ok((None, Vec::new()));
    }
    let mut bestRes: Option<cnfItemRangeResult> = None;
    let mut columnValues = vec![None; cols.len()];
    for (i, cond) in conds.iter().cloned().enumerate() {
        let tmpConds = vec![cond.clone()];
        if expression::ExtractColumns(cond.as_ref()).is_empty() {
            continue;
        }
        // When we build ranges for the CNF item, we choose not to merge consecutive ranges because we hope to get point
        // ranges here. See https://github.com/pingcap/tidb/issues/41572 for more details.
        // Here is an example. Assume that the index is `idx(a,b,c)` and the condition is `((a,b) in ((1,1),(1,2)) and c = 1`.
        // We build ranges for `(a,b) in ((1,1),(1,2))` and get `[1 1, 1 1] [1 2, 1 2]`, which are point ranges and we can
        // append `c = 1` to the point ranges. However, if we choose to merge consecutive ranges here, we get `[1 1, 1 2]`,
        // which are not point ranges, and we cannot append `c = 1` anymore.
        let res = detachCondAndBuildRangeRecursive(
            sctx,
            tmpConds,
            cols.clone(),
            lengths.clone(),
            newTpSlice.clone(),
            rangeMaxSize,
            convertToSortKey,
            false,
        )?;
        if res.Ranges.is_empty() {
            return Ok((
                Some(cnfItemRangeResult {
                    rangeResult: Some(res),
                    offset: i,
                    sameLenPointRanges: false,
                    maxColNum: 0,
                    minColNum: 0,
                }),
                Vec::new(),
            ));
        }
        // take the union of the two columnValues
        columnValues = unionColumnValues(columnValues, res.ColumnValues.clone());
        if res.AccessConds.is_empty() {
            continue;
        }
        let curRes = getCNFItemRangeResult(sctx, res, i);
        bestRes = mergeTwoCNFRanges(sctx, cond, bestRes, Some(curRes));
    }
    if let Some(best) = bestRes.as_mut() {
        if let Some(rangeResult) = best.rangeResult.as_mut() {
            rangeResult.IsDNFCond = false;
        }
    }
    Ok((bestRes, columnValues))
}

// unionColumnValues 合并两组列常量信息；Go 语义是 lhs 优先，只有 lhs 某列为空时才接收 rhs。
fn unionColumnValues(
    mut lhs: Vec<Option<valueInfo>>,
    rhs: Vec<Option<valueInfo>>,
) -> Vec<Option<valueInfo>> {
    if lhs.is_empty() {
        return rhs;
    }
    if !rhs.is_empty() {
        for (i, valInfo) in lhs.iter_mut().enumerate() {
            if i >= rhs.len() {
                break;
            }
            if valInfo.is_none() && rhs[i].is_some() {
                *valInfo = rhs[i].clone();
            }
        }
    }
    lhs
}

// Check which detach result is more selective. This function is called to choose between point ranges and the best CNF ranges.
// This is needed because sometimes the best CNF has full intersection and is more selective,
// and other times it is not when the intersection is not applied.
// chooseBetweenRangeAndPoint 在点范围结果和最佳 CNF 结果之间选择更强的 access 条件集合。
fn chooseBetweenRangeAndPoint(
    sctx: &rangerctx::RangerContext,
    r1: &mut DetachRangeResult,
    r2: Option<&cnfItemRangeResult>,
) {
    if fixcontrol::GetBoolWithDefault(&sctx.OptimizerFixControl, fixcontrol::Fix54337, false) {
        if !r1.Ranges.is_empty() {
            if let Some(r2) = r2 {
                if let Some(r2RangeResult) = r2.rangeResult.as_ref() {
                    let r1Minusr2 = removeConditions(
                        sctx.ExprCtx.GetEvalCtx(),
                        r1.AccessConds.clone(),
                        r2RangeResult.AccessConds.clone(),
                    );
                    let r2Minusr1 = removeConditions(
                        sctx.ExprCtx.GetEvalCtx(),
                        r2RangeResult.AccessConds.clone(),
                        r1.AccessConds.clone(),
                    );
                    // r2 is considered more selective (and more useful) than r1 if its AccessConds are a superset of r1's AccessConds.
                    // This means that r1.AccessConds minus r2.AccessConds should result in an empty set.
                    // The function `removeConditions` is used to perform this subtraction.
                    // For example, if A = {t1.a1 IN (44, 70, 76)} and B = {t1.a1 IN (44, 70, 76), (t1.a1 > 70 OR (t1.a1 = 70 AND t1.b1 > 41))},
                    // then A-B is empty and therefore B is a superset of A.
                    // Avoid the case when both r1 and r2 have the same AccessConds (r2Minusr1 is not empty).
                    if r1Minusr2.is_empty() && !r2Minusr1.is_empty() {
                        // Update final result and just update: Ranges, AccessConds and RemainedConds
                        r1.RemainedConds = removeConditions(
                            sctx.ExprCtx.GetEvalCtx(),
                            r1.RemainedConds.clone(),
                            r2RangeResult.AccessConds.clone(),
                        );
                        r1.Ranges = r2RangeResult.Ranges.clone();
                        r1.AccessConds = r2RangeResult.AccessConds.clone();
                    }
                }
            }
        }
    }
}

impl rangeDetacher<'_, '_> {
    // detachCNFCondAndBuildRangeForIndex will detach the index filters from table filters. These conditions are connected with `and`
    // It will first find the point query column and then extract the range query column.
    // considerDNF is true means it will try to extract access conditions from the DNF expressions.
    // detachCNFCondAndBuildRangeForIndex 处理 AND 连接的条件，先提取连续的 EQ/IN 前缀，
    // 再按 considerDNF 决定是否尝试 DNF/CNF 子项优化。
    fn detachCNFCondAndBuildRangeForIndex(
        &mut self,
        conditions: Vec<expression::ExprBox>,
        considerDNF: bool,
    ) -> GoResult<DetachRangeResult> {
        let mut eqCount = 0usize;
        let mut res = DetachRangeResult::default();

        let (mut accessConds, mut filterConds, mut newConditions, columnValues, emptyRange) =
            ExtractEqAndInCondition(
                self.sctx,
                conditions.clone(),
                self.cols.clone(),
                self.lengths.clone(),
            );
        if emptyRange {
            return Ok(res);
        }
        let (mut ranges, nextAccessConds, mut remainedConds) =
            self.buildRangeOnColsByCNFCond(accessConds.len(), accessConds.clone())?;
        accessConds = nextAccessConds;
        if !remainedConds.is_empty() {
            filterConds = removeConditions(
                self.sctx.ExprCtx.GetEvalCtx(),
                filterConds,
                remainedConds.clone(),
            );
            newConditions.extend(remainedConds.clone());
        }
        while eqCount < accessConds.len() {
            let sf = accessConds[eqCount].as_scalar_function().unwrap();
            if sf.FuncName.L != ast::EQ {
                break;
            }
            eqCount += 1;
        }
        let mut eqOrInCount = accessConds.len();
        res.EqCondCount = eqCount;
        res.EqOrInCount = eqOrInCount;

        // If index has prefix column and d.mergeConsecutive is true, ranges may not be point ranges anymore after UnionRanges.
        // Therefore, we need to calculate pointRanges separately so that it can be used to append tail ranges in considerDNF branch.
        // See https://github.com/pingcap/tidb/issues/26029 for details.
        // 前缀索引在合并连续 range 后可能不再是点范围，因此 Go 保留一份 pointRanges 专供尾部列追加。
        let mut pointRanges;
        if hasPrefix(&self.lengths) {
            if self.mergeConsecutive {
                pointRanges = Ranges(ranges.iter().map(|ran| ran.Clone()).collect());
                ranges =
                    UnionRanges(self.sctx, ranges, self.mergeConsecutive).map_err(errors::Trace)?;
                pointRanges = UnionRanges(self.sctx, pointRanges, false).map_err(errors::Trace)?;
            } else {
                ranges =
                    UnionRanges(self.sctx, ranges, self.mergeConsecutive).map_err(errors::Trace)?;
                pointRanges = ranges.clone();
            }
        } else {
            pointRanges = ranges.clone();
        }

        res.Ranges = ranges;
        res.AccessConds = accessConds.clone();
        res.RemainedConds = filterConds.clone();
        res.ColumnValues = columnValues;
        if eqOrInCount == self.cols.len() || newConditions.is_empty() {
            res.RemainedConds.extend(newConditions);
            return Ok(res);
        }
        let mut checker = conditionChecker {
            checkerCol: Some(self.cols[eqOrInCount].clone()),
            length: self.lengths[eqOrInCount] as isize,
            optPrefixIndexSingleScan: self.sctx.OptPrefixIndexSingleScan,
            ctx: self.sctx.ExprCtx.GetEvalCtx(),
        };

        if considerDNF {
            let (bestCNFItemRes, columnValues) = extractBestCNFItemRanges(
                self.sctx,
                conditions.clone(),
                self.cols.clone(),
                self.lengths.clone(),
                self.newTpSlice.clone(),
                self.rangeMaxSize,
                self.convertToSortKey,
            )?;
            res.ColumnValues = unionColumnValues(res.ColumnValues, columnValues);

            if let Some(best) = bestCNFItemRes.as_ref() {
                if let Some(bestRange) = best.rangeResult.as_ref() {
                    if bestRange.Ranges.is_empty() {
                        return Ok(DetachRangeResult::default());
                    }
                    if best.sameLenPointRanges && best.minColNum > eqOrInCount {
                        // 最佳 CNF 是更长的等长点范围时，直接替换主结果，并移除该 CNF item，
                        // 使剩余条件可以继续尝试追加尾部列 range。
                        let mut newRes = bestRange.clone();
                        newRes.ColumnValues = res.ColumnValues.clone();
                        res = newRes;
                        pointRanges = bestRange.Ranges.clone();
                        eqOrInCount = res.Ranges[0].LowVal.len();
                        newConditions.clear();
                        newConditions.extend(conditions[..best.offset].iter().cloned());
                        newConditions.extend(conditions[best.offset + 1..].iter().cloned());
                        if eqOrInCount == self.cols.len() || newConditions.is_empty() {
                            res.RemainedConds.extend(newConditions);
                            return Ok(res);
                        }
                    } else {
                        let considerCNFItemNonPointRanges = fixcontrol::GetBoolWithDefault(
                            &self.sctx.OptimizerFixControl,
                            fixcontrol::Fix44389,
                            false,
                        );
                        if considerCNFItemNonPointRanges
                            && !best.sameLenPointRanges
                            && eqOrInCount == 0
                            && best.minColNum > 0
                            && best.maxColNum > 1
                        {
                            // When eqOrInCount is 0, if we don't enter the IF branch, we would use detachColumnCNFConditions to build
                            // ranges on the first index column.
                            // Considering minColNum > 0 and maxColNum > 1, bestCNFItemRes is better than the ranges built by detachColumnCNFConditions
                            // in most cases.
                            let mut newRes = bestRange.clone();
                            newRes.ColumnValues = res.ColumnValues.clone();
                            res = newRes;
                            newConditions.clear();
                            newConditions.extend(conditions[..best.offset].iter().cloned());
                            newConditions.extend(conditions[best.offset + 1..].iter().cloned());
                            res.RemainedConds.extend(newConditions);
                            return Ok(res);
                        }
                    }
                }
            }

            if eqOrInCount > 0 {
                let newCols = self.cols[eqOrInCount..].to_vec();
                let newLengths = self.lengths[eqOrInCount..].to_vec();
                let tailRes = detachCondAndBuildRange(
                    self.sctx,
                    newConditions.clone(),
                    newCols,
                    newLengths,
                    self.rangeMaxSize,
                    self.convertToSortKey,
                    self.mergeConsecutive,
                )?;
                if tailRes.Ranges.is_empty() {
                    return Ok(DetachRangeResult::default());
                }
                if !tailRes.AccessConds.is_empty() {
                    let (newRanges, rangeFallback) = AppendRanges2PointRanges(
                        pointRanges,
                        tailRes.Ranges.clone(),
                        self.rangeMaxSize,
                    );
                    if rangeFallback {
                        self.sctx.RecordRangeFallback(self.rangeMaxSize);
                        res.RemainedConds.extend(tailRes.AccessConds.clone());
                        // Some conditions may be in both tailRes.AccessConds and tailRes.RemainedConds so we call AppendConditionsIfNotExist here.
                        res.RemainedConds = AppendConditionsIfNotExist(
                            self.sctx.ExprCtx.GetEvalCtx(),
                            res.RemainedConds,
                            tailRes.RemainedConds,
                        );
                        return Ok(res);
                    }
                    res.Ranges = newRanges;
                    res.AccessConds.extend(tailRes.AccessConds.clone());
                    res.RemainedConds.extend(tailRes.RemainedConds.clone());
                    // For cases like `((a = 1 and b = 1) or (a = 2 and b = 2)) and c = 1` on index (a,b,c), eqOrInCount is 2,
                    // res.EqOrInCount is 0, and tailRes.EqOrInCount is 1. We should not set res.EqOrInCount to 1, otherwise,
                    // `b = CorrelatedColumn` would be extracted as access conditions as well, which is not as expected at least for now.
                    if res.EqOrInCount > 0 {
                        if res.EqOrInCount == res.EqCondCount {
                            res.EqCondCount += tailRes.EqCondCount;
                        }
                        res.EqOrInCount += tailRes.EqOrInCount;
                    }
                    return Ok(res);
                }
                res.RemainedConds.extend(tailRes.RemainedConds.clone());
                // Check if `bestCNFItemRes` is more selective than the ranges derived from the IN list.
                // This can occur if `bestCNFItemRes` represents the intersection of the IN list values
                // and additional conditions, resulting in a more restrictive filter.
                chooseBetweenRangeAndPoint(self.sctx, &mut res, bestCNFItemRes.as_ref());
                return Ok(res);
            }

            // `eqOrInCount` must be 0 when coming here.
            let (nextAccessConds, nextRemainedConds) = detachColumnCNFConditions(
                self.sctx.ExprCtx.as_ref(),
                newConditions.clone(),
                &mut checker,
            );
            res.AccessConds = nextAccessConds;
            res.RemainedConds = nextRemainedConds;
            let (nextRanges, nextAccess, nextRemained) =
                self.buildCNFIndexRange(0, res.AccessConds.clone())?;
            ranges = nextRanges;
            res.AccessConds = nextAccess;
            remainedConds = nextRemained;
            // detachColumnCNFConditions extracts `a = 10 or a = 30` from `(a = 10 and b = 20) or (a = 30 and b = 40)`. If
            // [10, 10] [30, 30] exceeds range mem limit, we add `a = 10 or a = 30` back to RemainedConds, which is actually
            // unnecessary because `(a = 10 and b = 20) or (a = 30 and b = 40)` is already in RemainedConds.
            // TODO: we will optimize it later.
            res.RemainedConds = AppendConditionsIfNotExist(
                self.sctx.ExprCtx.GetEvalCtx(),
                res.RemainedConds,
                remainedConds,
            );
            res.Ranges = ranges;
            // Choosing between point ranges and bestCNF is needed since bestCNF does not cover the intersection
            // of all conjuncts. Even when we add support for intersection, it could be turned off by a flag or it could be
            // incomplete due to a long list of conjuncts.
            if let Some(best) = bestCNFItemRes.as_ref() {
                if let Some(bestRange) = best.rangeResult.as_ref() {
                    if !res.Ranges.is_empty() {
                        let bestCNFIsSubset = bestRange
                            .Ranges
                            .Subset(self.sctx.TypeCtx.clone(), res.Ranges.clone());
                        let pointRangeIsSubset = res
                            .Ranges
                            .Subset(self.sctx.TypeCtx.clone(), bestRange.Ranges.clone());
                        // Pick bestCNFIsSubset if it is more selective than point ranges(res).
                        // Apply optimization if bestCNFItemRes is a proper subset of point ranges.
                        if bestCNFIsSubset && !pointRangeIsSubset {
                            // Update final result and just update: Ranges, AccessConds and RemainedConds
                            res.RemainedConds = removeConditions(
                                self.sctx.ExprCtx.GetEvalCtx(),
                                res.RemainedConds,
                                bestRange.AccessConds.clone(),
                            );
                            res.Ranges = bestRange.Ranges.clone();
                            res.AccessConds = bestRange.AccessConds.clone();
                        }
                    }
                }
            }
            return Ok(res);
        }

        for cond in newConditions {
            let (isAccessCond, shouldReserve) = checker.check(cond.as_ref());
            if !isAccessCond {
                filterConds.push(cond);
                continue;
            }
            accessConds.push(cond.clone());
            if shouldReserve {
                filterConds.push(cond);
            }
            // TODO: if it's prefix column, we need to add cond to filterConds?
        }
        let (nextRanges, nextAccess, nextRemained) =
            self.buildCNFIndexRange(eqOrInCount, accessConds)?;
        res.Ranges = nextRanges;
        res.AccessConds = nextAccess;
        filterConds.extend(nextRemained);
        res.RemainedConds = filterConds;
        Ok(res)
    }
}

// excludeToIncludeForIntPoint converts `(i` to `[i+1` and `i)` to `i-1]` if `i` is integer.
// For example, if p is `(3`, i.e., point { value: int(3), excl: true, start: true }, it is equal to `[4`, i.e., point { value: int(4), excl: false, start: true }.
// Similarly, if p is `8)`, i.e., point { value: int(8), excl: true, start: false}, it is equal to `7]`, i.e., point { value: int(7), excl: false, start: false }.
// If return value is nil, it means p is unsatisfiable. For example, `(MaxUint64` is unsatisfiable.
// The boundary value will be treated as the bigger type: For example, `(MaxInt64` of type KindInt64 will become `[MaxInt64+1` of type KindUint64,
// and vice versa for `0)` of type KindUint64 will become `-1]` of type KindInt64.
// excludeToIncludeForIntPoint 把整数开区间边界转换成等价闭区间边界。
// 无法满足的边界返回 None，保持 Go nil 语义。
fn excludeToIncludeForIntPoint(mut p: point) -> Option<point> {
    if !p.excl {
        return Some(p);
    }
    if p.value.Kind() == types::KindInt64 {
        let val = p.value.GetInt64();
        if p.start {
            if val == i64::MAX {
                p.value.SetUint64((val + 1) as u64);
            } else {
                p.value.SetInt64(val + 1);
            }
            p.excl = false;
        } else {
            if val == i64::MIN {
                return None;
            }
            p.value.SetInt64(val - 1);
            p.excl = false;
        }
    } else if p.value.Kind() == types::KindUint64 {
        let val = p.value.GetUint64();
        if p.start {
            if val == u64::MAX {
                return None;
            }
            p.value.SetUint64(val + 1);
            p.excl = false;
        } else {
            if val == 0 {
                p.value.SetInt64((val - 1) as i64);
            } else {
                p.value.SetUint64(val - 1);
            }
            p.excl = false;
        }
    }
    Some(p)
}

// If there exists an interval whose length is large than 0, return nil. Otherwise remove all unsatisfiable intervals
// and return array of single point intervals.
// allSinglePoints 仅在所有区间都退化为单点时返回点数组；存在非单点区间则返回 None。
fn allSinglePoints(typeCtx: types::Context, mut points: Vec<point>) -> Option<Vec<point>> {
    let mut pos = 0usize;
    let mut i = 0usize;
    while i < points.len() {
        // Remove unsatisfiable interval. For example, (MaxInt64, +inf) and (-inf, MinInt64) is unsatisfiable.
        let Some(left) = excludeToIncludeForIntPoint(points[i].clone()) else {
            i += 2;
            continue;
        };
        let Some(right) = excludeToIncludeForIntPoint(points[i + 1].clone()) else {
            i += 2;
            continue;
        };
        // If interval is not a single point, just return nil.
        if !left.start || right.start || left.excl || right.excl {
            return None;
        }
        // Since the point's collations are equal to the column's collation, we can use any of them.
        let collator = collate::GetCollator(&left.value.Collation());
        let cmp = left
            .value
            .Compare(typeCtx.clone(), &right.value, collator.as_ref());
        if cmp.is_err() || cmp.unwrap() != 0 {
            return None;
        }
        // If interval is a single point, add it back to array.
        points[pos] = left;
        points[pos + 1] = right;
        pos += 2;
        i += 2;
    }
    points.truncate(pos);
    Some(points)
}

// allEqOrIn 判断表达式树是否只由 OR、EQ、NullEQ、IN、IS NULL 组成。
fn allEqOrIn(expr: &dyn expression::Expression) -> bool {
    let Some(f) = expr.as_scalar_function() else {
        return false;
    };
    match f.FuncName.L.as_str() {
        ast::LogicOr => {
            for arg in f.GetArgs() {
                if !allEqOrIn(arg.as_ref()) {
                    return false;
                }
            }
            true
        }
        ast::EQ | ast::NullEQ | ast::In | ast::IsNull => true,
        _ => false,
    }
}

// extractValueInfo 从 EQ/NullEQ/IS NULL 中提取列常量值信息。
// 带参数标记或 deferred expr 的常量在 Go 中被视为 mutable，这里用 value=None 表示。
fn extractValueInfo(expr: &dyn expression::Expression) -> Option<valueInfo> {
    if let Some(f) = expr.as_scalar_function() {
        if f.FuncName.L == ast::IsNull {
            let mut val = types::Datum::default();
            val.SetNull();
            return Some(valueInfo {
                value: Some(val),
                mutable: false,
            });
        }
        if f.FuncName.L == ast::EQ || f.FuncName.L == ast::NullEQ {
            let getValueInfo = |c: &expression::Constant| -> valueInfo {
                let mutable = c.ParamMarker.is_some() || c.DeferredExpr.is_some();
                let value = if mutable { None } else { Some(c.Value.clone()) };
                valueInfo { value, mutable }
            };
            let args = f.GetArgs();
            if let Some(c) = args[0].as_constant() {
                return Some(getValueInfo(c));
            }
            if let Some(c) = args[1].as_constant() {
                return Some(getValueInfo(c));
            }
        }
    }
    None
}

// ExtractEqAndInCondition will split the given condition into three parts by the information of index columns and their lengths.
// accesses: The condition will be used to build range.
// filters: filters is the part that some access conditions need to be evaluated again since it's only the prefix part of char column.
// newConditions: We'll simplify the given conditions if there're multiple in conditions or eq conditions on the same column.
//	e.g. if there're a in (1, 2, 3) and a in (2, 3, 4). This two will be combined to a in (2, 3) and pushed to newConditions.
// columnValues: the constant column values for all index columns. columnValues[i] is nil if cols[i] is not constant.
// bool: indicate whether there's nil range when merging eq and in conditions.
// ExtractEqAndInCondition 提取连续索引列上的 EQ/IN/IS NULL 条件，并合并同一列上的多个条件。
// 返回 bool=true 表示合并后发现空 range，可以提前终止。
fn ExtractEqAndInCondition(
    sctx: &rangerctx::RangerContext,
    conditions: Vec<expression::ExprBox>,
    cols: Vec<expression::Column>,
    lengths: Vec<i32>,
) -> (
    Vec<expression::ExprBox>,
    Vec<expression::ExprBox>,
    Vec<expression::ExprBox>,
    Vec<Option<valueInfo>>,
    bool,
) {
    let mut rb = builder { sctx, err: None };
    let mut accesses: Vec<Option<expression::ExprBox>> = vec![None; cols.len()];
    let mut filters = Vec::new();
    let mut points: Vec<Vec<point>> = vec![Vec::new(); cols.len()];
    let mut mergedAccesses: Vec<Option<expression::ExprBox>> = vec![None; cols.len()];
    // Go 使用 defer PutExpressionSlices 归还临时 slice；用注释标出资源收尾点。
    // defer expression.PutExpressionSlices(mergedAccesses)
    let mut newConditions = Vec::with_capacity(conditions.len());
    let mut columnValues = vec![None; cols.len()];
    let mut offsets = vec![0isize; conditions.len()];

    for (i, cond) in conditions.iter().cloned().enumerate() {
        let offset = getPotentialEqOrInColOffset(sctx, cond.clone(), cols.clone());
        offsets[i] = offset;
        if offset == -1 {
            continue;
        }
        let offset = offset as usize;
        if accesses[offset].is_none() {
            accesses[offset] = Some(cond);
            continue;
        }
        // Multiple Eq/In conditions for one column in CNF, apply intersection on them
        // Lazily compute the points for the previously visited Eq/In
        let newTp = newFieldType(cols[offset].GetType(sctx.ExprCtx.GetEvalCtx()));
        let collator =
            collate::GetCollator(cols[offset].GetType(sctx.ExprCtx.GetEvalCtx()).GetCollate());
        if mergedAccesses[offset].is_none() {
            mergedAccesses[offset] = accesses[offset].clone();
            // Note that this is a relatively special usage of build(). We will restore the points back to Expression for
            // later use and may build the Expression to points again.
            // We need to keep the original value here, which means we neither cut prefix nor convert to sort key.
            points[offset] = rb.build(
                accesses[offset].as_ref().unwrap().as_ref(),
                &newTp,
                types::UnspecifiedLength,
                false,
            );
        }
        let built = rb.build(cond.as_ref(), &newTp, types::UnspecifiedLength, false);
        points[offset] = rb.intersection(points[offset].clone(), built, collator.as_ref());
        if points[offset].is_empty() {
            // Early termination if false expression found
            if conditions.iter().any(|expr| {
                expression::MaybeOverOptimized4PlanCache(sctx.ExprCtx.as_ref(), expr.as_ref())
            }) {
                // `a>@x and a<@y` --> `invalid-range if @x>=@y`
                sctx.SetSkipPlanCache("some parameters may be overwritten");
            }
            return (Vec::new(), Vec::new(), Vec::new(), Vec::new(), true);
        }
    }

    for i in 0..mergedAccesses.len() {
        if mergedAccesses[i].is_none() {
            if let Some(access) = accesses[i].as_ref() {
                if allEqOrIn(access.as_ref()) {
                    columnValues[i] = extractValueInfo(access.as_ref());
                    if columnValues[i]
                        .as_ref()
                        .is_some_and(|v| v.value.as_ref().is_some_and(|datum| datum.IsNull()))
                    {
                        accesses[i] = None;
                    } else {
                        newConditions.push(access.clone());
                    }
                } else {
                    accesses[i] = None;
                }
            }
            continue;
        }

        let singlePoints = allSinglePoints(sctx.TypeCtx.clone(), points[i].clone());
        if singlePoints.is_none() {
            // There exists an interval whose length is larger than 0
            accesses[i] = None;
        } else if singlePoints.as_ref().unwrap().is_empty() {
            // Early termination if false expression found
            if conditions.iter().any(|expr| {
                expression::MaybeOverOptimized4PlanCache(sctx.ExprCtx.as_ref(), expr.as_ref())
            }) {
                // `a>@x and a<@y` --> `invalid-range if @x>=@y`
                sctx.SetSkipPlanCache("some parameters may be overwritten");
            }
            return (Vec::new(), Vec::new(), Vec::new(), Vec::new(), true);
        } else {
            // All Intervals are single points
            let rebuilt =
                points2EqOrInCond(sctx.ExprCtx.as_ref(), &singlePoints.unwrap(), &cols[i]);
            if let Some(f) = rebuilt.as_scalar_function() {
                if f.FuncName.L == ast::EQ {
                    // Actually the constant column value may not be mutable. Here we assume it is mutable to keep it simple.
                    // Maybe we can improve it later.
                    columnValues[i] = Some(valueInfo {
                        value: None,
                        mutable: true,
                    });
                }
            }
            newConditions.push(rebuilt.clone());
            accesses[i] = Some(rebuilt);
            if conditions.iter().any(|expr| {
                expression::MaybeOverOptimized4PlanCache(sctx.ExprCtx.as_ref(), expr.as_ref())
            }) {
                // `a=@x and a=@y` --> `a=@x if @x==@y`
                sctx.SetSkipPlanCache("some parameters may be overwritten");
            }
        }
    }

    for (i, offset) in offsets.iter().copied().enumerate() {
        if offset == -1 || accesses[offset as usize].is_none() {
            newConditions.push(conditions[i].clone());
        }
    }
    for i in 0..accesses.len() {
        if accesses[i].is_none() {
            accesses.truncate(i);
            break;
        }

        // Currently, if the access cond is on a prefix index, we will also add this cond to table filters.
        // A possible optimization is that, if the value in the cond is shorter than the length of the prefix index, we don't
        // need to add this cond to table filters.
        // e.g. CREATE TABLE t(a varchar(10), index i(a(5))); SELECT * FROM t USE INDEX i WHERE a > 'aaa';
        // However, please notice that if you're implementing this, please (1) set StatementContext.OptimDependOnMutableConst to true,
        // or (2) don't do this optimization when StatementContext.UseCache is true. That's because this plan is affected by
        // flen of user variable, we cannot cache this plan.
        let isFullLength = lengths[i] == types::UnspecifiedLength
            || lengths[i] == cols[i].GetType(sctx.ExprCtx.GetEvalCtx()).GetFlen() as i32;
        if !isFullLength {
            filters.push(accesses[i].as_ref().unwrap().clone());
        }
    }
    // We should remove all accessConds, so that they will not be added to filter conditions.
    let accesses: Vec<_> = accesses.into_iter().flatten().collect();
    newConditions = removeConditions(sctx.ExprCtx.GetEvalCtx(), newConditions, accesses.clone());
    // expression.PutExpressionSlices(mergedAccesses) 在 Go defer 中发生；这里不真实管理池化资源。
    (accesses, filters, newConditions, columnValues, false)
}

impl rangeDetacher<'_, '_> {
    // detachDNFCondAndBuildRangeForIndex will detach the index filters from table filters when it's a DNF(Disjunctive Normal Form).
    // We will detach the conditions of every DNF items, then compose them to a DNF.
    // detachDNFCondAndBuildRangeForIndex 按 OR 分支分别拆 CNF 条件并合并 ranges。
    // 任一分支不能构造 access range 时，Go 会退回 FullRange 并把整体作为残留 filter。
    fn detachDNFCondAndBuildRangeForIndex(
        &mut self,
        condition: &expression::ScalarFunction,
    ) -> GoResult<(
        Ranges,
        Vec<expression::ExprBox>,
        Vec<Option<valueInfo>>,
        bool,
        isize,
    )> {
        let mut firstColumnChecker = conditionChecker {
            checkerCol: Some(self.cols[0].clone()),
            length: self.lengths[0] as isize,
            optPrefixIndexSingleScan: self.sctx.OptPrefixIndexSingleScan,
            ctx: self.sctx.ExprCtx.GetEvalCtx(),
        };
        let mut rb = builder {
            sctx: self.sctx,
            err: None,
        };
        let dnfItems = expression::FlattenDNFConditions(condition);
        let mut newAccessItems = Vec::with_capacity(dnfItems.len());
        let mut minAccessConds: isize = -1;
        let mut totalRanges = Ranges::default();
        let mut totalRangesMemUsage: i64 = 0;
        let mut columnValues = vec![None; self.cols.len()];
        let mut hasResidual = false;

        for (i, item) in dnfItems.iter().cloned().enumerate() {
            if let Some(sf) = item.as_scalar_function() {
                if sf.FuncName.L == ast::LogicAnd {
                    let cnfItems = expression::FlattenCNFConditions(sf);
                    let res = self.detachCNFCondAndBuildRangeForIndex(cnfItems, true)?;
                    let ranges = res.Ranges.clone();
                    // If DNF item always false, we can return ignore this DNF item.
                    if ranges.is_empty() {
                        continue;
                    }
                    let accesses = res.AccessConds.clone();
                    let filters = res.RemainedConds.clone();
                    if accesses.is_empty() {
                        return Ok((FullRange(), Vec::new(), Vec::new(), true, -1));
                    }
                    if !filters.is_empty() {
                        hasResidual = true;
                    }
                    totalRanges.extend(ranges.clone());
                    totalRangesMemUsage += ranges.MemUsage();
                    if self.rangeMaxSize > 0 && totalRangesMemUsage > self.rangeMaxSize {
                        self.sctx.RecordRangeFallback(self.rangeMaxSize);
                        return Ok((FullRange(), Vec::new(), Vec::new(), true, -1));
                    }
                    if let Some(composed) =
                        expression::ComposeCNFCondition(self.sctx.ExprCtx.as_ref(), &accesses)
                    {
                        newAccessItems.push(composed);
                    }
                    if !res.ColumnValues.is_empty() {
                        if i == 0 {
                            columnValues = res.ColumnValues.clone();
                        } else {
                            // take the intersection of the two columnValues
                            for j in 0..columnValues.len() {
                                if columnValues[j].is_none() {
                                    continue;
                                }
                                let sameValue = isSameValue(
                                    self.sctx.TypeCtx.clone(),
                                    columnValues[j].clone(),
                                    res.ColumnValues[j].clone(),
                                )
                                .map_err(errors::Trace)?;
                                if !sameValue {
                                    columnValues[j] = None;
                                }
                            }
                        }
                    }
                    if minAccessConds == -1 || (accesses.len() as isize) < minAccessConds {
                        minAccessConds = accesses.len() as isize;
                    }
                    continue;
                }
            }

            let (isAccessCond, shouldReserve) = firstColumnChecker.check(item.as_ref());
            if !isAccessCond {
                return Ok((FullRange(), Vec::new(), Vec::new(), true, -1));
            }
            if shouldReserve {
                hasResidual = true;
            }
            let points = rb.build(
                item.as_ref(),
                &self.newTpSlice[0],
                self.lengths[0],
                self.convertToSortKey,
            );
            let mut tmpNewTp = self.newTpSlice[0].clone();
            if self.convertToSortKey {
                tmpNewTp = convertStringFTToBinaryCollate(&tmpNewTp);
            }
            // TODO: restrict the mem usage of ranges
            let (ranges, rangeFallback) =
                points2Ranges(self.sctx, points, &tmpNewTp, self.rangeMaxSize)
                    .map_err(errors::Trace)?;
            if rangeFallback {
                self.sctx.RecordRangeFallback(self.rangeMaxSize);
                return Ok((FullRange(), Vec::new(), Vec::new(), true, -1));
            }
            totalRanges.extend(ranges.clone());
            totalRangesMemUsage += ranges.MemUsage();
            if self.rangeMaxSize > 0 && totalRangesMemUsage > self.rangeMaxSize {
                self.sctx.RecordRangeFallback(self.rangeMaxSize);
                return Ok((FullRange(), Vec::new(), Vec::new(), true, -1));
            }
            newAccessItems.push(item.clone());
            if i == 0 {
                columnValues[0] = extractValueInfo(item.as_ref());
            } else if columnValues[0].is_some() {
                let valInfo = extractValueInfo(item.as_ref());
                let sameValue =
                    isSameValue(self.sctx.TypeCtx.clone(), columnValues[0].clone(), valInfo)
                        .map_err(errors::Trace)?;
                if !sameValue {
                    columnValues[0] = None;
                }
            }
            if minAccessConds == -1 || minAccessConds > 1 {
                minAccessConds = 1;
            }
        }

        totalRanges =
            UnionRanges(self.sctx, totalRanges, self.mergeConsecutive).map_err(errors::Trace)?;
        Ok((
            totalRanges,
            expression::ComposeDNFCondition(self.sctx.ExprCtx.as_ref(), &newAccessItems)
                .into_iter()
                .collect(),
            columnValues,
            hasResidual,
            minAccessConds,
        ))
    }
}

// valueInfo is used for recording the constant column value in DetachCondAndBuildRangeForIndex.
// valueInfo 记录索引列被等值约束到的常量值；mutable=true 表示依赖参数或 deferred expr，不能安全用于计划缓存优化。
#[derive(Clone, Default)]
/// 记录索引列等值常量；mutable 表示依赖参数，不宜用于计划缓存优化。
pub struct valueInfo {
    value: Option<types::Datum>, // If not mutable, value is the constant column value. Otherwise value is nil.
    mutable: bool,               // If true, the constant column value depends on mutable constant.
}

// isSameValue 比较两个 valueInfo 是否代表同一常量。Go 代码对 mutable 直接返回 false，
// 避免把依赖用户变量/参数的值纳入缓存敏感优化。
fn isSameValue(
    typeCtx: types::Context,
    lhs: Option<valueInfo>,
    rhs: Option<valueInfo>,
) -> GoResult<bool> {
    // We assume `lhs` and `rhs` are not the same when either `lhs` or `rhs` is mutable to keep it simple. If we consider
    // mutable valueInfo, we need to set `sc.OptimDependOnMutableConst = true`, which makes the plan not able to be cached.
    // On the other hand, the equal condition may not be used for optimization. Hence we simply regard mutable valueInfos different
    // from others. Maybe we can improve it later.
    // TODO: is `lhs.value.Kind() != rhs.value.Kind()` necessary?
    let (Some(lhs), Some(rhs)) = (lhs, rhs) else {
        return Ok(false);
    };
    let (Some(lhsValue), Some(rhsValue)) = (lhs.value, rhs.value) else {
        return Ok(false);
    };
    if lhs.mutable || rhs.mutable || lhsValue.Kind() != rhsValue.Kind() {
        return Ok(false);
    }
    // binary collator may not the best choice, but it can make sure the result is correct.
    let collator = collate::GetBinaryCollator();
    let cmp = lhsValue.Compare(typeCtx, &rhsValue, collator.as_ref())?;
    Ok(cmp == 0)
}

// DetachRangeResult wraps up results when detaching conditions and builing ranges.
// DetachRangeResult 汇总条件拆分与 range 构造的输出，字段顺序保持 Go 结构体一致。
#[derive(Clone, Default)]
/// 条件拆分与 Range 构造的汇总结果，字段顺序对齐 Go。
pub struct DetachRangeResult {
    // Ranges is the ranges extracted and built from conditions.
    pub Ranges: Ranges,
    // AccessConds is the extracted conditions for access.
    pub AccessConds: Vec<expression::ExprBox>,
    // RemainedConds is the filter conditions which should be kept after access.
    pub RemainedConds: Vec<expression::ExprBox>,
    // ColumnValues records the constant column values for all index columns.
    // For the ith column, if it is evaluated as constant, ColumnValues[i] is its value. Otherwise ColumnValues[i] is nil.
    pub ColumnValues: Vec<Option<valueInfo>>,
    // EqCondCount is the number of equal conditions extracted.
    pub EqCondCount: usize,
    // EqOrInCount is the number of equal/in conditions extracted.
    pub EqOrInCount: usize,
    // IsDNFCond indicates if the top layer of conditions are in DNF.
    // Please see comments of planner/util/AccessPath.MinAccessCondsForDNFCond for more details.
    pub IsDNFCond: bool,
    pub MinAccessCondsForDNFCond: isize,
}

// DetachCondAndBuildRangeForIndex will detach the index filters from table filters.
// rangeMaxSize is the max memory limit for ranges. O indicates no memory limit. If you ask that all conditions must be used
// for building ranges, set rangeMemQuota to 0 to avoid range fallback.
// The returned values are encapsulated into a struct DetachRangeResult, see its comments for explanation.
// DetachCondAndBuildRangeForIndex 是索引 range 构造的公开入口，默认转换 sort key 且合并连续 range。
/// 索引 Range 公开入口：转换 sort key 并合并连续区间。
pub fn DetachCondAndBuildRangeForIndex(
    sctx: &rangerctx::RangerContext,
    conditions: Vec<expression::ExprBox>,
    cols: Vec<expression::Column>,
    lengths: Vec<i32>,
    rangeMaxSize: i64,
) -> GoResult<DetachRangeResult> {
    detachCondAndBuildRange(sctx, conditions, cols, lengths, rangeMaxSize, true, true)
}

// detachCondAndBuildRange detaches the index filters from table filters and uses them to build ranges.
// detachCondAndBuildRange 准备每列的 FieldType 副本，再进入可递归的 rangeDetacher 入口。
fn detachCondAndBuildRange(
    sctx: &rangerctx::RangerContext,
    conditions: Vec<expression::ExprBox>,
    cols: Vec<expression::Column>,
    lengths: Vec<i32>,
    rangeMaxSize: i64,
    convertToSortKey: bool,
    mergeConsecutive: bool,
) -> GoResult<DetachRangeResult> {
    let mut newTpSlice = Vec::with_capacity(cols.len());
    for col in cols.iter() {
        newTpSlice.push(newFieldType(col.GetType(sctx.ExprCtx.GetEvalCtx())));
    }

    detachCondAndBuildRangeRecursive(
        sctx,
        conditions,
        cols,
        lengths,
        newTpSlice,
        rangeMaxSize,
        convertToSortKey,
        mergeConsecutive,
    )
}

// detachCondAndBuildRangeRecursive 创建 rangeDetacher 状态对象；Go 里用于 extractBestCNFItemRanges 的递归构建。
fn detachCondAndBuildRangeRecursive(
    sctx: &rangerctx::RangerContext,
    conditions: Vec<expression::ExprBox>,
    cols: Vec<expression::Column>,
    lengths: Vec<i32>,
    newTpSlice: Vec<types::FieldType>,
    rangeMaxSize: i64,
    convertToSortKey: bool,
    mergeConsecutive: bool,
) -> GoResult<DetachRangeResult> {
    let mut d = rangeDetacher {
        sctx,
        allConds: conditions,
        cols,
        lengths,
        newTpSlice,
        mergeConsecutive,
        convertToSortKey,
        rangeMaxSize,
    };
    d.detachCondAndBuildRangeForCols()
}

// DetachCondAndBuildRangeForPartition will detach the index filters from table filters.
// rangeMaxSize is the max memory limit for ranges. O indicates no memory limit. If you ask that all conditions must be used
// for building ranges, set rangeMemQuota to 0 to avoid range fallback.
// The returned values are encapsulated into a struct DetachRangeResult, see its comments for explanation.
// DetachCondAndBuildRangeForPartition 是分区场景入口，不转换 sort key，也不合并连续 range。
/// 分区场景入口：不转换 sort key，也不合并连续区间。
pub fn DetachCondAndBuildRangeForPartition(
    sctx: &rangerctx::RangerContext,
    conditions: Vec<expression::ExprBox>,
    cols: Vec<expression::Column>,
    lengths: Vec<i32>,
    rangeMaxSize: i64,
) -> GoResult<DetachRangeResult> {
    detachCondAndBuildRange(sctx, conditions, cols, lengths, rangeMaxSize, false, false)
}

// rangeDetacher 保存一次 range detach 过程需要的上下文和配置，字段顺序对应 Go 结构体。
pub(crate) struct rangeDetacher<'a, 'ctx> {
    pub(crate) sctx: &'a rangerctx::RangerContext<'ctx>,
    pub(crate) allConds: Vec<expression::ExprBox>,
    pub(crate) cols: Vec<expression::Column>,
    pub(crate) lengths: Vec<i32>,
    pub(crate) newTpSlice: Vec<types::FieldType>,
    pub(crate) mergeConsecutive: bool,
    pub(crate) convertToSortKey: bool,
    pub(crate) rangeMaxSize: i64,
}

impl rangeDetacher<'_, '_> {
    // detachCondAndBuildRangeForCols 根据顶层是否为单个 OR，分发到 DNF 或 CNF 路径。
    fn detachCondAndBuildRangeForCols(&mut self) -> GoResult<DetachRangeResult> {
        let mut res = DetachRangeResult::default();
        if self.allConds.len() == 1 {
            let condition = self.allConds[0].clone();
            if let Some(sf) = condition.as_scalar_function() {
                if sf.FuncName.L == ast::LogicOr {
                    let (ranges, accesses, columnValues, hasResidual, minAccessConds) =
                        self.detachDNFCondAndBuildRangeForIndex(sf)?;
                    res.Ranges = ranges;
                    res.AccessConds = accesses;
                    res.ColumnValues = columnValues;
                    res.IsDNFCond = true;
                    if minAccessConds != -1 {
                        res.MinAccessCondsForDNFCond = minAccessConds;
                    }
                    // If this DNF have something cannot be to calculate range, then all this DNF should be pushed as filter condition.
                    if hasResidual {
                        res.RemainedConds = self.allConds.clone();
                        return Ok(res);
                    }
                    return Ok(res);
                }
            }
        }
        self.detachCNFCondAndBuildRangeForIndex(self.allConds.clone(), true)
    }
}

// DetachSimpleCondAndBuildRangeForIndex will detach the index filters from table filters.
// It will find the point query column firstly and then extract the range query column.
// rangeMaxSize is the max memory limit for ranges. O indicates no memory limit. If you ask that all conditions must be used
// for building ranges, set rangeMemQuota to 0 to avoid range fallback.
// The returned remainedConds are conditions that must be re-applied as filters (e.g. when a
// collation mismatch makes the range approximate but the condition is still needed for correctness).
// DetachSimpleCondAndBuildRangeForIndex 是简化入口，直接走 CNF 路径且不考虑 DNF 优化。
/// 简化入口：直接走 CNF，不做 DNF 优化。
pub fn DetachSimpleCondAndBuildRangeForIndex(
    sctx: &rangerctx::RangerContext,
    conditions: Vec<expression::ExprBox>,
    cols: Vec<expression::Column>,
    lengths: Vec<i32>,
    rangeMaxSize: i64,
) -> GoResult<(Ranges, Vec<expression::ExprBox>, Vec<expression::ExprBox>)> {
    let mut newTpSlice = Vec::with_capacity(cols.len());
    for col in cols.iter() {
        newTpSlice.push(newFieldType(col.GetType(sctx.ExprCtx.GetEvalCtx())));
    }
    let mut d = rangeDetacher {
        sctx,
        allConds: conditions.clone(),
        cols,
        lengths,
        newTpSlice,
        mergeConsecutive: true,
        convertToSortKey: true,
        rangeMaxSize,
    };
    let res = d.detachCNFCondAndBuildRangeForIndex(conditions, false)?;
    Ok((res.Ranges, res.AccessConds, res.RemainedConds))
}

// removeConditions 从 conditions 中移除 condsToRemove 已包含的表达式，比较逻辑委托给 expression::Contains。
fn removeConditions(
    ectx: &dyn expression::EvalContext,
    conditions: Vec<expression::ExprBox>,
    condsToRemove: Vec<expression::ExprBox>,
) -> Vec<expression::ExprBox> {
    let mut filterConds = Vec::with_capacity(conditions.len());
    for cond in conditions {
        if !condsToRemove
            .iter()
            .any(|candidate| candidate.Equal(ectx, cond.as_ref()))
        {
            filterConds.push(cond);
        }
    }
    filterConds
}

// AppendConditionsIfNotExist appends conditions if they are absent.
// AppendConditionsIfNotExist 只追加尚不存在的条件，避免 access/filter 重复。
/// 仅追加尚不存在的条件，避免 access/filter 重复。
pub fn AppendConditionsIfNotExist(
    ectx: &dyn expression::EvalContext,
    mut conditions: Vec<expression::ExprBox>,
    condsToAppend: Vec<expression::ExprBox>,
) -> Vec<expression::ExprBox> {
    let mut shouldAppend = Vec::with_capacity(condsToAppend.len());
    for cond in condsToAppend {
        if !conditions
            .iter()
            .any(|candidate| candidate.Equal(ectx, cond.as_ref()))
        {
            shouldAppend.push(cond);
        }
    }
    conditions.extend(shouldAppend);
    conditions
}

// ExtractAccessConditionsForColumn extracts the access conditions used for range calculation. Since
// we don't need to return the remained filter conditions, it is much simpler than DetachCondsForColumn.
// ExtractAccessConditionsForColumn 只返回指定列可用于 range 的条件，不返回残留 filter。
/// 只返回指定列可用于 Range 的条件。
pub fn ExtractAccessConditionsForColumn(
    ctx: &rangerctx::RangerContext,
    conds: Vec<expression::ExprBox>,
    col: expression::Column,
) -> Vec<expression::ExprBox> {
    let mut checker = conditionChecker {
        checkerCol: Some(col),
        length: types::UnspecifiedLength as isize,
        optPrefixIndexSingleScan: ctx.OptPrefixIndexSingleScan,
        ctx: ctx.ExprCtx.GetEvalCtx(),
    };
    conds
        .into_iter()
        .filter(|expr| checker.check(expr.as_ref()).0)
        .collect()
}

// DetachCondsForColumn detaches access conditions for specified column from other filter conditions.
// DetachCondsForColumn 返回指定列的 access 条件和其它条件，内部复用 CNF 拆分逻辑。
/// 返回指定列的 access 条件与其它残留条件。
pub fn DetachCondsForColumn(
    sctx: &rangerctx::RangerContext,
    conds: Vec<expression::ExprBox>,
    col: expression::Column,
) -> (Vec<expression::ExprBox>, Vec<expression::ExprBox>) {
    let mut checker = conditionChecker {
        checkerCol: Some(col),
        length: types::UnspecifiedLength as isize,
        optPrefixIndexSingleScan: sctx.OptPrefixIndexSingleScan,
        ctx: sctx.ExprCtx.GetEvalCtx(),
    };
    detachColumnCNFConditions(sctx.ExprCtx.as_ref(), conds, &mut checker)
}

// MergeDNFItems4Col receives a slice of DNF conditions, merges some of them which can be built into ranges on a single column, then returns.
// For example, [a > 5, b > 6, c > 7, a = 1, b > 3] will become [a > 5 or a = 1, b > 6 or b > 3, c > 7].
// MergeDNFItems4Col 按列合并可构造单列 range 的 DNF item，避免 Selectivity 递归过深。
/// 合并同一列上的 DNF 子项，便于统一构造 Range。
pub fn MergeDNFItems4Col(
    ctx: &rangerctx::RangerContext,
    dnfItems: Vec<expression::ExprBox>,
) -> Vec<expression::ExprBox> {
    let mut mergedDNFItems = Vec::with_capacity(dnfItems.len());
    let mut col2DNFItems: std::collections::HashMap<i64, Vec<expression::ExprBox>> =
        std::collections::HashMap::new();
    for dnfItem in dnfItems {
        let cols = expression::ExtractColumns(dnfItem.as_ref());
        // If this condition contains multiple columns, we can't merge it.
        // If this column is _tidb_rowid, we also can't merge it since Selectivity() doesn't handle it, or infinite recursion will happen.
        if cols.len() != 1 || cols[0].ID == model::ExtraHandleID {
            mergedDNFItems.push(dnfItem);
            continue;
        }

        let uniqueID = cols[0].UniqueID;
        let mut checker = conditionChecker {
            checkerCol: Some(cols[0].clone()),
            length: types::UnspecifiedLength as isize,
            optPrefixIndexSingleScan: ctx.OptPrefixIndexSingleScan,
            ctx: ctx.ExprCtx.GetEvalCtx(),
        };
        // If we can't use this condition to build range, we can't merge it.
        // Currently, we assume if every condition in a DNF expression can pass this check, then `Selectivity` must be able to
        // cover this entire DNF directly without recursively call `Selectivity`. If this doesn't hold in the future, this logic
        // may cause infinite recursion in `Selectivity`.
        let (isAccessCond, _) = checker.check(dnfItem.as_ref());
        if !isAccessCond {
            mergedDNFItems.push(dnfItem);
            continue;
        }

        col2DNFItems.entry(uniqueID).or_default().push(dnfItem);
    }
    for (_uniqueID, items) in col2DNFItems {
        if let Some(composed) = expression::ComposeDNFCondition(ctx.ExprCtx.as_ref(), &items) {
            mergedDNFItems.push(composed);
        }
    }
    mergedDNFItems
}

// AddGcColumnCond add the `tidb_shard(x) = xxx` to the condition
// @param[in] cols the columns of shard index, such as [tidb_shard(a), a, ...]
// @param[in] accessCond the conditions relative to the index and arranged by the index column order.
//	e.g. the index is uk(tidb_shard(a), a, b) and the where clause is
//	`WHERE b = 1 AND a = 2 AND c = 3`, the param accessCond is {a = 2, b = 1} that is
//	only relative to uk's columns.
// @param[in] columnValues the values of index columns in param accessCond. if accessCond is {a = 2, b = 1},
//	columnValues is {2, 1}. if accessCond the "IN" function like `a IN (1, 2)`, columnValues
//	is empty.
// @retval - []expression.Expression the new conditions after adding `tidb_shard() = xxx` prefix
// error if error gernerated, return error
// AddGcColumnCond 根据第二列条件类型分发到 EQ 或 IN 的 tidb_shard 前缀补条件逻辑。
/// 为分片索引补全 GC（生成列）相关条件。
pub fn AddGcColumnCond(
    sctx: &rangerctx::RangerContext,
    cols: &[expression::Column],
    accessesCond: Vec<Option<expression::ExprBox>>,
    columnValues: Vec<Option<valueInfo>>,
) -> GoResult<Vec<expression::ExprBox>> {
    if let Some(cond) = accessesCond.get(1).and_then(Option::as_ref) {
        if let Some(f) = cond.as_scalar_function() {
            match f.FuncName.L.as_str() {
                ast::EQ => return AddGcColumn4EqCond(sctx, cols, accessesCond, columnValues),
                ast::In => return AddGcColumn4InCond(sctx, cols, accessesCond),
                _ => {}
            }
        }
    }
    Ok(accessesCond.into_iter().flatten().collect())
}

// AddGcColumn4InCond add the `tidb_shard(x) = xxx` for `IN` condition
// For param explanation, please refer to the function `AddGcColumnCond`.
// @retval - []expression.Expression the new conditions after adding `tidb_shard() = xxx` prefix
// error if error gernerated, return error
// AddGcColumn4InCond 把 `a IN (...)` 展开为多个 `(tidb_shard(a)=x AND a=value)` 再 OR 起来。
/// 对 IN 条件补全分片索引生成列约束。
pub fn AddGcColumn4InCond(
    sctx: &rangerctx::RangerContext,
    cols: &[expression::Column],
    accessesCond: Vec<Option<expression::ExprBox>>,
) -> GoResult<Vec<expression::ExprBox>> {
    let mut newAccessCond = Vec::new();
    let mut record = vec![types::Datum::default(); 1];

    let expr = cols[0]
        .VirtualExpr
        .as_ref()
        .ok_or_else(|| errors::New("shard index column has no virtual expression"))?
        .clone();
    let andType = types::NewFieldType(mysql::TypeTiny);

    let sf = accessesCond[1]
        .as_ref()
        .unwrap()
        .as_scalar_function()
        .unwrap();
    let c = sf.GetArgs()[0].as_column().unwrap();
    let mut andOrExpr: Option<expression::ExprBox> = None;
    let evalCtx = sctx.ExprCtx.GetEvalCtx();
    for (i, arg) in sf.GetArgs().iter().skip(1).cloned().enumerate() {
        // get every const value and calculate tidb_shard(val)
        // 对每个 IN 常量值求 tidb_shard；这里发生表达式求值，真实迁移需要确认 Eval 的上下文与 row 构造。
        let con = arg.as_constant().unwrap();
        let conVal = con.Eval(evalCtx, chunk::Row::default())?;

        record[0] = conVal;
        let mutRow = chunk::mutrow::MutRowFromDatums(record.clone());
        let exprVal = expr.Eval(evalCtx, mutRow.ToRow())?;

        // tmpArg1 is like `tidb_shard(a) = 8`, tmpArg2 is like `a = 100`
        let shard_type = cols[0].GetType(evalCtx).clone();
        let exprCon = expression::Constant::with_type(exprVal, shard_type.clone());
        let tmpArg1 = expression::NewFunction(
            sctx.ExprCtx.as_ref(),
            ast::EQ,
            shard_type,
            vec![Box::new(cols[0].clone()), Box::new(exprCon)],
        )?;
        let tmpArg2 = expression::NewFunction(
            sctx.ExprCtx.as_ref(),
            ast::EQ,
            c.GetType(evalCtx).clone(),
            vec![Box::new(c.clone()), arg],
        )?;

        // make a LogicAnd, e.g. `tidb_shard(a) = 8 AND a = 100`
        let andExpr = expression::NewFunction(
            sctx.ExprCtx.as_ref(),
            ast::LogicAnd,
            (*andType).clone(),
            vec![tmpArg1, tmpArg2],
        )?;

        if i == 0 {
            andOrExpr = Some(andExpr);
        } else {
            // if the LogicAnd more than one, make a LogicOr,
            // e.g. `(tidb_shard(a) = 8 AND a = 100) OR (tidb_shard(a) = 161 AND a = 200)`
            andOrExpr = Some(expression::NewFunction(
                sctx.ExprCtx.as_ref(),
                ast::LogicOr,
                (*andType).clone(),
                vec![andOrExpr.take().unwrap(), andExpr],
            )?);
        }
    }

    if let Some(andOrExpr) = andOrExpr {
        newAccessCond.push(andOrExpr);
    }
    Ok(newAccessCond)
}

// AddGcColumn4EqCond add the `tidb_shard(x) = xxx` prefix for equal condition
// For param explanation, please refer to the function `AddGcColumnCond`.
// @retval - []expression.Expression the new conditions after adding `tidb_shard() = xxx` prefix
// []*valueInfo the values of every columns in the returned new conditions
// error if error gernerated, return error
// AddGcColumn4EqCond 对全等值 shard index 计算 tidb_shard 前缀，并回填 accessesCond[0] 与 columnValues[0]。
/// 对 EQ 条件补全分片索引生成列约束。
pub fn AddGcColumn4EqCond(
    sctx: &rangerctx::RangerContext,
    cols: &[expression::Column],
    mut accessesCond: Vec<Option<expression::ExprBox>>,
    mut columnValues: Vec<Option<valueInfo>>,
) -> GoResult<Vec<expression::ExprBox>> {
    let expr = cols[0]
        .VirtualExpr
        .as_ref()
        .ok_or_else(|| errors::New("shard index column has no virtual expression"))?
        .clone();
    let mut record = vec![types::Datum::default(); columnValues.len().saturating_sub(1)];

    for i in 1..columnValues.len() {
        let Some(cv) = columnValues[i].clone() else {
            break;
        };
        record[i - 1] = cv.value.unwrap();
    }

    let mutRow = chunk::mutrow::MutRowFromDatums(record);
    let exprCtx = sctx.ExprCtx.as_ref();
    let evaluated = expr.Eval(exprCtx.GetEvalCtx(), mutRow.ToRow())?;
    let vi = valueInfo {
        value: Some(evaluated.clone()),
        mutable: false,
    };
    let shard_type = cols[0].GetType(exprCtx.GetEvalCtx()).clone();
    let con = expression::Constant::with_type(evaluated, shard_type.clone());
    // make a tidb_shard() function, e.g. `tidb_shard(a) = 8`
    let cond = expression::NewFunction(
        exprCtx,
        ast::EQ,
        shard_type,
        vec![Box::new(cols[0].clone()), Box::new(con)],
    )?;

    accessesCond[0] = Some(cond);
    columnValues[0] = Some(vi);
    Ok(accessesCond.into_iter().flatten().collect())
}

// AddExpr4EqAndInCondition add the `tidb_shard(x) = xxx` prefix
// Add tidb_shard() for EQ and IN function. e.g. input condition is `WHERE a = 1`,
// output condition is `WHERE tidb_shard(a) = 214 AND a = 1`. e.g. input condition
// is `WHERE a IN (1, 2 ,3)`, output condition is `WHERE (tidb_shard(a) = 214 AND a = 1)
// OR (tidb_shard(a) = 143 AND a = 2) OR (tidb_shard(a) = 156 AND a = 3)`
// @param[in] conditions the original condition to be processed
// @param[in] cols the columns of shard index, such as [tidb_shard(a), a, ...]
// @param[in] lengths the length for every column of shard index
// @retval - the new condition after adding tidb_shard() prefix
// AddExpr4EqAndInCondition 在原始条件集合中补入 tidb_shard 前缀条件。
/// 必要时为 EQ/IN 追加分片前缀表达式。
pub fn AddExpr4EqAndInCondition(
    sctx: &rangerctx::RangerContext,
    conditions: Vec<expression::ExprBox>,
    cols: Vec<expression::Column>,
) -> GoResult<Vec<expression::ExprBox>> {
    let mut accesses: Vec<Option<expression::ExprBox>> = vec![None; cols.len()];
    let mut columnValues = vec![None; cols.len()];
    let mut offsets = vec![0isize; conditions.len()];
    let mut addGcCond = true;

    // the array accesses stores conditions of every column in the index in the definition order
    // e.g. the original condition is `WHERE b = 100 AND a = 200 AND c = 300`, the definition of
    // index is (tidb_shard(a), a, b), then accesses is "[a = 200, b = 100]"
    for (i, cond) in conditions.iter().cloned().enumerate() {
        let offset = getPotentialEqOrInColOffset(sctx, cond.clone(), cols.clone());
        offsets[i] = offset;
        if offset == -1 {
            continue;
        }
        let offset = offset as usize;
        if accesses[offset].is_none() {
            accesses[offset] = Some(cond);
            continue;
        }
        // if the same field appear twice or more, don't add tidb_shard()
        // e.g. `WHERE a > 100 and a < 200`
        addGcCond = false;
    }

    for (i, cond) in accesses.iter().enumerate() {
        let Some(cond) = cond.as_ref() else {
            continue;
        };
        if !allEqOrIn(cond.as_ref()) {
            addGcCond = false;
            break;
        }
        columnValues[i] = extractValueInfo(cond.as_ref());
    }

    if !addGcCond || !NeedAddGcColumn4ShardIndex(&cols, &accesses, &columnValues) {
        return Ok(conditions);
    }

    // remove the accesses from newConditions
    let mut newConditions = Vec::with_capacity(conditions.len());
    newConditions.extend(conditions.clone());
    let access_exprs: Vec<_> = accesses.iter().flatten().cloned().collect();
    newConditions = removeConditions(sctx.ExprCtx.GetEvalCtx(), newConditions, access_exprs);

    // add Gc condition for accesses and return new condition to newAccesses
    let newAccesses = AddGcColumnCond(sctx, &cols, accesses, columnValues)?;

    // merge newAccesses and original condition execept accesses
    newConditions.extend(newAccesses);
    Ok(newConditions)
}

// NeedAddGcColumn4ShardIndex check whether to add `tidb_shard(x) = xxx`
// @param[in] cols the columns of shard index, such as [tidb_shard(a), a, ...]
// @param[in] accessCond the conditions relative to the index and arranged by the index column order.
//	e.g. the index is uk(tidb_shard(a), a, b) and the where clause is
//	`WHERE b = 1 AND a = 2 AND c = 3`, the param accessCond is {a = 2, b = 1} that is
//	only relative to uk's columns.
// @param[in] columnValues the values of index columns in param accessCond. if accessCond is {a = 2, b = 1},
//	columnValues is {2, 1}. if accessCond the "IN" function like `a IN (1, 2)`, columnValues
//	is empty.
// @retval - return true if it needs to addr tidb_shard() prefix, ohterwise return false
// NeedAddGcColumn4ShardIndex 判断 shard index 是否需要自动补 tidb_shard 前缀条件。
/// 判断分片索引是否需要追加 GC 列条件。
pub fn NeedAddGcColumn4ShardIndex(
    cols: &[expression::Column],
    accessCond: &[Option<expression::ExprBox>],
    columnValues: &[Option<valueInfo>],
) -> bool {
    // the columns of shard index shoude be more than 2, like (tidb_shard(a),a,...)
    // check cols and columnValues in the sub call function
    if accessCond.len() < 2 || cols.len() < 2 {
        return false;
    }

    if !IsValidShardIndex(cols) {
        return false;
    }

    // accessCond[0] shoudle be nil, because it has no access condition for
    // the prefix tidb_shard() of the shard index
    if let Some(cond) = accessCond.get(1).and_then(Option::as_ref) {
        if let Some(f) = cond.as_scalar_function() {
            match f.FuncName.L.as_str() {
                ast::EQ => return NeedAddColumn4EqCond(cols, accessCond, columnValues),
                ast::In => return NeedAddColumn4InCond(cols, accessCond, f),
                _ => {}
            }
        }
    }

    false
}

// NeedAddColumn4EqCond `tidb_shard(x) = xxx`
// For param explanation, please refer to the function `NeedAddGcColumn4ShardIndex`.
// It checks whether EQ conditions need to be added tidb_shard() prefix.
// (1) columns in accessCond are all columns of the index except the first.
// (2) every column in accessCond has a constan value
// NeedAddColumn4EqCond 判断 EQ 条件是否覆盖 shard index 除第一列外的所有列，且值均为常量。
/// 判断 EQ 条件是否需要补充分片列。
pub fn NeedAddColumn4EqCond(
    cols: &[expression::Column],
    accessCond: &[Option<expression::ExprBox>],
    columnValues: &[Option<valueInfo>],
) -> bool {
    let mut valCnt = 0usize;
    let mut matchedKeyFldCnt = 0usize;

    // the columns of shard index shoude be more than 2, like (tidb_shard(a),a,...)
    if columnValues.len() < 2 {
        return false;
    }

    for cond in accessCond.iter().skip(1) {
        let Some(cond) = cond.as_ref() else {
            break;
        };

        let Some(f) = cond.as_scalar_function() else {
            return false;
        };
        if f.FuncName.L != ast::EQ {
            return false;
        }

        matchedKeyFldCnt += 1;
    }
    for val in columnValues.iter().skip(1) {
        if val.is_none() {
            break;
        }
        valCnt += 1;
    }

    if matchedKeyFldCnt != cols.len() - 1
        || valCnt != cols.len() - 1
        || accessCond[0].is_some()
        || columnValues[0].is_some()
    {
        return false;
    }

    true
}

// NeedAddColumn4InCond `tidb_shard(x) = xxx`
// For param explanation, please refer to the function `NeedAddGcColumn4ShardIndex`.
// It checks whether "IN" conditions need to be added tidb_shard() prefix.
// (1) columns in accessCond are all columns of the index except the first.
// (2) the first param of "IN" function should be a column not a expression like `a + b`
// (3) the rest params of "IN" function all should be constant
// (4) the first param of "IN" function should be the column in the expression of first index field.
//	e.g. uk(tidb_shard(a), a). If the conditions is `WHERE b in (1, 2, 3)`, the first param of "IN" function
//	is `b` that's not the column in `tidb_shard(a)`.
// @param sf	"IN" function, e.g. `a IN (1, 2, 3)`
// NeedAddColumn4InCond 判断 IN 条件是否可由 tidb_shard(a) 前缀展开。
/// 判断 IN 条件是否需要补充分片列。
pub fn NeedAddColumn4InCond(
    cols: &[expression::Column],
    accessCond: &[Option<expression::ExprBox>],
    sf: &expression::ScalarFunction,
) -> bool {
    if cols.is_empty() || accessCond.is_empty() {
        return false;
    }

    if accessCond[0].is_some() {
        return false;
    }

    let Some(virtual_sf) = cols[0]
        .VirtualExpr
        .as_deref()
        .and_then(|expr| expr.as_scalar_function())
    else {
        return false;
    };
    let fields = ExtractColumnsFromExpr(virtual_sf);

    let args = sf.GetArgs();
    let Some(c) = args[0].as_column() else {
        return false;
    };

    for arg in args.iter().skip(1) {
        if arg.as_constant().is_none() {
            return false;
        }
    }

    if fields.len() != 1 || !fields[0].EqualColumn(c) {
        return false;
    }

    true
}

// ExtractColumnsFromExpr get all fields from input expression virtaulExpr
// ExtractColumnsFromExpr 递归收集 virtual expression 中出现的列，并按 InColumnArray 去重。
/// 从标量函数表达式中提取列引用。
pub fn ExtractColumnsFromExpr(virtaulExpr: &expression::ScalarFunction) -> Vec<expression::Column> {
    let mut fields = Vec::new();

    for arg in virtaulExpr.GetArgs() {
        if let Some(sf) = arg.as_scalar_function() {
            fields.extend(ExtractColumnsFromExpr(sf));
        } else if let Some(c) = arg.as_column() {
            if !c.InColumnArray(&fields) {
                fields.push(c.clone());
            }
        }
    }

    fields
}

// IsValidShardIndex Check whether the definition of shard index is valid. The form of index
// should like `index(tidb_shard(a), a, ....)`.
// 1) the column count shoudle be >= 2
// 2) the first column should be tidb_shard(xxx)
// 3) the parameter of tidb_shard shoudle be a column that is the second column of index
// @param[in] cols the columns of shard index, such as [tidb_shard(a), a, ...]
// @retval - if the shard index is valid return true, otherwise return false
// IsValidShardIndex 校验 shard index 形态是否为 index(tidb_shard(a), a, ...)。
/// 判断列集是否构成合法分片索引前缀。
pub fn IsValidShardIndex(cols: &[expression::Column]) -> bool {
    // definition of index should like the form: index(tidb_shard(a), a, ....)
    if cols.len() < 2 {
        return false;
    }

    // the first coulmn of index must be GC column and the expr must be tidb_shard
    if !expression::GcColumnExprIsTidbShard(cols[0].VirtualExpr.as_deref()) {
        return false;
    }

    let Some(shardFunc) = cols[0]
        .VirtualExpr
        .as_deref()
        .and_then(|expr| expr.as_scalar_function())
    else {
        return false;
    };

    let argCount = shardFunc.GetArgs().len();
    if argCount != 1 {
        return false;
    }

    // parameter of tidb_shard must be the second column of the input index columns
    let Some(col) = shardFunc.GetArgs()[0].as_column() else {
        return false;
    };
    if !col.EqualColumn(&cols[1]) {
        return false;
    }

    true
}
