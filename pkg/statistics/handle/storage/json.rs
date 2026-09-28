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

// 统计信息的 JSON 表示与分块持久化。
//
// 将 `TableStats` 编成可导入/导出的 `JsonTable`，再经 gzip + 定长分块写入
// `mysql.stats_history`；亦可从历史表按快照还原。直方图（histogram）桶、
// TopN、CMSketch、FM Sketch 均序列进二进制 payload。

use std::collections::HashMap;

use crate::{Bucket, ColumnStats, Error, Histogram, SqlStore, TableStats, TopNItem};

/// 谓词列使用信息：上次使用与上次 ANALYZE 时间。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PredicateColumn {
    /// 列 ID。
    pub id: i64,
    /// 上次出现在谓词中的时间。
    pub last_used_at: Option<String>,
    /// 上次 ANALYZE 该列的时间。
    pub last_analyzed_at: Option<String>,
}

/// 可导入/导出的整表统计 JSON 视图。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JsonTable {
    /// 库名。
    pub database_name: String,
    /// 表名。
    pub table_name: String,
    /// 表级统计主体。
    pub stats: TableStats,
    /// 谓词列使用列表。
    pub predicate_columns: Vec<PredicateColumn>,
    /// 是否来自历史统计表。
    pub is_historical_stats: bool,
}

/// 由内存 `TableStats` 与列使用 map 生成 `JsonTable`。
pub fn generate_json_table_from_stats(
    database_name: &str,
    table_name: &str,
    table: &TableStats,
    usage: &HashMap<i64, (Option<String>, Option<String>)>,
    mut check_cancelled: impl FnMut() -> Result<(), Error>,
) -> Result<JsonTable, Error> {
    // Go checks SQLKiller after every column and index conversion.
    // Go 在每个列/索引转换后检查 SQLKiller；此处同样按项回调取消检查。
    for _ in table.columns.values().chain(table.indices.values()) {
        check_cancelled()?;
    }
    Ok(JsonTable {
        database_name: database_name.into(),
        table_name: table_name.into(),
        stats: table.clone(),
        predicate_columns: {
            let mut predicate_columns = usage
                .iter()
                .map(|(id, (used, analyzed))| PredicateColumn {
                    id: *id,
                    last_used_at: used.clone(),
                    last_analyzed_at: analyzed.clone(),
                })
                .collect::<Vec<_>>();
            predicate_columns.sort_by_key(|column| column.id);
            predicate_columns
        },
        is_historical_stats: false,
    })
}

/// 从 `JsonTable` 还原 `TableStats`，并补齐旧版本缺失的 `stats_version`。
pub fn table_stats_from_json(physical_id: i64, json: &JsonTable) -> TableStats {
    let mut table = json.stats.clone();
    table.physical_id = physical_id;
    // v4.0 and older did not store StatsVer: data-bearing items become version 1.
    // v4.0 及更早不存 StatsVer：有 NDV/NULL 数据的项视为版本 1。
    for item in table.columns.values_mut().chain(table.indices.values_mut()) {
        if item.stats_version == 0 && (item.histogram.ndv > 0 || item.histogram.null_count > 0) {
            item.stats_version = 1;
        }
        table.stats_version = table.stats_version.max(item.stats_version);
    }
    table
}

/// 将 `JsonTable` 编码为 gzip 压缩后的定长字节块列表。
pub fn json_table_to_blocks(table: &JsonTable, block_size: usize) -> Result<Vec<Vec<u8>>, Error> {
    if block_size == 0 {
        return Err(Error("block size must be positive".into()));
    }
    let binary = encode_table(table);
    let json = format!("{{\"payload\":\"{}\"}}", hex(&binary));
    let compressed = gzip_store(json.as_bytes());
    Ok(compressed.chunks(block_size).map(<[u8]>::to_vec).collect())
}

/// 拼接分块并解压还原为 `JsonTable`。
pub fn blocks_to_json_table(blocks: &[Vec<u8>]) -> Result<JsonTable, Error> {
    if blocks.is_empty() {
        return Err(Error("Block empty error".into()));
    }
    let joined: Vec<u8> = blocks.iter().flatten().copied().collect();
    let json =
        String::from_utf8(gunzip_store(&joined)?).map_err(|error| Error(error.to_string()))?;
    let payload = json
        .strip_prefix("{\"payload\":\"")
        .and_then(|value| value.strip_suffix("\"}"))
        .ok_or_else(|| Error("invalid statistics JSON".into()))?;
    decode_table(&unhex(payload)?)
}

