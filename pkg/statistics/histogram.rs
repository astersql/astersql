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

// 直方图：列/索引值分布的等频桶结构，支撑优化器行数估计与分区统计合并。
//
// 桶存累计 Count、Repeat（上界重复次数）与 NDV；可结合 TopN、标量边界做区间估计，
// 并支持 Proto 编解码、多直方图合并及加载/驱逐状态。

// 直方图：按值域分桶的列/索引分布统计，供优化器估计谓词选择率与行数。
//
// 每个桶保存累计 Count、上界重复次数 Repeat、桶内 NDV；Bounds 交错存放上下界。
// Version2 起配合 TopN；分区直方图可合并为全局直方图。

use std::fmt::Write as _;
use std::mem::size_of;

use protobuf::RepeatedField;

use crate::TopNMeta;

/// 越界区间估计时使用的默认放大倍率。
/// 越界估计时，单值行数相对实时行数的下限分母。
pub const outOfRangeBetweenRate: f64 = 100.0;
/// 统计版本 0（早期格式）。
/// 统计版本 0：未 ANALYZE / 伪统计。
pub const Version0: i32 = 0;
/// 统计版本 1。
/// 统计版本 1：基础直方图。
pub const Version1: i32 = 1;
/// 统计版本 2（含 TopN、桶 NDV 等增强）。
/// 统计版本 2：直方图 + TopN，桶可带 NDV。
pub const Version2: i32 = 2;
/// 统计全量驻留内存。
/// 加载状态：详细统计已全部在内存。
pub const AllLoaded: i32 = 0;
/// 统计已全部驱逐。
/// 加载状态：详细统计已全部驱逐。
pub const AllEvicted: i32 = 1;

/// Count-independent out-of-range geometry, reusable while row counts change.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct OutOfRangeShape {
    /// No histogram buckets: scaling always returns zero.
    pub Empty: bool,
    /// Invalid or unsigned-negative range; determinate mode still uses OneValue.
    pub Impossible: bool,
    /// Average non-null count per histogram distinct value, excluding TopN.
    pub OneValue: f64,
    /// Histogram NDV floored at one.
    pub HistNDV: i64,
    /// Half-weighted triangular overlap, capped at one.
    pub TotalPercent: f64,
    /// Full triangular overlap for the worst-case count, capped at one.
    pub MaxTotalPercent: f64,
}

#[derive(Clone)]
/// 等频直方图：Bounds 交错存上下界 Datum，Buckets 存累计计数与 NDV。
///
/// NDV 为 distinct 值估计；NullCount/TotColSize/Correlation 供代价模型使用。
/// 列或索引的直方图主体。
pub struct Histogram {
    pub Tp: types::FieldType,
    pub Bounds: Vec<types::Datum>,
    pub Buckets: Vec<Bucket>,
    pub Scalars: Vec<scalar>,
    pub ID: i64,
    pub NDV: i64,
    pub NullCount: i64,
    pub LastUpdateVersion: u64,
    pub TotColSize: i64,
    pub Correlation: f64,
}

impl std::fmt::Debug for Histogram {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Histogram")
            .field("ID", &self.ID)
            .field("NDV", &self.NDV)
            .field("NullCount", &self.NullCount)
            .field("Buckets", &self.Buckets)
            .field("bound_count", &self.Bounds.len())
            .finish()
    }
}

/// 空 Histogram 结构体大小。
/// 空 Histogram 结构体固定大小。
pub const EmptyHistogramSize: i64 = size_of::<Histogram>() as i64;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// 单个直方图桶：累计行数、上界重复次数与桶内 NDV。
/// 单个直方图桶：累计行数、上界重复次数、桶内不同值数。
pub struct Bucket {
    pub Count: i64,
    pub Repeat: i64,
    pub NDV: i64,
}

/// 空 Bucket 结构体大小。
/// 空 Bucket 结构体固定大小。
pub const EmptyBucketSize: i64 = size_of::<Bucket>() as i64;

#[derive(Clone, Copy, Debug, Default)]
/// 桶边界的数值化表示，用于分数估计与公共前缀处理。
/// 桶边界的标量化表示，用于分数插值估计。
pub struct scalar {
    pub lower: f64,
    pub upper: f64,
    pub commonPfxLen: usize,
}

/// 空 scalar 结构体大小。
/// 空 scalar 结构体固定大小。
pub const EmptyScalarSize: i64 = size_of::<scalar>() as i64;

/// 初始化全局伪直方图 chunk（占位，与 Go 对齐）。
/// 初始化全局伪直方图共享 chunk（占位）。
pub fn initGlobalPseudoChunk() {}

/// 获取全局伪 chunk（当前返回空 Vec）。
/// 返回全局伪直方图边界 chunk（当前为空）。
pub fn getGlobalPseudoChunk() -> Vec<types::Datum> {
    Vec::new()
}

/// 为直方图规范化字段类型（字符串强制 bin 校对）。
/// 为直方图准备字段类型：字符串统一为 binary collation。
pub fn prepareFieldTypeForHistogram(field_type: &types::FieldType) -> types::FieldType {
    let mut result = field_type.clone();
    if result.EvalType() == types_field::ETString {
        result.SetCollate(types::charset::CollationBin.to_owned());
    }
    result
}

/// 创建空伪直方图（无桶）。
/// 构造无桶伪直方图（占位统计）。
pub fn NewPseudoHistogram(id: i64, field_type: &types::FieldType) -> Histogram {
    NewHistogram(id, 0, 0, 0, field_type, 0, 0)
}

/// 创建指定 NDV/空值/版本与桶容量的直方图。
/// 构造指定容量的空直方图。
pub fn NewHistogram(
    id: i64,
    ndv: i64,
    null_count: i64,
    version: u64,
    field_type: &types::FieldType,
    bucket_size: usize,
    total_column_size: i64,
) -> Histogram {
    Histogram {
        Tp: prepareFieldTypeForHistogram(field_type),
        Bounds: Vec::with_capacity(bucket_size.saturating_mul(2)),
        Buckets: Vec::with_capacity(bucket_size),
        Scalars: Vec::new(),
        ID: id,
        NDV: ndv,
        NullCount: null_count,
        LastUpdateVersion: version,
        TotColSize: total_column_size,
        Correlation: 0.0,
    }
}

/// 在无警告 StmtCtx 下比较两个 Datum。
/// 二进制校对下比较两个 Datum，返回 -1/0/1。
fn compareDatum(left: &types::Datum, right: &types::Datum) -> i32 {
    left.Compare(
        (*types::DefaultStmtNoWarningContext).clone(),
        right,
        collate::GetBinaryCollator().as_ref(),
    )
    .unwrap_or(0)
}

impl Histogram {
    /// 返回第 index 个桶的下界 Datum。
    /// 第 index 桶下界。
    pub fn GetLower(&self, index: usize) -> &types::Datum {
        &self.Bounds[index * 2]
    }

    /// 将第 index 个桶下界写入 destination。
    /// 将下界拷贝到 destination。
    pub fn LowerToDatum(&self, index: usize, destination: &mut types::Datum) {
        *destination = self.GetLower(index).clone();
    }

    /// 返回第 index 个桶的上界 Datum。
    /// 第 index 桶上界。
    pub fn GetUpper(&self, index: usize) -> &types::Datum {
        &self.Bounds[index * 2 + 1]
    }

    /// 将第 index 个桶上界写入 destination。
    /// 将上界拷贝到 destination。
    pub fn UpperToDatum(&self, index: usize, destination: &mut types::Datum) {
        *destination = self.GetUpper(index).clone();
    }

