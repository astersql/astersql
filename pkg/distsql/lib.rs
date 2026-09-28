// Copyright 2026 AsterSQL.

// DistSQL（分布式 SQL）crate 入口。
//
// 本 crate 提供把查询下推到 TiKV/TiFlash 的基础类型与简化实现：
// - 请求构造（`RequestBuilder` / `KeyRange`）；
// - 选择结果迭代（`SelectResult` / `ResponseSource`）；
// - 以及与 Go 侧对齐的子模块（`distsql`、`request_builder`、`select_result`）。
//
// 此处的 `RequestBuilder` 是包级简化版本，供单元测试与上层快速拼装请求；
// 更完整的 KV 请求构造见 `request_builder` 模块。

#![allow(
    dead_code,
    non_camel_case_types,
    non_snake_case,
    non_upper_case_globals
)]

/// DistSQL 发送/编码/TiFlash 元数据等核心 API。
pub mod distsql;
/// 完整版 KV 请求构造器与表/索引 range 编码。
pub mod request_builder;
/// SelectResult 迭代器与响应源抽象。
pub mod select_result;

use std::collections::VecDeque;
use std::fmt;
use std::time::Duration;

/// DistSQL 层错误：以可读消息字符串承载，便于测试与日志。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DistSqlError(pub String);

impl fmt::Display for DistSqlError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}
impl std::error::Error for DistSqlError {}

/// DistSQL 操作的统一 Result 别名。
pub type DistSqlResult<T> = Result<T, DistSqlError>;

/// Coprocessor 请求类型：DAG（执行计划下推）、Analyze（统计信息）、Checksum（校验和）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequestType {
    /// 下推 DAG（Directed Acyclic Graph，算子图）到存储节点执行。
    Dag,
    /// 收集列统计信息（ANALYZE）。
    Analyze,
    /// 对表数据做校验和。
    Checksum,
}

/// 目标存储引擎类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreType {
    /// 行存 TiKV。
    TiKv,
    /// 列存加速引擎 TiFlash。
    TiFlash,
}

/// 半开 key 区间 `[start, end)`，用于描述扫描范围。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyRange {
    /// 区间起始 key（含）。
    pub start: Vec<u8>,
    /// 区间结束 key（不含）。
    pub end: Vec<u8>,
}

impl KeyRange {
    /// 构造 key range；要求 `start < end`，否则返回错误。
    pub fn new(start: impl Into<Vec<u8>>, end: impl Into<Vec<u8>>) -> DistSqlResult<Self> {
        let range = Self {
            start: start.into(),
            end: end.into(),
        };
        if range.start >= range.end {
            return Err(DistSqlError("invalid key range".into()));
        }
        Ok(range)
    }
}

/// 一次 DistSQL KV 请求的描述（类型、范围、并发、有序性、存储等）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Request {
    /// 请求类型（DAG / Analyze / Checksum）。
    pub request_type: RequestType,
    /// 待扫描的 key 区间列表（已合并）。
    pub key_ranges: Vec<KeyRange>,
    /// Coprocessor 并发度（并行 Region 任务数上限）。
    pub concurrency: usize,
    /// 是否保持返回行顺序（有序扫描）。
    pub keep_order: bool,
    /// 是否降序扫描；要求同时开启 `keep_order`。
    pub descending: bool,
    /// 是否使用流式响应。
    pub streaming: bool,
    /// 目标存储类型。
    pub store_type: StoreType,
    /// 事务开始时间戳（start_ts，MVCC 可见性基准）。
    pub start_ts: u64,
    /// 请求超时。
    pub timeout: Duration,
}

/// 简化版请求构造器：链式设置字段后 `build` 出不可变 `Request`。
#[derive(Clone, Debug)]
pub struct RequestBuilder {
    request: Request,
}

impl RequestBuilder {
    /// 按请求类型创建构造器，并填入 DistSQL 会话侧默认值。
    pub fn new(request_type: RequestType) -> Self {
        Self {
            request: Request {
                request_type,
                key_ranges: Vec::new(),
                concurrency: 1,
                keep_order: false,
                descending: false,
                streaming: false,
                store_type: StoreType::TiKv,
                start_ts: 0,
                timeout: Duration::from_secs(60),
            },
        }
    }

    /// 设置 key ranges：先按 start 排序，再合并重叠/相邻区间。
    pub fn key_ranges(mut self, mut ranges: Vec<KeyRange>) -> Self {
        ranges.sort_by(|left, right| left.start.cmp(&right.start));
        let mut merged: Vec<KeyRange> = Vec::with_capacity(ranges.len());
        for range in ranges {
            // 若当前区间与上一段重叠或相连，则扩展上一段的 end。
            if let Some(previous) = merged.last_mut() {
                if range.start <= previous.end {
                    if range.end > previous.end {
                        previous.end = range.end;
                    }
                    continue;
                }
            }
            merged.push(range);
        }
        self.request.key_ranges = merged;
        self
    }

