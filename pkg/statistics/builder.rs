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

// 从采样数据构建列/索引直方图与 TopN。
//
// `SortedBuilder` 按有序流在线切桶；`BuildHistAndTopN` 先抽高频 TopN，再对剩余样本建直方图。
// NDV（Distinct Value 个数）与相关性等指标供优化器估计选择率。

use crate::{
    Histogram, NewHistogram, NewTopN, SampleCollector, SampleItem, TopN, TopNMeta, Version2,
    sortSampleItems,
};

pub const topNPruningThreshold: usize = 10;
/// TopN 剪枝阈值：候选堆长度达到 `num_top_n / 该阈值` 时，可跳过频次为 1 的值。
pub const bucketNDVDivisor: i64 = 2;
/// 桶 NDV 除数：TopN 未填满且使用默认桶数时，用剩余 NDV 除以该值收紧桶数。

#[derive(Clone, Debug, PartialEq, Eq)]
/// TopN 条目及其在有序样本中的起止下标，供建直方图时跳过这些区间。
pub struct TopNWithRange {
    pub TopNMeta: TopNMeta,
    pub startIdx: i64,
    pub endIdx: i64,
}

#[derive(Clone, Debug)]
/// 按 startIdx 有序扫描，判断采样下标是否落在某个 TopN 覆盖区间内。
pub struct SequentialRangeChecker {
    ranges: Vec<TopNWithRange>,
    currentRangeIdx: usize,
}

pub fn NewSequentialRangeChecker(mut ranges: Vec<TopNWithRange>) -> SequentialRangeChecker {
    // 按 startIdx 排序后构造顺序区间检查器。
    ranges.sort_by_key(|range| range.startIdx);
    SequentialRangeChecker {
        ranges,
        currentRangeIdx: 0,
    }
}

impl SequentialRangeChecker {
    /// 若 `index` 落在当前或后续某个 TopN 区间内则返回 true，并推进内部游标。
    pub fn IsIndexInTopNRange(&mut self, index: i64) -> bool {
        while self.currentRangeIdx < self.ranges.len()
            && index > self.ranges[self.currentRangeIdx].endIdx
        {
            self.currentRangeIdx += 1;
        }
        self.ranges
            .get(self.currentRangeIdx)
            .is_some_and(|range| index >= range.startIdx && index <= range.endIdx)
    }
}

pub fn processTopNValue(
    // 将一个取值的频次候选压入 TopN 堆，按 Count 降序截断到 `num_top_n`。
    //
    // 在允许剪枝且非最后一批时，频次为 1 且堆已较满（或 sample_factor>1）则直接丢弃。
    heap: &mut Vec<TopNWithRange>,
    encoded: Vec<u8>,
    current_count: f64,
    start_index: i64,
    end_index: i64,
    num_top_n: usize,
    allow_pruning: bool,
    sample_factor: f64,
    last_value: bool,
) {
    if !last_value
        && current_count == 1.0
        && allow_pruning
        && (heap.len() >= num_top_n / topNPruningThreshold || sample_factor > 1.0)
    {
        return;
    }
    heap.push(TopNWithRange {
        TopNMeta: TopNMeta {
            Encoded: encoded,
            Count: current_count as u64,
        },
        startIdx: start_index,
        endIdx: end_index,
    });
    heap.sort_by(|left, right| {
        right
            .TopNMeta
            .Count
            .cmp(&left.TopNMeta.Count)
            .then_with(|| left.TopNMeta.Encoded.cmp(&right.TopNMeta.Encoded))
    });
    heap.truncate(num_top_n);
}

fn compareDatum(
    // 用二进制排序规则比较两个 Datum。
    left: &types::Datum,
    right: &types::Datum,
) -> Result<i32, astersql_errors::SharedError> {
    left.Compare(
        (*types::DefaultStmtNoWarningContext).clone(),
        right,
        collate::GetBinaryCollator().as_ref(),
    )
    .map_err(|error| astersql_errors::New(error.to_string()))
}

