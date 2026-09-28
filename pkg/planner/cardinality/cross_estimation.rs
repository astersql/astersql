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

// 基于列间相关性的跨列行数估算。
//
// 优化器在 LIMIT 场景下用直方图与 handle/索引列的顺序相关性（correlation）
// 修正表扫描与索引扫描的候选行数；失败时退回均匀分布启发式，并可叠加
// 排序索引选择率风险补偿。本模块只读统计信息，不访问存储或执行计划。

use crate::*;

// 这里只读取统计信息并计算候选行数，不会访问数据库、扫描存储、执行计划或触发业务动作。
// expression、property、statistics、ranger 等类型是跨文件依赖，调用形状按 Go 原实现保留。

// SelectionFactor 是存在剩余过滤条件时对扫描行数进行反推的默认选择率。
/// 存在剩余过滤条件时，对扫描行数反推所用的默认选择率（0.8）。
pub const SelectionFactor: f64 = 0.8;

/// 按 LIMIT 期望行数调整表扫描候选行数，优先采用可靠的跨列相关性估算。
// AdjustRowCountForTableScanByLimit 按 LIMIT 调整表扫描行数，并优先采用可靠的跨列相关性估算。
pub fn AdjustRowCountForTableScanByLimit(
    sctx: &dyn planctx::PlanContext,
    dsStatsInfo: &property::StatsInfo,
    dsTableStats: &property::StatsInfo,
    dsStatisticTable: &statistics::Table,
    path: &util::AccessPath,
    expectedCnt: f64,
    isMatchProp: bool,
    desc: bool,
) -> f64 {
    let mut rowCount = path.CountAfterAccess;
    if expectedCnt < dsStatsInfo.RowCount {
        let selectivity = dsStatsInfo.RowCount / path.CountAfterAccess;
        let uniformEst = path.CountAfterAccess.min(expectedCnt / selectivity);
        let (corrEst, ok, corr) = crossEstimateTableRowCount(
            sctx,
            dsStatsInfo,
            dsTableStats,
            dsStatisticTable,
            path,
            expectedCnt,
            desc,
        );
        if ok {
            // 为降低相关性修正风险，至少保留均匀分布估算；原实现尚未加入“至少扫描一个 region”的下界。
            rowCount = uniformEst.max(corrEst);
        } else if corr.abs() < 1.0 {
            // 相关性未达到精确估算门槛时，用指数因子平滑退回启发式估算。
            let correlationFactor =
                (1.0 - corr.abs()).powf(sctx.GetSessionVars().CorrelationExpFactor as f64);
            rowCount = path.CountAfterAccess.min(uniformEst / correlationFactor);
        }
    }

    if isMatchProp && path.CountAfterAccess > rowCount {
        let orderRatio = sctx.GetSessionVars().OptOrderingIdxSelRatio;
        // 记录变量使用情况，供 EXPLAIN EXPLORE 展示本次估算受哪些会话变量影响。
        sctx.GetSessionVars()
            .RecordRelevantOptVar(vardef::TiDBOptOrderingIdxSelRatio);
        if orderRatio > 0.0 {
            rowCount += (path.CountAfterAccess - rowCount).max(0.0) * orderRatio;
        }
    }
    rowCount
}

// crossEstimateTableRowCount 使用过滤列直方图与 handle 的顺序相关性估算表扫描行数。
fn crossEstimateTableRowCount(
    sctx: &dyn planctx::PlanContext,
    dsStatsInfo: &property::StatsInfo,
    dsTableStats: &property::StatsInfo,
    dsStatisticTable: &statistics::Table,
    path: &util::AccessPath,
    expectedCnt: f64,
    desc: bool,
) -> (f64, bool, f64) {
    // 伪统计、无表过滤器或会话关闭相关性修正时，不进入跨列估算。
    if dsStatisticTable.Pseudo
        || path.TableFilters.is_empty()
        || !sctx.GetSessionVars().EnableCorrelationAdjustment
    {
        return (0.0, false, 0.0);
    }
    let (col, corr) = getMostCorrCol4Handle(
        &path.TableFilters,
        dsStatisticTable,
        sctx.GetSessionVars().CorrelationThreshold,
    );
    crossEstimateRowCount(
        sctx,
        dsStatsInfo,
        dsTableStats,
        path,
        &path.TableFilters,
        col.as_ref(),
        corr,
        expectedCnt,
        desc,
    )
}

