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

// util 包常用杂项工具。
//
// 由 `util.go` 迁移。涵盖字节换算、集合转换、慢查询/通用日志字段生成、
// 非 ASCII 转义、带 IO 计数的 TCP 包装、按行读取、标识符校验、panic 恢复、
// protobuf 克隆以及 PD 地址同集群检测。

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::io::{self, BufRead, Read, Write};
use std::net::TcpStream;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Result, anyhow};
use prost::Message;
use task_sessmgr::ProcessInfo;

/// 一 GiB 的字节数（1024³）。
pub const ByteNumOneGiB: i64 = 1024 * 1024 * 1024;

/// 将字节数换算为 GiB（浮点）。
pub fn ByteToGiB(bytes: f64) -> f64 {
    bytes / ByteNumOneGiB as f64
}

/// 字符串切片转为 `map[string]struct{}` 风格的 HashMap（仅作集合）。
pub fn SliceToMap(slice: &[String]) -> HashMap<String, ()> {
    slice.iter().cloned().map(|value| (value, ())).collect()
}

/// 字符串切片装箱为 `[]interface{}` 风格的 `Vec<Box<dyn Any>>`。
pub fn StringsToInterfaces(strs: &[String]) -> Vec<Box<dyn Any + Send + Sync>> {
    strs.iter()
        .cloned()
        .map(|value| Box::new(value) as Box<dyn Any + Send + Sync>)
        .collect()
}

/// 逗号分隔字符串解析为 i64 集合；解析失败记为 0。
pub fn Str2Int64Map(input: &str) -> HashSet<i64> {
    input
        .split(',')
        .map(|value| value.parse::<i64>().unwrap_or(0))
        .collect()
}

/// 日志字段值的简易类型标记（对应 zap 常用类型）。
#[derive(Clone, Debug, PartialEq)]
pub enum LogValue {
    String(String),
    Unsigned(u64),
    Signed(i64),
}

/// 一条结构化日志字段。
#[derive(Clone, Debug, PartialEq)]
pub struct LogField {
    pub key: String,
    pub value: LogValue,
}

impl LogField {
    /// 构造字符串值字段。
    fn string(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: LogValue::String(value.into()),
        }
    }

    /// 构造无符号整数字段。
    fn unsigned(key: impl Into<String>, value: u64) -> Self {
        Self {
            key: key.into(),
            value: LogValue::Unsigned(value),
        }
    }
}

/// Drop 时 Decrease 引用计数，配对 TryIncrease。
struct ReferenceGuard(Arc<task_sessmgr::stmtctx::ReferenceCount>);

impl Drop for ReferenceGuard {
    fn drop(&mut self) {
        self.0.Decrease();
    }
}

