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

// 索引统计行数估算。
//
// 按索引 Range 结合索引直方图、TopN、CM Sketch 与列统计，估算命中行数；
// 覆盖 StatsVer1/V2、点查询、区间、指数退避（exp-backoff，多列独立性加权）
// 以及 out-of-range（查询值落在直方图边界外）等路径。

use crate::*;

// 优化器读取索引统计、列统计和范围边界来估算行数。
// bytes、slices、time、failpoint、statistics、types、codec、collate、ranger 等依赖均保留为跨文件接线占位。

/// GetRowCountByIndexRanges 对应 Go 的导出入口：按索引 Range 估算行数。
/// idxCols 在索引统计无效、虚拟列回退和 exp-backoff 中用于定位列统计。
pub fn GetRowCountByIndexRanges(
    sctx: &dyn planctx::PlanContext,
    coll: &statistics::HistColl,
    idxID: i64,
    indexRanges: &[&ranger::Range],
    idxCols: &[&expression::Column],
) -> Result<statistics::RowEstimate, errors::Error> {
    let sc = sctx.GetExprCtx().GetEvalCtx();
    let idx = coll.GetIdx(idxID);
    recordUsedItemStatsStatus(sctx, UsedStatsItem::Index(idx), coll.PhysicalID, idxID);
    // 覆盖全量且非 MV/非 partial 的索引可以直接返回实时行数，避免触发异步加载。
    if idx.is_some() && canSkipIndexEstimation(idx.unwrap(), indexRanges) {
        let (realtimeCnt, _) = coll.GetScaledRealtimeAndModifyCnt(idx.unwrap());
        return Ok(statistics::DefaultRowEst(realtimeCnt as f64));
    }
    if statistics::IndexStatsIsInvalid(idx, coll.Pseudo) {
        if hasColumnStats(sctx, coll, idxCols)
            && !indexRanges.iter().any(|range| range.IsFullRange(false))
        {
            let (count, maxCount) = getPseudoRowCountWithPartialStats(
                sctx,
                coll,
                indexRanges,
                coll.RealtimeCount as f64,
                idxCols,
            )?;
            return Ok(statistics::RowEstimate {
                Est: count,
                MinEst: count,
                MaxEst: maxCount,
            });
        }
        let mut colsLen = -1isize;
        if let Some(idx) = idx {
            if idx.InfoRef().Unique {
                colsLen = idx.InfoRef().Columns.len() as isize;
            }
        }
        let count = getPseudoRowCountByIndexRanges(
            &sc.TypeCtx(),
            indexRanges,
            coll.RealtimeCount as f64,
            colsLen as usize,
        )?;
        return Ok(statistics::DefaultRowEst(count));
    }
    let idx = idx.expect("IndexStatsIsInvalid 为 false 时索引统计应存在");
    let (realtimeCnt, modifyCount) = coll.GetScaledRealtimeAndModifyCnt(idx);
    let result = if idx.CMSketch.is_some() && idx.StatsVer == statistics::Version1 as i64 {
        let count = getIndexRowCountForStatsV1(sctx, coll, idxID, indexRanges)?;
        statistics::DefaultRowEst(count)
    } else {
        getIndexRowCountForStatsV2(
            sctx,
            idx,
            Some(coll),
            indexRanges,
            idxCols,
            realtimeCnt,
            modifyCount,
        )?
    };
    Ok(result)
}

