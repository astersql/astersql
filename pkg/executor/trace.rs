// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// TRACE 语句执行器：捕获子语句执行轨迹并按格式输出。
//
// TRACE 可包裹任意 SQL，收集 basictracer/appdash 风格的 span 树或日志事件，
// 支持 `log` / `row` / `json` 等输出格式；用于诊断执行路径与耗时。

#![allow(non_snake_case)]

use std::fmt::Display;

/// TRACE FORMAT='log'：按 span 日志事件输出。
pub const TRACE_FORMAT_LOG: &str = "log";
/// TRACE FORMAT='json'：将轨迹树序列化为 JSON 分片输出。
pub const TRACE_FORMAT_JSON: &str = "json";
/// 日志字段键名：仅该键的 field 会进入 log 格式结果行。
pub const TRACE_EVENT_KEY: &str = "event";
/// JSON 结果单行最大字节数（与 Go 侧 4096 对齐，可能截断多字节字符）。
const MAX_JSON_ROW_LEN: usize = 4096;

/// A row sink with the operations used by the TRACE executor.
/// TRACE 执行器写入结果行的 Chunk 接口。
pub trait TraceChunk {
    type Time;

    fn reset(&mut self);
    fn num_rows(&self) -> usize;
    fn append_string(&mut self, column: usize, value: &str);
    /// Appends Go string bytes without UTF-8 repair. JSON rows can split in the
    /// middle of a multibyte sequence at the 4096-byte boundary.
    /// 追加原始字节（不做 UTF-8 修复）；JSON 按 4096 切分可能断在多字节中间。
    fn append_bytes(&mut self, column: usize, value: &[u8]);
    fn append_time(&mut self, column: usize, value: &Self::Time);
}

/// One log field captured by basictracer.
#[derive(Clone, Debug, Eq, PartialEq)]
/// basictracer 捕获的单个日志字段。
pub struct TraceLogField {
    pub key: String,
    pub value: String,
}

/// One basictracer log record.
#[derive(Clone, Debug, Eq, PartialEq)]
/// 一条 basictracer 日志记录（时间戳 + 字段列表）。
pub struct TraceLog<T> {
    pub timestamp: T,
    pub fields: Vec<TraceLogField>,
}

/// The portion of `basictracer.RawSpan` consumed by TRACE FORMAT='log'.
#[derive(Clone, Debug, Eq, PartialEq)]
/// TRACE FORMAT='log' 消费的 RawSpan 子集。
pub struct RawSpan<T> {
    pub operation: String,
    pub start: T,
    /// The backend formats non-empty tags with the same `%v` representation
    /// used by Go. `None` represents an empty tag map.
    /// 非空 tag 的 Go `%v` 格式化串；`None` 表示空 tag map。
    pub formatted_tags: Option<String>,
    pub logs: Vec<TraceLog<T>>,
}

/// Time metadata used when rendering an appdash trace tree.
#[derive(Clone, Debug, Eq, PartialEq)]
/// 渲染 appdash 轨迹树时使用的时间元数据。
pub struct TraceTimespan {
    /// A monotonic, comparable start value used for sibling ordering.
    /// 单调可比较的起始值，用于兄弟节点排序。
    pub start_order: i128,
    /// Go's `15:04:05.000000` rendering of the start time.
    /// Go `15:04:05.000000` 格式的起始时间。
    pub formatted_start: String,
    /// Go's `time.Duration.String()` rendering.
    /// Go `time.Duration.String()` 格式的耗时。
    pub formatted_duration: String,
}

/// The appdash tree data consumed by row and JSON TRACE formats.
#[derive(Clone, Debug, Eq, PartialEq)]
/// row / JSON TRACE 格式消费的 appdash 树节点。
pub struct TraceNode {
    pub operation: String,
    pub timespan: Option<TraceTimespan>,
    pub children: Vec<TraceNode>,
}

/// Record-set behavior needed by `drainRecordSet`.
/// `drainRecordSet` 所需的结果集（RecordSet）行为。
pub trait TraceRecordSet {
    type Context;
    type Error;
    type Chunk: TraceChunk;

    fn new_chunk(&mut self) -> Self::Chunk;
    fn next(&mut self, context: &Self::Context, chunk: &mut Self::Chunk)
    -> Result<(), Self::Error>;
    fn close(&mut self) -> Result<(), Self::Error>;
}