/// 根据 ProcessInfo 生成慢查询/通用日志字段列表。
///
/// 含耗时、执行细节、统计信息、连接/用户/库表、内存峰值、规范化 SQL 等。
/// 若 StmtCtx 引用计数 TryIncrease 失败则返回空（会话已释放）。
pub fn GenLogFields(
    cost_time: Duration,
    info: &ProcessInfo,
    need_truncate_sql: bool,
) -> Vec<LogField> {
    // 持有 StmtCtx 引用，避免日志生成期间被并发释放。
    let _reference_guard = match &info.RefCountOfStmtCtx {
        Some(reference) if reference.TryIncrease() => Some(ReferenceGuard(Arc::clone(reference))),
        Some(_) => return Vec::new(),
        None => None,
    };

    let mut fields = Vec::with_capacity(20);
    fields.push(LogField::string(
        "cost_time",
        format!("{}s", cost_time.as_secs_f64()),
    ));

    // 语句执行细节（TiKV 耗时等）转为 zap 风格字符串字段。
    if let Some(statement_context) = info.StmtCtx.as_deref() {
        let details = statement_context.GetExecDetails();
        for field in details.ToZapFields() {
            let value = match field.value {
                task_sessmgr::execdetails::zap::Value::String(value) => value,
                task_sessmgr::execdetails::zap::Value::Int(value) => value.to_string(),
            };
            fields.push(LogField::string(field.key, value));
        }
    }

    // StatsInfo 回调返回表统计版本；0 表示 pseudo（无真实统计）。
    if let Some(stats_info) = info.StatsInfo {
        let empty_plan = ();
        let plan: &dyn Any = info
            .Plan
            .as_deref()
            .map(|plan| plan as &dyn Any)
            .unwrap_or(&empty_plan);
        let mut stats = stats_info(plan).into_iter().collect::<Vec<_>>();
        stats.sort_unstable_by(|left, right| left.0.cmp(&right.0));
        if !stats.is_empty() {
            fields.push(LogField::string(
                "stats",
                stats
                    .into_iter()
                    .map(|(key, value)| {
                        format!(
                            "{key}:{}",
                            if value == 0 {
                                "pseudo".to_owned()
                            } else {
                                value.to_string()
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(","),
            ));
        }
    }
    if info.ID != 0 {
        fields.push(LogField::unsigned("conn", info.ID));
    }
    if !info.User.is_empty() {
        fields.push(LogField::string("user", info.User.clone()));
    }
    if !info.DB.is_empty() {
        fields.push(LogField::string("database", info.DB.clone()));
    }
    if !info.TableIDs.is_empty() {
        fields.push(LogField::string(
            "table_ids",
            format!(
                "[{}]",
                info.TableIDs
                    .iter()
                    .map(i64::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        ));
    }
    if !info.IndexNames.is_empty() {
        fields.push(LogField::string(
            "index_names",
            format!("[{}]", info.IndexNames.join(",")),
        ));
    }
    fields.push(LogField::unsigned("txn_start_ts", info.CurTxnStartTS));
    if let Some(memory_tracker) = info.MemTracker.as_deref() {
        let maximum = memory_tracker.MaxConsumed();
        fields.push(LogField::string(
            "mem_max",
            format!(
                "{} Bytes ({})",
                maximum,
                memory_tracker.FormatBytes(maximum)
            ),
        ));
    }
    // task-344 exposes the non-arbitrator build used by task-507. In that
    // build MemArbitration and WaitArbitrate have Go's zero-value result, so
    // the conditional `mem_arbitration` field is intentionally absent.

    // 规范化 SQL（脱敏），过长且允许截断时保留前缀并附原长度。
    let mut sql = if info.Info.is_empty() {
        String::new()
    } else {
        task_parser::digester_impl::Normalize(&info.Info, &info.RedactSQL)
    };
    const LOG_SQL_LEN: usize = 8 * 1024;
    if sql.len() > LOG_SQL_LEN && need_truncate_sql {
        let original_len = sql.len();
        // 按 char_indices 找不超过 LOG_SQL_LEN 的 UTF-8 安全切点。
        let boundary = sql
            .char_indices()
            .map(|(index, _)| index)
            .take_while(|index| *index <= LOG_SQL_LEN)
            .last()
            .unwrap_or(0);
        sql = format!("{} len({original_len})", &sql[..boundary]);
    }
    fields.push(LogField::string("sql", sql));
    fields.push(LogField::string("session_alias", info.SessionAlias.clone()));
    let affected_rows = info
        .StmtCtx
        .as_deref()
        .map(|statement_context| statement_context.AffectedRows())
        .unwrap_or(0);
    fields.push(LogField::unsigned("affected rows", affected_rows));
    fields
}

/// 判断字节是否为可打印 ASCII（空格到 `~`）。
pub fn PrintableASCII(byte: u8) -> bool {
    (32..127).contains(&byte)
}

/// 将非 ASCII 字节转成 `\xHH` 形式，可限制展示长度；可选是否显示 DEL(0x7f)。
pub fn FmtNonASCIIPrintableCharToHex(
    input: &str,
    max_bytes_to_show: usize,
    display_delete_character: bool,
) -> String {
    let mut output = String::with_capacity(max_bytes_to_show * 2);
    for (index, byte) in input.bytes().enumerate() {
        if index >= max_bytes_to_show {
            output.push_str("...");
            break;
        }
        if PrintableASCII(byte) {
            output.push(byte as char);
        } else if byte != 0x7f || display_delete_character {
            output.push_str(&format!("\\x{byte:02X}"));
        }
    }
    output
}

/// 包装 TcpStream，累计读写字节数到原子计数器。
pub struct TCPConnWithIOCounter {
    connection: TcpStream,
    counter: Arc<AtomicU64>,
}

/// 构造带 IO 计数的 TCP 连接包装。
pub fn NewTCPConnWithIOCounter(
    connection: TcpStream,
    counter: Arc<AtomicU64>,
) -> TCPConnWithIOCounter {
    TCPConnWithIOCounter {
        connection,
        counter,
    }
}

impl Read for TCPConnWithIOCounter {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let count = self.connection.read(buffer)?;
        self.counter.fetch_add(count as u64, Ordering::Relaxed);
        Ok(count)
    }
}

impl Write for TCPConnWithIOCounter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let count = self.connection.write(buffer)?;
        self.counter.fetch_add(count as u64, Ordering::Relaxed);
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.connection.flush()
    }
}

/// 读一行（去 `\n`/`\r\n`），超过 `max_line_size` 报错；EOF 返回 UnexpectedEof。
pub fn ReadLine<R: BufRead>(reader: &mut R, max_line_size: usize) -> Result<Vec<u8>> {
    let mut line = Vec::new();
    let mut fragments = 0;
    loop {
        let (consumed, found_newline) = {
            let available = reader.fill_buf()?;
            if available.is_empty() {
                if line.is_empty() {
                    return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into());
                }
                return Ok(line);
            }

            let consumed = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(available.len(), |position| position + 1);
            line.extend_from_slice(&available[..consumed]);
            (consumed, available[consumed - 1] == b'\n')
        };
        reader.consume(consumed);
        fragments += 1;

        if found_newline {
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
        }
        // Go checks the limit only after ReadLine reports an initial prefix and
        // another fragment has been appended. Preserve that observable quirk.
        if fragments > 1 && line.len() > max_line_size {
            return Err(anyhow!("single line length exceeds limit: {max_line_size}"));
        }
        if found_newline {
            return Ok(line);
        }
    }
}

/// 连续读取至多 `count` 行；中途 UnexpectedEof 且已有行时返回已读内容。
pub fn ReadLines<R: BufRead>(
    reader: &mut R,
    count: usize,
    max_line_size: usize,
) -> Result<Vec<Vec<u8>>> {
    let mut lines = Vec::with_capacity(count);
    for _ in 0..count {
        match ReadLine(reader, max_line_size) {
            Ok(line) => lines.push(line),
            Err(error)
                if !lines.is_empty()
                    && error
                        .downcast_ref::<io::Error>()
                        .is_some_and(|error| error.kind() == io::ErrorKind::UnexpectedEof) =>
            {
                return Ok(lines);
            }
            Err(error) => return Err(error),
        }
    }
    Ok(lines)
}

/// 标识符是否非法：空串或以空格结尾（MySQL 标识符规则相关）。
pub fn IsInCorrectIdentifierName(name: &str) -> bool {
    name.is_empty() || name.as_bytes().last() == Some(&b' ')
}

/// 将 panic 载荷转为 anyhow::Error，并 dump 飞行记录器。
pub fn GetRecoverError(payload: &(dyn Any + Send)) -> anyhow::Error {
    task_traceevent::traceevent::DumpFlightRecorderToLogger("GetRecoverError");
    if let Some(message) = payload.downcast_ref::<&str>() {
        anyhow!(*message)
    } else if let Some(message) = payload.downcast_ref::<String>() {
        anyhow!(message.clone())
    } else {
        anyhow!("panic with non-string payload")
    }
}

/// 通过 encode/decode 深拷贝 protobuf 消息（对应 Go proto.Clone）。
pub fn ProtoV1Clone<T>(message: &T) -> Result<T>
where
    T: Message + Default,
{
    Ok(T::decode(message.encode_to_vec().as_slice())?)
}

/// 比较两组 PD 地址是否有交集，判断是否同一集群。
pub fn CheckIfSameCluster<C, F1, F2>(
    context: C,
    first_getter: F1,
    second_getter: F2,
) -> Result<(bool, Vec<String>, Vec<String>)>
where
    C: Clone,
    F1: FnOnce(C) -> Result<Vec<String>>,
    F2: FnOnce(C) -> Result<Vec<String>>,
{
    let first = first_getter(context.clone())?;
    let first_set = first.iter().cloned().collect::<HashSet<_>>();
    let second = second_getter(context)?;
    // 任一第二组地址落在第一组集合中即视为同集群。
    let same = second.iter().any(|address| first_set.contains(address));
    Ok((same, first, second))
}

/// 查询结果游标：逐行取 PD 地址。
pub trait PdAddressRows {
    fn next_address(&mut self) -> Result<Option<String>>;
}

/// 可执行查询并返回 PD 地址行集的数据库抽象。
pub trait PdAddressDatabase<C> {
    type Rows: PdAddressRows;

    fn query_context(&self, context: C, query: &str) -> Result<Self::Rows>;
}

/// 返回闭包：从 INFORMATION_SCHEMA.CLUSTER_INFO 查询 PD 的 STATUS_ADDRESS（无 scheme）。
pub fn GetPDsAddrWithoutScheme<'a, C, D>(database: &'a D) -> impl Fn(C) -> Result<Vec<String>> + 'a
where
    D: PdAddressDatabase<C> + 'a,
{
    move |context| {
        let mut rows = database.query_context(
            context,
            "SELECT STATUS_ADDRESS FROM INFORMATION_SCHEMA.CLUSTER_INFO WHERE TYPE = 'pd'",
        )?;
        let mut addresses = Vec::new();
        while let Some(address) = rows.next_address()? {
            addresses.push(address);
        }
        Ok(addresses)
    }
}