/// getIndexRowCountForStatsV1 对应 Go 的旧版统计路径。
/// 等值前缀走 CM Sketch，首个范围列再借助列或索引统计估算剩余区间选择率。
pub fn getIndexRowCountForStatsV1(
    sctx: &dyn planctx::PlanContext,
    coll: &statistics::HistColl,
    idxID: i64,
    indexRanges: &[&ranger::Range],
) -> Result<f64, errors::Error> {
    let sc = sctx.GetExprCtx().GetEvalCtx();
    let idx = coll.GetIdx(idxID).expect("索引统计应存在");
    let mut totalCount = 0.0;
    for ran in indexRanges {
        let mut rangePosition = getOrdinalOfRangeCond(sc, ran);
        let mut rangeVals: Option<Vec<types::Datum>> = None;
        // 尝试枚举最后一列小范围，把等值前缀扩展到 rangePosition。
        if rangePosition != ran.LowVal.len() {
            rangeVals = statistics::EnumRangeValues(
                ran.LowVal[rangePosition].clone(),
                ran.HighVal[rangePosition].clone(),
                ran.LowExclude,
                ran.HighExclude,
            );
            if rangeVals.is_some() {
                rangePosition += 1;
            }
        }
        if rangePosition == 0 || isSingleColIdxNullRange(idx, ran) {
            let (realtimeCnt, modifyCount) = coll.GetScaledRealtimeAndModifyCnt(idx);
            let rowEstimate = getIndexRowCountForStatsV2(
                sctx,
                idx,
                None,
                &[*ran],
                &[],
                realtimeCnt,
                modifyCount,
            )?;
            totalCount += rowEstimate.Est;
            continue;
        }

        let mut selectivity = 0.0;
        if rangeVals.is_none() {
            let bytes = codec::EncodeKey(
                sc.Location(),
                Vec::new(),
                ran.LowVal[..rangePosition].to_vec(),
            )
            .map_err(|err| errors::NewNoStackError(err.to_string()))?;
            selectivity = getEqualCondSelectivity(sctx, coll, idx, bytes, rangePosition, ran)?;
        } else {
            let mut bytes = codec::EncodeKey(
                sc.Location(),
                Vec::new(),
                ran.LowVal[..rangePosition - 1].to_vec(),
            )
            .map_err(|err| errors::NewNoStackError(err.to_string()))?;
            let prefixLen = bytes.len();
            for val in rangeVals.unwrap() {
                bytes.truncate(prefixLen);
                bytes = codec::EncodeKey(sc.Location(), bytes, vec![val])?;
                selectivity +=
                    getEqualCondSelectivity(sctx, coll, idx, bytes.clone(), rangePosition, ran)?;
            }
        }

        // 若还有范围列，利用该列自己的统计对前缀选择率继续缩放。
        if rangePosition != ran.LowVal.len() {
            let rang = ranger::Range {
                LowVal: vec![ran.LowVal[rangePosition].clone()],
                LowExclude: ran.LowExclude,
                HighVal: vec![ran.HighVal[rangePosition].clone()],
                HighExclude: ran.HighExclude,
                Collators: vec![collate::GetCollator(&ran.LowVal[rangePosition].Collation())],
                ..Default::default()
            };
            let colUniqueIDs = coll
                .Idx2ColUniqueIDs
                .get(&idxID)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let colUniqueID = if rangePosition >= colUniqueIDs.len() {
                -1
            } else {
                colUniqueIDs[rangePosition]
            };
            let count = if let Some(idxIDs) = coll.ColUniqueID2IdxIDs.get(&colUniqueID) {
                if !idxIDs.is_empty() {
                    GetRowCountByIndexRanges(sctx, coll, idxIDs[0], &[&rang], &[])?.Est
                } else {
                    GetRowCountByColumnRanges(sctx, coll, colUniqueID, &[&rang], false)?.Est
                }
            } else {
                GetRowCountByColumnRanges(sctx, coll, colUniqueID, &[&rang], false)?.Est
            };
            selectivity = selectivity * count / idx.TotalRowCount();
        }
        totalCount += selectivity * idx.TotalRowCount();
    }
    Ok(totalCount.min(idx.TotalRowCount()))
}

/// isSingleColIdxNullRange 判断单列索引上的 [NULL, NULL] 范围。
pub fn isSingleColIdxNullRange(idx: &statistics::Index, ran: &ranger::Range) -> bool {
    if idx.InfoRef().Columns.len() > 1 {
        return false;
    }
    let l = &ran.LowVal[0];
    let h = &ran.HighVal[0];
    l.IsNull() && h.IsNull()
}

