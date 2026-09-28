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

// Extract handler 单元测试。
//
// 覆盖 failpoint mock、默认/显式时间窗口建任务，以及 dump 流式写出路径。

use std::collections::HashMap;
use std::sync::Mutex;

use crate::extractor::{
    ExtractError, ExtractReader, ExtractResult, ExtractRuntime, ExtractTask, ExtractType,
    HttpRequest, HttpResponseWriter, NewExtractTaskServeHandler, RequestContext, Timestamp,
    buildExtractPlanTask, buildExtractTask,
};

/// 内存中的假 reader，按偏移返回预设字节。
#[derive(Default)]
struct MockReader {
    data: Vec<u8>,
    offset: usize,
}

impl ExtractReader for MockReader {
    fn read(&mut self, buffer: &mut [u8]) -> ExtractResult<usize> {
        if self.offset >= self.data.len() {
            return Ok(0);
        }
        let remaining = &self.data[self.offset..];
        let count = remaining.len().min(buffer.len());
        buffer[..count].copy_from_slice(&remaining[..count]);
        self.offset += count;
        Ok(count)
    }

    fn close(&mut self) -> ExtractResult<()> {
        Ok(())
    }
}

/// 假运行时：固定 now、可选 failpoint，并记录已提交任务与错误日志。
#[derive(Default)]
struct MockRuntime {
    now: Timestamp,
    failpoint: bool,
    extracted: Mutex<Vec<ExtractTask>>,
    errors: Mutex<Vec<String>>,
}

impl ExtractRuntime for MockRuntime {
    fn now(&self) -> Timestamp {
        self.now
    }

    fn parse_time(&self, value: &str) -> ExtractResult<Timestamp> {
        value
            .parse::<i64>()
            .map(Timestamp)
            .map_err(|error| ExtractError(format!("parse time {value}: {error}")))
    }

    fn extract_task(&self, _context: &RequestContext, task: ExtractTask) -> ExtractResult<String> {
        self.extracted.lock().expect("extracted lock").push(task);
        Ok("task-token".to_owned())
    }

    fn extract_task_directory(&self) -> String {
        "/tmp/extract".to_owned()
    }

    fn open_extract(
        &self,
        _context: &RequestContext,
        _path: &str,
    ) -> ExtractResult<Box<dyn ExtractReader>> {
        Ok(Box::new(MockReader {
            data: b"zip-bytes".to_vec(),
            offset: 0,
        }))
    }

    fn failpoint_enabled(&self, name: &str) -> bool {
        self.failpoint && name == "extractTaskServeHandler"
    }

    fn log_error(&self, message: &str, error: &ExtractError) {
        self.errors
            .lock()
            .expect("errors lock")
            .push(format!("{message}: {error}"));
    }

    fn log_warning(&self, message: &str, error: &ExtractError) {
        self.errors
            .lock()
            .expect("errors lock")
            .push(format!("{message}: {error}"));
    }
}

/// 假 HTTP 写出器：记录状态码、头、body 与错误。
#[derive(Default)]
struct MockWriter {
    status: u16,
    headers: HashMap<String, String>,
    body: Vec<u8>,
    error: Option<ExtractError>,
}

impl HttpResponseWriter for MockWriter {
    fn set_header(&mut self, name: &str, value: &str) {
        self.headers.insert(name.to_owned(), value.to_owned());
    }

    fn write_status(&mut self, status: u16) {
        self.status = status;
    }

    fn write(&mut self, data: &[u8]) -> ExtractResult<usize> {
        self.body.extend_from_slice(data);
        Ok(data.len())
    }

    fn write_error(&mut self, error: ExtractError) {
        self.status = 500;
        self.error = Some(error);
    }
}

/// 由键值对构造带 query 的 `HttpRequest`。
fn query_request(entries: &[(&str, &str)]) -> HttpRequest {
    let mut query = HashMap::new();
    for (name, value) in entries {
        query.insert((*name).to_owned(), (*value).to_owned());
    }
    HttpRequest {
        query,
        context: RequestContext::default(),
    }
}

/// TestExtractHandler 对应 Go 测试：在 extractTaskServeHandler failpoint 下返回 mock dump。
#[test]
fn TestExtractHandler() {
    let runtime = MockRuntime {
        now: Timestamp(1_700_000_000),
        failpoint: true,
        ..Default::default()
    };
    let handler = NewExtractTaskServeHandler(runtime);
    let mut writer = MockWriter::default();
    let begin = "1700000000";
    let end = "1700000060";
    handler.ServeHTTP(
        &mut writer,
        &query_request(&[("type", "plan"), ("begin", begin), ("end", end)]),
    );
    assert_eq!(200, writer.status);
    assert_eq!(b"mock", writer.body.as_slice());
    assert!(writer.error.is_none());
}

