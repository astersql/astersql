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

// 运行时统计构建器：从解码后的 SQL 行构造直方图、TopN 与 FM Sketch。
//
// ANALYZE（分析统计信息）在会话侧读回样本时，需用与行编解码一致的宽松类型标志，
// 并按列/索引路径分别编码 Datum，最终产出与 Go 侧兼容的统计对象。

use std::cell::Cell;

use crate::{BuildHistAndTopN, EmptySampleItemSize, Histogram, SampleCollector, TopN};

/// RAII 守卫：在 Drop 时执行一次回调，用于释放已记账的内存跟踪额度。
struct DropGuard<F: FnOnce()> {
    callback: Option<F>,
}

impl<F: FnOnce()> DropGuard<F> {
    /// 包装将在析构时调用的回调。
    fn new(callback: F) -> Self {
        Self {
            callback: Some(callback),
        }
    }
}

impl<F: FnOnce()> Drop for DropGuard<F> {
    fn drop(&mut self) {
        if let Some(callback) = self.callback.take() {
            callback();
        }
    }
}

/// 估算采样收集器当前占用的内存（样本槽位、FM/CM Sketch、TopN 及 Datum 载荷）。
fn collector_memory(collector: &SampleCollector) -> i64 {
    collector.Samples.capacity() as i64 * EmptySampleItemSize
        + collector.FMSketch.MemoryUsage()
        + collector
            .Samples
            .iter()
            .map(|sample| {
                sample
                    .Value
                    .MemUsage()
                    .saturating_sub(types::EmptyDatumSize)
            })
            .sum::<i64>()
        + collector
            .CMSketch
            .as_ref()
            .map_or(0, |cmsketch| cmsketch.MemoryUsage())
        + collector.TopN.as_ref().map_or(0, |topn| topn.MemoryUsage())
}

/// 将可选的文本样本值按列类型转为 Datum；BIT 优先解析 `0x` 十六进制字面量。
fn datum(
    statement_context: &stmtctx::StatementContext,
    value: Option<&String>,
    field_type: &types::FieldType,
) -> Result<types::Datum, astersql_errors::SharedError> {
    match value {
        None => Ok(types::Datum::default()),
        Some(value) if field_type.GetType() == types::mysql::TypeBit => {
            // Session DML stores MysqlBit as `0x…` so ANALYZE can round-trip
            // without collapsing int `0` and string `"0"` into the same text.
            // 会话 DML 把 BIT 存成 `0x…`，ANALYZE 回读时按十六进制还原，避免与文本 `"0"` 混淆。
            if let Some(hex) = value
                .strip_prefix("0x")
                .or_else(|| value.strip_prefix("0X"))
            {
                if hex.len() % 2 == 0 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                    let mut bytes = Vec::with_capacity(hex.len() / 2);
                    for chunk in hex.as_bytes().chunks(2) {
                        let hi = (chunk[0] as char)
                            .to_digit(16)
                            .ok_or_else(|| astersql_errors::New("invalid BIT hex digit"))?;
                        let lo = (chunk[1] as char)
                            .to_digit(16)
                            .ok_or_else(|| astersql_errors::New("invalid BIT hex digit"))?;
                        bytes.push(((hi << 4) | lo) as u8);
                    }
                    return Ok(types::NewMysqlBitDatum(types::BinaryLiteral(bytes)));
                }
            }
            types::NewStringDatum(value.clone())
                .ConvertTo(statement_context.TypeCtx(), field_type)
                .map_err(|error| astersql_errors::New(error.to_string()))
        }
        Some(value) => types::NewStringDatum(value.clone())
            .ConvertTo(statement_context.TypeCtx(), field_type)
            .map_err(|error| astersql_errors::New(error.to_string())),
    }
}

/// 运行时统计构建器：持有一条语句上下文（含时区与内存跟踪），批量构建直方图等。
pub struct RuntimeStatsBuilder {
    statement_context: Box<stmtctx::StatementContext>,
}

/// Go decodes ANALYZE samples straight from the row codec, so the values never
/// pass a `sql_mode` gate a second time. Reading them back through the textual
/// SQL surface has to be just as permissive, otherwise a row that `sql_mode=''`
/// accepted at INSERT time would fail the analyze job.
///
/// 为已落盘样本设置宽松类型标志（忽略零日期等），与行编解码路径一致，避免二次 sql_mode 拦截。
fn stored_value_flags(flags: types::Flags) -> types::Flags {
    flags
        .WithIgnoreZeroDateErr(true)
        .WithIgnoreZeroInDate(true)
        .WithIgnoreInvalidDateErr(true)
}