    /// 设置 Coprocessor 并发度。
    pub fn concurrency(mut self, concurrency: usize) -> Self {
        self.request.concurrency = concurrency;
        self
    }
    /// 设置是否保持行序。
    pub fn keep_order(mut self, keep: bool) -> Self {
        self.request.keep_order = keep;
        self
    }
    /// 设置是否降序扫描。
    pub fn descending(mut self, descending: bool) -> Self {
        self.request.descending = descending;
        self
    }
    /// 设置是否流式返回。
    pub fn streaming(mut self, streaming: bool) -> Self {
        self.request.streaming = streaming;
        self
    }
    /// 设置目标存储类型（TiKV / TiFlash）。
    pub fn store_type(mut self, store_type: StoreType) -> Self {
        self.request.store_type = store_type;
        self
    }
    /// 设置事务 start_ts。
    pub fn start_ts(mut self, timestamp: u64) -> Self {
        self.request.start_ts = timestamp;
        self
    }
    /// 设置请求超时。
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.request.timeout = timeout;
        self
    }

    /// 校验并产出最终 `Request`：至少一段 range、正并发，降序须 keep_order。
    pub fn build(self) -> DistSqlResult<Request> {
        if self.request.key_ranges.is_empty() {
            return Err(DistSqlError("request has no key ranges".into()));
        }
        if self.request.concurrency == 0 {
            return Err(DistSqlError("concurrency must be positive".into()));
        }
        if self.request.descending && !self.request.keep_order {
            return Err(DistSqlError("descending scan requires keep_order".into()));
        }
        Ok(self.request)
    }
}

/// Select 结果累计统计：响应批次数、行数、告警数、扫描 key 数。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SelectStats {
    /// 已拉取的响应批次数。
    pub response_count: usize,
    /// 已展开的行数。
    pub row_count: usize,
    /// 累计告警条数。
    pub warning_count: usize,
    /// 累计扫描的 key 数。
    pub scanned_keys: u64,
}

/// 单批 Select 响应：行、告警、扫描统计与可选错误。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SelectResponse {
    /// 本批行数据（每行若干字符串列，测试用简化表示）。
    pub rows: Vec<Vec<String>>,
    /// 本批告警消息。
    pub warnings: Vec<String>,
    /// 本批扫描 key 数。
    pub scanned_keys: u64,
    /// 若存在则表示本批失败原因。
    pub error: Option<String>,
}

/// 响应源抽象：按批拉取 `SelectResponse`，并支持关闭。
pub trait ResponseSource: Send {
    /// 拉取下一批响应；`Ok(None)` 表示结束。
    fn next_response(&mut self) -> DistSqlResult<Option<SelectResponse>>;
    /// 关闭底层资源；默认空实现。
    fn close(&mut self) -> DistSqlResult<()> {
        Ok(())
    }
}

/// 基于内存队列的测试用响应源。
pub struct VecResponseSource {
    responses: VecDeque<DistSqlResult<SelectResponse>>,
    closed: bool,
}

impl VecResponseSource {
    /// 用预设的响应序列构造响应源。
    pub fn new(responses: Vec<DistSqlResult<SelectResponse>>) -> Self {
        Self {
            responses: responses.into(),
            closed: false,
        }
    }
    /// 是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

impl ResponseSource for VecResponseSource {
    fn next_response(&mut self) -> DistSqlResult<Option<SelectResponse>> {
        if self.closed {
            return Ok(None);
        }
        self.responses.pop_front().transpose()
    }
    fn close(&mut self) -> DistSqlResult<()> {
        self.closed = true;
        self.responses.clear();
        Ok(())
    }
}

/// 将批响应展开为逐行迭代的 Select 结果。
pub struct SelectResult<S: ResponseSource> {
    source: Option<S>,
    buffered: VecDeque<Vec<String>>,
    stats: SelectStats,
    closed: bool,
}

impl<S: ResponseSource> SelectResult<S> {
    /// 包装一个响应源，惰性按行产出。
    pub fn new(source: S) -> Self {
        Self {
            source: Some(source),
            buffered: VecDeque::new(),
            stats: SelectStats::default(),
            closed: false,
        }
    }

    /// 返回下一行；必要时从响应源拉取下一批并缓冲。
    pub fn next_row(&mut self) -> DistSqlResult<Option<Vec<String>>> {
        loop {
            if self.closed {
                return Ok(None);
            }
            if let Some(row) = self.buffered.pop_front() {
                return Ok(Some(row));
            }
            // 缓冲耗尽则拉取下一批；结束或错误时关闭。
            let Some(response) = self
                .source
                .as_mut()
                .expect("response source is present until consumed")
                .next_response()?
            else {
                self.close()?;
                return Ok(None);
            };
            self.stats.response_count += 1;
            self.stats.warning_count += response.warnings.len();
            self.stats.scanned_keys += response.scanned_keys;
            if let Some(error) = response.error {
                self.close()?;
                return Err(DistSqlError(error));
            }
            self.stats.row_count += response.rows.len();
            self.buffered.extend(response.rows);
        }
    }

    /// 返回累计统计引用。
    pub fn stats(&self) -> &SelectStats {
        &self.stats
    }
    /// 关闭结果集并清空缓冲；幂等。
    pub fn close(&mut self) -> DistSqlResult<()> {
        if !self.closed {
            self.closed = true;
            self.buffered.clear();
            if let Some(source) = self.source.as_mut() {
                source.close()?;
            }
        }
        Ok(())
    }
    /// 取出底层响应源（只能消费一次）。
    pub fn into_source(mut self) -> S {
        self.source
            .take()
            .expect("response source can only be consumed once")
    }
}

impl<S: ResponseSource> Drop for SelectResult<S> {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

#[cfg(test)]
#[path = "bench_test.rs"]
mod bench_test;
#[cfg(test)]
#[path = "context_test.rs"]
mod context_test;
#[cfg(test)]
#[path = "distsql_test.rs"]
mod distsql_test;
#[cfg(test)]
#[path = "main_test.rs"]
mod main_test;
#[cfg(test)]
#[path = "request_builder_test.rs"]
mod request_builder_test;
#[cfg(test)]
#[path = "select_result_test.rs"]
mod select_result_test;
