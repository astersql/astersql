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

// Extract 任务 HTTP handler 实现。
//
// 解析查询参数构建 `ExtractTask`（执行计划抽取任务），按 `is_dump` 决定返回
// 任务名或流式下载 zip；支持 failpoint 注入 mock 响应，对齐 Go 侧行为。

use std::collections::HashMap;
use std::error::Error as StdError;
use std::fmt;

use astersql_server_handler::util::{BEGIN, END, IS_DUMP, IS_HISTORY_VIEW, IS_SKIP_STATS, TYPE};

/// 查询参数 `type=plan` 对应的抽取任务类型字符串。
const EXTRACT_PLAN_TASK_TYPE: &str = "plan";

/// Extract 路径上的错误包装，消息与 Go 侧字符串对齐。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractError(pub String);

impl fmt::Display for ExtractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl StdError for ExtractError {}

/// Extract 操作结果类型别名。
pub type ExtractResult<T> = Result<T, ExtractError>;

/// 请求上下文：携带 request_id，以及是否已取消。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RequestContext {
    pub request_id: String,
    pub cancelled: bool,
}

/// 简化的 HTTP 请求视图：仅保留 query 与上下文。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HttpRequest {
    pub query: HashMap<String, String>,
    pub context: RequestContext,
}

/// 时间戳（秒级 Unix 时间），用于抽取窗口 begin/end。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
pub struct Timestamp(pub i64);

/// 抽取任务类型枚举；当前仅支持 Plan（执行计划）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtractType {
    Plan,
}

/// 一次 Extract 任务的参数集合。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExtractTask {
    /// 抽取类型（如 Plan）。
    pub extract_type: ExtractType,
    /// 是否作为后台作业运行。
    pub is_background_job: bool,
    /// 时间窗口起点。
    pub begin: Timestamp,
    /// 时间窗口终点。
    pub end: Timestamp,
    /// 是否跳过统计信息。
    pub skip_stats: bool,
    /// 是否使用历史视图（history view）读取元数据。
    pub use_history_view: bool,
}

/// 可流式读取抽取产物（如 zip）的 reader 抽象。
pub trait ExtractReader: Send {
    fn read(&mut self, buffer: &mut [u8]) -> ExtractResult<usize>;
    fn close(&mut self) -> ExtractResult<()>;
}

/// Extract 运行时依赖：时间、任务提交、目录、打开文件、failpoint 与日志。
pub trait ExtractRuntime: Send + Sync {
    fn now(&self) -> Timestamp;
    fn parse_time(&self, value: &str) -> ExtractResult<Timestamp>;
    fn extract_task(&self, context: &RequestContext, task: ExtractTask) -> ExtractResult<String>;
    fn extract_task_directory(&self) -> String;
    fn open_extract(
        &self,
        context: &RequestContext,
        path: &str,
    ) -> ExtractResult<Box<dyn ExtractReader>>;
    fn failpoint_enabled(&self, name: &str) -> bool;
    fn log_error(&self, message: &str, error: &ExtractError);
    fn log_warning(&self, message: &str, error: &ExtractError);
}

/// HTTP 响应写出抽象。
pub trait HttpResponseWriter {
    fn set_header(&mut self, name: &str, value: &str);
    fn write_status(&mut self, status: u16);
    fn write(&mut self, data: &[u8]) -> ExtractResult<usize>;
    fn write_error(&mut self, error: ExtractError);
}

// ExtractTaskServeHandler is the HTTP serve handler for extract tasks.
/// Extract 任务 HTTP serve handler，持有 `ExtractRuntime` 实现。
pub struct ExtractTaskServeHandler<R> {
    pub ExtractHandler: R,
}

/// 构造 `ExtractTaskServeHandler`。
pub fn NewExtractTaskServeHandler<R: ExtractRuntime>(
    extractHandler: R,
) -> ExtractTaskServeHandler<R> {
    ExtractTaskServeHandler {
        ExtractHandler: extractHandler,
    }
}

impl<R: ExtractRuntime> ExtractTaskServeHandler<R> {
    /// 处理 Extract HTTP：建任务 →（可选 failpoint mock）→ 提交 / dump 流式下载。
    pub fn ServeHTTP(&self, writer: &mut dyn HttpResponseWriter, request: &HttpRequest) {
        let (task, is_dump) = match buildExtractTask(request, &self.ExtractHandler) {
            Ok(task) => task,
            Err(error) => {
                self.ExtractHandler
                    .log_error("build extract task failed", &error);
                writer.write_error(error);
                return;
            }
        };
        // failpoint 开启时短路返回 mock 字节，便于单测不依赖真实抽取。
        if self
            .ExtractHandler
            .failpoint_enabled("extractTaskServeHandler")
        {
            writer.write_status(200);
            if let Err(error) = writer.write(b"mock") {
                writer.write_error(error);
            }
            return;
        }

        // 提交抽取任务，成功则得到产物名（token / 文件名）。
        let name = match self
            .ExtractHandler
            .extract_task(&RequestContext::default(), task)
        {
            Ok(name) => name,
            Err(error) => {
                self.ExtractHandler.log_error("extract task failed", &error);
                writer.write_error(error);
                return;
            }
        };
        // 非 dump：仅返回任务名；dump：将 zip 流式写回客户端。
        if !is_dump {
            writer.write_status(200);
            if let Err(error) = writer.write(name.as_bytes()) {
                self.ExtractHandler
                    .log_error("extract handler failed", &error);
            }
            return;
        }
        if let Err(error) =
            streamExtractResponse(&request.context, writer, &name, &self.ExtractHandler)
        {
            self.ExtractHandler
                .log_warning("stream extract response failed", &error);
            writer.write_error(error);
        }
    }
}

