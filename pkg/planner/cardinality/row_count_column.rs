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

// 列统计行数估算。
//
// 依据列直方图（Histogram，按值域分桶的频率分布）、TopN（高频值列表）、
// CM Sketch（Count-Min 近似频率结构）或伪统计，把 ranger 生成的列 Range
// 转成优化器可用的行数估计；主键 handle 路径可退回整型伪估算。

use crate::*;

// 优化器依据列直方图、TopN、CM Sketch 或伪统计估算行数。
// errors、expression、statistics、types、codec、collate、mathutil 与 ranger 等名称保留为后续跨文件接线的外部依赖。

/// init 对应 Go 包初始化：把 cardinality 中的列/索引行数估算函数挂到 statistics 包变量。
pub fn init() {
    // Rust callers use the explicit cardinality APIs. Keeping registration
    // explicit avoids a statistics -> planner dependency cycle.
}

/// GetRowCountByColumnRanges 对应 Go 的导出函数：按一组列 Range 估算行数。
/// pkIsHandle 表示该列是否为单列主键 handle，伪统计路径会因此改用整型范围估算。
pub fn GetRowCountByColumnRanges(
    sctx: &dyn planctx::PlanContext,
    coll: &statistics::HistColl,
    colUniqueID: i64,
    colRanges: &[&ranger::Range],
    pkIsHandle: bool,
) -> Result<statistics::RowEstimate, errors::Error> {
    let sc = sctx.GetExprCtx().GetEvalCtx();
    let c = coll.GetCol(colUniqueID);
    let mut colInfoID = colUniqueID;
    if !coll.UniqueID2colInfoID.is_empty() {
        // Go map lookup returns zero for an absent key.  A plan can retain a
        // synthetic column while a pseudo histogram only contains table
        // columns, so preserve that non-panicking behavior for stats usage
        // bookkeeping.
        // Go 的 map 缺失键返回零值；伪统计缺少合成列时不得在记账阶段 panic。
        colInfoID = coll
            .UniqueID2colInfoID
            .get(&colUniqueID)
            .copied()
            .unwrap_or_default();
    }
    recordUsedItemStatsStatus(sctx, UsedStatsItem::Column(c), coll.PhysicalID, colInfoID);
    if statistics::ColumnStatsIsInvalid(c, coll.Pseudo) {
        let pseudoResult = if pkIsHandle {
            if colRanges.is_empty() {
                return Ok(statistics::DefaultRowEst(0.0));
            }
            // 主键 handle 的 Datum 类型决定有符号或无符号整型伪估算。
            if colRanges[0].LowVal[0].Kind() == types::KindInt64 {
                getPseudoRowCountBySignedIntRanges(colRanges, coll.RealtimeCount as f64)
            } else {
                getPseudoRowCountByUnsignedIntRanges(colRanges, coll.RealtimeCount as f64)
            }
        } else {
            getPseudoRowCountByColumnRanges(&sc.TypeCtx(), coll.RealtimeCount as f64, colRanges, 0)?
        };
        return Ok(statistics::DefaultRowEst(pseudoResult));
    }
    let c = c.expect("ColumnStatsIsInvalid 为 false 时列统计应存在");
    getColumnRowCount(
        sctx,
        c,
        colRanges,
        coll.RealtimeCount,
        coll.ModifyCount,
        pkIsHandle,
    )
    .map_err(errors::Trace)
}