/// TestExtractHandlerInfoSchemaV2 对应 Go 测试：无时间范围请求同样在 failpoint 下成功。
#[test]
fn TestExtractHandlerInfoSchemaV2() {
    let runtime = MockRuntime {
        now: Timestamp(1_700_000_100),
        failpoint: true,
        ..Default::default()
    };
    let handler = NewExtractTaskServeHandler(runtime);
    let mut writer = MockWriter::default();
    handler.ServeHTTP(&mut writer, &query_request(&[("type", "plan")]));
    assert_eq!(200, writer.status);
    assert_eq!(b"mock", writer.body.as_slice());
}

/// 校验默认时间窗口与显式参数下 `buildExtractPlanTask` / `buildExtractTask` 与 Go 一致。
#[test]
fn extract_plan_task_defaults_and_explicit_window_match_go() {
    let runtime = MockRuntime {
        now: Timestamp(1_000),
        ..Default::default()
    };

    let (task, is_dump) =
        buildExtractPlanTask(&query_request(&[("type", "plan")]), &runtime).expect("defaults");
    assert_eq!(ExtractType::Plan, task.extract_type);
    assert!(!task.is_background_job);
    assert_eq!(Timestamp(1_000 + 30 * 60), task.begin);
    assert_eq!(Timestamp(1_000), task.end);
    assert!(!task.skip_stats);
    assert!(task.use_history_view);
    assert!(!is_dump);

    let (task, is_dump) = buildExtractTask(
        &query_request(&[
            ("type", "plan"),
            ("begin", "10"),
            ("end", "20"),
            ("isDump", "true"),
            ("isSkipStats", "true"),
            ("isHistoryView", "false"),
        ]),
        &runtime,
    )
    .expect("explicit");
    assert_eq!(Timestamp(10), task.begin);
    assert_eq!(Timestamp(20), task.end);
    assert!(is_dump);
    assert!(task.skip_stats);
    assert!(!task.use_history_view);
}

/// Go 使用 handler 包公开的 camelCase query key，并接受 strconv.ParseBool 的短写法。
#[test]
fn extract_plan_task_uses_go_query_keys_and_bool_syntax() {
    let runtime = MockRuntime {
        now: Timestamp(1_000),
        ..Default::default()
    };

    let (task, is_dump) = buildExtractTask(
        &query_request(&[
            ("type", "plan"),
            ("begin", "10"),
            ("end", "20"),
            ("isDump", "1"),
            ("isSkipStats", "T"),
            ("isHistoryView", "FALSE"),
        ]),
        &runtime,
    )
    .expect("Go-compatible query keys and bool syntax");

    assert!(is_dump);
    assert!(task.skip_stats);
    assert!(!task.use_history_view);
}

#[test]
fn extract_plan_task_logs_time_parse_failures_like_go() {
    let runtime = MockRuntime {
        now: Timestamp(1_000),
        ..Default::default()
    };

    let error = buildExtractTask(
        &query_request(&[("type", "plan"), ("begin", "not-a-time")]),
        &runtime,
    )
    .expect_err("invalid begin must fail");
    assert!(error.0.starts_with("parse time not-a-time:"));
    assert_eq!(
        vec![format!("extract task begin time failed: {error}")],
        *runtime.errors.lock().expect("errors lock")
    );
}

/// failpoint 关闭且 is_dump=true 时，应设置 zip 头并流式写出 mock zip 字节。
#[test]
fn extract_handler_streams_dump_when_failpoint_disabled() {
    let runtime = MockRuntime {
        now: Timestamp(50),
        failpoint: false,
        ..Default::default()
    };
    let handler = NewExtractTaskServeHandler(runtime);
    let mut writer = MockWriter::default();
    handler.ServeHTTP(
        &mut writer,
        &query_request(&[
            ("type", "plan"),
            ("begin", "1"),
            ("end", "2"),
            ("isDump", "true"),
        ]),
    );
    assert_eq!(0, writer.status);
    assert_eq!(
        Some("application/zip"),
        writer.headers.get("Content-Type").map(String::as_str)
    );
    assert_eq!(b"zip-bytes", writer.body.as_slice());
    let extracted = handler
        .ExtractHandler
        .extracted
        .lock()
        .expect("extracted lock");
    assert_eq!(1, extracted.len());
    assert_eq!(ExtractType::Plan, extracted[0].extract_type);
}
