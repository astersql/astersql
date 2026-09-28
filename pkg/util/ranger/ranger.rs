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

// 索引与表扫描 Range 构造及合并工具，对齐 ranger.go。
//
// 将端点转为可扫描的 Range、处理多列 fanout、UnionRanges 合并，
// 以及前缀索引裁剪与 Range 的 SQL 字符串化。

// 索引与表扫描 Range 构造及合并工具，对齐 ranger.go。

use crate::detacher_impl::rangeDetacher;
use crate::points_impl::{builder, getFullRange, point};
use crate::{
    EmptyRangeSize, FullIntRange, FullNotNullRange, FullRange, Range, Ranges, ast, bytes, charset,
    codec, collate, errctx, errors, format, mysql, types,
};

// validInterval 对应 Go 的 validInterval。
// 它把低/高端点编码成 KV key，再结合开闭区间调整边界，用字节序比较判断区间是否非空。
fn validInterval(
    ec: &errctx::Context,
    loc: &chrono_tz::Tz,
    low: &point,
    high: &point,
) -> Result<bool, errors::Error> {
    // EncodeKey 可能因为 Datum 与上下文不兼容而返回警告/错误；Go 代码先交给 ErrCtx 统一处理。
    let mut l = match codec::EncodeKey(*loc, Vec::new(), vec![low.value.clone()]) {
        Ok(v) => v,
        Err(err) => match ec.HandleError(Some(err)) {
            Some(err) => return Err(errors::Trace(err.into())),
            None => Vec::new(),
        },
    };
    // 低端点是开区间时，Go 用 PrefixNext 跳到下一个可扫描 key。
    if low.excl {
        l = kv::Key(l).PrefixNext().0;
    }

    let mut r = match codec::EncodeKey(*loc, Vec::new(), vec![high.value.clone()]) {
        Ok(v) => v,
        Err(err) => match ec.HandleError(Some(err)) {
            Some(err) => return Err(errors::Trace(err.into())),
            None => Vec::new(),
        },
    };
    // 高端点是闭区间时，需要 PrefixNext 让上界在 KV 扫描中包含该值。
    if !high.excl {
        r = kv::Key(r).PrefixNext().0;
    }

    Ok(bytes::Compare(&l, &r) < 0)
}

// convertPointsInPlace does some preprocessing on rangePoints to make them ready to build ranges. It converts
// points to the specified type in place, validates intervals, and compacts valid intervals to the front of rangePoints.
// convertPointsInPlace 对应 Go 的原地预处理：按新 FieldType 转换端点、过滤空区间，并把有效区间压紧到前部。
fn convertPointsInPlace(
    sctx: &rangerctx::RangerContext,
    mut rangePoints: Vec<point>,
    newTp: &types::FieldType,
    skipNull: bool,
    tableRange: bool,
) -> Result<Vec<point>, errors::Error> {
    let mut i = 0usize;
    let numPoints = rangePoints.len();
    let mut minValueDatum = types::Datum::default();
    let mut maxValueDatum = types::Datum::default();

    if tableRange {
        // 表扫描的 KV range 不能直接接受 MaxValueDatum；这里把无穷边界替换成 int/uint 的真实边界值。
        let isUnsigned = mysql::HasUnsignedFlag(newTp.GetFlag());
        if isUnsigned {
            minValueDatum.SetUint64(0);
            maxValueDatum.SetUint64(u64::MAX);
        } else {
            minValueDatum.SetInt64(i64::MIN);
            maxValueDatum.SetInt64(i64::MAX);
        }
    }

    for j in (0..numPoints).step_by(2) {
        // Go 的 rangePoints 是 []*point，这里按 Vec<point> 表达；原语义是在同一批端点上原地修改。
        let mut startPoint = rangePoints[j].clone();
        convertPointInPlace(sctx, &mut startPoint, newTp)?;
        if tableRange {
            if startPoint.value.Kind() == types::KindNull {
                // 表 range 的 NULL 下界要转为最小整数，并改成闭区间。
                startPoint.value = minValueDatum.clone();
                startPoint.excl = false;
            } else if startPoint.value.Kind() == types::KindMinNotNull {
                startPoint.value = minValueDatum.clone();
            }
        }

        let mut endPoint = rangePoints[j + 1].clone();
        convertPointInPlace(sctx, &mut endPoint, newTp)?;
        if tableRange && endPoint.value.Kind() == types::KindMaxValue {
            endPoint.value = maxValueDatum.clone();
        }

        // skipNull 用在 NOT NULL 或表 range 场景：上界为 NULL 的区间不可能产生有效扫描范围。
        if skipNull && endPoint.value.Kind() == types::KindNull {
            continue;
        }
        let less = validInterval(
            &sctx.ErrCtx,
            &sctx.TypeCtx.Location(),
            &startPoint,
            &endPoint,
        )?;
        if !less {
            continue;
        }

        // 对应 Go 的 rangePoints[i] = startPoint，rangePoints[i+1] = endPoint。
        rangePoints[i] = startPoint;
        rangePoints[i + 1] = endPoint;
        i += 2;
    }

    rangePoints.truncate(i);
    Ok(rangePoints)
}

// estimateMemUsageForPoints2Ranges estimates the memory usage of ranges converted from points.
// estimateMemUsageForPoints2Ranges 估算端点转单列 Range 后的内存占用。
fn estimateMemUsageForPoints2Ranges(rangePoints: &[point]) -> i64 {
    // 16 is the size of Range.Collators
    // 每两个 point 形成一个 Range；额外的 16 字节沿用 Go 对 Collator slice 的估算。
    (EmptyRangeSize + 16) * (rangePoints.len() as i64) / 2 + getPointsTotalDatumSize(rangePoints)
}