/// Production boundary for TiDB session, SQL executor, tracer, and logger APIs.
///
/// Every method corresponds to a concrete Go call. There are deliberately no
/// defaults: an integration must provide the real session and tracing behavior.
/// 生产边界：会话、SQL 执行、tracer 与 logger API（对应 Go 侧具体调用，无默认实现）。
pub trait TraceBackend<S> {
    type Context: Clone;
    type Error: Display;
    type StatementContext: Clone;
    type Time: Clone;
    type RecordSet: TraceRecordSet<Context = Self::Context, Error = Self::Error, Chunk = Self::Chunk>;
    type Chunk: TraceChunk<Time = Self::Time>;
    type BasicTrace;
    type TreeTrace;

    fn save_statement_context(&self) -> Self::StatementContext;
    fn restore_statement_context(&mut self, statement_context: Self::StatementContext);
    fn context_with_trace_exec_details(&self, context: Self::Context) -> Self::Context;

    fn begin_basic_trace(
        &mut self,
        context: Self::Context,
    ) -> Result<(Self::Context, Self::BasicTrace), Self::Error>;
    fn finish_basic_trace(
        &mut self,
        trace: Self::BasicTrace,
    ) -> Result<Vec<RawSpan<Self::Time>>, Self::Error>;

    fn begin_tree_trace(
        &mut self,
        context: Self::Context,
    ) -> Result<(Self::Context, Self::TreeTrace), Self::Error>;
    fn finish_tree_trace(&mut self, trace: Self::TreeTrace) -> Result<Vec<TraceNode>, Self::Error>;
    fn marshal_trace_json(&self, traces: &[TraceNode]) -> Result<Vec<u8>, Self::Error>;
    fn deprecated_optimizer_trace_error(&self) -> Self::Error;

    fn restricted_sql(&self) -> bool;
    fn set_restricted_sql(&mut self, restricted: bool);
    fn with_internal_trace_source(&self, context: Self::Context) -> Self::Context;
    fn execute_stmt(
        &mut self,
        context: &Self::Context,
        statement: &S,
    ) -> (Option<Self::RecordSet>, Option<Self::Error>);
    fn sql_error_code(&self, error: &Self::Error) -> u16;
    fn affected_rows(&self) -> u64;
    fn event(&mut self, context: &Self::Context, message: String);
    fn close_error(&mut self, context: &Self::Context, error: &Self::Error);
}

/// Root executor of a TRACE query.
/// TRACE 查询的根执行器。
pub struct TraceExec<B: TraceBackend<S>, S, R, E> {
    /// 后端依赖（会话/执行/tracer）。
    pub BaseExecutor: B,
    /// 已收集的 RawSpan（log 路径）。
    pub CollectedSpans: Vec<RawSpan<B::Time>>,
    /// 是否已输出完毕（TRACE 只产生一批结果）。
    pub exhausted: bool,
    /// 被 TRACE 包裹的子语句 AST/节点。
    pub stmtNode: S,
    /// 名称解析上下文（可选）。
    pub resolveCtx: Option<R>,
    /// 执行计划构建器（可选）。
    pub builder: Option<E>,
    /// 输出格式：log / json / row 等。
    pub format: String,
    /// 是否为优化器 TRACE（已废弃路径会报错）。
    pub optimizerTrace: bool,
    /// 优化器 TRACE 目标路径/标识。
    pub optimizerTraceTarget: String,
}