#[derive(Clone, Debug)]
/// 有序流直方图构建器：按值顺序迭代，桶满时合并并加倍每桶容量。
pub struct SortedBuilder {
    hist: Histogram,
    numBuckets: usize,
    valuesPerBucket: i64,
    lastNumber: i64,
    bucketIdx: usize,
    pub Count: i64,
    needBucketNDV: bool,
}

pub fn NewSortedBuilder(
    // 创建 SortedBuilder；`stats_version >= Version2` 时为每桶维护 NDV。
    num_buckets: usize,
    id: i64,
    field_type: &types::FieldType,
    stats_version: i32,
) -> SortedBuilder {
    SortedBuilder {
        hist: NewHistogram(id, 0, 0, 0, field_type, num_buckets, 0),
        numBuckets: num_buckets.max(1),
        valuesPerBucket: 1,
        lastNumber: 0,
        bucketIdx: 0,
        Count: 0,
        needBucketNDV: stats_version >= Version2,
    }
}

impl SortedBuilder {
    /// 借用当前直方图。
    pub fn Hist(&self) -> &Histogram {
        &self.hist
    }

    /// 消费构建器并取出直方图。
    pub fn IntoHist(self) -> Histogram {
        self.hist
    }

    /// 喂入下一个有序 Datum：相同值累加；否则扩展末桶或开新桶，桶数耗尽则合并。
    pub fn Iterate(&mut self, data: types::Datum) -> Result<(), astersql_errors::SharedError> {
        self.Count += 1;
        // 首个值：开第一桶并初始化 NDV。
        if self.Count == 1 {
            if self.needBucketNDV {
                self.hist.AppendBucketWithNDV(&data, &data, 1, 1, 1);
            } else {
                self.hist.AppendBucket(&data, &data, 1, 1);
            }
            self.hist.NDV = 1;
            return Ok(());
        }
        // 与当前桶上界相同：只增加 Count/Repeat。
        if compareDatum(self.hist.GetUpper(self.bucketIdx), &data)? == 0 {
            self.hist.Buckets[self.bucketIdx].Count += 1;
            self.hist.Buckets[self.bucketIdx].Repeat += 1;
            return Ok(());
        }
        // 仍装得下：扩展末桶上界。
        if self.hist.Buckets[self.bucketIdx].Count + 1 - self.lastNumber <= self.valuesPerBucket {
            self.hist.updateLastBucket(
                &data,
                self.hist.Buckets[self.bucketIdx].Count + 1,
                1,
                self.needBucketNDV,
            );
            self.hist.NDV += 1;
            return Ok(());
        }
        // 桶数已满：两两合并，每桶目标行数加倍。
        if self.bucketIdx + 1 == self.numBuckets {
            self.hist.mergeBuckets();
            self.valuesPerBucket *= 2;
            self.bucketIdx /= 2;
            self.lastNumber = if self.bucketIdx == 0 {
                0
            } else {
                self.hist.Buckets[self.bucketIdx - 1].Count
            };
        }
        if self.hist.Buckets[self.bucketIdx].Count + 1 - self.lastNumber <= self.valuesPerBucket {
            self.hist.updateLastBucket(
                &data,
                self.hist.Buckets[self.bucketIdx].Count + 1,
                1,
                self.needBucketNDV,
            );
        } else {
            self.lastNumber = self.hist.Buckets[self.bucketIdx].Count;
            self.bucketIdx += 1;
            if self.needBucketNDV {
                self.hist
                    .AppendBucketWithNDV(&data, &data, self.lastNumber + 1, 1, 1);
            } else {
                self.hist.AppendBucket(&data, &data, self.lastNumber + 1, 1);
            }
        }
        self.hist.NDV += 1;
        Ok(())
    }
}