/// 按快照从 `stats_meta_history` / `stats_history` 还原历史 `JsonTable`。
pub fn table_historical_stats_to_json(
    store: &dyn SqlStore,
    physical_id: i64,
    snapshot: u64,
) -> Result<(JsonTable, bool), Error> {
    // 先找 <= snapshot 的最新 meta 版本，再取同版本 count，最后拼 history 分块。
    let meta = store.execute(&format!("select distinct version from mysql.stats_meta_history where table_id={physical_id} and version<={snapshot} order by version desc limit 1"))?;
    let Some(meta_row) = meta.first() else {
        return Ok((JsonTable::default(), false));
    };
    let meta_version = meta_row.uint(0);
    let counts = store.execute(&format!("select modify_count,count from mysql.stats_meta_history where table_id={physical_id} and version={meta_version}"))?;
    let history = store.execute(&format!("select distinct version from mysql.stats_history where table_id={physical_id} and version<={snapshot} order by version desc limit 1"))?;
    let Some(history_row) = history.first() else {
        return Ok((JsonTable::default(), false));
    };
    let rows = store.execute(&format!("select stats_data from mysql.stats_history where table_id={physical_id} and version={} order by seq_no", history_row.uint(0)))?;
    let mut table = blocks_to_json_table(&rows.iter().map(|row| row.bytes(0)).collect::<Vec<_>>())?;
    if let Some(row) = counts.first() {
        table.stats.modify_count = row.int(0);
        table.stats.count = row.int(1);
    }
    table.is_historical_stats = true;
    Ok((table, true))
}

/// 将 `JsonTable` 编码为紧凑小端二进制（再包进 hex JSON）。
fn encode_table(table: &JsonTable) -> Vec<u8> {
    let mut out = Vec::new();
    put_string(&mut out, &table.database_name);
    put_string(&mut out, &table.table_name);
    put_i64(&mut out, table.stats.physical_id);
    put_i64(&mut out, table.stats.count);
    put_i64(&mut out, table.stats.modify_count);
    put_u64(&mut out, table.stats.version);
    put_i64(&mut out, table.stats.stats_version);
    put_columns(&mut out, &table.stats.columns);
    put_columns(&mut out, &table.stats.indices);
    put_u64(&mut out, table.predicate_columns.len() as u64);
    for item in &table.predicate_columns {
        put_i64(&mut out, item.id);
        put_option_string(&mut out, &item.last_used_at);
        put_option_string(&mut out, &item.last_analyzed_at);
    }
    out.push(u8::from(table.is_historical_stats));
    out
}

/// 从二进制 payload 解码 `JsonTable`；要求精确消费全部字节。
fn decode_table(data: &[u8]) -> Result<JsonTable, Error> {
    let mut cursor = Cursor { data, at: 0 };
    let database_name = cursor.string()?;
    let table_name = cursor.string()?;
    let stats = TableStats {
        physical_id: cursor.i64()?,
        count: cursor.i64()?,
        modify_count: cursor.i64()?,
        version: cursor.u64()?,
        stats_version: cursor.i64()?,
        columns: cursor.columns()?,
        indices: cursor.columns()?,
    };
    let mut predicate_columns = Vec::new();
    for _ in 0..cursor.u64()? {
        predicate_columns.push(PredicateColumn {
            id: cursor.i64()?,
            last_used_at: cursor.option_string()?,
            last_analyzed_at: cursor.option_string()?,
        });
    }
    let is_historical_stats = match cursor.byte()? {
        0 => false,
        1 => true,
        _ => return Err(Error("invalid historical statistics flag".into())),
    };
    if cursor.at != data.len() {
        return Err(Error("statistics JSON payload has trailing bytes".into()));
    }
    Ok(JsonTable {
        database_name,
        table_name,
        stats,
        predicate_columns,
        is_historical_stats,
    })
}