impl<B, S, R, E> TraceExec<B, S, R, E>
where
    B: TraceBackend<S>,
{
    /// 执行子语句并一次性写出 TRACE 结果。
    /// Executes the child statement and emits TRACE output once.
    pub fn Next(&mut self, context: B::Context, request: &mut B::Chunk) -> Result<(), B::Error> {
        request.reset();
        if self.exhausted {
            return Ok(());
        }

        // 等价 Go defer：保存后任意返回路径都恢复 StmtCtx。
        // Go uses defer here, so every path after the save restores StmtCtx.
        let statement_context = self.BaseExecutor.save_statement_context();
        let result = if self.optimizerTrace {
            Err(self.BaseExecutor.deprecated_optimizer_trace_error())
        } else {
            let context = self.BaseExecutor.context_with_trace_exec_details(context);
            if self.format == TRACE_FORMAT_LOG {
                self.nextTraceLog(context, request)
            } else {
                self.nextRowJSON(context, request)
            }
        };
        self.BaseExecutor
            .restore_statement_context(statement_context);
        result
    }

    /// log 格式：开启 basic tracer，执行子语句，再把 span 写成结果行。
    fn nextTraceLog(
        &mut self,
        context: B::Context,
        request: &mut B::Chunk,
    ) -> Result<(), B::Error> {
        let (context, trace) = self.BaseExecutor.begin_basic_trace(context)?;
        self.executeChild(context);
        let spans = self.BaseExecutor.finish_basic_trace(trace)?;
        generateLogResult(&spans, request);
        self.exhausted = true;
        Ok(())
    }

    /// row/json 格式：开启树形 tracer，执行子语句，再 DFS 或 JSON 分片输出。
    fn nextRowJSON(&mut self, context: B::Context, request: &mut B::Chunk) -> Result<(), B::Error> {
        let (context, trace) = self.BaseExecutor.begin_tree_trace(context)?;
        self.executeChild(context);
        let mut traces = self.BaseExecutor.finish_tree_trace(trace)?;

        if self.format != TRACE_FORMAT_JSON {
            if let Some(trace) = traces.first_mut() {
                dfsTree(trace, "", false, request);
            }
            self.exhausted = true;
            return Ok(());
        }

        let data = self.BaseExecutor.marshal_trace_json(&traces)?;
        for row in data.chunks(MAX_JSON_ROW_LEN) {
            request.append_bytes(0, row);
        }
        // Go 在 JSON 长度恰为 4096 倍数时追加空末行（循环条件为 len > maxRowLen）。
        // Go appends an empty final row when JSON length is an exact multiple
        // of 4096, because its loop uses `len(data) > maxRowLen`.
        if data.len().is_multiple_of(MAX_JSON_ROW_LEN) {
            request.append_bytes(0, &[]);
        }
        self.exhausted = true;
        Ok(())
    }

    /// 以 restricted SQL + 内部 TRACE 源执行子语句，并排空结果集。
    fn executeChild(&mut self, context: B::Context) {
        let restricted = self.BaseExecutor.restricted_sql();
        self.BaseExecutor.set_restricted_sql(true);
        let context = self.BaseExecutor.with_internal_trace_source(context);

        let (record_set, execute_error) = self.BaseExecutor.execute_stmt(&context, &self.stmtNode);
        if let Some(error) = execute_error {
            let code = self.BaseExecutor.sql_error_code(&error);
            self.BaseExecutor
                .event(&context, format!("execute with error({code}): {error}"));
        }
        if let Some(mut record_set) = record_set {
            drainRecordSet(&mut self.BaseExecutor, &context, &mut record_set);
            if let Err(error) = record_set.close() {
                self.BaseExecutor.close_error(&context, &error);
            }
        }

        self.BaseExecutor.event(
            &context,
            format!(
                "execute done, modify row: {}",
                self.BaseExecutor.affected_rows()
            ),
        );
        self.BaseExecutor.set_restricted_sql(restricted);
    }
}

/// 消费结果集直至出错或遇到空 Chunk。
/// Consumes a record set until an error or an empty chunk is observed.
pub fn drainRecordSet<B, S>(backend: &mut B, context: &B::Context, record_set: &mut B::RecordSet)
where
    B: TraceBackend<S>,
{
    let mut request = record_set.new_chunk();
    let mut row_count = 0usize;
    loop {
        match record_set.next(context, &mut request) {
            Err(error) => {
                let code = backend.sql_error_code(&error);
                backend.event(context, format!("execute with error({code}): {error}"));
                return;
            }
            Ok(()) if request.num_rows() == 0 => {
                backend.event(
                    context,
                    format!(
                        "execute done, ReturnRow: {row_count}, ModifyRow: {}",
                        backend.affected_rows()
                    ),
                );
                return;
            }
            Ok(()) => {
                row_count += request.num_rows();
                request.reset();
            }
        }
    }
}