// points2Ranges build index ranges from range points.
// Only one column is built there. If there're multiple columns, use appendPoints2Ranges.
// rangeMaxSize is the max memory limit for ranges. O indicates no memory limit.
// If the second return value is true, it means that the estimated memory usage of ranges exceeds rangeMaxSize and it falls back to full range.
// points2Ranges 把单列端点构造成索引 Range；第二个返回值表示是否因内存配额回退到全范围。
pub(crate) fn points2Ranges(
    sctx: &rangerctx::RangerContext,
    rangePoints: Vec<point>,
    newTp: &types::FieldType,
    rangeMaxSize: i64,
) -> Result<(Ranges, bool), errors::Error> {
    let rangePoints = convertPointsInPlace(
        sctx,
        rangePoints,
        newTp,
        mysql::HasNotNullFlag(newTp.GetFlag()),
        false,
    )?;

    // 先估算再真正构造 Range，避免长 IN 条件在超过内存配额后仍做大批分配。
    if rangeMaxSize > 0 && estimateMemUsageForPoints2Ranges(&rangePoints) > rangeMaxSize {
        let fullRange = if mysql::HasNotNullFlag(newTp.GetFlag()) {
            FullNotNullRange()
        } else {
            FullRange()
        };
        return Ok((fullRange, true));
    }

    let rangeCount = rangePoints.len() / 2;
    // Keep emitted ranges and their single-column backing slices in batch
    // storage to avoid per-range heap allocations on long-IN workloads.
    // Go 代码批量分配 Range、LowVal、HighVal、Collator 的后备数组，降低长 IN 场景的堆分配次数。
    let mut ranges = Ranges(Vec::with_capacity(rangeCount));
    let mut rangeObjs: Vec<Range> = vec![Range::default(); rangeCount];
    let mut lowValBuf: Vec<types::Datum> = vec![types::Datum::default(); rangeCount];
    let mut highValBuf: Vec<types::Datum> = vec![types::Datum::default(); rangeCount];
    let rangeCollator = collate::GetCollator(newTp.GetCollate());

    for i in 0..rangeCount {
        let startPoint = &rangePoints[i * 2];
        let endPoint = &rangePoints[i * 2 + 1];

        // Batch-allocate the backing arrays, but clamp each slice to len==cap.
        // Some callers append tail datums to an emitted range later, and that append
        // must not overwrite the neighboring ranges that share the same buffer.
        // Go 的三下标切片把 len 与 cap 都限制为 1，避免后续 append 覆盖相邻 Range 共用的后备数组。
        lowValBuf[i] = startPoint.value.clone();
        highValBuf[i] = endPoint.value.clone();
        rangeObjs[i] = Range {
            LowVal: vec![lowValBuf[i].clone()],
            LowExclude: startPoint.excl,
            HighVal: vec![highValBuf[i].clone()],
            HighExclude: endPoint.excl,
            Collators: vec![rangeCollator.Clone()],
        };
        ranges.push(rangeObjs[i].clone());
    }

    Ok((ranges, false))
}

// convertPointInPlace 对应 Go 的单端点类型转换。
// 它既负责调用 Datum.ConvertTo，也根据转换前后比较结果调整开闭区间。
pub(crate) fn convertPointInPlace(
    sctx: &rangerctx::RangerContext,
    p: &mut point,
    newTp: &types::FieldType,
) -> Result<(), errors::Error> {
    match p.value.Kind() {
        types::KindMaxValue | types::KindMinNotNull => return Ok(()),
        _ => {}
    }

    // Go 的 ConvertTo 返回 (casted, err)：某些可容忍错误仍会返回修剪后的 casted 边界值。
    // Rust 用同样的“双返回值”形状表达，避免假设存在额外的 helper API。
    let mut casted = match p.value.ConvertTo(sctx.TypeCtx.clone(), newTp) {
        Ok(casted) => casted,
        Err(err) => {
            // 类型转换异常会导致 plan cache 不安全；Go 代码在这里记录跳过 plan cache 的原因。
            sctx.SetSkipPlanCache(&format!("{} when converting {}", err, p.value.String()));

            // 下列分支逐一保留 Go 对特定 TiDB/MySQL 转换错误的容忍策略。
            if newTp.GetType() == mysql::TypeYear && err.Equal(&types::ErrWarnDataOutOfRange) {
                // see issue #20101: overflow when converting integer to year
                // year 溢出在后续边界修剪中处理，这里故意忽略错误，继续使用 ConvertTo 返回的 casted。
            } else if newTp.GetType() == mysql::TypeBit && err.Equal(&types::ErrDataTooLong) {
                // see issue #19067: we should ignore the types.ErrDataTooLong when we convert value to TypeBit value
            } else if (newTp.GetType() == mysql::TypeNewDecimal
                || mysql::IsIntegerType(newTp.GetType())
                || newTp.GetType() == mysql::TypeFloat)
                && err.Equal(&types::ErrOverflow)
            {
                // Ignore the types.ErrOverflow when we convert TypeNewDecimal/TypeTiny/TypeShort/TypeInt24/TypeLong/TypeLonglong/TypeFloat values.
                // A trimmed valid boundary point value would be returned then. Accordingly, the `excl` of the point
                // would be adjusted. Impossible ranges would be skipped by the `validInterval` call later.
                // tests in TestIndexRange/TestIndexRangeForDecimal
                // Go 的 ConvertTo 在这些溢出场景仍会返回修剪后的 casted 值，后面用比较结果修正开闭边界。
            } else if p.value.Kind() == types::KindMysqlTime
                && newTp.GetType() == mysql::TypeTimestamp
                && err.Equal(&types::ErrWrongValue)
            {
                // See issue #28424: query failed after add index
                // Ignore conversion from Date[Time] to Timestamp since it must be either out of range or impossible date, which will not match a point select
            } else if newTp.GetType() == mysql::TypeEnum && err.Equal(&types::ErrTruncated) {
                // Ignore the types.ErrorTruncated when we convert TypeEnum values.
                // We should cover Enum upper overflow, and convert to the biggest value.
            } else if err.Equal(&charset::ERR_INVALID_CHARACTER_STRING) {
                // The invalid string can be produced by changing datum's underlying bytes directly.
                // For example, newBuildFromPatternLike calculates the end point by adding 1 to bytes.
                // We need to skip these invalid strings.
                // 非法字符串端点无法安全转换；Go 语义是保持原值并返回 nil，让后续区间过滤处理。
                return Ok(());
            } else {
                return Err(errors::Trace(err));
            }
            // Rust's Datum conversion returns the error separately from the
            // converted boundary. Re-run with truncation errors ignored to recover
            // the same clamped value Go returns alongside its tolerated error.
            let mut recovered = p.value.ConvertTo(
                sctx.TypeCtx
                    .WithFlags(sctx.TypeCtx.Flags().WithIgnoreTruncateErr(true)),
                newTp,
            )?;
            if newTp.GetType() == mysql::TypeEnum
                && err.Equal(&types::ErrTruncated)
                && p.value.GetInt64() > 0
            {
                let upperEnum =
                    types::ParseEnumValue(newTp.GetElems(), newTp.GetElems().len() as u64)?;
                recovered.SetMysqlEnum(upperEnum, newTp.GetCollate().to_owned());
            }
            recovered
        }
    };

    let valCmpCasted = p
        .value
        .Compare(
            sctx.TypeCtx.clone(),
            &mut casted,
            collate::GetCollator(newTp.GetCollate()).as_ref(),
        )
        .map_err(errors::Trace)?;
    p.value = casted;
    if valCmpCasted == 0 {
        return Ok(());
    }

    // 转换后边界值发生移动时，需要根据“起点/终点”和“开/闭区间”调整 excl，避免扩大或缩小错误方向。
    if p.start {
        if p.excl {
            if valCmpCasted < 0 {
                // e.g. "a > 1.9" convert to "a >= 2".
                p.excl = false;
            }
        } else if valCmpCasted > 0 {
            // e.g. "a >= 1.1 convert to "a > 1"
            p.excl = true;
        }
    } else if p.excl {
        if valCmpCasted > 0 {
            // e.g. "a < 1.1" convert to "a <= 1"
            p.excl = false;
        }
    } else if valCmpCasted < 0 {
        // e.g. "a <= 1.9" convert to "a < 2"
        p.excl = true;
    }
    Ok(())
}