/// 按 LIMIT 期望行数调整索引扫描候选行数，并补偿索引外过滤带来的风险。
// AdjustRowCountForIndexScanByLimit 按 LIMIT 调整索引扫描行数，并补偿索引外过滤条件带来的风险。
pub fn AdjustRowCountForIndexScanByLimit(
    sctx: &dyn planctx::PlanContext,
    dsStatsInfo: &property::StatsInfo,
    dsTableStats: &property::StatsInfo,
    dsStatisticTable: &statistics::Table,
    path: &util::AccessPath,
    expectedCnt: f64,
    desc: bool,
) -> f64 {
    let mut rowCount = path.CountAfterAccess;
    let (count, ok, corr) = crossEstimateIndexRowCount(
        sctx,
        dsStatsInfo,
        dsTableStats,
        dsStatisticTable,
        path,
        expectedCnt,
        desc,
    );
    if ok {
        rowCount = count;
    } else if corr.abs() < 1.0 {
        // 均匀分布假设：选择率 0.1 表示平均每扫描十行找到一行，再由相关性因子修正。
        let correlationFactor =
            (1.0 - corr.abs()).powf(sctx.GetSessionVars().CorrelationExpFactor as f64);
        let selectivity = dsStatsInfo.RowCount / rowCount;
        rowCount = (expectedCnt / selectivity / correlationFactor).min(rowCount);
    }

    let orderRatio = sctx.GetSessionVars().OptOrderingIdxSelRatio;
    sctx.GetSessionVars()
        .RecordRelevantOptVar(vardef::TiDBOptOrderingIdxSelRatio);
    if path.CountAfterAccess > rowCount
        && orderRatio > 0.0
        && (!path.IndexFilters.is_empty() || !path.TableFilters.is_empty())
    {
        // 排序索引若不能完成全部过滤，LIMIT 行可能直到扫描区间后部才出现；比例用于表达该风险。
        let rowsToMeetFirst = (path.CountAfterAccess - rowCount) * orderRatio;
        rowCount += rowsToMeetFirst;
    }
    rowCount
}

// crossEstimateIndexRowCount 合并表过滤与索引过滤，再调用公共跨列估算逻辑。
fn crossEstimateIndexRowCount(
    sctx: &dyn planctx::PlanContext,
    dsStatsInfo: &property::StatsInfo,
    dsTableStats: &property::StatsInfo,
    dsStatisticTable: &statistics::Table,
    path: &util::AccessPath,
    expectedCnt: f64,
    desc: bool,
) -> (f64, bool, f64) {
    let filtersLen = path.TableFilters.len() + path.IndexFilters.len();
    if dsStatisticTable.Pseudo
        || filtersLen == 0
        || !sctx.GetSessionVars().EnableCorrelationAdjustment
    {
        return (0.0, false, 0.0);
    }
    let mut filters: Vec<Box<dyn expression::Expression>> = Vec::with_capacity(filtersLen);
    filters.extend(path.TableFilters.iter().cloned());
    filters.extend(path.IndexFilters.iter().cloned());
    crossEstimateRowCount(
        sctx,
        dsStatsInfo,
        dsTableStats,
        path,
        &filters,
        None,
        0.0,
        expectedCnt,
        desc,
    )
}