/// 序列化列/索引 map：名称、直方图元数据、CMSketch、FM Sketch、TopN、桶。
fn put_columns(out: &mut Vec<u8>, columns: &HashMap<String, ColumnStats>) {
    put_u64(out, columns.len() as u64);
    // Go's encoding/json emits map keys in lexical order.  Sorting here gives
    // the same canonical byte representation and makes historical round trips
    // independent of HashMap's per-process random seed.
    let mut entries = columns.iter().collect::<Vec<_>>();
    entries.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
    for (name, item) in entries {
        put_string(out, name);
        put_i64(out, item.histogram.id);
        put_i64(out, item.histogram.ndv);
        put_i64(out, item.histogram.null_count);
        put_u64(out, item.histogram.last_update_version);
        put_i64(out, item.histogram.total_column_size);
        put_u64(out, item.histogram.correlation.to_bits());
        put_i64(out, item.stats_version);
        put_bytes(out, item.cmsketch.as_deref().unwrap_or_default());
        put_bytes(out, item.fm_sketch.as_deref().unwrap_or_default());
        put_u64(out, item.top_n.len() as u64);
        for top in &item.top_n {
            put_bytes(out, &top.encoded);
            put_u64(out, top.count);
        }
        put_u64(out, item.histogram.buckets.len() as u64);
        for bucket in &item.histogram.buckets {
            put_i64(out, bucket.count);
            put_i64(out, bucket.repeat);
            put_bytes(out, &bucket.lower);
            put_bytes(out, &bucket.upper);
            put_i64(out, bucket.ndv);
        }
    }
}
/// 写入小端 `u64`。
fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend(value.to_le_bytes());
}
/// 写入小端 `i64`（按位转 `u64`）。
fn put_i64(out: &mut Vec<u8>, value: i64) {
    put_u64(out, value as u64);
}
/// 写入长度前缀字节串。
fn put_bytes(out: &mut Vec<u8>, value: &[u8]) {
    put_u64(out, value.len() as u64);
    out.extend(value);
}
/// 写入 UTF-8 字符串。
fn put_string(out: &mut Vec<u8>, value: &str) {
    put_bytes(out, value.as_bytes());
}
/// 写入可选字符串：先 0/1 标记再跟内容。
fn put_option_string(out: &mut Vec<u8>, value: &Option<String>) {
    out.push(u8::from(value.is_some()));
    if let Some(value) = value {
        put_string(out, value);
    }
}

/// 二进制解码游标。
struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
}
impl Cursor<'_> {
    /// 读取一个字节。
    fn byte(&mut self) -> Result<u8, Error> {
        let value = *self
            .data
            .get(self.at)
            .ok_or_else(|| Error("truncated statistics JSON".into()))?;
        self.at += 1;
        Ok(value)
    }
    /// 读取小端 `u64`。
    fn u64(&mut self) -> Result<u64, Error> {
        let end = self
            .at
            .checked_add(8)
            .ok_or_else(|| Error("oversized statistics JSON".into()))?;
        let bytes: [u8; 8] = self
            .data
            .get(self.at..end)
            .ok_or_else(|| Error("truncated statistics JSON".into()))?
            .try_into()
            .unwrap();
        self.at = end;
        Ok(u64::from_le_bytes(bytes))
    }
    /// 读取小端 `i64`。
    fn i64(&mut self) -> Result<i64, Error> {
        Ok(self.u64()? as i64)
    }
    /// 读取长度前缀字节串。
    fn bytes(&mut self) -> Result<Vec<u8>, Error> {
        let len =
            usize::try_from(self.u64()?).map_err(|_| Error("oversized statistics JSON".into()))?;
        let end = self
            .at
            .checked_add(len)
            .ok_or_else(|| Error("oversized statistics JSON".into()))?;
        let value = self
            .data
            .get(self.at..end)
            .ok_or_else(|| Error("truncated statistics JSON".into()))?
            .to_vec();
        self.at = end;
        Ok(value)
    }
    /// 读取 UTF-8 字符串。
    fn string(&mut self) -> Result<String, Error> {
        String::from_utf8(self.bytes()?).map_err(|error| Error(error.to_string()))
    }
    /// 读取可选字符串。
    fn option_string(&mut self) -> Result<Option<String>, Error> {
        if self.byte()? == 0 {
            Ok(None)
        } else {
            self.string().map(Some)
        }
    }
    /// 解码列/索引 map（与 `put_columns` 对称）。
    fn columns(&mut self) -> Result<HashMap<String, ColumnStats>, Error> {
        let mut result = HashMap::new();
        for _ in 0..self.u64()? {
            let name = self.string()?;
            let mut histogram = Histogram {
                id: self.i64()?,
                ndv: self.i64()?,
                null_count: self.i64()?,
                last_update_version: self.u64()?,
                total_column_size: self.i64()?,
                correlation: f64::from_bits(self.u64()?),
                buckets: Vec::new(),
            };
            let stats_version = self.i64()?;
            let cms = self.bytes()?;
            let fm = self.bytes()?;
            let mut top_n = Vec::new();
            for _ in 0..self.u64()? {
                top_n.push(TopNItem {
                    encoded: self.bytes()?,
                    count: self.u64()?,
                });
            }
            for _ in 0..self.u64()? {
                histogram.buckets.push(Bucket {
                    count: self.i64()?,
                    repeat: self.i64()?,
                    lower: self.bytes()?,
                    upper: self.bytes()?,
                    ndv: self.i64()?,
                });
            }
            result.insert(
                name.clone(),
                ColumnStats {
                    name,
                    histogram,
                    cmsketch: (!cms.is_empty()).then_some(cms),
                    top_n,
                    fm_sketch: (!fm.is_empty()).then_some(fm),
                    stats_version,
                },
            );
        }
        Ok(result)
    }
}