// getRangesTotalDatumSize 汇总一组 Range 的 LowVal 与 HighVal Datum 内存占用。
fn getRangesTotalDatumSize(ranges: &Ranges) -> i64 {
    let mut sum = 0i64;
    for ran in ranges {
        for val in &ran.LowVal {
            sum += val.MemUsage();
        }
        for val in &ran.HighVal {
            sum += val.MemUsage();
        }
    }
    sum
}

// getPointsTotalDatumSize 汇总端点 Datum 内存占用；Go 里 points 是 []*point。
fn getPointsTotalDatumSize(points: &[point]) -> i64 {
    let mut sum = 0i64;
    for pt in points {
        sum += pt.value.MemUsage();
    }
    sum
}

// estimateMemUsageForAppendPoints2Ranges estimates the memory usage of results of appending points to ranges.
// estimateMemUsageForAppendPoints2Ranges 估算把下一列 point fanout 追加到已有 Range 后的内存。
fn estimateMemUsageForAppendPoints2Ranges(origin: &Ranges, rangePoints: &[point]) -> i64 {
    if origin.is_empty() || rangePoints.is_empty() {
        return 0;
    }
    let originDatumSize = getRangesTotalDatumSize(origin);
    let pointDatumSize = getPointsTotalDatumSize(rangePoints);
    let len1 = origin.len() as i64;
    let len2 = rangePoints.len() as i64 / 2;
    // (int64(len(origin[0].LowVal))+1)*16 is the size of Range.Collators.
    (EmptyRangeSize + ((origin[0].LowVal.len() as i64) + 1) * 16) * len1 * len2
        + originDatumSize * len2
        + pointDatumSize * len1
}

// appendPoints2Ranges appends additional column ranges for multi-column index. The additional column ranges can only be
// appended to point ranges. For example, we have an index (a, b), if the condition is (a > 1 and b = 2), then we can not
// build a conjunctive ranges for this index.
// rangeMaxSize is the max memory limit for ranges. O indicates no memory limit.
// If the second return value is true, it means that the estimated memory usage of ranges after appending points exceeds
// rangeMaxSize and the function rejects appending points to ranges.
// appendPoints2Ranges 对应多列索引追加下一列等值/IN 点范围。
fn appendPoints2Ranges(
    sctx: &rangerctx::RangerContext,
    origin: Ranges,
    rangePoints: Vec<point>,
    newTp: &types::FieldType,
    rangeMaxSize: i64,
) -> Result<(Ranges, bool), errors::Error> {
    let rangePoints = convertPointsInPlace(sctx, rangePoints, newTp, false, false)?;
    // 追加前先估算内存；超限时返回原范围并报告 fallback，不再做 fanout。
    if rangeMaxSize > 0
        && estimateMemUsageForAppendPoints2Ranges(&origin, &rangePoints) > rangeMaxSize
    {
        return Ok((origin, true));
    }

    let mut newIndexRanges = Ranges::default();
    for oRange in origin {
        if !oRange.IsPoint(sctx) {
            // 只有点范围能继续追加下一列；非点范围直接保留。
            newIndexRanges.push(oRange);
        } else {
            let newRanges = appendPoints2IndexRange(&oRange, &rangePoints, newTp)?;
            newIndexRanges.extend(newRanges);
        }
    }
    Ok((newIndexRanges, false))
}

// appendPoints2IndexRange 把一组下一列端点追加到一个已有点 Range，形成笛卡尔 fanout。
fn appendPoints2IndexRange(
    origin: &Range,
    rangePoints: &[point],
    ft: &types::FieldType,
) -> Result<Ranges, errors::Error> {
    let rangeCount = rangePoints.len() / 2;
    // Keep emitted ranges in batch storage; each range will take one widened
    // low/high/collator segment from the backing buffers below.
    // Go 中批量分配加宽后的 low/high/collator 后备数组，避免每个结果 Range 单独分配。
    let mut newRanges = Ranges(Vec::with_capacity(rangeCount));
    let mut rangeObjs: Vec<Range> = vec![Range::default(); rangeCount];
    let lowWidth = origin.LowVal.len() + 1;
    let highWidth = origin.HighVal.len() + 1;
    let extraCollator = collate::GetCollator(ft.GetCollate());

    let mut lowValBuf: Vec<types::Datum> = vec![types::Datum::default(); rangeCount * lowWidth];
    let mut highValBuf: Vec<types::Datum> = vec![types::Datum::default(); rangeCount * highWidth];
    for i in (0..rangePoints.len()).step_by(2) {
        let rangeIdx = i / 2;
        let startPoint = &rangePoints[i];
        let endPoint = &rangePoints[i + 1];

        // Batch-allocate the backing arrays, but clamp each slice to len==cap.
        // Some callers append tail datums to an emitted range later, and that append
        // must not overwrite the neighboring ranges that share the same buffer.
        let lowOffset = rangeIdx * lowWidth;
        lowValBuf[lowOffset..lowOffset + origin.LowVal.len()].clone_from_slice(&origin.LowVal);
        lowValBuf[lowOffset + origin.LowVal.len()] = startPoint.value.clone();

        let highOffset = rangeIdx * highWidth;
        highValBuf[highOffset..highOffset + origin.HighVal.len()].clone_from_slice(&origin.HighVal);
        highValBuf[highOffset + origin.HighVal.len()] = endPoint.value.clone();

        rangeObjs[rangeIdx] = Range {
            LowVal: lowValBuf[lowOffset..lowOffset + lowWidth].to_vec(),
            LowExclude: startPoint.excl,
            HighVal: highValBuf[highOffset..highOffset + highWidth].to_vec(),
            HighExclude: endPoint.excl,
            Collators: origin
                .Collators
                .iter()
                .map(|collator| collator.Clone())
                .chain(std::iter::once(extraCollator.Clone()))
                .collect(),
        };
        newRanges.push(rangeObjs[rangeIdx].clone());
    }
    Ok(newRanges)
}

// estimateMemUsageForAppendRanges2PointRanges estimates the memory usage of results of appending ranges to point ranges.
// estimateMemUsageForAppendRanges2PointRanges 估算把普通范围追加到点范围后的组合结果内存。
fn estimateMemUsageForAppendRanges2PointRanges(pointRanges: &Ranges, ranges: &Ranges) -> i64 {
    let len1 = pointRanges.len() as i64;
    let len2 = ranges.len() as i64;
    if len1 == 0 || len2 == 0 {
        return 0;
    }
    let collatorSize = ((pointRanges[0].LowVal.len() + ranges[0].LowVal.len()) as i64) * 16;
    (EmptyRangeSize + collatorSize) * len1 * len2
        + getRangesTotalDatumSize(pointRanges) * len2
        + getRangesTotalDatumSize(ranges) * len1
}