/// equalRowCountOnColumn 对应 Go 的等值列谓词估算。
/// StatsVer < 2 使用 CM Sketch/Histogram；StatsVer == 2 优先 TopN，再看桶末值，最后均匀分布。
pub fn equalRowCountOnColumn(
    sctx: &dyn planctx::PlanContext,
    c: &statistics::Column,
    val: types::Datum,
    encodedVal: Vec<u8>,
    realtimeRowCount: i64,
    modifyCount: i64,
) -> Result<statistics::RowEstimate, errors::Error> {
    if val.IsNull() {
        return Ok(statistics::DefaultRowEst(c.NullCount as f64));
    }
    if c.StatsVer < statistics::Version2 as i64 {
        // 旧版统计中 Bounds 为空表示所有值都是 NULL。
        if c.Histogram.Bounds.is_empty() {
            return Ok(statistics::DefaultRowEst(0.0));
        }
        if c.Histogram.NDV > 0 && c.OutOfRange(&val) {
            let outOfRangeCnt = outOfRangeEQSelectivity(
                sctx,
                c.Histogram.NDV,
                realtimeRowCount,
                c.TotalRowCount() as i64,
            ) * c.TotalRowCount();
            return Ok(statistics::DefaultRowEst(outOfRangeCnt));
        }
        if let Some(cm) = &c.CMSketch {
            let count = statistics::QueryValue(None, cm, c.TopN.as_ref(), val)
                .map_err(|err| errors::NewNoStackError(err.to_string()))?;
            return Ok(statistics::DefaultRowEst(count as f64));
        }
        let (histRowCount, _) = c.Histogram.EqualRowCount(&val, false);
        return Ok(statistics::DefaultRowEst(histRowCount));
    }

    // Stats version == 2：TopN + Histogram + NULL 共同覆盖已分析数据。
    if c.Histogram.Bounds.is_empty() && c.TopN.Num() == 0 {
        return Ok(statistics::DefaultRowEst(0.0));
    }
    if let Some(topn) = &c.TopN {
        let (rowcount, ok) = topn.QueryTopN(&encodedVal);
        if ok {
            return Ok(statistics::DefaultRowEst(rowcount as f64));
        }
    }
    let (histCnt, matched) = c.Histogram.EqualRowCount(&val, true);
    let histNDV = (c.Histogram.NDV - c.TopN.Num() as i64) as f64;
    // 桶末值如果疑似被新增数据稀释，则不直接信任 Repeat，转入均匀分布兜底。
    if matched
        && histCnt > 0.0
        && !IsLastBucketEndValueUnderrepresented(
            sctx,
            &c.Histogram,
            val,
            histCnt,
            histNDV,
            realtimeRowCount,
            modifyCount,
        )
    {
        return Ok(statistics::DefaultRowEst(histCnt));
    }
    Ok(estimateRowCountWithUniformDistribution(
        sctx,
        c,
        realtimeRowCount,
        modifyCount,
    ))
}