/// getIndexRowCountForStatsV2 对应 Go 的新版统计路径。
/// 它在点范围、一般区间、exp-backoff 和 out-of-range 修正之间按顺序切换。
pub fn getIndexRowCountForStatsV2(
    sctx: &dyn planctx::PlanContext,
    idx: &statistics::Index,
    coll: Option<&statistics::HistColl>,
    indexRanges: &[&ranger::Range],
    idxCols: &[&expression::Column],
    realtimeRowCount: i64,
    modifyCount: i64,
) -> Result<statistics::RowEstimate, errors::Error> {
    let sc = sctx.GetExprCtx().GetEvalCtx();
    let isSingleColIdx = idx.InfoRef().Columns.len() == 1;
    let mut totalCount = statistics::RowEstimate::default();
    for indexRange in indexRanges {
        let mut count = statistics::RowEstimate::default();
        let mut lb = codec::EncodeKey(sc.Location(), Vec::new(), indexRange.LowVal.clone())?;
        let mut rb = codec::EncodeKey(sc.Location(), Vec::new(), indexRange.HighVal.clone())?;
        let fullLen = indexRange.LowVal.len() == indexRange.HighVal.len()
            && indexRange.LowVal.len() == idx.InfoRef().Columns.len();
        if lb == rb {
            // case 1: 点范围。唯一索引且非 NULL 时至多一行。
            if indexRange.LowExclude || indexRange.HighExclude {
                continue;
            }
            if fullLen {
                if idx.InfoRef().Unique {
                    if !indexRange.IsOnlyNull() {
                        totalCount.AddAll(1.0);
                        continue;
                    }
                    totalCount = statistics::DefaultRowEst(idx.NullCount as f64);
                    continue;
                }
                count = equalRowCountOnIndex(sctx, idx, lb.clone(), realtimeRowCount, modifyCount);
                count.MultiplyAll(idx.GetIncreaseFactor(realtimeRowCount));
                totalCount.Add(count);
                continue;
            }
        }

        // case 2: 一般区间。Go 将最终区间调整为 [low, high)。
        if indexRange.LowExclude {
            lb = kv::Key(lb).PrefixNext().0;
        }
        if !indexRange.HighExclude {
            rb = kv::Key(rb).PrefixNext().0;
        }
        let l = types::NewBytesDatum(lb.clone());
        let r = types::NewBytesDatum(rb.clone());
        let lowIsNull = lb == nullKeyBytes();
        if isSingleColIdx && lowIsNull {
            count.AddAll(idx.Histogram.NullCount as f64);
        }

        let mut expBackoffSuccess = false;
        if getOrdinalOfRangeCond(sc, indexRange) > 0
            && idx.StatsVer >= statistics::Version2 as i64
            && coll.is_some()
        {
            let (expBackoffSel, minSel, maxSel, success) =
                expBackoffEstimation(sctx, idx, coll.unwrap(), indexRange, idxCols)?;
            expBackoffSuccess = success;
            if expBackoffSuccess {
                let mut expBackoffResult = statistics::RowEstimate {
                    Est: expBackoffSel,
                    MinEst: minSel,
                    MaxEst: maxSel,
                };
                expBackoffResult.MultiplyAll(idx.TotalRowCount());
                let mut upperLimit = expBackoffResult.Est;
                // 多列直方图给 exp-backoff 一个上界，避免独立性修正超过实际区间容量。
                if idx.Histogram.Len() > 0 {
                    let (_, lowerBkt, _, _) = idx.Histogram.LocateBucket(&l);
                    let (_, upperBkt, _, _) = idx.Histogram.LocateBucket(&r);
                    let mut preCount = 0.0;
                    if lowerBkt > 0 {
                        preCount = idx.Histogram.Buckets[lowerBkt - 1].Count as f64;
                    }
                    let upperCnt = idx.Histogram.Buckets[upperBkt].Count as f64;
                    upperLimit = upperCnt - preCount;
                    upperLimit += idx
                        .TopN
                        .as_ref()
                        .map_or(0, |topn| topn.BetweenCount(&lb, &rb))
                        as f64;
                }
                if expBackoffResult.Est > upperLimit {
                    expBackoffResult.Est = upperLimit;
                }
                count.Add(expBackoffResult);
            }
        }
        if !expBackoffSuccess {
            count.Add(betweenRowCountOnIndex(sctx, idx, l.clone(), r.clone()));
        }

        count.MultiplyAll(idx.GetIncreaseFactor(realtimeRowCount));

        let atFullRange = count.Est >= realtimeRowCount as f64 * (1.0 - cost::ToleranceFactor);
        if !atFullRange
            && ((outOfRangeOnIndex(idx, l.clone()) && !(isSingleColIdx && lowIsNull))
                || outOfRangeOnIndex(idx, r.clone()))
        {
            let mut histNDV = idx.NDV;
            if idx.StatsVer == statistics::Version2 as i64 {
                let coll = coll.expect("StatsVer2 out-of-range 路径需要 HistColl");
                let colIDs = coll
                    .Idx2ColUniqueIDs
                    .get(&idx.Histogram.ID)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let c = if !colIDs.is_empty() {
                    coll.GetCol(colIDs[0])
                } else {
                    None
                };
                let isSingleColRange = indexRange.LowVal.len() == indexRange.HighVal.len()
                    && indexRange.LowVal.len() == 1;
                if isSingleColRange
                    && c.is_some()
                    && c.unwrap().Histogram.NDV > 0
                    && c.unwrap().Histogram.Len() > 0
                {
                    let c = c.unwrap();
                    histNDV = c.Histogram.NDV - c.TopN.Num() as i64;
                    count.Add(c.Histogram.OutOfRangeRowCount(
                        &indexRange.LowVal[0],
                        &indexRange.HighVal[0],
                        realtimeRowCount,
                        modifyCount,
                        histNDV,
                        true,
                        sctx.GetSessionVars().RiskRangeSkewRatio,
                    ));
                } else {
                    // 多列 out-of-range 仍使用编码后的索引直方图，和 Go TODO 行为保持一致。
                    histNDV -= idx.TopN.Num() as i64;
                    count.Add(idx.Histogram.OutOfRangeRowCount(
                        &l,
                        &r,
                        realtimeRowCount,
                        modifyCount,
                        histNDV,
                        true,
                        sctx.GetSessionVars().RiskRangeSkewRatio,
                    ));
                }
            } else {
                count.Add(idx.Histogram.OutOfRangeRowCount(
                    &l,
                    &r,
                    realtimeRowCount,
                    modifyCount,
                    histNDV,
                    true,
                    sctx.GetSessionVars().RiskRangeSkewRatio,
                ));
            }
        }
        totalCount.Add(count);
    }
    totalCount.Clamp(1.0, realtimeRowCount as f64);
    Ok(totalCount)
}

