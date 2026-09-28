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

// mock Coprocessor ANALYZE 请求处理：收集列/索引直方图与采样。
//
// ANALYZE 为优化器统计表数据分布；本 mock 扫描 KV 后构建直方图桶、NDV、
// 蓄水池采样（reservoir sampling），并编码为响应字节。

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::copr_handler::{
    AnalyzeRequest, AnalyzeType, CopError, Datum, KeyRange, KvReader, Request, RequestPayload,
    Response, Row, coprHandler,
};

/// 直方图单个桶：下界、上界、累计行数与上界重复次数。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HistogramBucket {
    pub lower: Datum,
    pub upper: Datum,
    pub count: u64,
    pub repeats: u64,
}

/// 单列（或索引）ANALYZE 结果：直方图、NULL 数、样本与 distinct 估计。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ColumnAnalyzeResult {
    pub histogram: Vec<HistogramBucket>,
    pub null_count: u64,
    pub samples: Vec<Datum>,
    pub distinct_count: u64,
}

/// 一次 ANALYZE 响应：多列结果，可选附带索引统计。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AnalyzeResult {
    pub columns: Vec<ColumnAnalyzeResult>,
    pub index: Option<ColumnAnalyzeResult>,
}

impl AnalyzeResult {
    /// 将分析结果编码为 mock 协议字节流（大端长度前缀 + 字段）。
    pub fn encode(&self) -> Vec<u8> {
        let mut output = Vec::new();
        output.extend_from_slice(&(self.columns.len() as u32).to_be_bytes());
        for column in &self.columns {
            encode_column(column, &mut output);
        }
        output.push(u8::from(self.index.is_some()));
        if let Some(index) = &self.index {
            encode_column(index, &mut output);
        }
        output
    }
}

/// 编码单列分析结果到输出缓冲。
fn encode_column(column: &ColumnAnalyzeResult, output: &mut Vec<u8>) {
    output.extend_from_slice(&column.null_count.to_be_bytes());
    output.extend_from_slice(&column.distinct_count.to_be_bytes());
    output.extend_from_slice(&(column.histogram.len() as u32).to_be_bytes());
    for bucket in &column.histogram {
        bucket.lower.encode(output);
        bucket.upper.encode(output);
        output.extend_from_slice(&bucket.count.to_be_bytes());
        output.extend_from_slice(&bucket.repeats.to_be_bytes());
    }
    output.extend_from_slice(&(column.samples.len() as u32).to_be_bytes());
    for sample in &column.samples {
        sample.encode(output);
    }
}

/// 对一组值排序后构建直方图、统计 NDV，并做蓄水池采样。
fn analyze_values(
    mut values: Vec<Datum>,
    bucket_size: usize,
    sample_size: usize,
) -> ColumnAnalyzeResult {
    let null_count = values
        .iter()
        .filter(|value| matches!(value, Datum::Null))
        .count() as u64;
    values.retain(|value| !matches!(value, Datum::Null));
    values.sort();
    // windows(2) 统计相邻不等次数，再加是否非空，得到 distinct 估计。
    let distinct_count = values.windows(2).filter(|pair| pair[0] != pair[1]).count() as u64
        + u64::from(!values.is_empty());
    let samples = reservoir_sample(&values, sample_size);
    let mut histogram = Vec::new();
    if !values.is_empty() {
        let bucket_size = bucket_size.max(1);
        // 按目标桶数均分行，构造累计直方图。
        let rows_per_bucket = values.len().div_ceil(bucket_size).max(1);
        let mut cumulative = 0_u64;
        for values in values.chunks(rows_per_bucket) {
            cumulative += values.len() as u64;
            let upper = values
                .last()
                .cloned()
                .expect("histogram chunk is non-empty");
            let repeats = values
                .iter()
                .rev()
                .take_while(|value| **value == upper)
                .count() as u64;
            histogram.push(HistogramBucket {
                lower: values[0].clone(),
                upper,
                count: cumulative,
                repeats,
            });
        }
    }
    ColumnAnalyzeResult {
        histogram,
        null_count,
        samples,
        distinct_count,
    }
}

/// Deterministic reservoir sampling keeps tests reproducible while preserving
/// Go's uniform replacement rule.
///
/// 确定性蓄水池采样：固定种子伪随机，保证测试可复现，并保留 Go 的均匀替换规则。
fn reservoir_sample(values: &[Datum], sample_size: usize) -> Vec<Datum> {
    if sample_size == 0 {
        return Vec::new();
    }
    let mut samples = Vec::with_capacity(sample_size.min(values.len()));
    let mut state = 0x9e3779b97f4a7c15_u64;
    for (index, value) in values.iter().enumerate() {
        if samples.len() < sample_size {
            samples.push(value.clone());
            continue;
        }
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let replacement = (state as usize) % (index + 1);
        if replacement < sample_size {
            samples[replacement] = value.clone();
        }
    }
    samples
}