/// getColumnRowCount 对应 Go 的列 Range 行数估算。
/// 每个 Range 分为点范围、小范围枚举和一般区间三类，再按边界开闭修正。
pub fn getColumnRowCount(
    sctx: &dyn planctx::PlanContext,
    c: &statistics::Column,
    ranges: &[&ranger::Range],
    realtimeRowCount: i64,
    modifyCount: i64,
    pkIsHandle: bool,
) -> Result<statistics::RowEstimate, errors::Error> {
    let sc = sctx.GetExprCtx().GetEvalCtx();
    let mut totalCount = statistics::RowEstimate::default();
    for rg in ranges {
        let mut highVal = rg.HighVal[0].clone();
        let mut lowVal = rg.LowVal[0].clone();
        // 字符串列先转换为 collation key，再进入二进制比较和编码。
        if highVal.Kind() == types::KindString {
            highVal.SetBytes(collate::GetCollator(&highVal.Collation()).Key(&highVal.GetString()));
        }
        if lowVal.Kind() == types::KindString {
            lowVal.SetBytes(collate::GetCollator(&lowVal.Collation()).Key(&lowVal.GetString()));
        }
        let cmp = lowVal
            .Compare(
                sc.TypeCtx(),
                &highVal,
                collate::GetBinaryCollator().as_ref(),
            )
            .map_err(errors::Trace)?;
        let lowEncoded = codec::EncodeKey(sc.Location(), Vec::new(), vec![lowVal.clone()])?;
        let highEncoded = codec::EncodeKey(sc.Location(), Vec::new(), vec![highVal.clone()])?;
        if cmp == 0 {
            // case 1: 点范围；主键 handle 至多命中一行。
            if !rg.LowExclude && !rg.HighExclude {
                if pkIsHandle {
                    totalCount.AddAll(1.0);
                    continue;
                }
                let mut cnt = equalRowCountOnColumn(
                    sctx,
                    c,
                    lowVal,
                    lowEncoded,
                    realtimeRowCount,
                    modifyCount,
                )?;
                // 表行数变化后，Go 会按统计增长系数整体放缩估算。
                cnt.MultiplyAll(c.GetIncreaseFactor(realtimeRowCount));
                totalCount.Add(cnt);
            }
            continue;
        }

        // StatsVer 1 对小范围逐点枚举，利用 CM Sketch 提高点查精度。
        if c.StatsVer < 2 {
            let rangeVals = statistics::EnumRangeValues(
                lowVal.clone(),
                highVal.clone(),
                rg.LowExclude,
                rg.HighExclude,
            );
            if let Some(vals) = rangeVals {
                for val in vals {
                    let mut cnt = equalRowCountOnColumn(
                        sctx,
                        c,
                        val,
                        lowEncoded.clone(),
                        realtimeRowCount,
                        modifyCount,
                    )?;
                    cnt.MultiplyAll(c.GetIncreaseFactor(realtimeRowCount));
                    totalCount.Add(cnt);
                }
                continue;
            }
        }

        // case 3: 一般区间。betweenRowCount 返回 [l, h)，随后单独修正边界。
        let mut cnt = betweenRowCountOnColumn(
            sctx,
            c,
            lowVal.clone(),
            highVal.clone(),
            lowEncoded.clone(),
            highEncoded.clone(),
        );
        if rg.LowExclude
            && !lowVal.IsNull()
            && lowVal.Kind() != types::KindMaxValue
            && lowVal.Kind() != types::KindMinNotNull
        {
            let lowCnt = equalRowCountOnColumn(
                sctx,
                c,
                lowVal.clone(),
                lowEncoded.clone(),
                realtimeRowCount,
                modifyCount,
            )?;
            cnt.Subtract(lowCnt);
            cnt.Clamp(0.0, c.NotNullCount());
        }
        if !rg.LowExclude && lowVal.IsNull() {
            cnt.AddAll(c.NullCount as f64);
        }
        if !rg.HighExclude
            && highVal.Kind() != types::KindMaxValue
            && highVal.Kind() != types::KindMinNotNull
        {
            let highCnt = equalRowCountOnColumn(
                sctx,
                c,
                highVal.clone(),
                highEncoded,
                realtimeRowCount,
                modifyCount,
            )?;
            cnt.Add(highCnt);
        }
        cnt.Clamp(0.0, realtimeRowCount as f64);

        let increaseFactor = c.GetIncreaseFactor(realtimeRowCount);
        cnt.MultiplyAll(increaseFactor);

        // 已覆盖实时行数近似全集时，不再补 out-of-range 估算，避免重复计数。
        let atFullRange = cnt.Est >= realtimeRowCount as f64 * (1.0 - cost::ToleranceFactor);
        if !atFullRange && ((c.OutOfRange(&lowVal) && !lowVal.IsNull()) || c.OutOfRange(&highVal)) {
            let mut histNDV = c.NDV;
            if c.StatsVer == statistics::Version2 as i64 {
                histNDV -= c.TopN.Num() as i64;
            }
            let mut count = statistics::RowEstimate::default();
            count.Add(c.Histogram.OutOfRangeRowCount(
                &lowVal,
                &highVal,
                realtimeRowCount,
                modifyCount,
                histNDV,
                true,
                sctx.GetSessionVars().RiskRangeSkewRatio,
            ));
            cnt.Add(count);
        }
        totalCount.Add(cnt);
    }
    totalCount.Clamp(1.0, realtimeRowCount as f64);
    Ok(totalCount)
}