// crossEstimateRowCount 是表扫描与索引扫描共用的直方图跨列估算流程。
fn crossEstimateRowCount(
    sctx: &dyn planctx::PlanContext,
    dsStatsInfo: &property::StatsInfo,
    dsTableStats: &property::StatsInfo,
    path: &util::AccessPath,
    conds: &[Box<dyn expression::Expression>],
    col: Option<&expression::Column>,
    corr: f64,
    expectedCnt: f64,
    mut desc: bool,
) -> (f64, bool, f64) {
    // 非全范围扫描时，整表直方图不能代表当前访问范围；缺少相关列时同样退回调用方启发式估算。
    let Some(col) = col else {
        return (0.0, false, corr);
    };
    if !path.AccessConds.is_empty() {
        return (0.0, false, corr);
    }
    let colUniqueID = col.UniqueID;
    if corr < 0.0 {
        // 负相关意味着扫描方向与过滤列值顺序相反。
        desc = !desc;
    }
    let (accessConds, remained) = ranger::DetachCondsForColumn(
        sctx.GetRangerCtx(),
        conds.iter().cloned().collect(),
        col.clone(),
    );
    if accessConds.is_empty() {
        return (0.0, false, corr);
    }
    let mut ranger_ctx = sctx.GetRangerCtx().clone();
    let Some(ret_type) = col.RetType.as_ref() else {
        return (0.0, false, corr);
    };
    let rangeResult = ranger::BuildColumnRange(
        accessConds,
        &mut ranger_ctx,
        ret_type,
        types::UnspecifiedLength,
        sctx.GetSessionVars().RangeMaxSize,
    );
    let Ok((ranges, accessConds, _)) = rangeResult else {
        return (0.0, false, corr);
    };
    if ranges.is_empty() || accessConds.is_empty() {
        // 成功构造出空范围表示条件不可能命中，因此估算成功且结果为零。
        return (0.0, true, corr);
    }

    let Some(stats_hist) = dsStatsInfo
        .HistColl
        .as_ref()
        .and_then(|hist| hist.downcast_ref::<statistics::HistColl>())
    else {
        return (0.0, false, corr);
    };
    let Some(table_hist) = dsTableStats
        .HistColl
        .as_ref()
        .and_then(|hist| hist.downcast_ref::<statistics::HistColl>())
    else {
        return (0.0, false, corr);
    };
    let idxIDs = stats_hist.ColUniqueID2IdxIDs.get(&colUniqueID);
    let idxExists = idxIDs.is_some_and(|ids| !ids.is_empty());
    let idxID = idxIDs.and_then(|ids| ids.first()).copied().unwrap_or(-1);
    let (rangeCounts, _, _, ok) =
        getColumnRangeCounts(sctx, colUniqueID, &ranges, table_hist, idxID);
    if !ok {
        return (0.0, false, corr);
    }
    let (convertedRanges, count, isFull) =
        convertRangeFromExpectedCnt(&ranges, &rangeCounts, expectedCnt, desc);
    if isFull {
        return (path.CountAfterAccess, true, 0.0);
    }

    // 有可用索引直方图时按索引范围估算，否则按单列范围估算。
    let rangeCount = if idxExists {
        let converted_refs = convertedRanges.iter().collect::<Vec<_>>();
        match GetRowCountByIndexRanges(sctx, table_hist, idxID, &converted_refs, &[]) {
            Ok(result) => result.Est,
            Err(_) => return (0.0, false, corr),
        }
    } else {
        let converted_refs = convertedRanges.iter().collect::<Vec<_>>();
        match GetRowCountByColumnRanges(sctx, table_hist, colUniqueID, &converted_refs, false) {
            Ok(result) => result.Est,
            Err(_) => return (0.0, false, corr),
        }
    };

    let mut scanCount = rangeCount + expectedCnt - count;
    if !remained.is_empty() {
        // 范围外仍有过滤条件时，按默认选择率反推需要读取的原始行数。
        scanCount /= SelectionFactor;
    }
    (scanCount.min(path.CountAfterAccess), true, 0.0)
}