impl coprHandler {
    /// 分发 ANALYZE 请求到索引或列路径，编码成功结果或错误字符串。
    pub fn handleCopAnalyzeRequest(&self, request: &Request) -> Response {
        if request.ranges.is_empty() {
            return Response::default();
        }
        let RequestPayload::Analyze(analyze_request) = &request.payload else {
            return Response::default();
        };
        let result = match analyze_request.analyze_type {
            AnalyzeType::Index => self.handleAnalyzeIndexReq(request, analyze_request),
            AnalyzeType::Columns => self.handleAnalyzeColumnsReq(request, analyze_request),
        };
        match result {
            Ok(result) => Response {
                data: result.encode(),
                ..Response::default()
            },
            Err(error) => Response {
                other_error: Some(error.to_string()),
                ..Response::default()
            },
        }
    }

    /// 扫描索引键范围，将整行编码为 Bytes 后做单列式直方图统计。
    pub fn handleAnalyzeIndexReq(
        &self,
        request: &Request,
        analyze_request: &AnalyzeRequest,
    ) -> Result<AnalyzeResult, CopError> {
        let rows = self.reader.scan(&request.ranges, request.start_ts, false)?;
        let mut values = Vec::with_capacity(rows.len());
        for pair in rows {
            let mut encoded = Vec::new();
            for datum in pair.value {
                datum.encode(&mut encoded);
            }
            values.push(Datum::Bytes(encoded));
        }
        Ok(AnalyzeResult {
            columns: Vec::new(),
            index: Some(analyze_values(
                values,
                analyze_request.bucket_size,
                analyze_request.sample_size,
            )),
        })
    }

    /// 按列偏移拆分行，分别对每列做直方图与采样。
    pub fn handleAnalyzeColumnsReq(
        &self,
        request: &Request,
        analyze_request: &AnalyzeRequest,
    ) -> Result<AnalyzeResult, CopError> {
        let rows = self.reader.scan(&request.ranges, request.start_ts, false)?;
        let width = rows.iter().map(|pair| pair.value.len()).max().unwrap_or(0);
        let columns = (0..width)
            .map(|offset| {
                let values = rows
                    .iter()
                    .map(|pair| pair.value.get(offset).cloned().unwrap_or(Datum::Null))
                    .collect();
                analyze_values(
                    values,
                    analyze_request.bucket_size,
                    analyze_request.sample_size,
                )
            })
            .collect();
        Ok(AnalyzeResult {
            columns,
            index: None,
        })
    }
}

/// 列 ANALYZE 用的简易扫描执行器：惰性加载范围内行并按游标吐出。
pub struct analyzeColumnsExec {
    pub reader: Arc<dyn KvReader>,
    pub ranges: Vec<KeyRange>,
    pub start_ts: u64,
    rows: Vec<Row>,
    cursor: usize,
    loaded: bool,
}

impl analyzeColumnsExec {
    /// 构造尚未扫描的列 ANALYZE 执行器。
    pub fn new(reader: Arc<dyn KvReader>, ranges: Vec<KeyRange>, start_ts: u64) -> Self {
        Self {
            reader,
            ranges,
            start_ts,
            rows: Vec::new(),
            cursor: 0,
            loaded: false,
        }
    }
    /// 返回首行列数（字段宽度）；尚未加载时为 0。
    pub fn Fields(&self) -> usize {
        self.rows.first().map(Vec::len).unwrap_or(0)
    }
    /// 取下一行；首次调用时按 start_ts（MVCC 读时间戳）扫描并缓存全部行。
    pub fn getNext(&mut self) -> Result<Option<Row>, CopError> {
        if !self.loaded {
            self.rows = self
                .reader
                .scan(&self.ranges, self.start_ts, false)?
                .into_iter()
                .map(|pair| pair.value)
                .collect();
            self.loaded = true;
        }
        let row = self.rows.get(self.cursor).cloned();
        if row.is_some() {
            self.cursor += 1;
        }
        Ok(row)
    }
    /// 重置 chunk 后追加至多一行。
    pub fn Next(&mut self, chunk: &mut Vec<Row>) -> Result<(), CopError> {
        chunk.clear();
        if let Some(row) = self.getNext()? {
            chunk.push(row);
        }
        Ok(())
    }
    /// 分配空 chunk 容器。
    pub fn NewChunk(&self) -> Vec<Row> {
        Vec::new()
    }
    /// Go RecordSet 的 Close 在此执行器上无资源需要释放。
    pub fn Close(&mut self) {}
}