    /// 估算直方图跟踪内存（结构 + Bounds/Buckets/Scalars）。
    /// 估计内存占用；空直方图按 Go 语义返回 0。
    pub fn MemoryUsage(&self) -> i64 {
        // Match Go's chunk capacity semantics: an empty histogram does not
        // account for the fixed Histogram struct, even when Rust Vec has
        // reserved backing storage but contains no bounds or buckets.
        // 空直方图不计入结构体本身，与 Go chunk 容量语义对齐。
        if self.Bounds.is_empty() && self.Buckets.is_empty() && self.Scalars.is_empty() {
            return 0;
        }
        EmptyHistogramSize
            + self.Bounds.capacity() as i64 * size_of::<types::Datum>() as i64
            + self.Buckets.capacity() as i64 * EmptyBucketSize
            + self.Scalars.capacity() as i64 * EmptyScalarSize
    }

    /// 追加桶（NDV 默认 0）。
    /// 追加桶（NDV 记为 0）。
    pub fn AppendBucket(
        &mut self,
        lower: &types::Datum,
        upper: &types::Datum,
        count: i64,
        repeat: i64,
    ) {
        self.AppendBucketWithNDV(lower, upper, count, repeat, 0);
    }

    /// 追加带桶内 NDV 的桶。
    /// 追加带桶内 NDV 的桶，并写入上下界。
    pub fn AppendBucketWithNDV(
        &mut self,
        lower: &types::Datum,
        upper: &types::Datum,
        count: i64,
        repeat: i64,
        ndv: i64,
    ) {
        self.Buckets.push(Bucket {
            Count: count,
            Repeat: repeat,
            NDV: ndv,
        });
        self.Bounds.push(lower.clone());
        self.Bounds.push(upper.clone());
    }

    /// 更新最后一个桶的上界、计数与 Repeat。
    /// 更新最后一个桶的上界、累计计数与 Repeat，可选递增 NDV。
    pub fn updateLastBucket(
        &mut self,
        upper: &types::Datum,
        count: i64,
        repeat: i64,
        need_bucket_ndv: bool,
    ) {
        let index = self.Len() - 1;
        self.Bounds[index * 2 + 1] = upper.clone();
        if need_bucket_ndv && self.Buckets[index].NDV > 0 {
            self.Buckets[index].NDV += 1;
        }
        self.Buckets[index].Count = count;
        self.Buckets[index].Repeat = repeat;
    }

    /// 桶数量。
    /// 桶个数。
    pub fn Len(&self) -> usize {
        self.Buckets.len()
    }

    /// 深拷贝直方图。
    /// 深拷贝。
    pub fn Copy(&self) -> Histogram {
        self.clone()
    }

    /// 清空并归还池（占位）。
    /// 清空边界/桶/标量以便回收。
    pub fn DestroyAndPutToPool(&mut self) {
        self.Bounds.clear();
        self.Buckets.clear();
        self.Scalars.clear();
    }

    /// 按列类型将边界 Datum 解码到目标类型。
    /// 将字节边界解码为实际 Datum，并更新字段类型。
    pub fn DecodeTo(
        &mut self,
        field_type: &types::FieldType,
    ) -> Result<(), astersql_errors::SharedError> {
        for bound in &mut self.Bounds {
            if bound.Kind() == types::KindBytes {
                let (_, decoded) = codec::DecodeOne(&bound.GetBytes())?;
                *bound = decoded;
            }
        }
        self.Tp = prepareFieldTypeForHistogram(field_type);
        Ok(())
    }