// AppendRanges2PointRanges appends additional ranges to point ranges.
// rangeMaxSize is the max memory limit for ranges. O indicates no memory limit.
// If the second return value is true, it means that the estimated memory after appending additional ranges to point ranges
// exceeds rangeMaxSize and the function rejects appending additional ranges to point ranges.
// AppendRanges2PointRanges 对应导出的多列 range fanout 拼接函数。
/// 把普通 Range 追加拼接到点范围上（多列 fanout）。
pub fn AppendRanges2PointRanges(
    pointRanges: Ranges,
    ranges: Ranges,
    rangeMaxSize: i64,
) -> (Ranges, bool) {
    if ranges.is_empty() {
        return (pointRanges, false);
    }
    // 先估算内存；超限时拒绝追加，直接返回原 pointRanges。
    if rangeMaxSize > 0
        && estimateMemUsageForAppendRanges2PointRanges(&pointRanges, &ranges) > rangeMaxSize
    {
        return (pointRanges, true);
    }

    let rangeCount = pointRanges.len() * ranges.len();
    let mut sumPointLow = 0usize;
    let mut sumPointHigh = 0usize;
    for pointRange in &pointRanges {
        sumPointLow += pointRange.LowVal.len();
        sumPointHigh += pointRange.HighVal.len();
    }
    let mut sumRangeLow = 0usize;
    let mut sumRangeHigh = 0usize;
    for r in &ranges {
        sumRangeLow += r.LowVal.len();
        sumRangeHigh += r.HighVal.len();
    }
    let totalLowDatumCount = sumPointLow * ranges.len() + sumRangeLow * pointRanges.len();
    let totalHighDatumCount = sumPointHigh * ranges.len() + sumRangeHigh * pointRanges.len();

    // Allocate storage for the full fanout once. Individual result ranges take
    // capped subslices below, which avoids per-result slice allocation.
    // 这里完整保留 Go 的批量 buffer 思路；用 Vec 片段表达对应关系。
    let mut newRanges = Ranges(Vec::with_capacity(rangeCount));
    let mut rangeObjs: Vec<Range> = vec![Range::default(); rangeCount];
    let mut lowValBuf: Vec<types::Datum> = vec![types::Datum::default(); totalLowDatumCount];
    let mut highValBuf: Vec<types::Datum> = vec![types::Datum::default(); totalHighDatumCount];
    let mut rangeIdx = 0usize;
    let mut lowDatumOffset = 0usize;
    let mut highDatumOffset = 0usize;
    for pointRange in &pointRanges {
        let pointLowWidth = pointRange.LowVal.len();
        let pointHighWidth = pointRange.HighVal.len();
        for r in &ranges {
            let lowWidth = pointLowWidth + r.LowVal.len();
            let highWidth = pointHighWidth + r.HighVal.len();
            // Batch-allocate the backing arrays, but clamp each slice to len==cap.
            // Some callers append tail datums to an emitted range later, and that append
            // must not overwrite the neighboring ranges that share the same buffer.
            lowValBuf[lowDatumOffset..lowDatumOffset + pointLowWidth]
                .clone_from_slice(&pointRange.LowVal);
            lowValBuf[lowDatumOffset + pointLowWidth..lowDatumOffset + lowWidth]
                .clone_from_slice(&r.LowVal);

            highValBuf[highDatumOffset..highDatumOffset + pointHighWidth]
                .clone_from_slice(&pointRange.HighVal);
            highValBuf[highDatumOffset + pointHighWidth..highDatumOffset + highWidth]
                .clone_from_slice(&r.HighVal);

            rangeObjs[rangeIdx] = Range {
                LowVal: lowValBuf[lowDatumOffset..lowDatumOffset + lowWidth].to_vec(),
                LowExclude: r.LowExclude,
                HighVal: highValBuf[highDatumOffset..highDatumOffset + highWidth].to_vec(),
                HighExclude: r.HighExclude,
                Collators: pointRange
                    .Collators
                    .iter()
                    .chain(r.Collators.iter())
                    .map(|collator| collator.Clone())
                    .collect(),
            };
            newRanges.push(rangeObjs[rangeIdx].clone());
            rangeIdx += 1;
            lowDatumOffset += lowWidth;
            highDatumOffset += highWidth;
        }
    }
    (newRanges, false)
}

// points2TableRanges build ranges for table scan from range points.
// It will remove the nil and convert MinNotNull and MaxValue to MinInt64 or MinUint64 and MaxInt64 or MaxUint64.
// rangeMaxSize is the max memory limit for ranges. O indicates no memory limit.
// If the second return value is true, it means that the estimated memory usage of ranges exceeds rangeMaxSize and it falls back to full range.
// points2TableRanges 把端点转换为表扫描 Range，额外把无穷边界映射到整数 handle 边界。
fn points2TableRanges(
    sctx: &mut rangerctx::RangerContext,
    rangePoints: Vec<point>,
    newTp: &types::FieldType,
    rangeMaxSize: i64,
) -> Result<(Ranges, bool), errors::Error> {
    let rangePoints = convertPointsInPlace(sctx, rangePoints, newTp, true, true)?;
    if rangeMaxSize > 0 && estimateMemUsageForPoints2Ranges(&rangePoints) > rangeMaxSize {
        return Ok((FullIntRange(mysql::HasUnsignedFlag(newTp.GetFlag())), true));
    }

    let mut ranges = Ranges(Vec::with_capacity(rangePoints.len() / 2));
    for i in (0..rangePoints.len()).step_by(2) {
        let startPoint = &rangePoints[i];
        let endPoint = &rangePoints[i + 1];
        let ran = Range {
            LowVal: vec![startPoint.value.clone()],
            LowExclude: startPoint.excl,
            HighVal: vec![endPoint.value.clone()],
            HighExclude: endPoint.excl,
            Collators: vec![collate::GetCollator(newTp.GetCollate())],
        };
        ranges.push(ran);
    }
    Ok((ranges, false))
}

// buildColumnRange builds range from CNF conditions.
// rangeMaxSize is the max memory limit for ranges. O indicates no memory limit.
// The second return value is the conditions used to build ranges and the third return value is the remained conditions.
// buildColumnRange 是表/普通列 range 构造的内部公共入口。
fn buildColumnRange(
    accessConditions: Vec<expression::ExprBox>,
    sctx: &mut rangerctx::RangerContext,
    tp: &types::FieldType,
    tableRange: bool,
    colLen: i32,
    rangeMaxSize: i64,
) -> Result<(Ranges, Vec<expression::ExprBox>, Vec<expression::ExprBox>), errors::Error> {
    let mut rb = builder { sctx, err: None };
    let mut newTp = newFieldType(tp);
    let mut rangePoints = getFullRange();

    for cond in &accessConditions {
        // 普通列 range 构造先用二进制 collator 做区间交集，保持 Go 中对排序键的处理方式。
        let collator = collate::GetCollator(charset::CollationBin);
        let built = rb.build(cond.as_ref(), &newTp, colLen, true);
        rangePoints = rb.intersection(rangePoints, built, collator.as_ref());
        if let Some(err) = rb.err.take() {
            return Err(errors::Trace(err));
        }
    }

    let mut rangeFallback = false;
    newTp = convertStringFTToBinaryCollate(&newTp);
    let mut ranges;
    if tableRange {
        let (nextRanges, fallback) = points2TableRanges(sctx, rangePoints, &newTp, rangeMaxSize)?;
        ranges = nextRanges;
        rangeFallback = fallback;
    } else {
        let (nextRanges, fallback) = points2Ranges(sctx, rangePoints, &newTp, rangeMaxSize)?;
        ranges = nextRanges;
        rangeFallback = fallback;
    }

    if rangeFallback {
        // fallback 时所有 accessConditions 都变成 remained conditions，让上层保留过滤语义。
        sctx.RecordRangeFallback(rangeMaxSize);
        return Ok((ranges, Vec::new(), accessConditions));
    }
    if colLen != types::UnspecifiedLength {
        ranges = UnionRanges(sctx, ranges, true)?;
    }
    Ok((ranges, accessConditions, Vec::new()))
}

