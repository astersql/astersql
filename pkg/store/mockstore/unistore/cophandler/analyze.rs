// Copyright 2026 AsterSQL.
// Copyright 2019-present PingCAP, Inc.
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

// Coprocessor Analyze 统计收集（对应 Go analyze 路径）。
//
// 在 mock KV 上扫描指定范围，按索引/列/混合/全采样等类型构建直方图、
// CMS（Count-Min Sketch，近似频次）与 FM Sketch（近似基数），供优化器使用。

use crate::cop_handler::{CopError, Datum, KeyRange, KvReader, Row};
use std::collections::{BTreeMap, HashSet};

/// Analyze 请求类型：索引、聚簇主键、列、混合或全采样。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AnalyzeType {
    /// 索引列统计。
    Index,
    /// Common Handle（聚簇主键）前缀列统计。
    CommonHandle,
    /// 普通列统计。
    Columns,
    /// 列统计叠加索引 CMS/FM。
    Mixed,
    /// 全量或定长水库采样后再建 sketch。
    FullSampling,
}

/// Analyze 请求参数：类型、列偏移、桶数、采样与 sketch 尺寸等。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AnalyzeRequest {
    /// 统计类型。
    pub analyze_type: AnalyzeType,
    /// 参与统计的列偏移列表。
    pub column_offsets: Vec<usize>,
    /// 直方图桶数量上限。
    pub bucket_count: usize,
    /// 水库采样大小；全采样时 0 表示保留全部行。
    pub sample_size: usize,
    /// CMS 深度。
    pub sketch_depth: usize,
    /// CMS 宽度。
    pub sketch_width: usize,
    /// Common Handle 参与的主键列数。
    pub primary_column_count: usize,
}

/// 直方图单个桶：上下界、落入计数与上界重复次数。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HistogramBucket {
    /// 桶下界。
    pub lower: Datum,
    /// 桶上界。
    pub upper: Datum,
    /// 桶内累计行数。
    pub count: u64,
    /// 上界值的重复次数。
    pub repeats: u64,
}

/// 列/索引直方图：桶、NULL 数、总字节与基数。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Histogram {
    /// 有序桶列表。
    pub buckets: Vec<HistogramBucket>,
    /// NULL 值计数。
    pub null_count: u64,
    /// 非 NULL 值总字节估算。
    pub total_size: u64,
    /// 不同值个数（NDV）。
    pub distinct: u64,
}

/// Analyze 结果：行数、直方图、样本、CMS 与 FM Sketch。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AnalyzeResult {
    /// 扫描到的行数。
    pub row_count: u64,
    /// 各列/索引直方图。
    pub histograms: Vec<Histogram>,
    /// 采样行。
    pub samples: Vec<Row>,
    /// Count-Min Sketch 计数矩阵。
    pub cms: Vec<Vec<u64>>,
    /// FM Sketch 序列化字节。
    pub fms: Vec<u8>,
}

/// 按范围扫描后根据 `analyze_type` 分派，并编码为字节响应。
pub fn analyze(
    reader: &dyn KvReader,
    ranges: &[KeyRange],
    start_ts: u64,
    request: &AnalyzeRequest,
) -> Result<Vec<u8>, CopError> {
    // MVCC：仅可见 commit_ts <= start_ts 的版本（由 reader 保证）。
    let pairs = reader.scan(ranges, start_ts, false)?;
    let rows = pairs.into_iter().map(|pair| pair.value).collect::<Vec<_>>();
    let result = match request.analyze_type {
        AnalyzeType::Index => analyze_index(&rows, request)?,
        AnalyzeType::CommonHandle => analyze_common_handle(&rows, request)?,
        AnalyzeType::Columns => analyze_columns(&rows, request)?,
        AnalyzeType::Mixed => analyze_mixed(&rows, request)?,
        AnalyzeType::FullSampling => analyze_full_sampling(&rows, request)?,
    };
    Ok(encode_result(&result))
}

/// 将选定列编码为字节后建单列直方图与 CMS/FM。
pub fn analyze_index(rows: &[Row], request: &AnalyzeRequest) -> Result<AnalyzeResult, CopError> {
    let encoded_prefixes = rows
        .iter()
        .map(|row| encode_selected_prefixes(row, &request.column_offsets))
        .collect::<Result<Vec<_>, _>>()?;
    let encoded = encoded_prefixes
        .iter()
        .map(|prefixes| prefixes.last().cloned().unwrap_or_default())
        .collect::<Vec<_>>();
    let cms_values = encoded_prefixes.into_iter().flatten().collect::<Vec<_>>();
    let values = encoded
        .iter()
        .cloned()
        .map(Datum::Bytes)
        .collect::<Vec<_>>();
    Ok(AnalyzeResult {
        row_count: rows.len() as u64,
        histograms: vec![build_histogram(&values, request.bucket_count)],
        cms: build_cms(&cms_values, request.sketch_depth, request.sketch_width),
        fms: build_fm_sketch(&encoded),
        ..AnalyzeResult::default()
    })
}