/// nullKeyBytes 对应 Go 的包级变量，由 NULL datum 编码得到。
fn nullKeyBytes() -> Vec<u8> {
    codec::EncodeKey(chrono_tz::UTC, Vec::new(), vec![types::Datum::default()]).unwrap_or_default()
}

/// StatsProvider 对应 Go interface，为列统计和索引统计抽象统一分布估算输入。
pub trait StatsProvider {
    /// 返回直方图视图。
    fn GetHistogram(&self) -> &statistics::Histogram;
    /// 返回 TopN（可能为空）。
    fn GetTopN(&self) -> Option<&statistics::TopN>;
    /// 统计覆盖的总行数（含 NULL）。
    fn TotalRowCount(&self) -> f64;
    /// 相对实时行数的增长系数，用于把分析时刻的计数放大到当前。
    fn GetIncreaseFactor(&self, realtimeRowCount: i64) -> f64;
}

impl StatsProvider for statistics::Column {
    fn GetHistogram(&self) -> &statistics::Histogram {
        &self.Histogram
    }
    fn GetTopN(&self) -> Option<&statistics::TopN> {
        self.TopN.as_ref()
    }
    fn TotalRowCount(&self) -> f64 {
        statistics::Column::TotalRowCount(self)
    }
    fn GetIncreaseFactor(&self, realtimeRowCount: i64) -> f64 {
        statistics::Column::GetIncreaseFactor(self, realtimeRowCount)
    }
}

impl StatsProvider for statistics::Index {
    fn GetHistogram(&self) -> &statistics::Histogram {
        &self.Histogram
    }
    fn GetTopN(&self) -> Option<&statistics::TopN> {
        self.TopN.as_ref()
    }
    fn TotalRowCount(&self) -> f64 {
        statistics::Index::TotalRowCount(self)
    }
    fn GetIncreaseFactor(&self, realtimeRowCount: i64) -> f64 {
        statistics::Index::GetIncreaseFactor(self, realtimeRowCount)
    }
}