/// betweenRowCountOnColumn 对应 Go 的 [l, r) 区间估算。
/// StatsVer 2 在直方图结果上补 TopN 命中数，但只补 Est，不改变 Min/Max 边界。
pub fn betweenRowCountOnColumn(
    sctx: &dyn planctx::PlanContext,
    c: &statistics::Column,
    l: types::Datum,
    r: types::Datum,
    lowEncoded: Vec<u8>,
    highEncoded: Vec<u8>,
) -> statistics::RowEstimate {
    let mut histBetweenCnt = c.Histogram.BetweenRowCount(&l, &r);
    if c.StatsVer <= statistics::Version1 as i64 {
        return histBetweenCnt;
    }
    let topNCnt = c
        .TopN
        .as_ref()
        .map_or(0, |topn| topn.BetweenCount(&lowEncoded, &highEncoded)) as f64;
    histBetweenCnt.Est += topNCnt;
    histBetweenCnt
}

/// getPseudoRowCountWithPartialStats 对应 Go 在索引统计缺失但部分列统计可用时的伪估算。
/// 单列索引直接复用列估算，多列索引用独立性乘法和相关性上界同时累计。
pub fn getPseudoRowCountWithPartialStats(
    sctx: &dyn planctx::PlanContext,
    coll: &statistics::HistColl,
    indexRanges: &[&ranger::Range],
    tableRowCount: f64,
    idxCols: &[&expression::Column],
) -> Result<(f64, f64), errors::Error> {
    if tableRowCount == 0.0 {
        return Ok((0.0, 0.0));
    }
    if idxCols.len() == 1 {
        let countEst =
            GetRowCountByColumnRanges(sctx, coll, idxCols[0].UniqueID, indexRanges, false)?;
        return Ok((countEst.Est, 0.0));
    }
    let mut tmpRan = vec![ranger::Range {
        LowVal: vec![types::Datum::default()],
        HighVal: vec![types::Datum::default()],
        Collators: vec![collate::GetBinaryCollator()],
        ..Default::default()
    }];
    let mut totalCount = 0.0;
    let mut maxCount = 0.0;
    for indexRange in indexRanges {
        let mut selectivity = 1.0;
        let mut corrSelectivity: f64 = 1.0;
        for i in 0..indexRange.LowVal.len() {
            tmpRan[0].LowVal[0] = indexRange.LowVal[i].clone();
            tmpRan[0].HighVal[0] = indexRange.HighVal[i].clone();
            tmpRan[0].Collators[0] = partialStatsRangeCollator(indexRange);
            if i == indexRange.LowVal.len() - 1 {
                tmpRan[0].LowExclude = indexRange.LowExclude;
                tmpRan[0].HighExclude = indexRange.HighExclude;
            }
            let colID = idxCols[i].UniqueID;
            // GetRowCountByColumnRanges 内部会处理列统计无效并回退到伪估算。
            let countEst = GetRowCountByColumnRanges(sctx, coll, colID, &[&tmpRan[0]], false)
                .map_err(errors::Trace)?;
            let tempSelectivity = countEst.Est / tableRowCount;
            selectivity *= tempSelectivity;
            corrSelectivity = corrSelectivity.min(tempSelectivity);
        }
        totalCount += selectivity * tableRowCount;
        maxCount += corrSelectivity * tableRowCount;
    }
    totalCount = mathutil::Clamp(totalCount, 1.0, tableRowCount);
    Ok((totalCount, maxCount))
}

// Go reuses indexRange.Collators[0] for every temporary single-column range.
// This matters when an index range contains columns with different collations.
pub(crate) fn partialStatsRangeCollator(indexRange: &ranger::Range) -> Box<dyn collate::Collator> {
    indexRange.Collators[0].Clone()
}