/// Common Handle：截断到主键列数后复用索引分析。
pub fn analyze_common_handle(
    rows: &[Row],
    request: &AnalyzeRequest,
) -> Result<AnalyzeResult, CopError> {
    let count = request
        .primary_column_count
        .min(request.column_offsets.len());
    let mut primary_request = request.clone();
    primary_request.column_offsets.truncate(count);
    analyze_index(rows, &primary_request)
}

/// 按列分别建直方图，并做水库采样。
pub fn analyze_columns(rows: &[Row], request: &AnalyzeRequest) -> Result<AnalyzeResult, CopError> {
    let mut histograms = Vec::new();
    for offset in &request.column_offsets {
        let values = rows
            .iter()
            .map(|row| {
                row.get(*offset)
                    .cloned()
                    .ok_or(CopError::ColumnOffset(*offset))
            })
            .collect::<Result<Vec<_>, _>>()?;
        histograms.push(build_histogram(&values, request.bucket_count));
    }
    let samples = reservoir_sample(rows, request.sample_size);
    Ok(AnalyzeResult {
        row_count: rows.len() as u64,
        histograms,
        samples,
        ..AnalyzeResult::default()
    })
}

/// 混合：列统计结果上追加索引 CMS/FM 与索引直方图。
pub fn analyze_mixed(rows: &[Row], request: &AnalyzeRequest) -> Result<AnalyzeResult, CopError> {
    let mut columns = analyze_columns(rows, request)?;
    let index = analyze_index(rows, request)?;
    columns.cms = index.cms;
    columns.fms = index.fms;
    if let Some(histogram) = index.histograms.into_iter().next() {
        columns.histograms.push(histogram);
    }
    Ok(columns)
}

/// 全采样：先做列统计；sample_size==0 时保留全部行再建 sketch。
pub fn analyze_full_sampling(
    rows: &[Row],
    request: &AnalyzeRequest,
) -> Result<AnalyzeResult, CopError> {
    let mut result = analyze_columns(rows, request)?;
    // Full sampling keeps every row when sample_size is zero, like TiDB's
    // full-sampling request, otherwise it applies deterministic reservoir size.
    if request.sample_size == 0 {
        result.samples = rows.to_vec();
    }
    let encoded = result
        .samples
        .iter()
        .map(|row| encode_selected(row, &request.column_offsets))
        .collect::<Result<Vec<_>, _>>()?;
    result.cms = build_cms(&encoded, request.sketch_depth, request.sketch_width);
    result.fms = build_fm_sketch(&encoded);
    Ok(result)
}

/// 按值频次等深切分构建直方图（对齐常见 TiDB/TiKV 桶切分思路）。
pub fn build_histogram(values: &[Datum], bucket_count: usize) -> Histogram {
    let mut frequency = BTreeMap::<Datum, u64>::new();
    let mut null_count = 0;
    let mut total_size = 0;
    for value in values {
        if matches!(value, Datum::Null) {
            null_count += 1;
            continue;
        }
        total_size += datum_size(value) as u64;
        *frequency.entry(value.clone()).or_default() += 1;
    }
    let distinct = frequency.len() as u64;
    if frequency.is_empty() || bucket_count == 0 {
        return Histogram {
            null_count,
            total_size,
            distinct,
            buckets: Vec::new(),
        };
    }
    // 目标每桶约 (非空行数 / bucket_count) 行，按排序值累加切分。
    let target = ((values.len() - null_count as usize).max(1) + bucket_count - 1) / bucket_count;
    let mut buckets = Vec::new();
    let mut lower = None;
    let mut upper = None;
    let mut count = 0_u64;
    let mut bucket_count_so_far = 0_u64;
    let mut repeats = 0_u64;
    for (value, frequency) in frequency {
        lower.get_or_insert_with(|| value.clone());
        upper = Some(value);
        count += frequency;
        bucket_count_so_far += frequency;
        repeats = frequency;
        if bucket_count_so_far as usize >= target && buckets.len() + 1 < bucket_count {
            buckets.push(HistogramBucket {
                lower: lower.take().unwrap(),
                upper: upper.take().unwrap(),
                count,
                repeats,
            });
            bucket_count_so_far = 0;
        }
    }
    if let (Some(lower), Some(upper)) = (lower, upper) {
        buckets.push(HistogramBucket {
            lower,
            upper,
            count,
            repeats,
        });
    }
    Histogram {
        buckets,
        null_count,
        total_size,
        distinct,
    }
}