/// estimateRowCountWithUniformDistribution 对应 Go 的 TopN/直方图未覆盖值均匀分布估算。
pub fn estimateRowCountWithUniformDistribution(
    sctx: &dyn planctx::PlanContext,
    stats: &dyn StatsProvider,
    realtimeRowCount: i64,
    modifyCount: i64,
) -> statistics::RowEstimate {
    let histogram = stats.GetHistogram();
    let topN = stats.GetTopN();
    let histNDV = (histogram.NDV - topN.Num() as i64) as f64;
    let totalRowCount = stats.TotalRowCount();
    let increaseFactor = stats.GetIncreaseFactor(realtimeRowCount);
    let mut notNullCount = histogram.NotNullCount();
    let avgRowEstimate = if histNDV <= 0.0 || notNullCount == 0.0 {
        // Branch 1：TopN 覆盖全部 NDV，或没有可用直方图。
        if histNDV > 0.0 && modifyCount == 0 {
            return statistics::DefaultRowEst(((topN.MinCount() - 1) as f64).max(1.0));
        }
        if notNullCount <= 0.0 {
            notNullCount = totalRowCount - histogram.NullCount as f64;
        }
        outOfRangeFullNDV(
            histogram.NDV as f64,
            totalRowCount,
            notNullCount,
            realtimeRowCount as f64,
            increaseFactor,
            modifyCount,
        )
    } else {
        // Branch 2：剩余 NDV 落在直方图里，使用平均桶行数。
        notNullCount / histNDV
    };

    let skewRatio = sctx.GetSessionVars().RiskEqSkewRatio;
    sctx.GetSessionVars()
        .RecordRelevantOptVar(vardef::TiDBOptRiskEqSkewRatio);
    if skewRatio > 0.0 {
        let mut skewEstimate = notNullCount - (histNDV - 1.0);
        let minTopN = topN.MinCount();
        if minTopN > 0 {
            skewEstimate = skewEstimate.min(minTopN as f64);
        }
        return statistics::CalculateSkewRatioCounts(avgRowEstimate, skewEstimate, skewRatio);
    }
    statistics::DefaultRowEst(avgRowEstimate)
}

/// equalRowCountOnIndex 对应 Go 的索引等值估算。
pub fn equalRowCountOnIndex(
    sctx: &dyn planctx::PlanContext,
    idx: &statistics::Index,
    b: Vec<u8>,
    realtimeRowCount: i64,
    modifyCount: i64,
) -> statistics::RowEstimate {
    if idx.InfoRef().Columns.len() == 1 && b == nullKeyBytes() {
        return statistics::DefaultRowEst(idx.Histogram.NullCount as f64);
    }
    let val = types::NewBytesDatum(b.clone());
    if idx.StatsVer < statistics::Version2 as i64 {
        if idx.Histogram.NDV > 0 && outOfRangeOnIndex(idx, val.clone()) {
            let outOfRangeCnt = outOfRangeEQSelectivity(
                sctx,
                idx.Histogram.NDV,
                realtimeRowCount,
                idx.TotalRowCount() as i64,
            ) * idx.TotalRowCount();
            return statistics::DefaultRowEst(outOfRangeCnt);
        }
        if idx.CMSketch.is_some() {
            return statistics::DefaultRowEst(idx.QueryBytes(&b) as f64);
        }
        let (histRowCount, _) = idx.Histogram.EqualRowCount(&val, false);
        return statistics::DefaultRowEst(histRowCount);
    }
    if let Some(topn) = &idx.TopN {
        let (count, found) = topn.QueryTopN(&b);
        if found {
            return statistics::DefaultRowEst(count as f64);
        }
    }
    let (histCnt, matched) = idx.Histogram.EqualRowCount(&val, true);
    let histNDV = (idx.Histogram.NDV - idx.TopN.Num() as i64) as f64;
    if matched
        && !IsLastBucketEndValueUnderrepresented(
            sctx,
            &idx.Histogram,
            val,
            histCnt,
            histNDV,
            realtimeRowCount,
            modifyCount,
        )
    {
        return statistics::DefaultRowEst(histCnt);
    }
    // 剩余值统一走公共均匀分布估算，包含 out-of-range 的默认处理。
    estimateRowCountWithUniformDistribution(sctx, idx, realtimeRowCount, modifyCount)
}