impl Default for RuntimeStatsBuilder {
    /// 使用默认时区与宽松类型标志创建构建器，并初始化内存跟踪器。
    fn default() -> Self {
        let mut statement_context = stmtctx::NewStmtCtx();
        statement_context.SetTypeFlags(stored_value_flags(statement_context.TypeFlags()));
        statement_context.InitMemTracker(-1, -1);
        Self { statement_context }
    }
}

impl RuntimeStatsBuilder {
    /// Session location used to decode flattened statistics values.
    pub fn TimeZone(&self) -> chrono_tz::Tz {
        self.statement_context.TimeZone()
    }

    /// 按指定时区创建构建器。
    pub fn NewWithTimeZone(time_zone: chrono_tz::Tz) -> Self {
        let mut statement_context = stmtctx::NewStmtCtxWithTimeZone(time_zone);
        statement_context.SetTypeFlags(stored_value_flags(statement_context.TypeFlags()));
        statement_context.InitMemTracker(-1, -1);
        Self { statement_context }
    }

    /// 按时区名称解析并创建构建器；名称非法时返回错误。
    pub fn NewWithTimeZoneName(time_zone: &str) -> Result<Self, astersql_errors::SharedError> {
        time_zone
            .parse::<chrono_tz::Tz>()
            .map(Self::NewWithTimeZone)
            .map_err(|error| astersql_errors::New(format!("invalid statistics time zone: {error}")))
    }

    /// Builds Go-compatible histogram and TopN objects from decoded SQL rows.
    /// One statement context is retained for the complete ANALYZE table batch.
    ///
    /// 从解码后的 SQL 行构建与 Go 兼容的直方图与 TopN；整表 ANALYZE 批次共用同一语句上下文。
    pub fn build_histogram(
        &self,
        id: i64,
        field_types: &[types::FieldType],
        rows: &[Vec<Option<String>>],
        is_index: bool,
        topn: usize,
    ) -> Result<(Histogram, TopN), astersql_errors::SharedError> {
        self.build_histogram_with_buckets(
            id,
            field_types,
            rows,
            is_index,
            topn,
            crate::DefaultHistogramBuckets,
        )
    }