// BuildTableRange builds range of PK column for PhysicalTableScan.
// rangeMaxSize is the max memory limit for ranges. O indicates no memory limit. If you ask that all conds must be used
// for building ranges, set rangeMemQuota to 0 to avoid range fallback.
// The second return value is the conditions used to build ranges and the third return value is the remained conditions.
// If you use the function to build ranges for some access path, you need to update the path's access conditions and filter
// conditions by the second and third return values respectively.
// BuildTableRange 是主键/handle 表扫描 range 的导出入口。
/// 主键/handle 表扫描 Range 的导出入口。
pub fn BuildTableRange(
    accessConditions: Vec<expression::ExprBox>,
    sctx: &mut rangerctx::RangerContext,
    tp: &types::FieldType,
    rangeMaxSize: i64,
) -> Result<(Ranges, Vec<expression::ExprBox>, Vec<expression::ExprBox>), errors::Error> {
    buildColumnRange(
        accessConditions,
        sctx,
        tp,
        true,
        types::UnspecifiedLength,
        rangeMaxSize,
    )
}

// BuildColumnRange builds range from access conditions for general columns.
// rangeMaxSize is the max memory limit for ranges. O indicates no memory limit. If you ask that all conds must be used
// for building ranges, set rangeMemQuota to 0 to avoid range fallback.
// The second return value is the conditions used to build ranges and the third return value is the remained conditions.
// If you use the function to build ranges for some access path, you need to update the path's access conditions and filter
// conditions by the second and third return values respectively.
// BuildColumnRange 是普通列 range 的导出入口；空条件直接返回全范围。
/// 普通列 Range 导出入口；空条件返回全范围。
pub fn BuildColumnRange(
    conds: Vec<expression::ExprBox>,
    sctx: &mut rangerctx::RangerContext,
    tp: &types::FieldType,
    colLen: i32,
    rangeMemQuota: i64,
) -> Result<(Ranges, Vec<expression::ExprBox>, Vec<expression::ExprBox>), errors::Error> {
    if conds.is_empty() {
        return Ok((FullRange(), Vec::new(), Vec::new()));
    }
    buildColumnRange(conds, sctx, tp, false, colLen, rangeMemQuota)
}

impl<'a, 'ctx> rangeDetacher<'a, 'ctx> {
    // buildRangeOnColsByCNFCond 对应 Go 的 rangeDetacher 方法。
    // 它先处理前 eqAndInCount 个等值/IN 条件，再处理后续非等值条件。
    pub(crate) fn buildRangeOnColsByCNFCond(
        &mut self,
        eqAndInCount: usize,
        accessConds: Vec<expression::ExprBox>,
    ) -> Result<(Ranges, Vec<expression::ExprBox>, Vec<expression::ExprBox>), errors::Error> {
        let mut ranges = Ranges::default();
        let mut rangeFallback = false;

        for i in 0..eqAndInCount {
            // Build ranges for equal or in access conditions.
            // 等值/IN 条件可以逐列追加到已有点范围上。
            let point = {
                let mut rb = builder {
                    sctx: self.sctx,
                    err: None,
                };
                let point = rb.build(
                    accessConds[i].as_ref(),
                    &self.newTpSlice[i],
                    self.lengths[i],
                    self.convertToSortKey,
                );
                if let Some(err) = rb.err.take() {
                    return Err(errors::Trace(err));
                }
                point
            };

            let mut tmpNewTp = self.newTpSlice[i].clone();
            if self.convertToSortKey {
                tmpNewTp = convertStringFTToBinaryCollate(&tmpNewTp);
            }
            if i == 0 {
                let (nextRanges, fallback) =
                    points2Ranges(self.sctx, point, &tmpNewTp, self.rangeMaxSize)?;
                ranges = nextRanges;
                rangeFallback = fallback;
            } else {
                let (nextRanges, fallback) =
                    appendPoints2Ranges(self.sctx, ranges, point, &tmpNewTp, self.rangeMaxSize)?;
                ranges = nextRanges;
                rangeFallback = fallback;
            }
            if rangeFallback {
                // 内存 fallback 发生在第 i 列时，Go 返回已使用条件 accessConds[:i] 和剩余条件 accessConds[i:]。
                self.sctx.RecordRangeFallback(self.rangeMaxSize);
                return Ok((ranges, accessConds[..i].to_vec(), accessConds[i..].to_vec()));
            }
        }

        let mut rangePoints = getFullRange();
        // Build rangePoints for non-equal access conditions.
        for i in eqAndInCount..accessConds.len() {
            let mut collator = collate::GetCollator(self.newTpSlice[eqAndInCount].GetCollate());
            if self.convertToSortKey {
                collator = collate::GetCollator(charset::CollationBin);
            }
            rangePoints = {
                let mut rb = builder {
                    sctx: self.sctx,
                    err: None,
                };
                let built = rb.build(
                    accessConds[i].as_ref(),
                    &self.newTpSlice[eqAndInCount],
                    self.lengths[eqAndInCount],
                    self.convertToSortKey,
                );
                let intersection = rb.intersection(rangePoints, built, collator.as_ref());
                if let Some(err) = rb.err.take() {
                    return Err(errors::Trace(err));
                }
                intersection
            };
        }

        let mut tmpNewTp: Option<types::FieldType> = None;
        if eqAndInCount == 0 || eqAndInCount < accessConds.len() {
            if self.convertToSortKey {
                tmpNewTp = Some(convertStringFTToBinaryCollate(
                    &self.newTpSlice[eqAndInCount],
                ));
            } else {
                tmpNewTp = Some(self.newTpSlice[eqAndInCount].clone());
            }
        }

        if eqAndInCount == 0 {
            let (nextRanges, fallback) = points2Ranges(
                self.sctx,
                rangePoints,
                tmpNewTp.as_ref().unwrap(),
                self.rangeMaxSize,
            )?;
            ranges = nextRanges;
            rangeFallback = fallback;
        } else if eqAndInCount < accessConds.len() {
            let (nextRanges, fallback) = appendPoints2Ranges(
                self.sctx,
                ranges,
                rangePoints,
                tmpNewTp.as_ref().unwrap(),
                self.rangeMaxSize,
            )?;
            ranges = nextRanges;
            rangeFallback = fallback;
        }
        if rangeFallback {
            self.sctx.RecordRangeFallback(self.rangeMaxSize);
            return Ok((
                ranges,
                accessConds[..eqAndInCount].to_vec(),
                accessConds[eqAndInCount..].to_vec(),
            ));
        }
        Ok((ranges, accessConds, Vec::new()))
    }
}

