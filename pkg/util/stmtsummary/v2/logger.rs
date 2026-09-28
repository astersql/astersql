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

// 语句摘要持久化日志编码器。
//
// 将 `StmtRecord` 序列化为 JSON 行写入 `tidb-statements.log` 一类文件；
// 支持附加字段（如 keyspace）、淘汰（evicted）标记，以及按时间窗口批量落盘。
// 淘汰记录供历史查询侧过滤“被 LRU 挤出的汇总行”。

#![allow(dead_code, non_camel_case_types, non_snake_case)]

use crate::StmtRecord;
use serde::Serialize;
use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, RwLock};

/// 全局附加字段：序列化时嵌套到 `additional_fields`。
static STMT_LOG_ADDITIONAL_FIELDS: LazyLock<RwLock<HashMap<String, String>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));
/// 已成功写入的淘汰记录条数，供测试观测。
static PERSISTED_EVICTED_COUNT: AtomicU64 = AtomicU64::new(0);

/// 设置语句日志的额外键值字段（如 keyspace_name）。
pub fn setStmtLogAdditionalFields(fields: HashMap<String, String>) {
    *STMT_LOG_ADDITIONAL_FIELDS
        .write()
        .expect("stmt log fields lock poisoned") = fields;
}

/// 返回累计已持久化的淘汰记录条数。
pub fn persistedEvictedCount() -> u64 {
    PERSISTED_EVICTED_COUNT.load(Ordering::Relaxed)
}

/// 普通记录 + `evicted: true` 的扁平 JSON 包装。
#[derive(Serialize)]
struct evictedStmtRecord<'a> {
    #[serde(flatten)]
    record: &'a StmtRecord,
    #[serde(rename = "evicted")]
    evicted: bool,
}

/// 普通记录 + `additional_fields` 嵌套对象。
#[derive(Serialize)]
struct stmtRecordWithAdditionalFields<'a> {
    #[serde(flatten)]
    record: &'a StmtRecord,
    #[serde(rename = "additional_fields")]
    additional_fields: &'a HashMap<String, String>,
}

/// 同时带附加字段与淘汰标记的序列化包装。
#[derive(Serialize)]
struct evictedStmtRecordWithAdditionalFields<'a> {
    #[serde(flatten)]
    record: &'a StmtRecord,
    #[serde(rename = "additional_fields")]
    additional_fields: &'a HashMap<String, String>,
    #[serde(rename = "evicted")]
    evicted: bool,
}

/// 将非淘汰语句记录序列化为 JSON 字节。
pub fn marshalStmtRecord(record: &StmtRecord) -> Result<Vec<u8>, serde_json::Error> {
    marshalStmtRecordWithEvicted(record, false)
}

/// 将淘汰语句记录序列化为带 `evicted` 标记的 JSON。
pub fn marshalEvictedStmtRecord(record: &StmtRecord) -> Result<Vec<u8>, serde_json::Error> {
    marshalStmtRecordWithEvicted(record, true)
}

/// 按是否淘汰、是否有附加字段四分支选择序列化结构。
pub fn marshalStmtRecordWithEvicted(
    record: &StmtRecord,
    evicted: bool,
) -> Result<Vec<u8>, serde_json::Error> {
    let fields = STMT_LOG_ADDITIONAL_FIELDS
        .read()
        .expect("stmt log fields lock poisoned");
    // 与 Go 侧一致：无附加字段时保持扁平；有字段时嵌套 additional_fields。
    match (fields.is_empty(), evicted) {
        (true, false) => serde_json::to_vec(record),
        (true, true) => serde_json::to_vec(&evictedStmtRecord {
            record,
            evicted: true,
        }),
        (false, false) => serde_json::to_vec(&stmtRecordWithAdditionalFields {
            record,
            additional_fields: &fields,
        }),
        (false, true) => serde_json::to_vec(&evictedStmtRecordWithAdditionalFields {
            record,
            additional_fields: &fields,
            evicted: true,
        }),
    }
}

/// The statement-summary encoder deliberately emits only the message and a
/// trailing newline, matching the Go zap encoder.
/// 语句摘要编码器只输出消息本身加换行，对齐 Go zap encoder。
pub fn encodeStmtLogEntry(message: &str) -> Vec<u8> {
    let mut result = Vec::with_capacity(message.len() + 1);
    result.extend_from_slice(message.as_bytes());
    result.push(b'\n');
    result
}

/// 日志落盘所需的时间窗口视图：起止时间、遍历记录、取出淘汰汇总行。
pub trait StmtWindowForLog {
    fn beginUnix(&self) -> i64;
    fn forEachRecordMut(&mut self, visitor: &mut dyn FnMut(&mut StmtRecord));
    fn evictedForPersistMut(&mut self) -> Option<&mut StmtRecord>;
}

/// 面向任意 `Write` 的语句摘要日志存储器。
pub struct StmtLogStorage<W: Write> {
    writer: W,
}

/// 用底层 writer 构造日志存储器。
pub fn newStmtLogStorage<W: Write>(writer: W) -> StmtLogStorage<W> {
    StmtLogStorage { writer }
}

impl<W: Write> StmtLogStorage<W> {
    /// 将窗口内所有记录（含有执行次数的淘汰行）写入日志，并填充 Begin/End。
    pub fn persist<T: StmtWindowForLog>(&mut self, window: &mut T, end: i64) -> io::Result<()> {
        let begin = window.beginUnix();
        let mut result = Ok(());
        window.forEachRecordMut(&mut |record| {
            if result.is_err() {
                return;
            }
            record.Begin = begin;
            record.End = end;
            result = self.log(record);
        });
        result?;
        // 淘汰汇总行仅在 ExecCount>0 时落盘，避免空占位。
        if let Some(record) = window.evictedForPersistMut()
            && record.ExecCount > 0
        {
            record.Begin = begin;
            record.End = end;
            self.log(record)?;
        }
        Ok(())
    }

    /// 刷新底层 writer。
    pub fn sync(&mut self) -> io::Result<()> {
        self.writer.flush()
    }

    /// 序列化单条记录并追加换行写入。
    pub fn log(&mut self, record: &StmtRecord) -> io::Result<()> {
        let encoded = marshalStmtRecord(record).map_err(io::Error::other)?;
        self.writer.write_all(&encoded)?;
        self.writer.write_all(b"\n")
    }

    /// 批量写入淘汰记录；序列化失败的条目跳过，成功条数计入全局计数。
    pub fn logEvicted<'a, I>(&mut self, records: I) -> io::Result<usize>
    where
        I: IntoIterator<Item = &'a StmtRecord>,
    {
        let mut builder = Vec::new();
        let mut persisted = 0_u64;
        for record in records {
            let Ok(encoded) = marshalEvictedStmtRecord(record) else {
                continue;
            };
            if !builder.is_empty() {
                builder.push(b'\n');
            }
            builder.extend_from_slice(&encoded);
            persisted += 1;
        }
        if builder.is_empty() {
            return Ok(0);
        }
        builder.push(b'\n');
        self.writer.write_all(&builder)?;
        PERSISTED_EVICTED_COUNT.fetch_add(persisted, Ordering::Relaxed);
        Ok(persisted as usize)
    }

    /// 取出内部 writer（测试用）。
    pub fn intoInner(self) -> W {
        self.writer
    }
}