    /// 指定桶数构建直方图与 TopN；过程中对临时 Datum/编码/收集器增量记账，退出时统一释放。
    pub fn build_histogram_with_buckets(
        &self,
        id: i64,
        field_types: &[types::FieldType],
        rows: &[Vec<Option<String>>],
        is_index: bool,
        topn: usize,
        buckets: usize,
    ) -> Result<(Histogram, TopN), astersql_errors::SharedError> {
        let accounted = Cell::new(0_i64);
        let tracker = self.statement_context.MemTracker.as_deref();
        // 向内存跟踪器登记增量：正数 Consume，负数 Release。
        let account = |delta: i64| {
            let Some(tracker) = tracker else {
                return;
            };
            if delta >= 0 {
                tracker.Consume(delta);
            } else {
                tracker.Release(delta.saturating_neg());
            }
            accounted.set(accounted.get().saturating_add(delta));
        };
        // This owns every temporary allocation charged below. Unwinding through
        // datum conversion, key encoding, collection, or histogram construction
        // therefore releases the exact outstanding balance.
        // 守卫持有下方所有临时记账；无论正常返回或 panic/错误展开，都会释放未结余额。
        let _release = DropGuard::new(|| {
            if let Some(tracker) = tracker {
                tracker.Release(accounted.get());
                accounted.set(0);
            }
        });
        (|| {
            let field_type = field_types
                .first()
                .ok_or_else(|| astersql_errors::New("runtime statistics requires a field type"))?;
            let mut collector = SampleCollector::New(rows.len() as i64, crate::MaxSketchSize);
            collector.MemSize = collector_memory(&collector);
            account(collector.MemSize);
            let mut total_size = 0_i64;
            for (ordinal, row) in rows.iter().enumerate() {
                // 索引统计：各列 Datum 编码成完整索引键；列统计：只取首列。
                let value = if is_index {
                    if row.len() != field_types.len() {
                        return Err(astersql_errors::New(
                            "runtime index value and field type counts differ",
                        ));
                    }
                    let datums = row
                        .iter()
                        .zip(field_types)
                        .map(|(value, field_type)| {
                            datum(&self.statement_context, value.as_ref(), field_type)
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    // Go's row sampler accumulates the storage value size of
                    // every non-NULL index component (without each datum's
                    // flag byte), rather than the comparable index-key size.
                    for datum in datums.iter().filter(|datum| !datum.IsNull()) {
                        let encoded = codec::EncodeValue(
                            self.statement_context.TimeZone(),
                            Vec::new(),
                            vec![datum.clone()],
                        )?;
                        total_size += encoded.len().saturating_sub(1) as i64;
                    }
                    let datum_memory = datums.iter().map(types::Datum::MemUsage).sum::<i64>();
                    account(datum_memory);
                    let encoded =
                        codec::EncodeKey(self.statement_context.TimeZone(), Vec::new(), datums)?;
                    account(-datum_memory);
                    types::NewBytesDatum(encoded)
                } else {
                    let datum = datum(
                        &self.statement_context,
                        row.first().and_then(Option::as_ref),
                        field_type,
                    )?;
                    if !datum.IsNull() {
                        let encoded = codec::EncodeValue(
                            self.statement_context.TimeZone(),
                            Vec::new(),
                            vec![datum.clone()],
                        )?;
                        total_size += encoded.len().saturating_sub(1) as i64;
                    }
                    datum
                };
                let is_null = value.IsNull();
                let value_memory = value.MemUsage();
                account(value_memory);
                collector.Collect(&self.statement_context, value)?;
                // Column correlation uses the sample's position after rows
                // have been sorted by handle. NULL rows still occupy a
                // position even though they do not produce a SampleItem.
                if !is_index && !is_null {
                    if let Some(sample) = collector.Samples.last_mut() {
                        sample.Ordinal = ordinal as i32;
                    }
                }
                let previous = collector.MemSize;
                collector.MemSize = collector_memory(&collector);
                account(collector.MemSize.saturating_sub(previous));
                account(-value_memory);
            }
            // Go serializes and restores the collector before building stats;
            // restore drops samples longer than MaxSampleValueLength while
            // retaining the full TotalSize accumulated from all rows.
            // The Rust runtime path builds directly, so reproduce that boundary
            // here instead of allowing oversized JSON/text values into TopN or
            // histogram buckets.
            collector
                .Samples
                .retain(|item| item.Value.GetBytes().len() <= crate::MaxSampleValueLength);
            collector.TotalSize = total_size;
            let index_field_type;
            // 索引直方图边界按 Blob 类型处理；列直方图沿用原列类型。
            let histogram_field_type = if is_index {
                index_field_type = types::NewFieldType(types::mysql::TypeBlob);
                &index_field_type
            } else {
                field_type
            };
            let (histogram, topn) = BuildHistAndTopN(
                &self.statement_context,
                buckets.max(1),
                topn,
                id,
                &mut collector,
                histogram_field_type,
                !is_index,
            )?;
            account(histogram.MemoryUsage().saturating_add(topn.MemoryUsage()));
            Ok((histogram, topn))
        })()
    }

    /// Builds the FM sketch persisted beside a v2 histogram. Columns hash each
    /// datum directly; index statistics hash the complete encoded index row.
    ///
    /// 构建与 v2 直方图一并持久化的 FM Sketch；列直接哈希 Datum，索引哈希整行编码键。
    pub fn encode_fm_sketch(
        &self,
        field_types: &[types::FieldType],
        rows: &[Vec<Option<String>>],
        is_index: bool,
    ) -> Result<Vec<u8>, astersql_errors::SharedError> {
        let field_type = field_types
            .first()
            .ok_or_else(|| astersql_errors::New("runtime statistics requires a field type"))?;
        let mut sketch = crate::NewFMSketch(crate::MaxSketchSize);
        for row in rows {
            if is_index {
                if row.len() != field_types.len() {
                    return Err(astersql_errors::New(
                        "runtime index value and field type counts differ",
                    ));
                }
                let values = row
                    .iter()
                    .zip(field_types)
                    .map(|(value, field_type)| {
                        datum(&self.statement_context, value.as_ref(), field_type)
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                sketch.InsertRowValue(&self.statement_context, &values)?;
            } else {
                sketch.InsertValue(
                    &self.statement_context,
                    datum(
                        &self.statement_context,
                        row.first().and_then(Option::as_ref),
                        field_type,
                    )?,
                )?;
            }
        }
        crate::EncodeFMSketch(Some(&sketch))
    }

    /// 当前语句上下文内存跟踪器已消耗字节数。
    pub fn memory_consumed(&self) -> i64 {
        self.statement_context
            .MemTracker
            .as_ref()
            .map_or(0, |tracker| tracker.BytesConsumed())
    }

    /// 内存跟踪器历史峰值消耗。
    pub fn max_memory_consumed(&self) -> i64 {
        self.statement_context
            .MemTracker
            .as_ref()
            .map_or(0, |tracker| tracker.MaxConsumed())
    }

    /// 将直方图某一边界编码为落盘/展示用字节；索引直接取字节，列 BIT 转十进制 Blob，其余 EncodeKey。
    pub fn encode_histogram_bound(
        &self,
        histogram: &Histogram,
        bound_index: usize,
        is_index: bool,
    ) -> Result<Vec<u8>, astersql_errors::SharedError> {
        let bound = histogram
            .Bounds
            .get(bound_index)
            .ok_or_else(|| astersql_errors::New("histogram bound index out of range"))?;
        if is_index {
            Ok(bound.GetBytes())
        } else if bound.Kind() == types::KindMysqlBit {
            // Match Go convertBoundToBlob for BIT: format as the integer decimal
            // string ("0"/"48") so mysql.stats_buckets + HEX() match upstream.
            // 对齐 Go convertBoundToBlob：BIT 转为十进制字符串 Blob，便于 stats_buckets + HEX() 对照。
            let blob_type = types::NewFieldType(types::mysql::TypeBlob);
            bound
                .ConvertTo(self.statement_context.TypeCtx(), blob_type.as_ref())
                .map(|converted| converted.GetBytes())
                .map_err(|error| astersql_errors::New(error.to_string()))
        } else {
            // Existing column SHOW/decode paths still expect EncodeKey bytes for
            // non-BIT types; keep that shape until those consumers move to Go's
            // convertBoundFromBlob semantics.
            // 非 BIT 列仍用 EncodeKey，兼容现有 SHOW/解码路径。
            codec::EncodeKey(
                self.statement_context.TimeZone(),
                Vec::new(),
                vec![bound.clone()],
            )
        }
    }

    /// Decode a persisted histogram bound back to the canonical Datum used by
    /// partition histogram merging. Index bounds remain complete key bytes.
    pub fn decode_histogram_bound(
        &self,
        encoded: &[u8],
        is_index: bool,
    ) -> Result<types::Datum, astersql_errors::SharedError> {
        if is_index {
            Ok(types::NewBytesDatum(encoded.to_vec()))
        } else {
            codec::DecodeOne(encoded).map(|(_, datum)| datum)
        }
    }
}

/// 便捷入口：用默认构建器从行样本构建直方图与 TopN。
pub fn BuildRuntimeStatsHistogram(
    id: i64,
    field_types: &[types::FieldType],
    rows: &[Vec<Option<String>>],
    is_index: bool,
    topn: usize,
) -> Result<(Histogram, TopN), astersql_errors::SharedError> {
    RuntimeStatsBuilder::default().build_histogram(id, field_types, rows, is_index, topn)
}

/// 便捷入口：编码直方图边界字节。
pub fn EncodeRuntimeHistogramBound(
    histogram: &Histogram,
    bound_index: usize,
    is_index: bool,
) -> Result<Vec<u8>, astersql_errors::SharedError> {
    RuntimeStatsBuilder::default().encode_histogram_bound(histogram, bound_index, is_index)
}

/// 将编码键解码为展示字符串（无字段类型提示时走通用 Decode）。
pub fn DecodeRuntimeStatsValue(
    encoded: &[u8],
    expected_values: usize,
) -> Result<String, astersql_errors::SharedError> {
    DecodeRuntimeStatsValueWithTypes(encoded, expected_values, &[])
}

/// 按可选字段类型解码运行时统计键，并格式化为单值或 `(a, b, …)` 多值字符串。
pub fn DecodeRuntimeStatsValueWithTypes(
    encoded: &[u8],
    expected_values: usize,
    field_types: &[u8],
) -> Result<String, astersql_errors::SharedError> {
    // 无类型信息时用 Decode；有类型时用 DecodeRange 并要求字节消费完。
    let datums = if field_types.is_empty() {
        codec::Decode(encoded.to_vec(), expected_values)?
    } else {
        if field_types.len() != expected_values {
            return Err(astersql_errors::New(format!(
                "runtime statistics has {} field types, expected {expected_values}",
                field_types.len()
            )));
        }
        let (datums, remaining) = codec::DecodeRange(
            encoded.to_vec(),
            expected_values,
            Some(field_types.to_vec()),
            chrono_tz::UTC,
        )?;
        if !remaining.is_empty() {
            return Err(astersql_errors::New(
                "runtime statistics key was not fully decoded",
            ));
        }
        datums
    };
    if datums.len() != expected_values {
        return Err(astersql_errors::New(format!(
            "runtime statistics key decoded {} values, expected {expected_values}",
            datums.len()
        )));
    }
    let values = datums
        .iter()
        .enumerate()
        .map(|(_index, datum)| {
            if datum.IsNull() {
                Ok("NULL".to_owned())
            } else {
                datum
                    .ToString()
                    .map_err(|error| astersql_errors::New(error.to_string()))
            }
        })
        .collect::<Result<Vec<_>, _>>()?;
    if values.len() > 1 {
        Ok(format!("({})", values.join(", ")))
    } else {
        Ok(values.into_iter().next().unwrap_or_default())
    }
}