/// 确定性 xorshift 水库采样，保证跨语言可复现。
fn reservoir_sample(rows: &[Row], size: usize) -> Vec<Row> {
    if size == 0 {
        return Vec::new();
    }
    let mut sample = Vec::with_capacity(size.min(rows.len()));
    let mut random = 0x9e3779b97f4a7c15_u64;
    for (index, row) in rows.iter().enumerate() {
        if index < size {
            sample.push(row.clone());
            continue;
        }
        // xorshift64 生成伪随机下标，决定是否替换样本槽。
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        let position = random as usize % (index + 1);
        if position < size {
            sample[position] = row.clone();
        }
    }
    sample
}

/// 构建 Count-Min Sketch：每行用不同种子哈希到计数槽。
fn build_cms(values: &[Vec<u8>], depth: usize, width: usize) -> Vec<Vec<u64>> {
    if depth == 0 || width == 0 {
        return Vec::new();
    }
    let mut sketch = vec![vec![0_u64; width]; depth];
    for value in values {
        for (row, counters) in sketch.iter_mut().enumerate() {
            let hash = seeded_hash(value, row as u64 + 1);
            counters[hash as usize % width] += 1;
        }
    }
    sketch
}

/// 构建简化 FM Sketch：收集哈希后排序截断并序列化为大端字节。
fn build_fm_sketch(values: &[Vec<u8>]) -> Vec<u8> {
    let mut hashes = HashSet::new();
    for value in values {
        hashes.insert(seeded_hash(value, 0));
    }
    let mut result = hashes.into_iter().collect::<Vec<_>>();
    result.sort_unstable();
    result.truncate(1024);
    result.into_iter().flat_map(u64::to_be_bytes).collect()
}

/// 按列偏移将行编码为连续字节。
fn encode_selected(row: &[Datum], offsets: &[usize]) -> Result<Vec<u8>, CopError> {
    let mut encoded = Vec::new();
    for offset in offsets {
        row.get(*offset)
            .ok_or(CopError::ColumnOffset(*offset))?
            .encode(&mut encoded);
    }
    Ok(encoded)
}

/// 按 Go analyzeIndexProcessor.Process 的行为编码并保留每个索引列前缀。
fn encode_selected_prefixes(row: &[Datum], offsets: &[usize]) -> Result<Vec<Vec<u8>>, CopError> {
    let mut encoded = Vec::new();
    let mut prefixes = Vec::with_capacity(offsets.len());
    for offset in offsets {
        row.get(*offset)
            .ok_or(CopError::ColumnOffset(*offset))?
            .encode(&mut encoded);
        prefixes.push(encoded.clone());
    }
    Ok(prefixes)
}

/// 将 AnalyzeResult 编码为紧凑二进制（行数、直方图桶边界与计数）。
fn encode_result(result: &AnalyzeResult) -> Vec<u8> {
    let mut output = Vec::new();
    output.extend_from_slice(&result.row_count.to_be_bytes());
    output.extend_from_slice(&(result.histograms.len() as u32).to_be_bytes());
    for histogram in &result.histograms {
        output.extend_from_slice(&histogram.null_count.to_be_bytes());
        output.extend_from_slice(&histogram.distinct.to_be_bytes());
        output.extend_from_slice(&(histogram.buckets.len() as u32).to_be_bytes());
        for bucket in &histogram.buckets {
            bucket.lower.encode(&mut output);
            bucket.upper.encode(&mut output);
            output.extend_from_slice(&bucket.count.to_be_bytes());
            output.extend_from_slice(&bucket.repeats.to_be_bytes());
        }
    }
    output
}

/// Datum 占用字节估算（NULL 为 0，数值固定 8，Bytes 取长度）。
fn datum_size(value: &Datum) -> usize {
    match value {
        Datum::Null => 0,
        Datum::Int(_) | Datum::Uint(_) | Datum::Real(_) => 8,
        Datum::Bytes(value) => value.len(),
    }
}

/// FNV-1a 风格带种子哈希，供 CMS/FM 使用。
fn seeded_hash(value: &[u8], seed: u64) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64 ^ seed;
    for byte in value {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}