/// 按与 Go 相同的树前缀布局渲染 appdash 轨迹树。
/// Renders an appdash trace tree in the same prefix layout as the Go executor.
pub fn dfsTree<C: TraceChunk>(trace: &mut TraceNode, prefix: &str, is_last: bool, chunk: &mut C) {
    // 根节点无前缀；末子用 └─，其余用 ├─，并扩展竖线前缀。
    let (new_prefix, suffix) = if prefix.is_empty() {
        (format!("{prefix}  "), "")
    } else if is_last {
        (format!("{prefix}  "), "└─")
    } else {
        (format!("{prefix}│ "), "├─")
    };

    let (start, duration) = trace
        .timespan
        .as_ref()
        .map(|span| {
            (
                span.formatted_start.as_str(),
                span.formatted_duration.as_str(),
            )
        })
        .unwrap_or(("00:00:00.000000", "0s"));
    chunk.append_string(0, &format!("{prefix}{suffix}{}", trace.operation));
    chunk.append_string(1, start);
    chunk.append_string(2, duration);

    // 兄弟节点按 start_order 排序后递归输出。
    trace.children.sort_by_key(|child| {
        child
            .timespan
            .as_ref()
            .map(|span| span.start_order)
            .unwrap_or_default()
    });
    let child_count = trace.children.len();
    for (index, child) in trace.children.iter_mut().enumerate() {
        dfsTree(child, &new_prefix, index + 1 == child_count, chunk);
    }
}

/// 写出 TRACE FORMAT='log' 的 span 起始行与 event 行。
/// Emits the span-start and trace-event rows used by TRACE FORMAT='log'.
pub fn generateLogResult<C>(spans: &[RawSpan<C::Time>], chunk: &mut C)
where
    C: TraceChunk,
{
    for span in spans {
        chunk.append_time(0, &span.start);
        chunk.append_string(1, &format!("--- start span {} ----", span.operation));
        chunk.append_string(2, "");
        chunk.append_string(3, &span.operation);

        let tags = span.formatted_tags.as_deref().unwrap_or("");
        for log in &span.logs {
            for field in &log.fields {
                // 仅输出 key=event 的字段，并附带 span tags 与 operation。
                if field.key == TRACE_EVENT_KEY {
                    chunk.append_time(0, &log.timestamp);
                    chunk.append_string(1, &field.value);
                    chunk.append_string(2, tags);
                    chunk.append_string(3, &span.operation);
                }
            }
        }
    }
}

/// 优化器 TRACE 归档写入外部存储的边界接口。
/// External-storage boundary used by optimizer trace archive creation.
pub trait OptimizerTraceStorage {
    type Context;
    type Writer;
    type Error;

    fn optimizer_trace_directory(&self) -> &str;
    fn unix_nanos(&self) -> i128;
    fn fill_random(&mut self, destination: &mut [u8]) -> Result<(), Self::Error>;
    fn create_archive(
        &mut self,
        context: &Self::Context,
        path: &str,
    ) -> Result<Self::Writer, Self::Error>;
}

/// 在外部存储创建 `optimizer_trace_<random>_<unix-nanos>.zip`。
/// Creates `optimizer_trace_<random>_<unix-nanos>.zip` in external storage.
pub fn generateOptimizerTraceFile<S: OptimizerTraceStorage>(
    context: &S::Context,
    storage: &mut S,
) -> Result<(S::Writer, String), S::Error> {
    let mut random = [0u8; 16];
    storage.fill_random(&mut random)?;
    let key = base64UrlEncode(&random);
    let file_name = format!("optimizer_trace_{key}_{}.zip", storage.unix_nanos());
    let path = joinPath(storage.optimizer_trace_directory(), &file_name);
    let writer = storage.create_archive(context, &path)?;
    Ok((writer, file_name))
}

/// 拼接目录与文件名（空或 `.` 目录时仅返回文件名）。
fn joinPath(directory: &str, file_name: &str) -> String {
    if directory.is_empty() || directory == "." {
        file_name.to_owned()
    } else {
        format!("{}/{file_name}", directory.trim_end_matches('/'))
    }
}

/// URL-safe Base64 编码（字母表含 `-` `_`，不足补 `=`）。
fn base64UrlEncode(input: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut encoded = String::with_capacity(input.len().div_ceil(3) * 4);
    for block in input.chunks(3) {
        let first = block[0];
        let second = block.get(1).copied().unwrap_or_default();
        let third = block.get(2).copied().unwrap_or_default();
        encoded.push(ALPHABET[(first >> 2) as usize] as char);
        encoded.push(ALPHABET[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
        if block.len() > 1 {
            encoded.push(ALPHABET[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char);
        } else {
            encoded.push('=');
        }
        if block.len() > 2 {
            encoded.push(ALPHABET[(third & 0x3f) as usize] as char);
        } else {
            encoded.push('=');
        }
    }
    encoded
}