pub fn BuildColumnHist(
    // 由已采样 Datum 切片构建列直方图：先排序，再 SortedBuilder，最后按总体行数缩放桶计数。
    num_buckets: usize,
    id: i64,
    samples: &mut [types::Datum],
    field_type: &types::FieldType,
    count: i64,
    mut ndv: i64,
    null_count: i64,
    total_size: i64,
) -> Result<Histogram, astersql_errors::SharedError> {
    ndv = ndv.min(count);
    if count == 0 || samples.is_empty() {
        return Ok(NewHistogram(
            id, ndv, null_count, 0, field_type, 0, total_size,
        ));
    }
    let mut comparison_error = None;
    samples.sort_by(|left, right| match compareDatum(left, right) {
        Ok(-1) => std::cmp::Ordering::Less,
        Ok(1) => std::cmp::Ordering::Greater,
        Ok(_) => std::cmp::Ordering::Equal,
        Err(error) => {
            comparison_error = Some(error);
            std::cmp::Ordering::Equal
        }
    });
    if let Some(error) = comparison_error {
        return Err(error);
    }
    let mut builder = NewSortedBuilder(num_buckets, id, field_type, Version2);
    for sample in samples.iter().cloned() {
        builder.Iterate(sample)?;
    }
    let mut histogram = builder.IntoHist();
    let scale = count as f64 / samples.len() as f64;
    for bucket in &mut histogram.Buckets {
        bucket.Count = (bucket.Count as f64 * scale).round() as i64;
        bucket.Repeat = (bucket.Repeat as f64 * scale).round() as i64;
    }
    histogram.NDV = ndv;
    histogram.NullCount = null_count;
    histogram.TotColSize = total_size;
    Ok(histogram)
}

pub fn buildHist(
    // 在排除 TopN 区间后，用样本构建/填充直方图桶，并累计相关性求和项。
    histogram: &mut Histogram,
    samples: &[SampleItem],
    count: i64,
    ndv: i64,
    num_buckets: i64,
    sample_count_excluding_top_n: i64,
    mut range_checker: Option<&mut SequentialRangeChecker>,
) -> Result<f64, astersql_errors::SharedError> {
    if samples.is_empty() || sample_count_excluding_top_n <= 0 || num_buckets <= 0 {
        return Ok(0.0);
    }
    let sample_factor = count as f64 / sample_count_excluding_top_n as f64;
    // sample_factor：总体行数相对“非 TopN 样本数”的放大倍数。
    let ndv_factor = (count as f64 / ndv.max(1) as f64).min(sample_factor);
    let values_per_bucket = count as f64 / num_buckets as f64 + sample_factor;
    let first = (0..samples.len())
        .find(|index| {
            range_checker
                .as_deref_mut()
                .is_none_or(|checker| !checker.IsIndexInTopNRange(*index as i64))
        })
        .unwrap_or(samples.len());
    if first == samples.len() {
        return Ok(0.0);
    }
    histogram.AppendBucket(
        &samples[first].Value,
        &samples[first].Value,
        sample_factor as i64,
        ndv_factor as i64,
    );
    let mut bucket_index = 0;
    let mut last_count = 0;
    let mut processed = 1_i64;
    let mut correlation_sum = 0.0;
    for (index, sample) in samples.iter().enumerate().skip(first + 1) {
        if range_checker
            .as_deref_mut()
            .is_some_and(|checker| checker.IsIndexInTopNRange(index as i64))
        {
            continue;
        }
        processed += 1;
        correlation_sum += index as f64 * sample.Ordinal as f64;
        let comparison = compareDatum(histogram.GetUpper(bucket_index), &sample.Value)?;
        let total_count = processed as f64 * sample_factor;
        if comparison == 0 {
            histogram.Buckets[bucket_index].Count = total_count as i64;
            if histogram.Buckets[bucket_index].Repeat == ndv_factor as i64 {
                histogram.Buckets[bucket_index].Repeat = (2.0 * sample_factor) as i64;
            } else {
                histogram.Buckets[bucket_index].Repeat += sample_factor as i64;
            }
        } else if total_count - last_count as f64 <= values_per_bucket {
            histogram.updateLastBucket(&sample.Value, total_count as i64, ndv_factor as i64, false);
        } else {
            last_count = histogram.Buckets[bucket_index].Count;
            bucket_index += 1;
            histogram.AppendBucket(
                &sample.Value,
                &sample.Value,
                total_count as i64,
                ndv_factor as i64,
            );
        }
    }
    Ok(correlation_sum)
}

