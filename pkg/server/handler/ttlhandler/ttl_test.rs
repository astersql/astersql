// Copyright 2026 AsterSQL.

// TTL handler 的单元测试。
//
// 通过可记录调用轨迹的运行时替身，对齐 Go handler 的请求方法校验、domain 获取、
// TTL 任务触发、响应写回及日志分支，并验证失败路径不会继续产生后续副作用。

use super::ttl::{NewTTLJobTriggerHandler, TTLHandlerRuntime, TTLResponse};
use astersql_ttl_client::TriggerNewTtlJobResponse;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 测试专用错误；静态字符串便于精确比较写回和日志记录的错误。
struct TestError(&'static str);

/// 记录 handler 与运行时边界的全部交互，供测试断言调用次序之外的可观察结果。
struct RecordingRuntime {
    method: String,
    db: String,
    table: String,
    domain_result: Result<u8, TestError>,
    trigger_result: Result<TTLResponse, TestError>,
    domain_calls: usize,
    trigger_calls: Vec<(u8, u32, String, String)>,
    written: Option<TTLResponse>,
    written_errors: Vec<TestError>,
    failures: Vec<(String, TestError)>,
    successes: Vec<(String, String, TTLResponse)>,
}

impl Default for RecordingRuntime {
    fn default() -> Self {
        Self {
            method: String::new(),
            db: String::new(),
            table: String::new(),
            domain_result: Ok(0),
            trigger_result: Ok(TriggerNewTtlJobResponse::default()),
            domain_calls: 0,
            trigger_calls: Vec::new(),
            written: None,
            written_errors: Vec::new(),
            failures: Vec::new(),
            successes: Vec::new(),
        }
    }
}

impl RecordingRuntime {
    /// 构造具备有效 domain 和默认成功响应的运行时，仅由各用例覆盖待测分支。
    fn new(method: &str) -> Self {
        Self {
            method: method.to_owned(),
            db: "MixedDB".to_owned(),
            table: "MixedTable".to_owned(),
            domain_result: Ok(7),
            trigger_result: Ok(TriggerNewTtlJobResponse::default()),
            ..Self::default()
        }
    }
}

impl TTLHandlerRuntime for RecordingRuntime {
    type Error = TestError;
    type Store = u8;
    type RequestContext = u32;
    type SessionDomain = u8;

    fn request_method(&self) -> &str {
        &self.method
    }

    fn path_value(&self, name: &str) -> String {
        match name {
            "db" => self.db.clone(),
            "table" => self.table.clone(),
            _ => String::new(),
        }
    }

    fn request_context(&self) -> Self::RequestContext {
        42
    }

    fn get_session_domain(
        &mut self,
        _store: &Self::Store,
    ) -> Result<Self::SessionDomain, Self::Error> {
        self.domain_calls += 1;
        self.domain_result.clone()
    }

    fn trigger_new_ttl_job(
        &mut self,
        domain: &Self::SessionDomain,
        context: Self::RequestContext,
        database: &str,
        table: &str,
    ) -> Result<TTLResponse, Self::Error> {
        self.trigger_calls
            .push((*domain, context, database.to_owned(), table.to_owned()));
        self.trigger_result.clone()
    }

    fn method_not_allowed_error(&mut self) -> Self::Error {
        TestError("This api only support POST method")
    }

    fn write_error(&mut self, error: Self::Error) {
        self.written_errors.push(error);
    }

    fn write_data(&mut self, response: &TTLResponse) {
        self.written = Some(response.clone());
    }

    fn log_success(&mut self, database: &str, table: &str, response: &TTLResponse) {
        self.successes
            .push((database.to_owned(), table.to_owned(), response.clone()));
    }

    fn log_failure(&mut self, message: &str, error: &Self::Error) {
        self.failures.push((message.to_owned(), error.clone()));
    }
}

#[test]
/// 非 POST 请求应在访问 domain 前被拒绝，且不触发写数据或日志等副作用。
fn method_other_than_post_is_rejected_without_side_effects() {
    let handler = NewTTLJobTriggerHandler(1_u8);
    let mut runtime = RecordingRuntime::new("GET");

    handler.ServeHTTP(&mut runtime);

    assert_eq!(
        runtime.written_errors,
        vec![TestError("This api only support POST method")]
    );
    assert_eq!(runtime.domain_calls, 0);
    assert!(runtime.trigger_calls.is_empty());
    assert!(runtime.written.is_none());
    assert!(runtime.failures.is_empty());
    assert!(runtime.successes.is_empty());
}

#[test]
/// domain 获取失败时应同时写回并记录原始错误，且不得尝试触发 TTL 任务。
fn domain_lookup_error_is_written_and_logged_without_triggering() {
    let handler = NewTTLJobTriggerHandler(1_u8);
    let mut runtime = RecordingRuntime::new("POST");
    runtime.domain_result = Err(TestError("domain unavailable"));

    handler.ServeHTTP(&mut runtime);

    assert_eq!(runtime.domain_calls, 1);
    assert!(runtime.trigger_calls.is_empty());
    assert_eq!(
        runtime.written_errors,
        vec![TestError("domain unavailable")]
    );
    assert_eq!(
        runtime.failures,
        vec![(
            "failed to get session domain".to_owned(),
            TestError("domain unavailable")
        )]
    );
    assert!(runtime.written.is_none());
}

#[test]
/// 触发失败仍须使用小写库表名，并将同一错误写回响应和失败日志。
fn trigger_error_uses_lowercase_route_values_and_is_logged() {
    let handler = NewTTLJobTriggerHandler(1_u8);
    let mut runtime = RecordingRuntime::new("POST");
    runtime.trigger_result = Err(TestError("trigger failed"));

    handler.ServeHTTP(&mut runtime);

    assert_eq!(runtime.domain_calls, 1);
    assert_eq!(
        runtime.trigger_calls,
        vec![(7, 42, "mixeddb".to_owned(), "mixedtable".to_owned())]
    );
    assert_eq!(runtime.written_errors, vec![TestError("trigger failed")]);
    assert_eq!(
        runtime.failures,
        vec![(
            "failed to trigger new TTL job".to_owned(),
            TestError("trigger failed")
        )]
    );
    assert!(runtime.written.is_none());
}

#[test]
/// 触发成功时应写回完整响应，并以规范化后的库表名记录成功日志。
fn successful_trigger_writes_and_logs_the_complete_response() {
    let handler = NewTTLJobTriggerHandler(1_u8);
    let response = TriggerNewTtlJobResponse {
        table_result: vec![Default::default()],
    };
    let mut runtime = RecordingRuntime::new("POST");
    runtime.trigger_result = Ok(response.clone());

    handler.ServeHTTP(&mut runtime);

    assert_eq!(
        runtime.trigger_calls,
        vec![(7, 42, "mixeddb".to_owned(), "mixedtable".to_owned())]
    );
    assert_eq!(runtime.written, Some(response.clone()));
    assert_eq!(
        runtime.successes,
        vec![("mixeddb".to_owned(), "mixedtable".to_owned(), response)]
    );
    assert!(runtime.written_errors.is_empty());
    assert!(runtime.failures.is_empty());
}