// getColumnRangeCounts 分别估算每个范围的行数，并在索引路径上同时返回估算上下界。
fn getColumnRangeCounts(
    sctx: &dyn planctx::PlanContext,
    colID: i64,
    ranges: &[ranger::Range],
    histColl: &statistics::HistColl,
    idxID: i64,
) -> (Vec<f64>, f64, f64, bool) {
    let mut rangeCounts = vec![0.0; ranges.len()];
    let mut minCount = 0.0;
    let mut maxCount = 0.0;
    for (index, range) in ranges.iter().enumerate() {
        let count = if idxID >= 0 {
            let idxHist = histColl.GetIdx(idxID);
            if statistics::IndexStatsIsInvalid(idxHist, histColl.Pseudo) {
                return (Vec::new(), 0.0, 0.0, false);
            }
            let one = [range];
            match GetRowCountByIndexRanges(sctx, histColl, idxID, &one, &[]) {
                Ok(result) => {
                    minCount = result.MinEst;
                    maxCount = result.MaxEst;
                    result.Est
                }
                Err(_) => return (Vec::new(), 0.0, 0.0, false),
            }
        } else {
            let colHist = histColl.GetCol(colID);
            if statistics::ColumnStatsIsInvalid(colHist, histColl.Pseudo) {
                return (Vec::new(), 0.0, 0.0, false);
            }
            let one = [range];
            match GetRowCountByColumnRanges(sctx, histColl, colID, &one, false) {
                Ok(result) => result.Est,
                Err(_) => return (Vec::new(), 0.0, 0.0, false),
            }
        };
        rangeCounts[index] = count;
    }
    (rangeCounts, minCount, maxCount, true)
}

// convertRangeFromExpectedCnt 从扫描起点累加范围行数，构造达到 expectedCnt 所需覆盖的边界范围。
pub(crate) fn convertRangeFromExpectedCnt(
    ranges: &[ranger::Range],
    rangeCounts: &[f64],
    expectedCnt: f64,
    desc: bool,
) -> (Vec<ranger::Range>, f64, bool) {
    let mut count = 0.0;
    if desc {
        let mut selected = None;
        for index in (0..ranges.len()).rev() {
            if count + rangeCounts[index] >= expectedCnt {
                selected = Some(index);
                break;
            }
            count += rangeCounts[index];
        }
        let Some(index) = selected else {
            return (Vec::new(), 0.0, true);
        };
        let converted = ranger::Range {
            LowVal: ranges[index].HighVal.clone(),
            HighVal: vec![types::MaxValueDatum()],
            LowExclude: !ranges[index].HighExclude,
            Collators: ranges[index]
                .Collators
                .iter()
                .map(|collator| collator.Clone())
                .collect(),
            ..Default::default()
        };
        (vec![converted], count, false)
    } else {
        let mut selected = None;
        for index in 0..ranges.len() {
            if count + rangeCounts[index] >= expectedCnt {
                selected = Some(index);
                break;
            }
            count += rangeCounts[index];
        }
        let Some(index) = selected else {
            return (Vec::new(), 0.0, true);
        };
        let converted = ranger::Range {
            LowVal: vec![types::Datum::default()],
            HighVal: ranges[index].LowVal.clone(),
            HighExclude: !ranges[index].LowExclude,
            Collators: ranges[index]
                .Collators
                .iter()
                .map(|collator| collator.Clone())
                .collect(),
            ..Default::default()
        };
        (vec![converted], count, false)
    }
}

// getMostCorrCol4Handle 查找与 handle 绝对相关性最大的过滤列。
// 只有条件恰含一列且达到阈值时才返回该列；多列场景仅返回最大相关系数供启发式分支使用。
fn getMostCorrCol4Handle(
    exprs: &[Box<dyn expression::Expression>],
    histColl: &statistics::Table,
    threshold: f64,
) -> (Option<expression::Column>, f64) {
    let cols = expression::ExtractColumnsMapFromExpressions(|_| true, exprs);
    if cols.is_empty() {
        return (None, 0.0);
    }
    let mut corr = 0.0_f64;
    let mut corrCol = None;
    for col in cols.values() {
        let Some(hist) = histColl.GetCol(col.ID) else {
            continue;
        };
        let curCorr = hist.Correlation;
        if corrCol.is_none() || corr.abs() < curCorr.abs() {
            corrCol = Some(col.clone());
            corr = curCorr;
        }
    }
    if cols.len() == 1 && corr.abs() >= threshold {
        (corrCol, corr)
    } else {
        (None, corr)
    }
}