/// expBackoffEstimation 对应 Go 的多列指数退避估算。
/// 先估算每个前缀列选择率，再排序并按 1, 1/2, 1/4... 权重组合。
pub fn expBackoffEstimation(
    sctx: &dyn planctx::PlanContext,
    idx: &statistics::Index,
    coll: &statistics::HistColl,
    indexRange: &ranger::Range,
    idxCols: &[&expression::Column],
) -> Result<(f64, f64, f64, bool), errors::Error> {
    let mut tmpRan = vec![ranger::Range {
        LowVal: vec![types::Datum::default()],
        HighVal: vec![types::Datum::default()],
        Collators: vec![collate::GetBinaryCollator()],
        ..Default::default()
    }];
    let colsIDs = coll
        .Idx2ColUniqueIDs
        .get(&idx.Histogram.ID)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut singleColumnEstResults = Vec::<f64>::with_capacity(indexRange.LowVal.len());
    let mut minSel: f64 = 1.0;
    let mut maxSel: f64 = 1.0;
    for i in 0..indexRange.LowVal.len() {
        tmpRan[0].LowVal[0] = indexRange.LowVal[i].clone();
        tmpRan[0].HighVal[0] = indexRange.HighVal[i].clone();
        tmpRan[0].Collators[0] = indexRange.Collators[0].Clone();
        if i == indexRange.LowVal.len() - 1 {
            tmpRan[0].LowExclude = indexRange.LowExclude;
            tmpRan[0].HighExclude = indexRange.HighExclude;
        }
        // 防御 Go 中的 colsIDs 越界 panic；缺失映射时跳过该列。
        if colsIDs.is_empty() || i >= colsIDs.len() {
            continue;
        }
        let colID = colsIDs[i];
        let mut selectivity = 0.0;
        let mut foundStats = false;
        if !statistics::ColumnStatsIsInvalid(coll.GetCol(colID), coll.Pseudo) {
            foundStats = true;
            let countEst = GetRowCountByColumnRanges(sctx, coll, colID, &[&tmpRan[0]], false)?;
            selectivity = countEst.Est / coll.RealtimeCount as f64;
            maxSel = maxSel.min(countEst.MaxEst / coll.RealtimeCount as f64);
        }
        if let Some(idxIDs) = coll.ColUniqueID2IdxIDs.get(&colID) {
            if !foundStats && indexRange.LowVal.len() > 1 {
                // 只在多列输入时递归索引估算，避免单列索引无限递归。
                for idxID in idxIDs {
                    let idxStats = coll.GetIdx(*idxID);
                    if idxStats.is_none() || statistics::IndexStatsIsInvalid(idxStats, coll.Pseudo)
                    {
                        continue;
                    }
                    foundStats = true;
                    let countResult =
                        GetRowCountByIndexRanges(sctx, coll, *idxID, &[&tmpRan[0]], &[])?;
                    let (realtimeCnt, _) = coll.GetScaledRealtimeAndModifyCnt(idxStats.unwrap());
                    selectivity = countResult.Est / realtimeCnt as f64;
                    maxSel = maxSel.min(countResult.MaxEst / coll.RealtimeCount as f64);
                    break;
                }
            }
        }
        if !foundStats {
            // 虚拟列无列统计时，若索引统计存在，交回索引直方图路径兜底。
            if i < idxCols.len()
                && idxCols[i].VirtualExpr.is_some()
                && (idx.Histogram.Len() > 0 || idx.TopN.Num() > 0)
            {
                return Ok((0.0, 0.0, 0.0, false));
            }
            continue;
        }
        singleColumnEstResults.push(selectivity);
        minSel *= selectivity;
    }
    singleColumnEstResults.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    if fail::eval("cleanEstResults", |_| ()).is_some() {
        singleColumnEstResults.clear();
    }
    let l = singleColumnEstResults.len();
    if l == 1 {
        return Ok((
            singleColumnEstResults[0],
            singleColumnEstResults[0],
            singleColumnEstResults[0],
            true,
        ));
    }
    if l == 0 {
        return Ok((0.0, 0.0, 0.0, false));
    }

    let mut histNDV = coll.RealtimeCount;
    if idx.NDV > 0 {
        histNDV = idx.NDV;
    }
    let mut idxLowBound = 1.0 / std::cmp::min(histNDV, coll.RealtimeCount) as f64;
    let mut minBound = idxLowBound;
    if l < idx.InfoRef().Columns.len() {
        idxLowBound /= 0.9;
    }
    maxSel = idxLowBound.max(maxSel);
    minSel = minBound.max(minSel);
    let maxCols = std::cmp::min(MaxExponentialBackoffCols, l);
    for i in 0..maxCols {
        minBound = minBound.min(singleColumnEstResults[i]);
    }
    let multResult = ApplyExponentialBackoff(&singleColumnEstResults, minBound, 1.0);
    Ok((multResult, minSel, maxSel, true))
}