// Emits a standards-compliant gzip member using uncompressed DEFLATE blocks.
// 用未压缩 DEFLATE 块发出符合标准的 gzip member（便于可控测试与精确长度）。
fn gzip_store(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x1f, 0x8b, 8, 0, 0, 0, 0, 0, 0, 255];
    for (index, chunk) in data.chunks(65_535).enumerate() {
        out.push(u8::from(index + 1 == data.chunks(65_535).len()));
        let len = chunk.len() as u16;
        out.extend(len.to_le_bytes());
        out.extend((!len).to_le_bytes());
        out.extend(chunk);
    }
    out.extend(crc32(data).to_le_bytes());
    out.extend((data.len() as u32).to_le_bytes());
    out
}
/// 解压本模块写出的未压缩 gzip member；拒绝尾随字节或校验失败。
fn gunzip_store(data: &[u8]) -> Result<Vec<u8>, Error> {
    if data.len() < 18 || data[..3] != [0x1f, 0x8b, 8] {
        return Err(Error("invalid gzip data".into()));
    }
    let mut at = 10;
    let mut out = Vec::new();
    loop {
        let header = *data.get(at).ok_or_else(|| Error("truncated gzip".into()))?;
        at += 1;
        if header & 0x06 != 0 {
            return Err(Error("unsupported compressed gzip block".into()));
        }
        let len = u16::from_le_bytes(
            data.get(at..at + 2)
                .ok_or_else(|| Error("truncated gzip".into()))?
                .try_into()
                .unwrap(),
        ) as usize;
        let nlen = u16::from_le_bytes(
            data.get(at + 2..at + 4)
                .ok_or_else(|| Error("truncated gzip".into()))?
                .try_into()
                .unwrap(),
        );
        if nlen != !(len as u16) {
            return Err(Error("corrupt gzip block".into()));
        }
        at += 4;
        out.extend(
            data.get(at..at + len)
                .ok_or_else(|| Error("truncated gzip".into()))?,
        );
        at += len;
        if header & 1 != 0 {
            break;
        }
    }
    if crc32(&out)
        != u32::from_le_bytes(
            data.get(at..at + 4)
                .ok_or_else(|| Error("truncated gzip trailer".into()))?
                .try_into()
                .unwrap(),
        )
    {
        return Err(Error("gzip checksum mismatch".into()));
    }
    let isize = u32::from_le_bytes(
        data.get(at + 4..at + 8)
            .ok_or_else(|| Error("truncated gzip trailer".into()))?
            .try_into()
            .unwrap(),
    );
    if isize != out.len() as u32 {
        return Err(Error("gzip size mismatch".into()));
    }
    if data.len() != at + 8 {
        return Err(Error("gzip member has trailing bytes".into()));
    }
    Ok(out)
}
/// IEEE CRC-32（gzip 尾部校验）。
fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for byte in data {
        crc ^= *byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}
/// 字节转小写十六进制。
fn hex(data: &[u8]) -> String {
    data.iter().map(|byte| format!("{byte:02x}")).collect()
}
/// 十六进制文本还原为字节。
fn unhex(value: &str) -> Result<Vec<u8>, Error> {
    if !value.len().is_multiple_of(2) {
        return Err(Error("invalid hex payload".into()));
    }
    (0..value.len())
        .step_by(2)
        .map(|at| {
            u8::from_str_radix(&value[at..at + 2], 16).map_err(|error| Error(error.to_string()))
        })
        .collect()
}