// convertStringFTToBinaryCollate 把普通字符串 FieldType 转成 binary collation 。
// Enum/Set 不走这个逻辑，因为 Go 代码对它们有单独处理。
pub(crate) fn convertStringFTToBinaryCollate(ft: &types::FieldType) -> types::FieldType {
    if ft.EvalType() != types::ETString
        || ft.GetType() == mysql::TypeEnum
        || ft.GetType() == mysql::TypeSet
    {
        return ft.clone();
    }
    let mut newTp = ft.Clone();
    newTp.SetCharset(charset::CharsetBin.to_owned());
    newTp.SetCollate(charset::CollationBin.to_owned());
    newTp
}

impl<'a, 'ctx> rangeDetacher<'a, 'ctx> {
    // buildCNFIndexRange builds the range for index where the top layer is CNF.
    // buildCNFIndexRange 在 CNF 场景下构造索引范围，并在前缀索引时合并区间。
    pub(crate) fn buildCNFIndexRange(
        &mut self,
        eqAndInCount: usize,
        accessConds: Vec<expression::ExprBox>,
    ) -> Result<(Ranges, Vec<expression::ExprBox>, Vec<expression::ExprBox>), errors::Error> {
        let (mut ranges, newAccessConds, remainedConds) =
            self.buildRangeOnColsByCNFCond(eqAndInCount, accessConds)?;

        // Take prefix index into consideration.
        // 前缀索引裁剪可能产生相邻或重叠区间，需要按 mergeConsecutive 规则再合并一次。
        if hasPrefix(&self.lengths) {
            ranges = UnionRanges(self.sctx, ranges, self.mergeConsecutive)?;
        }

        Ok((ranges, newAccessConds, remainedConds))
    }
}

// sortRange 对应 Go 中 UnionRanges 的排序辅助结构，保留原 Range 和编码后的左右边界。
struct sortRange {
    originalValue: Range,
    encodedStart: Vec<u8>,
    encodedEnd: Vec<u8>,
}

// UnionRanges sorts `ranges`, union adjacent ones if possible.
// For two intervals [a, b], [c, d], we have guaranteed that a <= c. If b >= c. Then two intervals are overlapped.
// And this two can be merged as [a, max(b, d)].
// Otherwise they aren't overlapped.
// UnionRanges 先把 Range 边界编码成 key 后排序，再合并重叠或可连续的区间。
/// 编码边界后排序并合并重叠或可连续的区间。
pub fn UnionRanges(
    sctx: &rangerctx::RangerContext,
    mut ranges: Ranges,
    mergeConsecutive: bool,
) -> Result<Ranges, errors::Error> {
    if ranges.is_empty() {
        return Ok(Ranges::default());
    }

    let mut objects: Vec<sortRange> = Vec::with_capacity(ranges.len());
    for ran in ranges.into_iter() {
        let mut left = codec::EncodeKey(sctx.TypeCtx.Location(), Vec::new(), ran.LowVal.clone())
            .map_err(|err| {
                errors::Trace(
                    sctx.ErrCtx
                        .HandleError(Some(err))
                        .map(Into::into)
                        .unwrap_or_else(|| errors::New("range lower bound encoding ignored")),
                )
            })?;
        if ran.LowExclude {
            left = kv::Key(left).PrefixNext().0;
        }

        let mut right = codec::EncodeKey(sctx.TypeCtx.Location(), Vec::new(), ran.HighVal.clone())
            .map_err(|err| {
                errors::Trace(
                    sctx.ErrCtx
                        .HandleError(Some(err))
                        .map(Into::into)
                        .unwrap_or_else(|| errors::New("range upper bound encoding ignored")),
                )
            })?;
        if !ran.HighExclude {
            right = kv::Key(right).PrefixNext().0;
        }
        objects.push(sortRange {
            originalValue: ran,
            encodedStart: left,
            encodedEnd: right,
        });
    }

    // Go 的 slices.SortFunc 使用 bytes.Compare；按 encodedStart 做同等排序。
    objects.sort_by(|i, j| bytes::Compare(&i.encodedStart, &j.encodedStart).cmp(&0));

    ranges = Ranges::default();
    let mut lastRange = objects.remove(0);
    for object in objects {
        let overlap_or_consecutive = if mergeConsecutive {
            bytes::Compare(&lastRange.encodedEnd, &object.encodedStart) >= 0
        } else {
            bytes::Compare(&lastRange.encodedEnd, &object.encodedStart) > 0
        };
        if overlap_or_consecutive {
            // 右边界更大时，沿用新对象的 HighVal/HighExclude，保持原 Range 值语义。
            if bytes::Compare(&lastRange.encodedEnd, &object.encodedEnd) < 0 {
                lastRange.encodedEnd = object.encodedEnd.clone();
                lastRange.originalValue.HighVal = object.originalValue.HighVal.clone();
                lastRange.originalValue.HighExclude = object.originalValue.HighExclude;
            }
        } else {
            ranges.push(lastRange.originalValue);
            lastRange = object;
        }
    }
    ranges.push(lastRange.originalValue);
    Ok(ranges)
}

// hasPrefix 检查是否存在前缀索引长度。
pub(crate) fn hasPrefix(lengths: &[i32]) -> bool {
    for l in lengths {
        if *l != types::UnspecifiedLength {
            return true;
        }
    }
    false
}

// cutPrefixForPoints cuts the prefix of points according to the prefix length of the prefix index.
// It may modify the point.value and point.excl. The modification is in-place.
// This function doesn't require the start and end points to be paired in the input.
// cutPrefixForPoints 按前缀索引长度裁剪端点，并在需要时把开区间改成闭区间。
pub(crate) fn cutPrefixForPoints(points: &mut [point], length: i32, tp: &types::FieldType) {
    if length == types::UnspecifiedLength {
        return;
    }
    for point in points.iter_mut() {
        let cut = CutDatumByPrefixLen(&mut point.value, length, tp);
        // In two cases, we need to convert the exclusive point to an inclusive point.
        // case 1: we actually cut the value to accommodate the prefix index.
        if cut
            ||
            // case 2: the value is already equal to the prefix index.
            // For example, col_varchar > 'xx' should be converted to range [xx, +inf) when the prefix index length of
            // `col_varchar` is 2. Otherwise, we would miss values like 'xxx' if we execute (xx, +inf) index range scan.
            (point.start && ReachPrefixLen(&point.value, length, tp))
        {
            // 被裁剪或刚好达到前缀长度时，开区间会漏掉同前缀的后续值，因此必须改闭区间。
            point.excl = false;
        }
    }
}