pub fn BuildColumn(
    // 从 SampleCollector 提取 Datum 后调用 BuildColumnHist。
    num_buckets: usize,
    id: i64,
    collector: &mut SampleCollector,
    field_type: &types::FieldType,
) -> Result<Histogram, astersql_errors::SharedError> {
    let mut samples = collector
        .Samples
        .iter()
        .map(|item| item.Value.clone())
        .collect::<Vec<_>>();
    BuildColumnHist(
        num_buckets,
        id,
        &mut samples,
        field_type,
        collector.Count,
        collector.FMSketch.NDV(),
        collector.NullCount,
        collector.TotalSize,
    )
}

pub fn BuildHistAndTopN(
    // 同时构建直方图与 TopN：编码样本、聚合同值、剪枝 TopN，再对剩余样本建桶。
    statement_context: &stmtctx::StatementContext,
    mut num_buckets: usize,
    num_top_n: usize,
    id: i64,
    collector: &mut SampleCollector,
    field_type: &types::FieldType,
    is_column: bool,
) -> Result<(Histogram, TopN), astersql_errors::SharedError> {
    let count = collector.Count;
    // 空表或零 NDV：返回空直方图与空 TopN。
    let ndv = collector.FMSketch.NDV().min(count);
    let mut histogram = NewHistogram(
        id,
        ndv,
        collector.NullCount,
        0,
        field_type,
        num_buckets,
        collector.TotalSize,
    );
    if count == 0 || collector.Samples.is_empty() || ndv == 0 {
        return Ok((histogram, NewTopN(0)));
    }
    sortSampleItems(&mut collector.Samples)?;
    // 列统计需要相关性：用样本下标与原始序数的点积估计。
    let sample_count = collector.Samples.len() as i64;
    if is_column {
        let correlation_sum = collector
            .Samples
            .iter()
            .enumerate()
            .map(|(index, sample)| index as f64 * sample.Ordinal as f64)
            .sum();
        histogram.Correlation = calcCorrelation(sample_count, correlation_sum);
    }
    let sample_factor = count as f64 / sample_count as f64;
    // 将样本编码为可比较的字节键（列走 EncodeKey，索引直接取字节）。
    let allow_pruning = num_top_n == crate::DefaultTopNValue;
    let compared = collector
        .Samples
        .iter()
        .map(|sample| {
            if is_column {
                codec::EncodeKey(
                    statement_context.TimeZone(),
                    Vec::new(),
                    vec![sample.Value.clone()],
                )
            } else {
                Ok(sample.Value.GetBytes())
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut candidates = Vec::new();
    // 扫描同值区间，生成 TopN 候选。
    let mut start = 0;
    let mut sample_ndv = 0_i64;
    while start < compared.len() {
        let mut end = start + 1;
        while end < compared.len() && compared[end] == compared[start] {
            end += 1;
        }
        sample_ndv += 1;
        processTopNValue(
            &mut candidates,
            compared[start].clone(),
            (end - start) as f64,
            start as i64,
            end as i64 - 1,
            num_top_n,
            allow_pruning,
            sample_factor,
            end == compared.len(),
        );
        start = end;
    }
    if allow_pruning {
        // 默认 TopN 规模时做统计显著性剪枝。
        candidates = pruneTopNItem(candidates, ndv, collector.NullCount, sample_count, count);
        if sample_ndv > 1
            && sample_factor > 1.0
            && ndv > sample_ndv
            && candidates.len() >= sample_ndv as usize
        {
            // 采样未覆盖真实 NDV 时，不能让 TopN 吞掉样本中的所有不同值；
            // 至少留一个值用于建桶，保持与 Go 优化器基数估计行为一致。
            candidates.truncate((sample_ndv - 1).max(1) as usize);
        }
    }
    let mut top_n = NewTopN(candidates.len());
    let mut top_sample_count = 0_i64;
    let mut top_total_count = 0_i64;
    for candidate in &candidates {
        top_sample_count += candidate.TopNMeta.Count as i64;
        let scaled = (candidate.TopNMeta.Count as f64 * sample_factor) as u64;
        top_total_count += scaled as i64;
        top_n.AppendTopN(candidate.TopNMeta.Encoded.clone(), scaled);
    }
    top_n.Sort();
    if candidates.len() as i64 == ndv || num_buckets == 0 {
        // TopN 已覆盖全部 NDV 或无需直方图桶时直接返回。
        return Ok((histogram, top_n));
    }
    let remaining_ndv = ndv - candidates.len() as i64;
    if candidates.len() < num_top_n && num_buckets == crate::DefaultHistogramBuckets {
        num_buckets = (remaining_ndv / bucketNDVDivisor)
            .max(1)
            .min(num_buckets as i64) as usize;
    }
    let mut checker = NewSequentialRangeChecker(candidates);
    // 跳过 TopN 覆盖的样本下标，用剩余样本填充直方图。
    buildHist(
        &mut histogram,
        &collector.Samples,
        count - top_total_count,
        remaining_ndv,
        num_buckets as i64,
        sample_count - top_sample_count,
        Some(&mut checker),
    )?;
    Ok((histogram, top_n))
}

pub fn calcCorrelation(sample_count: i64, correlation_sum: f64) -> f64 {
    // 由样本点积计算列值与物理序的 Pearson 式相关性。
    if sample_count <= 1 {
        return 1.0;
    }
    let count = sample_count as f64;
    let sum = (count - 1.0) * count / 2.0;
    let denominator = (count - 1.0) * count * (2.0 * count - 1.0) / 6.0 - sum * sum / count;
    if denominator == 0.0 {
        1.0
    } else {
        (correlation_sum - sum * sum / count) / denominator
    }
}

pub fn pruneTopNItem(
    // 从低频端剪掉不显著的 TopN 项：若末项频次无法显著高于“其余 NDV 均匀”假设则丢弃。
    mut top_n: Vec<TopNWithRange>,
    ndv: i64,
    null_count: i64,
    sample_rows: i64,
    total_rows: i64,
) -> Vec<TopNWithRange> {
    if total_rows <= 1 || top_n.len() as i64 >= ndv || top_n.len() <= 1 {
        return top_n;
    }
    let mut sum_count = top_n[..top_n.len() - 1]
        .iter()
        .map(|item| item.TopNMeta.Count)
        .sum::<u64>();
    let mut length = top_n.len();
    while length > 0 {
        // 从尾部向前：用选择率与方差阈值判断末项是否值得保留。
        let mut selectivity =
            1.0 - sum_count as f64 / sample_rows as f64 - null_count as f64 / total_rows as f64;
        selectivity = selectivity.clamp(0.0, 1.0);
        let other_ndv = ndv as f64 - (length as f64 - 1.0);
        if other_ndv > 1.0 {
            selectivity /= other_ndv;
        }
        let total = total_rows as f64;
        let samples = sample_rows as f64;
        let expected = total * top_n[length - 1].TopNMeta.Count as f64 / samples;
        let variance = samples * expected * (total - expected) * (total - samples)
            / (total * total * (total - 1.0));
        if top_n[length - 1].TopNMeta.Count as f64
            > selectivity * samples + 2.0 * variance.max(0.0).sqrt() + 0.5
        {
            break;
        }
        length -= 1;
        if length == 0 {
            break;
        }
        sum_count -= top_n[length - 1].TopNMeta.Count;
    }
    top_n.truncate(length);
    top_n
}