    /// 转换直方图字段类型并重建边界。
    /// 将边界转换到目标字段类型，返回新直方图。
    pub fn ConvertTo(
        &self,
        context: types::Context,
        field_type: &types::FieldType,
    ) -> Result<Histogram, astersql_errors::SharedError> {
        let mut result = self.clone();
        result.Tp = prepareFieldTypeForHistogram(field_type);
        result.Bounds = self
            .Bounds
            .iter()
            .map(|bound| {
                bound
                    .ConvertTo(context.clone(), field_type)
                    .map_err(|error| astersql_errors::New(error.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(result)
    }

    /// 比较两直方图是否相等（可选忽略 ID）。
    /// 比较两个直方图内容是否相等。
    pub fn Equal(&self, other: &Histogram, ignore_id: bool) -> bool {
        (ignore_id || self.ID == other.ID)
            && self.NDV == other.NDV
            && self.NullCount == other.NullCount
            && self.LastUpdateVersion == other.LastUpdateVersion
            && self.TotColSize == other.TotColSize
            && self.Correlation == other.Correlation
            && self.Buckets == other.Buckets
            && self.Bounds.len() == other.Bounds.len()
            && self
                .Bounds
                .iter()
                .zip(&other.Bounds)
                .all(|(left, right)| compareDatum(left, right) == 0)
    }

    /// 返回指定桶的非累计局部行数。
    /// 单桶行数（累计 Count 差分）。
    pub fn BucketCount(&self, index: usize) -> i64 {
        if index == 0 {
            self.Buckets[0].Count
        } else {
            self.Buckets[index].Count - self.Buckets[index - 1].Count
        }
    }

    /// 二分定位并从桶中减去指定值的计数。
    /// 从包含 value 的桶及后续累计 Count 中减去 count。
    pub fn BinarySearchRemoveVal(&mut self, value: &types::Datum, count: i64) {
        let mut found = None;
        for index in 0..self.Len() {
            if compareDatum(self.GetLower(index), value) <= 0
                && compareDatum(self.GetUpper(index), value) >= 0
            {
                if self.Buckets[index].NDV > 0 {
                    self.Buckets[index].NDV -= 1;
                }
                if compareDatum(self.GetUpper(index), value) == 0 {
                    self.Buckets[index].Repeat = 0;
                }
                self.Buckets[index].Count = (self.Buckets[index].Count - count).max(0);
                found = Some(index);
                break;
            }
        }
        if let Some(index) = found {
            for bucket in &mut self.Buckets[index + 1..] {
                bucket.Count = (bucket.Count - count).max(0);
            }
        }
    }

    /// 按 TopN 列表从直方图中移除对应值计数。
    /// 从直方图中移除一组 TopN 编码值及其计数。
    pub fn RemoveVals(&mut self, values: &[TopNMeta]) {
        for value in values {
            self.BinarySearchRemoveVal(
                &types::NewBytesDatum(value.Encoded.clone()),
                value.Count as i64,
            );
        }
    }

    /// V2 索引分析后标准化：去掉空桶并将桶 NDV 置 0。
    /// V2 索引直方图标准化：去掉空桶并将桶 NDV 清零。
    pub fn StandardizeForV2AnalyzeIndex(&mut self) {
        let mut buckets = Vec::with_capacity(self.Len());
        let mut bounds = Vec::with_capacity(self.Bounds.len());
        for index in 0..self.Len() {
            if self.BucketCount(index) <= 0 && self.Buckets[index].Repeat <= 0 {
                continue;
            }
            let mut bucket = self.Buckets[index];
            bucket.NDV = 0;
            buckets.push(bucket);
            bounds.push(self.GetLower(index).clone());
            bounds.push(self.GetUpper(index).clone());
        }
        self.Buckets = buckets;
        self.Bounds = bounds;
    }

    /// 格式化单个桶为调试字符串。
    /// 格式化单个桶为可读字符串。
    pub fn BucketToString(&self, index: usize, _index_columns: usize) -> String {
        format!(
            "num: {} lower_bound: {} upper_bound: {} repeats: {} ndv: {}",
            self.BucketCount(index),
            self.GetLower(index).ToString().unwrap_or_default(),
            self.GetUpper(index).ToString().unwrap_or_default(),
            self.Buckets[index].Repeat,
            self.Buckets[index].NDV,
        )
    }

    /// 格式化整个直方图。
    /// 格式化整幅直方图；index_columns>0 时按索引展示。
    pub fn ToString(&self, index_columns: usize) -> String {
        let mut output = if index_columns > 0 {
            format!("index:{} ndv:{}", self.ID, self.NDV)
        } else {
            format!(
                "column:{} ndv:{} totColSize:{}",
                self.ID, self.NDV, self.TotColSize
            )
        };
        for index in 0..self.Len() {
            let _ = write!(output, "\n{}", self.BucketToString(index, index_columns));
        }
        output
    }

    /// 定位值所在桶：返回 (恰好命中上界?, 桶下标, 在范围内?, 找到?)。
    /// 定位 value 所在桶，返回 (越界?, 桶下标, 在桶内?, 命中上界?)。
    pub fn LocateBucket(&self, value: &types::Datum) -> (bool, usize, bool, bool) {
        if self.Bounds.is_empty() {
            return (true, 0, false, false);
        }
        // 在交错上下界上二分，找到第一个 >= value 的边界下标。
        let index = self
            .Bounds
            .partition_point(|bound| compareDatum(bound, value) < 0);
        if index >= self.Bounds.len() {
            return (true, self.Len() - 1, false, false);
        }
        let matched = compareDatum(&self.Bounds[index], value) == 0;
        let bucket_index = index / 2;
        if index % 2 == 0 && !matched {
            return (false, bucket_index, false, false);
        }
        if (index % 2 == 1 && matched) || compareDatum(self.GetUpper(bucket_index), value) == 0 {
            return (false, bucket_index, true, true);
        }
        (false, bucket_index, true, false)
    }

    /// 估计等值行数；可利用桶 NDV 或 Repeat。
    /// 估计等值谓词行数；命中上界用 Repeat，否则按桶 NDV 或全局 NDV 均分。
    pub fn EqualRowCount(&self, value: &types::Datum, has_bucket_ndv: bool) -> (f64, bool) {
        let (_, bucket_index, in_bucket, matched) = self.LocateBucket(value);
        if !in_bucket {
            return (0.0, false);
        }
        if matched {
            return (self.Buckets[bucket_index].Repeat as f64, true);
        }
        if has_bucket_ndv && self.Buckets[bucket_index].NDV > 1 {
            return (
                (self.BucketCount(bucket_index) - self.Buckets[bucket_index].Repeat) as f64
                    / (self.Buckets[bucket_index].NDV - 1) as f64,
                true,
            );
        }
        if self.NDV == 0 {
            return (0.0, false);
        }
        (self.NotNullCount() / self.NDV as f64, false)
    }

    /// 估计严格大于 value 的行数。
    /// 估计大于 value 的行数。
    pub fn GreaterRowCount(&self, value: &types::Datum) -> f64 {
        let (equal, _) = self.EqualRowCount(value, false);
        (self.NotNullCount() - self.LessRowCount(value) - equal).max(0.0)
    }

    /// 估计严格小于 value 的行数，并返回桶下标。
    /// 估计小于 value 的行数，并返回所在桶下标。
    pub fn LessRowCountWithBktIdx(&self, value: &types::Datum) -> (f64, usize) {
        if self.Bounds.is_empty() {
            return (0.0, 0);
        }
        let (exceed, index, in_bucket, matched) = self.LocateBucket(value);
        if exceed {
            return (self.NotNullCount(), self.Len() - 1);
        }
        let previous = if index > 0 {
            self.Buckets[index - 1].Count as f64
        } else {
            0.0
        };
        if !in_bucket {
            return (previous, index);
        }
        let bucket = self.Buckets[index];
        if matched {
            return ((bucket.Count - bucket.Repeat) as f64, index);
        }
        (
            previous
                + self.calcFraction(index, value)
                    * (bucket.Count as f64 - bucket.Repeat as f64 - previous),
            index,
        )
    }

    /// 估计严格小于 value 的行数。
    /// 估计小于 value 的行数。
    pub fn LessRowCount(&self, value: &types::Datum) -> f64 {
        self.LessRowCountWithBktIdx(value).0
    }

    /// 估计 (lower, upper) 开区间行数，返回带偏斜的 RowEstimate。
    /// 估计区间 [lower, upper) 行数，含 Min/Est/Max。
    pub fn BetweenRowCount(&self, lower: &types::Datum, upper: &types::Datum) -> RowEstimate {
        let (less_lower, lower_bucket) = self.LessRowCountWithBktIdx(lower);
        let (less_upper, upper_bucket) = self.LessRowCountWithBktIdx(upper);
        let mut estimate = DefaultRowEst(less_upper - less_lower);
        let (lower_equal, _) = self.EqualRowCount(lower, false);
        let ndv_average = if self.NDV > 0 {
            self.NotNullCount() / self.NDV as f64
        } else {
            0.0
        };
        if estimate.Est < lower_equal.max(ndv_average) && self.NDV > 0 {
            estimate = DefaultRowEst(
                less_upper
                    .min(self.NotNullCount() - less_lower)
                    .min(lower_equal + ndv_average),
            );
        }
        if less_lower != less_upper && lower_bucket == upper_bucket {
            let bucket_count = self.BucketCount(lower_bucket) as f64;
            estimate.MaxEst = estimate.MaxEst.max(bucket_count);
        }
        estimate
    }

    /// 总行数 = 非空 + 空值。
    /// 总行数 = 非空 + NULL。
    pub fn TotalRowCount(&self) -> f64 {
        self.NotNullCount() + self.NullCount as f64
    }

    /// 统计总行数与实时行数的绝对差。
    /// 实时行数与直方图总行数的绝对差。
    pub fn AbsRowCountDifference(&self, realtime_row_count: i64) -> f64 {
        (realtime_row_count as f64 - self.TotalRowCount()).abs()
    }

    /// 非空行数（末桶累计 Count）。
    pub fn NotNullCount(&self) -> f64 {
        self.Buckets
            .last()
            .map_or(0.0, |bucket| bucket.Count as f64)
    }

    /// 两两合并相邻桶以压缩桶数。
    pub(crate) fn mergeBuckets(&mut self) {
        let mut buckets = Vec::with_capacity(self.Len().div_ceil(2));
        let mut bounds = Vec::with_capacity(self.Bounds.len().div_ceil(2));
        let mut index = 0;
        while index + 1 < self.Len() {
            buckets.push(Bucket {
                NDV: self.Buckets[index].NDV + self.Buckets[index + 1].NDV,
                Count: self.Buckets[index + 1].Count,
                Repeat: self.Buckets[index + 1].Repeat,
            });
            bounds.push(self.GetLower(index).clone());
            bounds.push(self.GetUpper(index + 1).clone());
            index += 2;
        }
        if index < self.Len() {
            buckets.push(self.Buckets[index]);
            bounds.push(self.GetLower(index).clone());
            bounds.push(self.GetUpper(index).clone());
        }
        self.Buckets = buckets;
        self.Bounds = bounds;
    }

    /// 实时总行数相对直方图的放大因子。
    /// 相对直方图总行数的放大系数。
    pub fn GetIncreaseFactor(&self, total_count: i64) -> f64 {
        let count = self.TotalRowCount();
        if count == 0.0 {
            1.0
        } else {
            total_count as f64 / count
        }
    }

    /// 每个非空 distinct 值的平均行数。
    /// 每个非空不同值的平均行数（含放大）。
    pub fn AvgCountPerNotNullValue(&self, total_count: i64) -> f64 {
        let factor = self.GetIncreaseFactor(total_count);
        self.NotNullCount() * factor / (self.NDV as f64 * factor).max(1.0)
    }

    /// 值是否落在直方图覆盖范围之外。
    /// value 是否落在直方图值域之外。
    pub fn OutOfRange(&self, value: &types::Datum) -> bool {
        !self.Bounds.is_empty()
            && (compareDatum(self.GetLower(0), value) > 0
                || compareDatum(self.GetUpper(self.Len() - 1), value) < 0)
    }

    /// Compose cached range geometry with the current row counts.
    pub fn OutOfRangeRowCount(
        &self,
        lower: &types::Datum,
        upper: &types::Datum,
        realtime_row_count: i64,
        modify_count: i64,
        histogram_ndv: i64,
        allow_modify_count: bool,
        skew_ratio: f64,
    ) -> RowEstimate {
        self.ScaleOutOfRangeShape(
            self.OutOfRangeShape(lower, upper, histogram_ndv),
            realtime_row_count,
            modify_count,
            allow_modify_count,
            skew_ratio,
        )
    }

    /// Compute geometry independently of realtime and modification counts.
    pub fn OutOfRangeShape(
        &self,
        lower: &types::Datum,
        upper: &types::Datum,
        histogram_ndv: i64,
    ) -> OutOfRangeShape {
        if self.Len() == 0 {
            return OutOfRangeShape {
                Empty: true,
                ..OutOfRangeShape::default()
            };
        }
        let histogram_ndv = histogram_ndv.max(1);
        let mut shape = OutOfRangeShape {
            OneValue: self.NotNullCount() / histogram_ndv as f64,
            HistNDV: histogram_ndv,
            ..OutOfRangeShape::default()
        };
        let common_prefix = if matches!(
            self.GetLower(0).Kind(),
            types::KindBytes | types::KindString
        ) {
            crate::commonPrefixLength(&[
                self.GetLower(0).GetBytes(),
                self.GetUpper(self.Len() - 1).GetBytes(),
                lower.GetBytes(),
                upper.GetBytes(),
            ])
        } else {
            0
        };
        let mut lower_value = crate::convertDatumToScalar(lower, common_prefix);
        let mut upper_value = crate::convertDatumToScalar(upper, common_prefix);
        if types::mysql::HasUnsignedFlag(self.Tp.GetFlag()) {
            let left_clamped = lower_value < 0.0;
            let right_clamped = upper_value < 0.0;
            lower_value = lower_value.max(0.0);
            upper_value = upper_value.max(0.0);
            if lower_value == 0.0 && upper_value == 0.0 && (left_clamped || right_clamped) {
                shape.Impossible = true;
                return shape;
            }
        }
        let histogram_lower = crate::convertDatumToScalar(self.GetLower(0), common_prefix);
        let histogram_upper =
            crate::convertDatumToScalar(self.GetUpper(self.Len() - 1), common_prefix);
        let mut histogram_width = histogram_upper - histogram_lower;
        if histogram_width < 0.0 || histogram_width == f64::INFINITY {
            histogram_width = 0.0;
        }
        let bound_lower = histogram_lower - histogram_width;
        let bound_upper = histogram_upper + histogram_width;
        if upper_value < lower_value {
            shape.Impossible = true;
            return shape;
        }
        if upper_value == lower_value {
            histogram_width = 0.0;
        }
        let left_percent = calculateLeftOverlapPercent(
            lower_value,
            upper_value,
            bound_lower,
            histogram_lower,
            histogram_width,
        );
        let right_percent = calculateRightOverlapPercent(
            lower_value,
            upper_value,
            histogram_upper,
            bound_upper,
            histogram_width,
        );
        shape.TotalPercent = (left_percent * 0.5 + right_percent * 0.5).min(1.0);
        shape.MaxTotalPercent = (left_percent + right_percent).min(1.0);
        shape
    }

    /// Scale a cached shape without probing histogram bounds.
    pub fn ScaleOutOfRangeShape(
        &self,
        shape: OutOfRangeShape,
        mut realtime_row_count: i64,
        modify_count: i64,
        allow_modify_count: bool,
        skew_ratio: f64,
    ) -> RowEstimate {
        if shape.Empty {
            return DefaultRowEst(0.0);
        }
        let mut one_value = shape.OneValue;
        if !allow_modify_count {
            return DefaultRowEst(one_value);
        }
        if (shape.HistNDV as f64) < outOfRangeBetweenRate {
            one_value = one_value
                .min(realtime_row_count as f64 / outOfRangeBetweenRate)
                .max(1.0);
        }
        if shape.Impossible {
            return DefaultRowEst(0.0);
        }
        let added_rows = self.AbsRowCountDifference(realtime_row_count);
        let multiplier = if skew_ratio > 0.0 { skew_ratio } else { 0.5 };
        let estimated = if shape.TotalPercent > 0.0 {
            added_rows * multiplier * shape.TotalPercent
        } else {
            one_value
        };
        let mut maximum_added = added_rows;
        if modify_count == 0 || added_rows == 0.0 {
            if realtime_row_count <= 0 {
                realtime_row_count = self.TotalRowCount() as i64;
            }
            maximum_added = maximum_added.max(realtime_row_count as f64 / outOfRangeBetweenRate);
        }
        if shape.MaxTotalPercent > 0.0 {
            maximum_added *= shape.MaxTotalPercent;
        }
        let mut result = if skew_ratio > 0.0 {
            CalculateSkewRatioCounts(estimated, maximum_added, skew_ratio)
        } else {
            RowEstimate {
                Est: estimated,
                MinEst: estimated.min(one_value),
                MaxEst: estimated,
            }
        };
        result.Est = result.Est.max(one_value);
        result.MaxEst = result.MaxEst.max(result.Est).max(maximum_added);
        result
    }

    /// 是否为索引直方图（字段类型为 Blob）。
    pub fn IsIndexHist(&self) -> bool {
        self.Tp.GetType() == types::mysql::TypeBlob
    }

    /// 截断为前 bucket_count 个桶。
    /// 截取前 bucket_count 个桶得到新直方图。
    pub fn TruncateHistogram(&self, bucket_count: usize) -> Histogram {
        let mut result = self.clone();
        result.Buckets.truncate(bucket_count);
        result.Bounds.truncate(bucket_count.saturating_mul(2));
        result.Scalars.truncate(bucket_count);
        result
    }

    /// 弹出第一个桶并调整后续累计 Count。
    /// 弹出第一个桶及其边界。
    pub fn popFirstBucket(&mut self) {
        if !self.Buckets.is_empty() {
            self.Buckets.remove(0);
            self.Bounds.drain(..2);
        }
    }

    /// 检查区间 Datum kind 是否与直方图类型匹配。
    /// 检查范围端点类型是否与直方图边界一致。
    pub fn typeMatch(&self, ranges: &[HistogramRange]) -> bool {
        let kind = self.GetLower(0).Kind();
        ranges
            .iter()
            .all(|range| checkKind(&range.LowVal, kind) && checkKind(&range.HighVal, kind))
    }

    /// 按桶边界拆分查询区间。
    /// 按桶下界切分查询范围，便于分桶代价估计。
    pub fn SplitRange(&self, old_ranges: &[HistogramRange]) -> (Vec<HistogramRange>, bool) {
        if self.Len() == 0 || !self.typeMatch(old_ranges) {
            return (old_ranges.to_vec(), false);
        }
        if self.Len() == 1 {
            return (old_ranges.to_vec(), true);
        }
        let mut result = Vec::new();
        for range in old_ranges {
            let mut current = range.clone();
            for index in 1..self.Len() {
                let split = self.GetLower(index);
                if compareDatum(&current.LowVal[0], split) < 0
                    && compareDatum(split, &current.HighVal[0]) <= 0
                {
                    let mut left = current.clone();
                    left.HighVal[0] = split.clone();
                    left.HighExclude = true;
                    result.push(left);
                    current.LowVal[0] = split.clone();
                    current.LowExclude = false;
                }
            }
            result.push(current);
        }
        (result, true)
    }

    /// 从直方图中抽取高频值形成 TopN，并更新桶计数。
    /// 从桶边界候选中抽取高频前缀写入 TopN，并从 CMSketch 扣除。
    pub fn ExtractTopN(
        &mut self,
        cmsketch: &mut crate::CMSketch,
        top_n: &mut crate::TopN,
        num_columns: usize,
        num_top_n: u32,
    ) -> Result<(), astersql_errors::SharedError> {
        if self.Len() == 0 || num_top_n == 0 {
            return Ok(());
        }
        self.PreCalculateScalar();
        let limit = self.NotNullCount() / self.Len() as f64;
        let mut seen = std::collections::HashSet::new();
        let mut candidates = Vec::<(Vec<u8>, u64)>::new();
        for bound in &self.Bounds {
            let data = bound.GetBytes();
            for prefix_length in GetIndexPrefixLens(&data, num_columns)? {
                let prefix = data[..prefix_length].to_vec();
                if !seen.insert(prefix.clone()) {
                    continue;
                }
                let upper = prefixNext(&prefix);
                let estimate = self
                    .BetweenRowCount(
                        &types::NewBytesDatum(prefix.clone()),
                        &types::NewBytesDatum(upper),
                    )
                    .Est;
                if estimate >= limit {
                    candidates.push((prefix, estimate as u64));
                }
            }
        }
        candidates.sort_by(|left, right| right.1.cmp(&left.1));
        candidates.truncate(num_top_n as usize);
        top_n.TopN.clear();
        for (data, _) in candidates {
            let (h1, h2) = crate::murmur3Sum128(&data);
            let count = cmsketch.QueryBytes(&data);
            cmsketch.SubValue(h1, h2, count);
            top_n.AppendTopN(data, count);
        }
        top_n.Sort();
        Ok(())
    }
}

#[derive(Clone)]
/// 直方图上的查询区间：[low, high] 及端点是否包含。
/// 直方图切分用的查询范围（含开闭区间标志）。
pub struct HistogramRange {
    pub LowVal: Vec<types::Datum>,
    pub HighVal: Vec<types::Datum>,
    pub LowExclude: bool,
    pub HighExclude: bool,
}

/// 检查 Datum 列表 kind 是否一致/兼容。
/// 检查 Datum 列表类型是否与期望 kind 兼容（含 NULL/Min/Max）。
pub fn checkKind(values: &[types::Datum], mut kind: u8) -> bool {
    if kind == types::KindString {
        kind = types::KindBytes;
    }
    for value in values {
        let mut value_kind = value.Kind();
        if matches!(
            value_kind,
            types::KindNull | types::KindMinNotNull | types::KindMaxValue
        ) {
            continue;
        }
        if value_kind == types::KindString {
            value_kind = types::KindBytes;
        }
        return value_kind == kind;
    }
    true
}

/// 区间端点是否有效（可比较且 low<=high）。
/// 范围下界是否严格小于上界，或等值且双侧包含。
pub fn validRange(range: &HistogramRange) -> bool {
    let comparison = compareDatum(&range.LowVal[0], &range.HighVal[0]);
    comparison < 0 || (comparison == 0 && !range.LowExclude && !range.HighExclude)
}

/// 将 Datum 转为调试/展示字符串。
/// Datum 转字符串。
pub fn ValueToString(value: &types::Datum) -> Result<String, astersql_errors::SharedError> {
    value
        .ToString()
        .map_err(|error| astersql_errors::New(error.to_string()))
}

/// 克隆切片元素。
/// 深拷贝切片。
pub fn DeepSlice<T: Clone>(values: &[T]) -> Vec<T> {
    values.to_vec()
}

/// 计算索引前缀各列长度。
/// 按索引列数切分编码键，返回各前缀长度。
pub fn GetIndexPrefixLens(
    data: &[u8],
    num_columns: usize,
) -> Result<Vec<usize>, astersql_errors::SharedError> {
    let mut remaining = data.to_vec();
    let mut lengths = Vec::with_capacity(num_columns);
    let mut prefix_length = 0;
    while !remaining.is_empty() {
        let (column, rest) = codec::CutOne(remaining)?;
        prefix_length += column.len();
        lengths.push(prefix_length);
        remaining = rest;
    }
    Ok(lengths)
}

/// Return the exclusive upper bound of a byte prefix, matching kv.Key.PrefixNext.
fn prefixNext(prefix: &[u8]) -> Vec<u8> {
    let mut next = prefix.to_vec();
    for index in (0..next.len()).rev() {
        next[index] = next[index].wrapping_add(1);
        if next[index] != 0 {
            return next;
        }
    }
    next.copy_from_slice(prefix);
    next.push(0);
    next
}

/// 比较两直方图（可选忽略 ID）。
/// `Histogram::Equal` 的自由函数包装。
pub fn HistogramEqual(left: &Histogram, right: &Histogram, ignore_id: bool) -> bool {
    if ignore_id {
        let mut right = right.clone();
        right.ID = left.ID;
        left.ToString(0) == right.ToString(0)
    } else {
        left.ToString(0) == right.ToString(0)
    }
}

/// 版本是否表示已 ANALYZE。
/// 版本非 Version0 视为已 ANALYZE。
pub fn IsAnalyzed(version: i32) -> bool {
    version != Version0
}

/// 列是否已分析或可由 NDV/空值推断为已合成。
/// 列已 ANALYZE，或合成统计（有 NDV/空值）。
pub fn IsColumnAnalyzedOrSynthesized(version: i32, ndv: i64, null_count: i64) -> bool {
    IsAnalyzed(version) || ndv > 0 || null_count > 0
}

/// 直方图 → tipb::Histogram。
/// 直方图序列化为 tipb。
pub fn HistogramToProto(histogram: &Histogram) -> tipb::Histogram {
    let mut result = tipb::Histogram::new();
    result.set_ndv(histogram.NDV);
    let buckets = (0..histogram.Len())
        .map(|index| {
            let mut bucket = tipb::Bucket::new();
            bucket.set_count(histogram.Buckets[index].Count);
            bucket.set_lower_bound(histogram.GetLower(index).GetBytes());
            bucket.set_upper_bound(histogram.GetUpper(index).GetBytes());
            bucket.set_repeats(histogram.Buckets[index].Repeat);
            bucket.set_ndv(histogram.Buckets[index].NDV);
            bucket
        })
        .collect();
    result.set_buckets(RepeatedField::from_vec(buckets));
    result
}

/// tipb::Histogram → 直方图。
/// 从 tipb 反序列化直方图。
pub fn HistogramFromProto(proto: &tipb::Histogram) -> Histogram {
    let field_type = types::NewFieldType(types::mysql::TypeBlob);
    let mut result = NewHistogram(
        0,
        proto.get_ndv(),
        0,
        0,
        &field_type,
        proto.get_buckets().len(),
        0,
    );
    for bucket in proto.get_buckets() {
        result.AppendBucketWithNDV(
            &types::NewBytesDatum(bucket.get_lower_bound().to_vec()),
            &types::NewBytesDatum(bucket.get_upper_bound().to_vec()),
            bucket.get_count(),
            bucket.get_repeats(),
            bucket.get_ndv(),
        );
    }
    result
}

/// 合并两直方图到限定桶数（同版本语义）。
/// 合并两个直方图到目标桶数；相邻边界相等时合并末桶。
pub fn MergeHistograms(
    mut left: Histogram,
    mut right: Histogram,
    bucket_size: usize,
    stats_version: i32,
) -> Histogram {
    if left.Len() == 0 {
        return right;
    }
    if right.Len() == 0 {
        return left;
    }
    left.NDV += right.NDV;
    let mut offset = 0;
    // 左右直方图相邻边界相等：合并末桶并修正 NDV 重复计数。
    if compareDatum(left.GetUpper(left.Len() - 1), right.GetLower(0)) == 0 {
        left.NDV -= 1;
        let last = left.Len() - 1;
        left.Buckets[last].NDV += right.Buckets[0].NDV;
        if right.Buckets[0].NDV > 0 && left.Buckets[last].Repeat > 0 {
            left.Buckets[last].NDV -= 1;
        }
        let upper = right.GetUpper(0).clone();
        left.updateLastBucket(
            &upper,
            left.Buckets[last].Count + right.Buckets[0].Count,
            right.Buckets[0].Repeat,
            false,
        );
        offset = right.Buckets[0].Count;
        right.Buckets.remove(0);
        right.Bounds.drain(..2);
    }
    while left.Len() > bucket_size {
        left.mergeBuckets();
    }
    while right.Len() > bucket_size {
        right.mergeBuckets();
    }
    let left_count = left.Buckets.last().map_or(0, |bucket| bucket.Count);
    if right.Len() == 0 {
        return left;
    }
    let right_count = right.Buckets.last().map_or(0, |bucket| bucket.Count) - offset;
    let mut left_average = left_count as f64 / left.Len() as f64;
    let mut right_average = right_count as f64 / right.Len() as f64;
    while left.Len() > 1 && left_average * 2.0 <= right_average {
        left.mergeBuckets();
        left_average *= 2.0;
    }
    while right.Len() > 1 && right_average * 2.0 <= left_average {
        right.mergeBuckets();
        right_average *= 2.0;
    }
    for index in 0..right.Len() {
        let count = right.Buckets[index].Count + left_count - offset;
        if stats_version >= Version2 {
            left.AppendBucketWithNDV(
                right.GetLower(index),
                right.GetUpper(index),
                count,
                right.Buckets[index].Repeat,
                right.Buckets[index].NDV,
            );
        } else {
            left.AppendBucket(
                right.GetLower(index),
                right.GetUpper(index),
                count,
                right.Buckets[index].Repeat,
            );
        }
    }
    while left.Len() > bucket_size {
        left.mergeBuckets();
    }
    left
}

#[derive(Clone)]
/// 合并用的中间桶：携带显式上下界与 disjointNDV。
/// 分区合并用的中间桶：独立 Count 与 disjointNDV。
pub struct bucket4Merging {
    pub lower: types::Datum,
    pub upper: types::Datum,
    pub Bucket: Bucket,
    pub disjointNDV: i64,
}

/// 创建空的合并中间桶（函数名保留 Go 拼写）。
/// 创建空的合并中间桶。
pub fn newBucket4Meging() -> bucket4Merging {
    bucket4Merging {
        lower: types::Datum::default(),
        upper: types::Datum::default(),
        Bucket: Bucket::default(),
        disjointNDV: 0,
    }
}

/// 获取可回收的合并桶实例。
/// 对象池风格：取一个可复用中间桶。
pub fn newbucket4MergingForRecycle() -> bucket4Merging {
    newBucket4Meging()
}

/// 归还合并桶实例。
/// 重置中间桶字段以便归还。
pub fn releasebucket4MergingForRecycle(bucket: &mut bucket4Merging) {
    bucket.Bucket = Bucket::default();
    bucket.disjointNDV = 0;
}

impl bucket4Merging {
    /// 克隆合并中间桶。
    /// 深拷贝中间桶。
    pub fn Clone(&self) -> bucket4Merging {
        self.clone()
    }

    /// 克隆为新的合并桶。
    /// 深拷贝别名。
    pub fn CloneBucket(&self) -> bucket4Merging {
        self.clone()
    }
}

impl TopNMeta {
    /// 由 TopNMeta 构建合并中间桶。
    /// 将 TopN 条目转为单点中间桶。
    pub fn buildBucket4Merging(
        &self,
        datum: &types::Datum,
        analyze_version: i32,
    ) -> bucket4Merging {
        bucket4Merging {
            lower: datum.clone(),
            upper: datum.clone(),
            Bucket: Bucket {
                Count: self.Count as i64,
                Repeat: self.Count as i64,
                NDV: i64::from(analyze_version > Version2),
            },
            disjointNDV: 0,
        }
    }
}

impl Histogram {
    /// 将直方图各桶转为合并中间表示。
    /// 将直方图各桶转为独立 Count 的中间桶列表。
    pub fn buildBucket4Merging(&self) -> Vec<bucket4Merging> {
        (0..self.Len())
            .map(|index| bucket4Merging {
                lower: self.GetLower(index).clone(),
                upper: self.GetUpper(index).clone(),
                Bucket: Bucket {
                    Count: self.BucketCount(index),
                    Repeat: self.Buckets[index].Repeat,
                    NDV: self.Buckets[index].NDV,
                },
                disjointNDV: 0,
            })
            .collect()
    }
}

/// 合并两中间桶的 NDV（处理相等/不相交/重叠）。
/// 按两桶区间关系合并 NDV（相同/包含/相交/不相交）。
pub fn mergeBucketNDV(
    left: &bucket4Merging,
    right: &bucket4Merging,
) -> Result<bucket4Merging, astersql_errors::SharedError> {
    if left.Bucket.Count == 0 {
        return Ok(right.clone());
    }
    if right.Bucket.Count == 0 {
        let mut result = right.clone();
        result.lower = left.lower.clone();
        result.upper = left.upper.clone();
        result.Bucket.NDV = left.Bucket.NDV;
        return Ok(result);
    }
    let upper_comparison = compareDatum(&right.upper, &left.upper);
    if upper_comparison < 0 {
        return Err(astersql_errors::New("illegal bucket order"));
    }
    let mut result = right.clone();
    if upper_comparison == 0 {
        let lower_comparison = compareDatum(&right.lower, &left.lower);
        if lower_comparison < 0 {
            return Err(astersql_errors::New("illegal bucket order"));
        }
        if lower_comparison == 0 {
            result.Bucket.NDV = left.Bucket.NDV.max(right.Bucket.NDV);
            return Ok(result);
        }
        let ratio = crate::calcFraction4Datums(&left.lower, &left.upper, &right.lower);
        result.Bucket.NDV = (ratio * left.Bucket.NDV as f64
            + ((1.0 - ratio) * left.Bucket.NDV as f64).max(right.Bucket.NDV as f64))
            as i64;
        result.lower = left.lower.clone();
        return Ok(result);
    }
    if compareDatum(&right.lower, &left.upper) >= 0 {
        result.lower = left.lower.clone();
        result.upper = left.upper.clone();
        result.disjointNDV += right.Bucket.NDV;
        result.Bucket.NDV = left.Bucket.NDV;
        return Ok(result);
    }
    let upper_ratio = crate::calcFraction4Datums(&right.lower, &right.upper, &left.upper);
    if compareDatum(&right.lower, &left.lower) >= 0 {
        let lower_ratio = crate::calcFraction4Datums(&left.lower, &left.upper, &right.lower);
        result.Bucket.NDV = (lower_ratio * left.Bucket.NDV as f64
            + ((1.0 - lower_ratio) * left.Bucket.NDV as f64)
                .max(upper_ratio * right.Bucket.NDV as f64)
            + (1.0 - upper_ratio) * right.Bucket.NDV as f64) as i64;
        result.lower = left.lower.clone();
    } else {
        let lower_ratio = crate::calcFraction4Datums(&right.lower, &right.upper, &left.lower);
        result.Bucket.NDV = (lower_ratio * right.Bucket.NDV as f64
            + (left.Bucket.NDV as f64).max((upper_ratio - lower_ratio) * right.Bucket.NDV as f64)
            + (1.0 - upper_ratio) * right.Bucket.NDV as f64) as i64;
    }
    Ok(result)
}

/// 合并分区桶列表到全局桶预算。
/// 将一组有序中间桶合并为一个全局桶并估计 NDV。
pub fn mergePartitionBuckets(
    buckets: &[bucket4Merging],
) -> Result<bucket4Merging, astersql_errors::SharedError> {
    let Some(last) = buckets.last() else {
        return Err(astersql_errors::New("not enough buckets to merge"));
    };
    let mut result = newBucket4Meging();
    result.upper = last.upper.clone();
    let mut right = last.clone();
    let mut total_ndv = 0;
    for (index, bucket) in buckets.iter().enumerate().rev() {
        total_ndv += bucket.Bucket.NDV;
        result.Bucket.Count += bucket.Bucket.Count;
        if compareDatum(&bucket.upper, &result.upper) == 0 {
            result.Bucket.Repeat += bucket.Bucket.Repeat;
        }
        if index + 1 != buckets.len() {
            right = mergeBucketNDV(bucket, &right)?;
        }
    }
    result.Bucket.NDV = right.Bucket.NDV + right.disjointNDV;
    result.Bucket.NDV = ((result.Bucket.NDV as f64 * 1.15_f64.powi(buckets.len() as i32 - 1))
        as i64)
        .min(total_ndv);
    result.lower = buckets
        .iter()
        .map(|bucket| &bucket.lower)
        .min_by(|left, right| compareDatum(left, right).cmp(&0))
        .cloned()
        .unwrap_or_default();
    Ok(result)
}

/// 按上界排序合并桶。
/// 按上界、再按下界排序中间桶。
pub fn sortBucketsByUpperBound(buckets: &mut [bucket4Merging]) {
    buckets.sort_by(|left, right| {
        let upper = compareDatum(&left.upper, &right.upper);
        if upper == 0 {
            compareDatum(&left.lower, &right.lower).cmp(&0)
        } else {
            upper.cmp(&0)
        }
    });
}

/// 检查合并桶是否已按上界有序。
/// 检查中间桶是否已按上界有序。
pub fn checkBucket4MergingIsSorted(buckets: &[bucket4Merging]) -> bool {
    buckets.windows(2).all(|pair| {
        compareDatum(&pair[0].upper, &pair[1].upper) < 0
            || (compareDatum(&pair[0].upper, &pair[1].upper) == 0
                && compareDatum(&pair[0].lower, &pair[1].lower) <= 0)
    })
}

/// 将分区直方图合并为全局直方图（可选合并 TopN）。
/// 多分区直方图（及弹出 TopN）合并为全局直方图。
pub fn MergePartitionHist2GlobalHist(
    histograms: &[Histogram],
    popped_top_n: &[TopNMeta],
    expected_bucket_count: usize,
    is_index: bool,
    analyze_version: i32,
) -> Result<Option<Histogram>, astersql_errors::SharedError> {
    MergePartitionHist2GlobalHistWithLocation(
        histograms,
        popped_top_n,
        expected_bucket_count,
        is_index,
        analyze_version,
        codec::time::UTC,
    )
}

/// Merge partition histograms using the session location for flattened TopN values.
pub fn MergePartitionHist2GlobalHistWithLocation(
    histograms: &[Histogram],
    popped_top_n: &[TopNMeta],
    expected_bucket_count: usize,
    is_index: bool,
    analyze_version: i32,
    location: chrono_tz::Tz,
) -> Result<Option<Histogram>, astersql_errors::SharedError> {
    if expected_bucket_count == 0 {
        return Err(astersql_errors::New("expBucketNumber can not be zero"));
    }
    let Some(first) = histograms.first() else {
        return Ok(None);
    };
    let total_null = histograms.iter().map(|histogram| histogram.NullCount).sum();
    let total_size = histograms
        .iter()
        .map(|histogram| histogram.TotColSize)
        .sum();
    let mut buckets = histograms
        .iter()
        .flat_map(Histogram::buildBucket4Merging)
        .collect::<Vec<_>>();
    for item in popped_top_n {
        let datum = crate::topNMetaToDatum(item, &first.Tp, is_index, location)?;
        buckets.push(item.buildBucket4Merging(&datum, analyze_version));
    }
    buckets.retain(|bucket| bucket.Bucket.Count != 0);
    if buckets.is_empty() {
        return Ok(Some(NewHistogram(
            first.ID,
            0,
            total_null,
            first.LastUpdateVersion,
            &first.Tp,
            0,
            total_size,
        )));
    }
    sortBucketsByUpperBound(&mut buckets);
    let total_count = buckets
        .iter()
        .map(|bucket| bucket.Bucket.Count)
        .sum::<i64>();
    let mut global_buckets = Vec::with_capacity(expected_bucket_count);
    let mut sum = 0_i64;
    let mut previous_sum = 0_i64;
    let mut right = buckets.len();
    let mut target_bucket = 1_i64;
    let threshold = (total_count / expected_bucket_count as i64) * 80 / 100;
    let mut current_left_most: Option<types::Datum> = None;
    let mut index = buckets.len() as isize - 1;
    while index >= 0 {
        let position = index as usize;
        if current_left_most
            .as_ref()
            .is_none_or(|left| compareDatum(left, &buckets[position].lower) > 0)
        {
            current_left_most = Some(buckets[position].lower.clone());
        }
        sum += buckets[position].Bucket.Count;
        if sum >= total_count * target_bucket / expected_bucket_count as i64
            && sum - previous_sum >= threshold
        {
            // Equal upper bounds belong to one global bucket.
            while index > 0
                && compareDatum(
                    &buckets[index as usize - 1].upper,
                    &buckets[index as usize].upper,
                ) == 0
            {
                index -= 1;
                sum += buckets[index as usize].Bucket.Count;
            }
            let position = index as usize;
            if compareDatum(
                current_left_most.as_ref().expect("left-most bucket bound"),
                &buckets[position].lower,
            ) > 0
            {
                current_left_most = Some(buckets[position].lower.clone());
            }

            // Pull wholly-overlapping buckets into this merge and split a
            // partially-overlapping bucket at the current left boundary.
            let left_most_valid = position;
            let mut merge_buffer = Vec::new();
            let mut cut_any = false;
            while index > 0 {
                let candidate = index as usize - 1;
                let left = current_left_most.as_ref().expect("left-most bucket bound");
                if compareDatum(&buckets[candidate].upper, left) < 0 {
                    break;
                }
                index -= 1;
                let candidate = index as usize;
                if compareDatum(&buckets[candidate].lower, left) >= 0 {
                    sum += buckets[candidate].Bucket.Count;
                    merge_buffer.push(buckets[candidate].clone());
                    continue;
                }
                let overlap = 1.0
                    - crate::calcFraction4Datums(
                        &buckets[candidate].lower,
                        &buckets[candidate].upper,
                        left,
                    );
                let overlap_count = (buckets[candidate].Bucket.Count as f64 * overlap) as i64;
                let overlap_ndv = (buckets[candidate].Bucket.NDV as f64 * overlap) as i64;
                sum += overlap_count;
                let old_upper = buckets[candidate].upper.clone();
                buckets[candidate].Bucket.Count =
                    (buckets[candidate].Bucket.Count - overlap_count).max(0);
                buckets[candidate].Bucket.NDV =
                    (buckets[candidate].Bucket.NDV - overlap_ndv).max(0);
                buckets[candidate].Bucket.Repeat = 0;
                buckets[candidate].upper = left.clone();
                merge_buffer.push(bucket4Merging {
                    lower: left.clone(),
                    upper: old_upper,
                    Bucket: Bucket {
                        Count: overlap_count,
                        Repeat: 0,
                        NDV: overlap_ndv,
                    },
                    disjointNDV: 0,
                });
                cut_any = true;
            }

            let mut merged = if !cut_any {
                mergePartitionBuckets(&buckets[index as usize..right])?
            } else {
                merge_buffer.reverse();
                merge_buffer.extend_from_slice(&buckets[left_most_valid..right]);
                let merged = mergePartitionBuckets(&merge_buffer)?;
                sortBucketsByUpperBound(&mut buckets[index as usize..left_most_valid]);
                // Fully-contained buckets have already been consumed. Only
                // the left remnants of split buckets participate next time.
                let left = current_left_most.as_ref().expect("left-most bucket bound");
                let mut next_right = left_most_valid;
                while next_right > index as usize
                    && compareDatum(&buckets[next_right - 1].lower, left) >= 0
                {
                    next_right -= 1;
                }
                index = next_right as isize;
                merged
            };
            merged.lower = current_left_most
                .take()
                .expect("merged bucket has a left bound");
            global_buckets.push(merged);
            right = index as usize;
            target_bucket += 1;
            previous_sum = sum;
        }
        index -= 1;
    }
    if right > 0 {
        let mut merged = mergePartitionBuckets(&buckets[..right])?;
        merged.lower = buckets[..right]
            .iter()
            .map(|bucket| &bucket.lower)
            .min_by(|left, right| compareDatum(left, right).cmp(&0))
            .cloned()
            .expect("non-empty remaining buckets");
        global_buckets.push(merged);
    }
    global_buckets.reverse();
    let mut result = NewHistogram(
        first.ID,
        0,
        total_null,
        first.LastUpdateVersion,
        &first.Tp,
        global_buckets.len(),
        total_size,
    );
    let mut cumulative = 0;
    for mut bucket in global_buckets {
        cumulative += bucket.Bucket.Count;
        bucket.Bucket.Count = cumulative;
        let repeat = histograms
            .iter()
            .map(|histogram| histogram.EqualRowCount(&bucket.upper, is_index).0)
            .sum::<f64>() as i64;
        bucket.Bucket.Repeat = bucket.Bucket.Repeat.max(repeat);
        if !is_index {
            bucket.Bucket.NDV = 0;
        }
        result.AppendBucketWithNDV(
            &bucket.lower,
            &bucket.upper,
            bucket.Bucket.Count,
            bucket.Bucket.Repeat,
            bucket.Bucket.NDV,
        );
    }
    Ok(Some(result))
}

/// 按偏斜比计算 Min/Est/Max 行数估计。
/// 按偏斜比在点估计与上界之间展开 Min/Est/Max。
pub fn CalculateSkewRatioCounts(estimate: f64, skew_estimate: f64, skew_ratio: f64) -> RowEstimate {
    let difference = skew_estimate - estimate;
    let skew = (difference * skew_ratio).max(0.0);
    let maximum_skew = difference.min(2.0 * skew);
    RowEstimate {
        Est: estimate + skew,
        MinEst: estimate,
        MaxEst: estimate + maximum_skew,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
/// 带最小/期望/最大的行数估计三元组。
/// 行数估计三元组：期望、乐观下界、风险上界。
pub struct RowEstimate {
    pub Est: f64,
    pub MinEst: f64,
    pub MaxEst: f64,
}

/// 用同一值构造对称 RowEstimate。
/// 三点估计取相同值。
pub fn DefaultRowEst(estimate: f64) -> RowEstimate {
    RowEstimate {
        Est: estimate,
        MinEst: estimate,
        MaxEst: estimate,
    }
}

impl RowEstimate {
    /// 分量相加。
    pub fn Add(&mut self, other: RowEstimate) {
        self.Est += other.Est;
        self.MinEst += other.MinEst;
        self.MaxEst += other.MaxEst;
    }

    /// 三个分量同加 value。
    /// 三点同加常量。
    pub fn AddAll(&mut self, value: f64) {
        self.Est += value;
        self.MinEst += value;
        self.MaxEst += value;
    }

    /// 分量相减。
    pub fn Subtract(&mut self, other: RowEstimate) {
        self.Est -= other.Est;
        self.MinEst -= other.MinEst;
        self.MaxEst -= other.MaxEst;
    }

    /// 三分量同乘。
    /// 三点同乘。
    pub fn MultiplyAll(&mut self, value: f64) {
        self.Est *= value;
        self.MinEst *= value;
        self.MaxEst *= value;
    }

    /// 三分量同除。
    /// 三点同除。
    pub fn DivideAll(&mut self, value: f64) {
        self.Est /= value;
        self.MinEst /= value;
        self.MaxEst /= value;
    }

    /// 将三分量钳制到 [minimum, maximum]。
    /// 将三点夹到 [minimum, maximum]，保持 Min≤Est≤Max。
    pub fn Clamp(&mut self, minimum: f64, maximum: f64) {
        self.Est = self.Est.clamp(minimum, maximum);
        self.MinEst = self.MinEst.min(self.Est).clamp(minimum, maximum);
        self.MaxEst = self.MaxEst.max(self.Est).clamp(minimum, maximum);
    }
}

/// 计算左侧重叠比例（区间估计用）。
/// 计算查询区间与直方图左外侧扩展带的重叠比例。
pub fn calculateLeftOverlapPercent(
    mut lower: f64,
    mut upper: f64,
    bound_lower: f64,
    histogram_lower: f64,
    histogram_width: f64,
) -> f64 {
    if histogram_width <= 0.0 {
        return 0.0;
    }
    lower = lower.max(bound_lower);
    upper = upper.min(histogram_lower);
    if lower >= upper {
        return 0.0;
    }
    ((upper - bound_lower).powi(2) - (lower - bound_lower).powi(2)) / histogram_width.powi(2)
}

/// 计算右侧重叠比例。
/// 计算查询区间与直方图右外侧扩展带的重叠比例。
pub fn calculateRightOverlapPercent(
    mut lower: f64,
    mut upper: f64,
    histogram_upper: f64,
    bound_upper: f64,
    histogram_width: f64,
) -> f64 {
    if histogram_width <= 0.0 {
        return 0.0;
    }
    lower = lower.max(histogram_upper);
    upper = upper.min(bound_upper);
    if lower >= upper {
        return 0.0;
    }
    ((bound_upper - lower).powi(2) - (bound_upper - upper).powi(2)) / histogram_width.powi(2)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// 统计加载状态：是否初始化及驱逐程度。
/// 统计加载/驱逐状态。
pub struct StatsLoadedStatus {
    pub(crate) statsInitialized: bool,
    pub(crate) evictedStatus: i32,
}

/// 构造“全量已加载”状态。
/// 构造全量加载状态。
pub fn NewStatsFullLoadStatus() -> StatsLoadedStatus {
    StatsLoadedStatus {
        statsInitialized: true,
        evictedStatus: AllLoaded,
    }
}

/// 构造“全部已驱逐”状态。
/// 构造全部驱逐状态。
pub fn NewStatsAllEvictedStatus() -> StatsLoadedStatus {
    StatsLoadedStatus {
        statsInitialized: true,
        evictedStatus: AllEvicted,
    }
}

impl StatsLoadedStatus {
    /// 拷贝加载状态。
    /// 拷贝状态。
    pub fn Copy(&self) -> StatsLoadedStatus {
        *self
    }

    /// 是否已初始化。
    pub fn IsStatsInitialized(&self) -> bool {
        self.statsInitialized
    }

    /// 是否仍需加载数据。
    /// 是否需要从存储重新加载。
    pub fn IsLoadNeeded(&self) -> bool {
        self.statsInitialized && self.evictedStatus > AllLoaded
    }

    /// 必要统计是否已在内存。
    /// 必要统计是否仍在内存。
    pub fn IsEssentialStatsLoaded(&self) -> bool {
        self.statsInitialized && self.evictedStatus < AllEvicted
    }

    /// 是否全部驱逐。
    /// 是否已全部驱逐。
    pub fn IsAllEvicted(&self) -> bool {
        self.statsInitialized && self.evictedStatus >= AllEvicted
    }

    /// 是否全量加载。
    pub fn IsFullLoad(&self) -> bool {
        self.statsInitialized && self.evictedStatus == AllLoaded
    }

    /// 状态的调试字符串。
    /// 状态可读字符串。
    pub fn StatusToString(&self) -> &'static str {
        if !self.statsInitialized {
            "unInitialized"
        } else if self.evictedStatus == AllLoaded {
            "allLoaded"
        } else if self.evictedStatus == AllEvicted {
            "allEvicted"
        } else {
            "unknown"
        }
    }
}