// CutDatumByPrefixLen cuts the datum according to the prefix length.
// If it's binary or ascii encoded, we will cut it by bytes rather than characters.
// CutDatumByPrefixLen 对字符串/字节 Datum 做前缀裁剪，并返回是否实际裁剪。
/// 按前缀长度裁剪字符串/字节 Datum，返回是否裁剪。
pub fn CutDatumByPrefixLen(v: &mut types::Datum, length: i32, tp: &types::FieldType) -> bool {
    if (v.Kind() == types::KindString || v.Kind() == types::KindBytes)
        && length != types::UnspecifiedLength
    {
        let colCharset = tp.GetCharset();
        let colValue = v.GetBytes();
        if colCharset == charset::CharsetBin || colCharset == charset::CharsetASCII {
            if colValue.len() > length as usize {
                // truncate value and limit its length
                // binary/ascii 按字节裁剪，保持 Go 对字符串底层字节的处理。
                if v.Kind() == types::KindBytes {
                    v.SetBytes(colValue[..length as usize].to_vec());
                } else {
                    // Binary/ASCII prefix lengths are byte counts.  The cut may
                    // therefore land inside a UTF-8 code point; keep the raw
                    // bytes and String datum kind just like Go strings do.
                    v.SetBytesAsString(
                        colValue[..length as usize].to_vec(),
                        tp.GetCollate().to_owned(),
                        length as u32,
                    );
                }
                return true;
            }
        } else if String::from_utf8_lossy(&colValue).chars().count() > length as usize {
            // 非 binary/ascii 字符集按 rune 裁剪，避免截断多字节字符。
            let truncateStr: String = String::from_utf8_lossy(&colValue)
                .chars()
                .take(length as usize)
                .collect();
            // truncate value and limit its length
            v.SetString(truncateStr, tp.GetCollate().to_owned());
            return true;
        }
    }
    false
}

// ReachPrefixLen checks whether the length of v is equal to the prefix length.
// ReachPrefixLen 判断 Datum 是否正好达到前缀长度；binary/ascii 按字节，其它字符集按 rune。
/// 判断 Datum 是否正好达到前缀长度。
pub fn ReachPrefixLen(v: &types::Datum, length: i32, tp: &types::FieldType) -> bool {
    if (v.Kind() == types::KindString || v.Kind() == types::KindBytes)
        && length != types::UnspecifiedLength
    {
        let colCharset = tp.GetCharset();
        let colValue = v.GetBytes();
        if colCharset == charset::CharsetBin || colCharset == charset::CharsetASCII {
            return colValue.len() == length as usize;
        }
        return String::from_utf8_lossy(&colValue).chars().count() == length as usize;
    }
    false
}

// In util/ranger, for each datum that is used in the Range, we will convert data type for them.
// But we cannot use the FieldType of column directly. e.g. the column a is int32 and we have a > 1111111111111111111.
// Obviously the constant is bigger than MaxInt32, so we will get overflow error if we use the FieldType of column a.
// In util/ranger here, we usually use "newTp" to emphasize its difference from the original FieldType of the column.
// newFieldType 为 range 构造创建更宽松的 FieldType，避免常量边界在转换时过早溢出或截断。
pub(crate) fn newFieldType(tp: &types::FieldType) -> types::FieldType {
    match tp.GetType() {
        // To avoid overflow error.
        mysql::TypeTiny
        | mysql::TypeShort
        | mysql::TypeInt24
        | mysql::TypeLong
        | mysql::TypeLonglong => {
            let mut newTp = types::NewFieldType(mysql::TypeLonglong);
            newTp.SetFlag(tp.GetFlag());
            newTp.SetCharset(tp.GetCharset().to_owned());
            *newTp
        }
        // To avoid data truncate error.
        mysql::TypeFloat
        | mysql::TypeDouble
        | mysql::TypeBlob
        | mysql::TypeTinyBlob
        | mysql::TypeMediumBlob
        | mysql::TypeLongBlob
        | mysql::TypeString
        | mysql::TypeVarchar
        | mysql::TypeVarString => {
            let mut newTp = types::NewFieldTypeWithCollation(
                tp.GetType(),
                tp.GetCollate().to_owned(),
                types::UnspecifiedLength as isize,
            );
            newTp.SetCharset(tp.GetCharset().to_owned());
            *newTp
        }
        _ => tp.clone(),
    }
}

// points2EqOrInCond constructs a 'EQUAL' or 'IN' scalar function based on the
// 'points'. `col` is the target column to construct the Equal or In condition.
// NOTE:
// 1. 'points' should not be empty.
// points2EqOrInCond 把端点集合恢复为 Equal 或 In 表达式，供部分 range 结果回写条件。
pub(crate) fn points2EqOrInCond(
    ctx: &dyn expression::BuildContext,
    points: &[point],
    col: &expression::Column,
) -> expression::ExprBox {
    // len(points) cannot be 0 here, since we impose early termination in ExtractEqAndInCondition
    // Constant and Column args should have same RetType, simply get from first arg
    let retType = col.GetType(ctx.GetEvalCtx()).clone();
    let mut args: Vec<expression::ExprBox> = Vec::with_capacity(points.len() / 2 + 1);
    args.push(Box::new(col.clone()) as expression::ExprBox);
    let mut orArgs: Vec<expression::ExprBox> = Vec::with_capacity(2);

    for i in (0..points.len()).step_by(2) {
        if points[i].value.IsNull() {
            // NULL 不能放进普通 IN 常量列表，需要单独生成 IS NULL 分支。
            orArgs.push(
                expression::NewFunctionInternal(
                    ctx,
                    ast::IsNull,
                    retType.clone(),
                    vec![Box::new(col.clone()) as expression::ExprBox],
                )
                .expect("IS NULL range condition must be constructible"),
            );
        } else {
            args.push(Box::new(expression::Constant::with_type(
                points[i].value.clone(),
                retType.clone(),
            )) as expression::ExprBox);
        }
    }

    let mut result: Option<expression::ExprBox> = None;
    if args.len() > 1 {
        let mut funcName = ast::EQ;
        if args.len() > 2 {
            funcName = ast::In;
        }
        result = Some(
            expression::NewFunctionInternal(
                ctx,
                funcName,
                col.GetType(ctx.GetEvalCtx()).clone(),
                args,
            )
            .expect("EQ/IN range condition must be constructible"),
        );
    }
    if orArgs.is_empty() {
        return result.expect("non-NULL points must build EQ/IN condition");
    }
    if let Some(expr) = result {
        orArgs.push(expr);
    }
    if orArgs.len() == 1 {
        return orArgs.remove(0);
    }
    expression::NewFunctionInternal(
        ctx,
        ast::LogicOr,
        col.GetType(ctx.GetEvalCtx()).clone(),
        orArgs,
    )
    .expect("range OR condition must be constructible")
}