/// 打开抽取目录下的产物并以 application/zip 流式写出。
pub fn streamExtractResponse<R: ExtractRuntime + ?Sized>(
    context: &RequestContext,
    writer: &mut dyn HttpResponseWriter,
    name: &str,
    runtime: &R,
) -> ExtractResult<()> {
    let path = join_path(&runtime.extract_task_directory(), name);
    let mut reader = runtime.open_extract(context, &path)?;
    writer.set_header("Content-Type", "application/zip");
    writer.set_header(
        "Content-Disposition",
        &format!("attachment; filename=\"{name}.zip\""),
    );

    // 分块拷贝：读满缓冲后写完再读下一块，避免一次性装入内存。
    let copy_result = (|| {
        let mut buffer = vec![0_u8; 32 * 1024];
        loop {
            let count = reader.read(&mut buffer)?;
            if count == 0 {
                return Ok(());
            }
            let mut written = 0;
            while written < count {
                let size = writer.write(&buffer[written..count])?;
                if size == 0 {
                    return Err(ExtractError("short response write".to_owned()));
                }
                written += size;
            }
        }
    })();
    let _ = reader.close();
    copy_result
}

/// 按 `type` 查询参数分发构建具体抽取任务；未知类型记日志并返回错误。
pub fn buildExtractTask<R: ExtractRuntime + ?Sized>(
    request: &HttpRequest,
    runtime: &R,
) -> ExtractResult<(ExtractTask, bool)> {
    let task_type = query_value(request, TYPE);
    if task_type.eq_ignore_ascii_case(EXTRACT_PLAN_TASK_TYPE) {
        return buildExtractPlanTask(request, runtime);
    }
    let error = ExtractError("unknown extract task type".to_owned());
    runtime.log_error("unknown extract task type", &error);
    Err(error)
}

/// 构建 Plan 类型抽取任务：缺省 begin=now+30min、end=now；解析布尔查询参数。
pub fn buildExtractPlanTask<R: ExtractRuntime + ?Sized>(
    request: &HttpRequest,
    runtime: &R,
) -> ExtractResult<(ExtractTask, bool)> {
    let begin_text = query_value(request, BEGIN);
    // 未指定 begin 时，Go 侧默认取 now+30 分钟作为窗口起点。
    let begin = if begin_text.is_empty() {
        Timestamp(runtime.now().0.saturating_add(30 * 60))
    } else {
        match runtime.parse_time(begin_text) {
            Ok(begin) => begin,
            Err(error) => {
                runtime.log_error("extract task begin time failed", &error);
                return Err(error);
            }
        }
    };
    let end_text = query_value(request, END);
    let end = if end_text.is_empty() {
        runtime.now()
    } else {
        match runtime.parse_time(end_text) {
            Ok(end) => end,
            Err(error) => {
                runtime.log_error("extract task end time failed", &error);
                return Err(error);
            }
        }
    };
    let is_dump = extractBoolParam(IS_DUMP, false, request);
    Ok((
        ExtractTask {
            extract_type: ExtractType::Plan,
            is_background_job: false,
            begin,
            end,
            skip_stats: extractBoolParam(IS_SKIP_STATS, false, request),
            use_history_view: extractBoolParam(IS_HISTORY_VIEW, true, request),
        },
        is_dump,
    ))
}

/// 从查询参数解析布尔值；空或解析失败时回退到 `defaultValue`。
pub fn extractBoolParam(param: &str, defaultValue: bool, request: &HttpRequest) -> bool {
    let value = query_value(request, param);
    if value.is_empty() {
        return defaultValue;
    }
    match value {
        "1" | "t" | "T" | "true" | "True" | "TRUE" => true,
        "0" | "f" | "F" | "false" | "False" | "FALSE" => false,
        _ => defaultValue,
    }
}

/// 读取命名查询参数，缺失时返回空串。
fn query_value<'request>(request: &'request HttpRequest, name: &str) -> &'request str {
    request.query.get(name).map_or("", String::as_str)
}

/// 按 Go `filepath.Join` 的 Unix 语义拼接并清理目录与文件名。
fn join_path(directory: &str, name: &str) -> String {
    let joined = match (directory.is_empty(), name.is_empty()) {
        (true, true) => String::new(),
        (true, false) => name.to_owned(),
        (false, true) => directory.to_owned(),
        (false, false) => format!("{directory}/{name}"),
    };
    if joined.is_empty() {
        return joined;
    }
    let absolute = joined.starts_with('/');
    let mut components = Vec::new();
    for component in joined.split('/') {
        match component {
            "" | "." => {}
            ".." if components.last().is_some_and(|last| *last != "..") => {
                components.pop();
            }
            ".." if !absolute => components.push(component),
            ".." => {}
            _ => components.push(component),
        }
    }
    let cleaned = components.join("/");
    if absolute {
        format!("/{cleaned}")
    } else if cleaned.is_empty() {
        ".".to_owned()
    } else {
        cleaned
    }
}