/// outOfRangeOnIndex 检查编码后的索引 Datum 是否落在直方图范围外。
pub fn outOfRangeOnIndex(idx: &statistics::Index, val: types::Datum) -> bool {
    if !idx.Histogram.OutOfRange(&val) {
        return false;
    }
    if idx.Histogram.Len() > 0 && matchPrefix(idx.Histogram.GetLower(0), &val) {
        return false;
    }
    true
}

/// matchPrefix 对应 Go 的字符串/字节前缀匹配，用于修正索引编码边界。
pub fn matchPrefix(row: &types::Datum, ad: &types::Datum) -> bool {
    match ad.Kind() {
        types::KindString | types::KindBytes | types::KindBinaryLiteral | types::KindMysqlBit => {
            row.GetString().starts_with(&ad.GetString())
        }
        _ => false,
    }
}

/// betweenRowCountOnIndex 对应 Go 的索引 [l, r) 区间估算。
pub fn betweenRowCountOnIndex(
    sctx: &dyn planctx::PlanContext,
    idx: &statistics::Index,
    l: types::Datum,
    r: types::Datum,
) -> statistics::RowEstimate {
    let mut histBetweenResult = idx.Histogram.BetweenRowCount(&l, &r);
    if idx.StatsVer == statistics::Version1 as i64 {
        return histBetweenResult;
    }
    let topNCnt = idx
        .TopN
        .as_ref()
        .map_or(0, |topn| topn.BetweenCount(&l.GetBytes(), &r.GetBytes())) as f64;
    histBetweenResult.AddAll(topNCnt);
    histBetweenResult
}

/// getOrdinalOfRangeCond 返回第一个非等值范围列位置；不存在时返回 LowVal 长度。
pub fn getOrdinalOfRangeCond(
    sc: &dyn expression::exprctx::EvalContext,
    ran: &ranger::Range,
) -> usize {
    for i in 0..ran.LowVal.len() {
        let cmp = ran.LowVal[i].Compare(sc.TypeCtx(), &ran.HighVal[i], ran.Collators[0].as_ref());
        if cmp.is_err() {
            return 0;
        }
        if cmp.unwrap() != 0 {
            return i;
        }
    }
    ran.LowVal.len()
}

/// canSkipIndexEstimation 判断是否可以对完整索引范围直接返回实时行数。
pub fn canSkipIndexEstimation(idx: &statistics::Index, indexRanges: &[&ranger::Range]) -> bool {
    if !idx.InfoRef().ConditionExprString.is_empty() || idx.InfoRef().MVIndex {
        return false;
    }
    indexRanges.iter().any(|ran| isFullRangeIncludingNulls(ran))
}

/// isFullRangeIncludingNulls 判断单个 Range 是否真正覆盖 NULL 到 +inf。
/// 与 ranger.IsFullRange 不同，这里要求低边界是包含 NULL，而不是 MinNotNull。
pub fn isFullRangeIncludingNulls(ran: &ranger::Range) -> bool {
    if ran.LowVal.len() != ran.HighVal.len() || ran.LowVal.is_empty() {
        return false;
    }
    if ran.LowExclude || ran.HighExclude {
        return false;
    }
    for i in 0..ran.LowVal.len() {
        if ran.LowVal[i].Kind() != types::KindNull {
            return false;
        }
        if ran.HighVal[i].Kind() != types::KindMaxValue {
            return false;
        }
    }
    true
}

/// hasColumnStats 检查给定索引列中是否至少有一个可用列统计。
pub fn hasColumnStats(
    sctx: &dyn planctx::PlanContext,
    coll: &statistics::HistColl,
    idxCols: &[&expression::Column],
) -> bool {
    if idxCols.is_empty() {
        return false;
    }
    for col in idxCols {
        if !statistics::ColumnStatsIsInvalid(coll.GetCol(col.UniqueID), coll.Pseudo) {
            return true;
        }
    }
    false
}