// RangesToString print a list of Ranges into a string which can appear in an SQL as a condition.
// RangesToString 把 Range 列表打印成可出现在 SQL 条件中的字符串。
/// 把 Range 列表打印成可出现在 SQL 条件中的字符串。
pub fn RangesToString(
    sc: &stmtctx::StatementContext,
    rans: &Ranges,
    colNames: &[String],
) -> Result<String, errors::Error> {
    for ran in rans {
        if ran.LowVal.len() != ran.HighVal.len() {
            return Err(errors::New("range length mismatch"));
        }
    }

    let mut buffer = String::new();
    for (i, ran) in rans.iter().enumerate() {
        buffer.push('(');
        for j in 0..ran.LowVal.len() {
            buffer.push('(');

            // The `Exclude` information is only useful for the last columns.
            // If it's not the last column, it should always be false, which means it's inclusive.
            // 多列 Range 的开闭区间只作用于最后一列，前缀列必须是等值条件。
            let mut lowExclude = false;
            if ran.LowExclude && j == ran.LowVal.len() - 1 {
                lowExclude = true;
            }
            let mut highExclude = false;
            if ran.HighExclude && j == ran.LowVal.len() - 1 {
                highExclude = true;
            }

            // sanity check: only last column of the `Range` can be an interval
            if j < ran.LowVal.len() - 1 {
                let cmp = ran.LowVal[j]
                    .Compare(sc.TypeCtx(), &ran.HighVal[j], ran.Collators[j].as_ref())
                    .map_err(|err| {
                        errors::New("comparing values error: ".to_owned() + &err.to_string())
                    })?;
                if cmp != 0 {
                    return Err(errors::New("unexpected form of range"));
                }
            }
            let s = RangeSingleColToString(
                sc,
                ran.LowVal[j].clone(),
                ran.HighVal[j].clone(),
                lowExclude,
                highExclude,
                &colNames[j],
                ran.Collators[j].as_ref(),
            )?;
            buffer.push_str(&s);
            buffer.push(')');
            if j < ran.LowVal.len() - 1 {
                // Conditions on different columns of a range are implicitly connected with AND.
                buffer.push_str(" and ");
            }
        }
        buffer.push(')');
        if i < rans.len() - 1 {
            // Conditions of different ranges are implicitly connected with OR.
            buffer.push_str(" or ");
        }
    }
    let result = buffer;

    // Simplify some useless conditions.
    // Go regexp 匹配形如 true、(true)、((true)) 的结果，统一简化为 true。
    if regex::Regex::new(r"^\(*true\)*$")
        .expect("static range simplification regex")
        .is_match(&result)
    {
        return Ok("true".to_string());
    }
    Ok(result)
}

// RangeSingleColToString prints a single column of a Range into a string which can appear in an SQL as a condition.
// RangeSingleColToString 把单列 range 打印成 SQL 条件片段。
/// 把单列 Range 打印成 SQL 条件片段。
pub fn RangeSingleColToString(
    sc: &stmtctx::StatementContext,
    lowVal: types::Datum,
    highVal: types::Datum,
    lowExclude: bool,
    highExclude: bool,
    colName: &str,
    collator: &dyn collate::Collator,
) -> Result<String, errors::Error> {
    // case 1: low and high are both special values(null, min not null, max value)
    let lowKind = lowVal.Kind();
    let highKind = highVal.Kind();
    if (lowKind == types::KindNull
        || lowKind == types::KindMinNotNull
        || lowKind == types::KindMaxValue)
        && (highKind == types::KindNull
            || highKind == types::KindMinNotNull
            || highKind == types::KindMaxValue)
    {
        // 特殊边界值可以直接转成 IS NULL、true、IS NOT NULL 或 false。
        if lowKind == types::KindNull && highKind == types::KindNull && !lowExclude && !highExclude
        {
            return Ok(format!("{} is null", colName));
        }
        if lowKind == types::KindNull && highKind == types::KindMaxValue && !lowExclude {
            return Ok("true".to_string());
        }
        if lowKind == types::KindMinNotNull && highKind == types::KindMaxValue {
            return Ok(format!("{} is not null", colName));
        }
        return Ok("false".to_string());
    }

    let mut buf = Vec::new();
    let mut restoreCtx = format::NewRestoreCtx(format::DefaultRestoreFlags, &mut buf);

    // case 2: low value and high value are the same, and low value and high value are both inclusive.
    let cmp = lowVal
        .Compare(sc.TypeCtx(), &highVal, collator)
        .map_err(errors::Trace)?;
    if cmp == 0 && !lowExclude && !highExclude && !lowVal.IsNull() {
        restoreCtx
            .WritePlain(colName)
            .map_err(|error| errors::New(error.to_string()))?;
        restoreCtx
            .WritePlain(" = ")
            .map_err(|error| errors::New(error.to_string()))?;
        let lowValExpr = driver::ValueExpr {
            Datum: lowVal,
            ..Default::default()
        };
        lowValExpr
            .Restore(&mut restoreCtx)
            .map_err(|error| errors::New(error.to_string()))?;
        drop(restoreCtx);
        return String::from_utf8(buf).map_err(|error| errors::New(error.to_string()));
    }

    // case 3: it's an interval.
    let mut useOR = false;
    let mut noLowerPart = false;

    // Handle the low value part.
    // 下界为 NULL 时需要用 OR 连接；下界为 MinNotNull 时不输出下界条件。
    if lowKind == types::KindNull {
        restoreCtx
            .WritePlain(&(colName.to_string() + " is null"))
            .map_err(|error| errors::New(error.to_string()))?;
        useOR = true;
    } else if lowKind == types::KindMinNotNull {
        noLowerPart = true;
    } else {
        restoreCtx
            .WritePlain(colName)
            .map_err(|error| errors::New(error.to_string()))?;
        if lowExclude {
            restoreCtx
                .WritePlain(" > ")
                .map_err(|error| errors::New(error.to_string()))?;
        } else {
            restoreCtx
                .WritePlain(" >= ")
                .map_err(|error| errors::New(error.to_string()))?;
        }
        let lowValExpr = driver::ValueExpr {
            Datum: lowVal,
            ..Default::default()
        };
        lowValExpr
            .Restore(&mut restoreCtx)
            .map_err(|error| errors::New(error.to_string()))?;
    }

    if !noLowerPart {
        if useOR {
            restoreCtx
                .WritePlain(" or ")
                .map_err(|error| errors::New(error.to_string()))?;
        } else {
            restoreCtx
                .WritePlain(" and ")
                .map_err(|error| errors::New(error.to_string()))?;
        }
    }

    // Handle the high value part
    // 上界为 MaxValue 时打印 true；否则根据 highExclude 输出 < 或 <=。
    if highKind == types::KindMaxValue {
        restoreCtx
            .WritePlain("true")
            .map_err(|error| errors::New(error.to_string()))?;
    } else {
        restoreCtx
            .WritePlain(colName)
            .map_err(|error| errors::New(error.to_string()))?;
        if highExclude {
            restoreCtx
                .WritePlain(" < ")
                .map_err(|error| errors::New(error.to_string()))?;
        } else {
            restoreCtx
                .WritePlain(" <= ")
                .map_err(|error| errors::New(error.to_string()))?;
        }
        let highValExpr = driver::ValueExpr {
            Datum: highVal,
            ..Default::default()
        };
        highValExpr
            .Restore(&mut restoreCtx)
            .map_err(|error| errors::New(error.to_string()))?;
    }

    drop(restoreCtx);
    String::from_utf8(buf).map_err(|error| errors::New(error.to_string()))
}
