// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// 索引统计 ANALYZE 下推与结果合并。
//
// 从存储层拉取索引直方图、CM/FM Sketch 与 TopN，按 v2 统计格式合并；
// 并提供仅估计 NDV 的轻量路径。

#![allow(non_snake_case)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};

/// 统计信息格式版本 2（直方图 + TopN + Sketch）。
const STATS_VERSION_2: i32 = 2;
/// FM Sketch 哈希集合的上限，超出则丢弃最大哈希。
const MAX_SKETCH_SIZE: usize = 10_000;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
/// 统计收集使用的通用数据值表示。
pub enum Datum {
    Null,
    Signed(i64),
    Unsigned(u64),
    Bytes(Vec<u8>),
    Text(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 索引扫描范围描述：是否含 NULL、是否全表。
pub struct Range {
    pub include_null: bool,
    pub full: bool,
}
impl Range {
    /// 含 NULL 的全范围。
    pub fn full() -> Self {
        Self {
            include_null: true,
            full: true,
        }
    }
    /// 不含 NULL 的全范围。
    pub fn full_not_null() -> Self {
        Self {
            include_null: false,
            full: true,
        }
    }
    /// 仅 NULL 值范围。
    pub fn null() -> Self {
        Self {
            include_null: true,
            full: false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 待分析索引的元数据。
pub struct IndexInfo {
    pub id: i64,
    pub column_count: usize,
    pub primary: bool,
    pub multi_valued: bool,
    pub global: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 索引 ANALYZE 错误种类。
pub enum AnalyzeIndexError {
    Backend(String),
    Cancelled,
    Decode(String),
    InvalidStatsVersion(i32),
    MissingResult,
    Merge(String),
}
impl Display for AnalyzeIndexError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{self:?}")
    }
}
impl std::error::Error for AnalyzeIndexError {}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 直方图桶：上下界、累计计数与重复数。
pub struct Bucket {
    pub lower: Datum,
    pub upper: Datum,
    pub count: i64,
    pub repeats: i64,
}
impl Default for Datum {
    fn default() -> Self {
        Self::Null
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 列/索引等值分布直方图。
pub struct Histogram {
    pub id: i64,
    pub ndv: i64,
    pub null_count: i64,
    pub buckets: Vec<Bucket>,
}
impl Histogram {
    /// 桶累计行数加上 NULL 计数。
    pub fn total_row_count(&self) -> i64 {
        self.buckets
            .last()
            .map_or(0, |bucket| bucket.count)
            .saturating_add(self.null_count)
    }
    // 从桶计数中扣除已计入 TopN 的频次，避免双重计算。
    /// 从直方图中扣除 TopN 已统计的频次。
    fn remove_topn(&mut self, topn: &TopN) {
        let values = topn.values.iter().collect::<Vec<_>>();
        let mut value_index = 0;
        let mut removed = 0_i64;
        for bucket in &mut self.buckets {
            while let Some((value, count)) = values.get(value_index) {
                if datum_cmp_bytes(&bucket.lower, value).is_gt() {
                    value_index += 1;
                    continue;
                }
                let upper_cmp = datum_cmp_bytes(&bucket.upper, value);
                if upper_cmp.is_lt() {
                    break;
                }
                removed = removed.saturating_add(**count as i64);
                value_index += 1;
                if upper_cmp.is_eq() {
                    bucket.repeats = 0;
                    break;
                }
            }
            bucket.count = bucket.count.saturating_sub(removed);
        }
    }
    /// 移除 v2 索引统计中既无桶内行数、也无重复值的空桶。
    fn standardize_v2(&mut self) {
        let mut previous = 0_i64;
        self.buckets.retain(|bucket| {
            let bucket_count = bucket.count.saturating_sub(previous);
            previous = bucket.count;
            bucket_count > 0 || bucket.repeats > 0
        });
    }
}

fn datum_cmp_bytes(datum: &Datum, bytes: &[u8]) -> std::cmp::Ordering {
    match datum {
        Datum::Bytes(value) => value.as_slice().cmp(bytes),
        Datum::Text(value) => value.as_bytes().cmp(bytes),
        Datum::Signed(value) => value.to_be_bytes().as_slice().cmp(bytes),
        Datum::Unsigned(value) => value.to_be_bytes().as_slice().cmp(bytes),
        Datum::Null => [].as_slice().cmp(bytes),
    }
}

fn merge_bucket_pairs(histogram: &mut Histogram) {
    let mut merged = Vec::with_capacity(histogram.buckets.len().div_ceil(2));
    for pair in histogram.buckets.chunks(2) {
        if pair.len() == 1 {
            merged.push(pair[0].clone());
        } else {
            merged.push(Bucket {
                lower: pair[0].lower.clone(),
                upper: pair[1].upper.clone(),
                count: pair[1].count,
                repeats: pair[1].repeats,
            });
        }
    }
    histogram.buckets = merged;
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// Count-Min Sketch：近似频次估计结构。
pub struct CMSketch {
    pub depth: usize,
    pub width: usize,
    pub counters: BTreeMap<Vec<u8>, u64>,
    pub default_value: u64,
}
impl CMSketch {
    /// 合并同维度 CMS；维度不一致则报错。
    fn merge(&mut self, other: &Self) -> Result<(), AnalyzeIndexError> {
        if self.depth != other.depth || self.width != other.width {
            return Err(AnalyzeIndexError::Merge("CMS dimensions differ".into()));
        }
        for (key, count) in &other.counters {
            *self.counters.entry(key.clone()).or_default() += count;
        }
        Ok(())
    }
    /// 按 NDV 估算未显式计数键的默认频次。
    fn calculate_default(&mut self, ndv: i64) {
        let total: u64 = self.counters.values().sum();
        self.default_value = total / (ndv.max(1) as u64);
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// Flajolet-Martin Sketch：用于估计 NDV（不同值个数）。
pub struct FMSketch {
    pub hashes: BTreeSet<u64>,
}
impl FMSketch {
    /// 合并哈希集合并裁剪到最大容量。
    fn merge(&mut self, other: &Self) {
        self.hashes.extend(other.hashes.iter().copied());
        while self.hashes.len() > MAX_SKETCH_SIZE {
            let Some(last) = self.hashes.last().copied() else {
                break;
            };
            self.hashes.remove(&last);
        }
    }
    /// 以哈希集合大小作为 NDV 估计。
    pub fn ndv(&self) -> u64 {
        self.hashes.len() as u64
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// 高频值及其出现次数。
pub struct TopN {
    pub values: BTreeMap<Vec<u8>, u64>,
}
impl TopN {
    /// TopN 各值频次之和。
    pub fn total_count(&self) -> u64 {
        self.values.values().sum()
    }
    /// 合并 TopN，超出容量的溢出到 CMS。
    fn merge(&mut self, other: &Self, capacity: usize, cms: &mut CMSketch) {
        for (value, count) in &other.values {
            *self.values.entry(value.clone()).or_default() += count;
        }
        let mut ranked = self
            .values
            .iter()
            .map(|(value, count)| (value.clone(), *count))
            .collect::<Vec<_>>();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        self.values.clear();
        for (index, (value, count)) in ranked.into_iter().enumerate() {
            if index < capacity {
                self.values.insert(value, count);
            } else {
                *cms.counters.entry(value).or_default() += count;
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
/// 单次下推响应中的直方图与各类 Sketch。
pub struct AnalyzeIndexResponse {
    pub histogram: Histogram,
    pub cms: Option<CMSketch>,
    pub fm: Option<FMSketch>,
    pub topn: Option<TopN>,
}

/// 索引分析结果流：逐条取响应并可关闭。
pub trait AnalyzeResultStream: Send {
    fn next_response(&mut self) -> Result<Option<AnalyzeIndexResponse>, AnalyzeIndexError>;
    fn close(&mut self) -> Result<(), AnalyzeIndexError>;
}

/// 索引分析后端：打开结果流、检查 kill、更新作业进度。
pub trait AnalyzeIndexBackend: Send + Sync + 'static {
    fn open_index_result(
        &self,
        index: &IndexInfo,
        ranges: &[Range],
        common_handle: bool,
        null_range: bool,
        snapshot: Option<u64>,
        concurrency: usize,
    ) -> Result<Box<dyn AnalyzeResultStream>, AnalyzeIndexError>;
    fn killed(&self) -> Result<(), AnalyzeIndexError>;
    fn update_job_progress(&self, rows: i64);
}

#[derive(Clone, Debug, Default)]
/// 桶数、TopN 容量与 CMS 维度配置。
pub struct AnalyzeIndexOptions {
    pub buckets: usize,
    pub topn: usize,
    pub cms_depth: usize,
    pub cms_width: usize,
}

#[derive(Clone, Debug, Default)]
/// 一组直方图/CMS/FM/TopN 结果；`is_index` 区分列与索引。
pub struct AnalyzeResult {
    pub histograms: Vec<Histogram>,
    pub cms: Vec<CMSketch>,
    pub fm: Vec<FMSketch>,
    pub topn: Vec<TopN>,
    pub is_index: bool,
}
#[derive(Clone, Debug, Default)]
/// 索引 ANALYZE 完整输出（可含错误）。
pub struct AnalyzeResults {
    pub table_id: i64,
    pub results: Vec<AnalyzeResult>,
    pub stats_version: i32,
    pub count: i64,
    pub snapshot: u64,
    pub for_mv_or_global: bool,
    pub error: Option<AnalyzeIndexError>,
}

/// 索引统计下推执行器。
pub struct AnalyzeIndexExec<B: AnalyzeIndexBackend> {
    pub backend: B,
    pub table_id: i64,
    pub idxInfo: IndexInfo,
    pub isCommonHandle: bool,
    pub result: Option<Box<dyn AnalyzeResultStream>>,
    pub countNullRes: Option<Box<dyn AnalyzeResultStream>>,
    pub options: AnalyzeIndexOptions,
    pub stats_version: i32,
    pub snapshot: u64,
    pub enable_snapshot: bool,
    pub concurrency: usize,
}

/// 完整索引统计下推：构建直方图/FM/TopN 并打包结果。
pub fn analyzeIndexPushdown<B: AnalyzeIndexBackend>(
    idx_exec: &mut AnalyzeIndexExec<B>,
) -> AnalyzeResults {
    let ranges = if idx_exec.idxInfo.column_count == 1 {
        vec![Range::full_not_null()]
    } else {
        vec![Range::full()]
    };
    let built = idx_exec.buildStats(&ranges, true);
    match built {
        Ok((histogram, _cms, fm, topn)) => {
            if idx_exec.stats_version != STATS_VERSION_2 {
                return AnalyzeResults {
                    table_id: idx_exec.table_id,
                    error: Some(AnalyzeIndexError::InvalidStatsVersion(
                        idx_exec.stats_version,
                    )),
                    ..Default::default()
                };
            }
            let count = histogram
                .total_row_count()
                .saturating_add(topn.total_count() as i64);
            AnalyzeResults {
                table_id: idx_exec.table_id,
                results: vec![AnalyzeResult {
                    histograms: vec![histogram],
                    cms: Vec::new(),
                    fm: vec![fm],
                    topn: vec![topn],
                    is_index: true,
                }],
                stats_version: idx_exec.stats_version,
                count,
                snapshot: idx_exec.snapshot,
                for_mv_or_global: idx_exec.idxInfo.multi_valued || idx_exec.idxInfo.global,
                error: None,
            }
        }
        Err(error) => AnalyzeResults {
            table_id: idx_exec.table_id,
            error: Some(error),
            ..Default::default()
        },
    }
}

impl<B: AnalyzeIndexBackend> AnalyzeIndexExec<B> {
    /// 打开范围、合并主结果与 NULL 结果，写回索引 ID。
    pub fn buildStats(
        &mut self,
        ranges: &[Range],
        consider_null: bool,
    ) -> Result<(Histogram, CMSketch, FMSketch, TopN), AnalyzeIndexError> {
        self.open(ranges, consider_null)?;
        let mut main = self.result.take().ok_or(AnalyzeIndexError::MissingResult)?;
        let mut null_result = self.countNullRes.take();
        let result = (|| {
            let mut built = self.buildStatsFromResult(main.as_mut(), true)?;
            if let Some(null_result) = null_result.as_mut() {
                let (null_histogram, _, _, _) =
                    self.buildStatsFromResult(null_result.as_mut(), false)?;
                if let Some(bucket) = null_histogram.buckets.last() {
                    built.0.null_count = bucket.count;
                }
            }
            built.0.id = self.idxInfo.id;
            Ok(built)
        })();
        let main_close = main.close();
        let null_close = null_result.as_mut().map(|result| result.close());
        match result {
            Err(error) => Err(error),
            Ok(built) => {
                main_close?;
                if let Some(close) = null_close {
                    close?;
                }
                Ok(built)
            }
        }
    }

    /// 拉取主范围结果；单列索引可额外拉取 NULL 范围。
    pub fn open(&mut self, ranges: &[Range], consider_null: bool) -> Result<(), AnalyzeIndexError> {
        self.fetchAnalyzeResult(ranges, false)?;
        if consider_null && self.idxInfo.column_count == 1 {
            self.fetchAnalyzeResult(&[Range::null()], true)?;
        }
        Ok(())
    }

    /// 向后端打开索引结果流并挂到 result / countNullRes。
    pub fn fetchAnalyzeResult(
        &mut self,
        ranges: &[Range],
        null_range: bool,
    ) -> Result<(), AnalyzeIndexError> {
        let snapshot = self.enable_snapshot.then_some(self.snapshot);
        let result = self.backend.open_index_result(
            &self.idxInfo,
            ranges,
            self.isCommonHandle && self.idxInfo.primary,
            null_range,
            snapshot,
            self.concurrency.max(1),
        )?;
        if null_range {
            self.countNullRes = Some(result);
        } else {
            self.result = Some(result);
        }
        Ok(())
    }

    /// 迭代结果流合并直方图与 Sketch，必要时剥离 TopN。
    pub fn buildStatsFromResult(
        &self,
        result: &mut dyn AnalyzeResultStream,
        need_cms: bool,
    ) -> Result<(Histogram, CMSketch, FMSketch, TopN), AnalyzeIndexError> {
        if self.stats_version != STATS_VERSION_2 {
            return Err(AnalyzeIndexError::InvalidStatsVersion(self.stats_version));
        }
        let mut histogram = Histogram::default();
        let mut cms = CMSketch {
            depth: self.options.cms_depth,
            width: self.options.cms_width,
            ..Default::default()
        };
        let mut fm = FMSketch::default();
        let mut topn = TopN::default();
        loop {
            self.backend.killed()?;
            let Some(response) = result.next_response()? else {
                break;
            };
            (histogram, cms, fm, topn) = updateIndexResult(
                self.backend_ref(),
                response,
                histogram,
                cms,
                fm,
                topn,
                self.options.buckets,
                self.options.topn,
                self.stats_version,
                need_cms,
            )?;
        }
        if need_cms && topn.total_count() > 0 {
            histogram.remove_topn(&topn);
        }
        histogram.standardize_v2();
        if need_cms {
            cms.calculate_default(histogram.ndv);
        }
        Ok((histogram, cms, fm, topn))
    }

    /// 仅构建 FM Sketch（及可选 NULL 直方图）的轻量路径。
    pub fn buildSimpleStats(
        &mut self,
        ranges: &[Range],
        consider_null: bool,
    ) -> Result<(FMSketch, Option<Histogram>), AnalyzeIndexError> {
        self.open(ranges, consider_null)?;
        let mut main = self.result.take().ok_or(AnalyzeIndexError::MissingResult)?;
        let mut null_result = self.countNullRes.take();
        let result = (|| {
            let (_, _, fm, _) = self.buildStatsFromResult(main.as_mut(), false)?;
            let null_histogram = if let Some(null_result) = null_result.as_mut() {
                let (histogram, _, _, _) =
                    self.buildStatsFromResult(null_result.as_mut(), false)?;
                (!histogram.buckets.is_empty()).then_some(histogram)
            } else {
                None
            };
            Ok((fm, null_histogram))
        })();
        let main_close = main.close();
        let null_close = null_result.as_mut().map(|result| result.close());
        match result {
            Err(error) => Err(error),
            Ok(built) => {
                main_close?;
                if let Some(close) = null_close {
                    close?;
                }
                Ok(built)
            }
        }
    }

    /// 返回后端引用。
    fn backend_ref(&self) -> &B {
        &self.backend
    }
}

/// 仅估计索引 NDV 的下推路径（用于特殊索引）。
pub fn analyzeIndexNDVPushDown<B: AnalyzeIndexBackend>(
    idx_exec: &mut AnalyzeIndexExec<B>,
) -> AnalyzeResults {
    let ranges = if idx_exec.idxInfo.column_count == 1 {
        vec![Range::full_not_null()]
    } else {
        vec![Range::full()]
    };
    match idx_exec.buildSimpleStats(&ranges, idx_exec.idxInfo.column_count == 1) {
        Ok((fm, null_histogram)) => {
            let count = null_histogram
                .as_ref()
                .and_then(|histogram| histogram.buckets.last())
                .map_or(0, |bucket| bucket.count);
            AnalyzeResults {
                table_id: idx_exec.table_id,
                results: vec![AnalyzeResult {
                    histograms: vec![Histogram {
                        id: idx_exec.idxInfo.id,
                        ..Default::default()
                    }],
                    cms: Vec::new(),
                    fm: vec![fm],
                    topn: Vec::new(),
                    is_index: true,
                }],
                stats_version: idx_exec.stats_version,
                count,
                snapshot: idx_exec.snapshot,
                for_mv_or_global: false,
                error: None,
            }
        }
        Err(error) => AnalyzeResults {
            table_id: idx_exec.table_id,
            error: Some(error),
            ..Default::default()
        },
    }
}

/// 将单次响应合并进累积的直方图/CMS/FM/TopN。
pub fn updateIndexResult<B: AnalyzeIndexBackend>(
    backend: &B,
    response: AnalyzeIndexResponse,
    mut histogram: Histogram,
    mut cms: CMSketch,
    mut fm: FMSketch,
    mut topn: TopN,
    bucket_count: usize,
    topn_count: usize,
    stats_version: i32,
    need_cms: bool,
) -> Result<(Histogram, CMSketch, FMSketch, TopN), AnalyzeIndexError> {
    if stats_version != STATS_VERSION_2 {
        return Err(AnalyzeIndexError::InvalidStatsVersion(stats_version));
    }
    backend.update_job_progress(response.histogram.total_row_count());
    histogram = merge_histograms(histogram, response.histogram, bucket_count.max(1));
    if need_cms {
        if let Some(other_cms) = response.cms {
            cms.merge(&other_cms)?;
        }
        if let Some(other_topn) = response.topn {
            topn.merge(&other_topn, topn_count, &mut cms);
        }
    }
    if let Some(other_fm) = response.fm {
        fm.merge(&other_fm);
    }
    Ok((histogram, cms, fm, topn))
}

// 按响应顺序合并两直方图的累计计数，超桶时成对折叠。
/// 合并直方图并在超过 `max_buckets` 时两两折叠。
fn merge_histograms(mut left: Histogram, right: Histogram, max_buckets: usize) -> Histogram {
    if left.buckets.is_empty() {
        return right;
    }
    if right.buckets.is_empty() {
        return left;
    }
    let mut right = right;
    left.ndv = left.ndv.saturating_add(right.ndv);
    left.null_count = left.null_count.saturating_add(right.null_count);
    let mut offset = 0_i64;
    if left.buckets.last().map(|bucket| &bucket.upper)
        == right.buckets.first().map(|bucket| &bucket.lower)
    {
        left.ndv = left.ndv.saturating_sub(1);
        let first = right.buckets.remove(0);
        let last = left
            .buckets
            .last_mut()
            .expect("left histogram is not empty");
        last.upper = first.upper;
        last.count = last.count.saturating_add(first.count);
        last.repeats = first.repeats;
        offset = first.count;
    }
    while left.buckets.len() > max_buckets {
        merge_bucket_pairs(&mut left);
    }
    while right.buckets.len() > max_buckets {
        merge_bucket_pairs(&mut right);
    }
    if right.buckets.is_empty() {
        return left;
    }
    let left_count = left.buckets.last().map_or(0, |bucket| bucket.count);
    let right_count = right
        .buckets
        .last()
        .map_or(0, |bucket| bucket.count)
        .saturating_sub(offset);
    let mut left_average = left_count as f64 / left.buckets.len() as f64;
    let mut right_average = right_count as f64 / right.buckets.len() as f64;
    while left.buckets.len() > 1 && left_average * 2.0 <= right_average {
        merge_bucket_pairs(&mut left);
        left_average *= 2.0;
    }
    while right.buckets.len() > 1 && right_average * 2.0 <= left_average {
        merge_bucket_pairs(&mut right);
        right_average *= 2.0;
    }
    for mut bucket in right.buckets {
        bucket.count = bucket
            .count
            .saturating_add(left_count)
            .saturating_sub(offset);
        left.buckets.push(bucket);
    }
    while left.buckets.len() > max_buckets {
        merge_bucket_pairs(&mut left);
    }
    left
}
