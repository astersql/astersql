// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// DXF（分布式执行框架）HTTP API 测试。通过真实 status TCP listener 与
// 带状态的 task/storage 运行时覆盖调度、任务历史和 schedule tune 语义。

// runAndCheckReqFn 对应 Go 函数 `func runAndCheckReqFn(t *testing.T, code int, resMsg string, doReqFn func() (*http.Response, error)) []byte {`。
/// 执行请求并校验状态码和响应正文；成功时交还已读取的正文。
///
/// `HttpResponse` 已将 TCP 响应完整读入 `body`，因此资源收尾在
/// `TestServerClient` 的连接关闭时完成，保留 Go helper 的调用顺序和断言语义。
pub fn run_and_check_req_fn<F>(
    expected_status: u16,
    expected_message: &str,
    do_request: F,
) -> Result<Vec<u8>, String>
where
    F: FnOnce() -> Result<astersql_server_internal_testserverclient::HttpResponse, String>,
{
    let response = do_request().map_err(|error| format!("status request failed: {error}"))?;
    if response.status != expected_status {
        return Err(format!(
            "expected HTTP {expected_status}, got HTTP {}: {}",
            response.status,
            String::from_utf8_lossy(&response.body)
        ));
    }
    let response_text = String::from_utf8_lossy(&response.body);
    if !response_text.contains(expected_message) {
        return Err(format!(
            "expected response body to contain {expected_message:?}, got {response_text:?}"
        ));
    }
    Ok(response.body)
}

#[test]
fn run_and_check_req_fn_rejects_an_unexpected_status() {
    let error = run_and_check_req_fn(200, "expected payload", || {
        Ok(astersql_server_internal_testserverclient::HttpResponse {
            status: 503,
            reason: "Service Unavailable".into(),
            headers: Default::default(),
            body: b"handler is not configured".to_vec(),
        })
    })
    .expect_err("the helper must preserve Go's status-code assertion");

    assert!(error.contains("expected HTTP 200"));
}

#[test]
fn run_and_check_req_fn_returns_body_only_after_all_go_assertions_hold() {
    let body = run_and_check_req_fn(200, "schedule", || {
        Ok(astersql_server_internal_testserverclient::HttpResponse {
            status: 200,
            reason: "OK".into(),
            headers: Default::default(),
            body: b"schedule status".to_vec(),
        })
    })
    .expect("matching response must pass the helper");

    assert_eq!(body, b"schedule status");
}

#[test]
fn run_and_check_req_fn_preserves_request_and_body_failures() {
    let request_error = run_and_check_req_fn(200, "payload", || Err("connection reset".into()))
        .expect_err("request errors must fail the helper");
    assert!(request_error.contains("status request failed: connection reset"));

    let body_error = run_and_check_req_fn(200, "expected payload", || {
        Ok(astersql_server_internal_testserverclient::HttpResponse {
            status: 200,
            reason: "OK".into(),
            headers: Default::default(),
            body: b"different payload".to_vec(),
        })
    })
    .expect_err("body mismatches must fail the helper");
    assert!(body_error.contains("expected response body to contain"));
}

#[derive(Default)]
struct DxfResponseRecorder {
    data: Option<astersql_server_handler_tikvhandler::dxf::JsonValue>,
    error: Option<(Option<u16>, String)>,
}

impl astersql_server_handler_tikvhandler::dxf::ResponseWriter for DxfResponseRecorder {
    fn write_data(&mut self, value: astersql_server_handler_tikvhandler::dxf::JsonValue) {
        self.data = Some(value);
    }

    fn write_error(&mut self, error: astersql_server_handler_tikvhandler::DxfError) {
        self.error = Some((None, error.message));
    }

    fn write_error_with_code(
        &mut self,
        status: u16,
        error: astersql_server_handler_tikvhandler::DxfError,
    ) {
        self.error = Some((Some(status), error.message));
    }
}

fn dxf_history_task_json(
    id: i64,
    key: &str,
    keyspace: &str,
) -> astersql_server_handler_tikvhandler::dxf::JsonValue {
    use astersql_server_handler_tikvhandler::dxf::JsonValue;

    JsonValue::Object(vec![
        ("id".into(), JsonValue::Integer(id)),
        ("key".into(), JsonValue::String(key.into())),
        ("keyspace".into(), JsonValue::String(keyspace.into())),
        ("state".into(), JsonValue::String("succeed".into())),
        (
            "start_time".into(),
            JsonValue::String("1970-01-01T00:01:40Z".into()),
        ),
        (
            "state_update_time".into(),
            JsonValue::String("1970-01-01T00:41:40Z".into()),
        ),
        (
            "end_time".into(),
            JsonValue::String("1970-01-01T00:41:40Z".into()),
        ),
    ])
}

fn dxf_import_history_job_json(
    job_id: i64,
    keyspace: &str,
) -> astersql_server_handler_tikvhandler::dxf::JsonValue {
    use astersql_server_handler_tikvhandler::dxf::JsonValue;

    JsonValue::Object(vec![
        ("job_id".into(), JsonValue::Integer(job_id)),
        ("keyspace".into(), JsonValue::String(keyspace.into())),
        ("task_id".into(), JsonValue::Integer(42)),
        ("state".into(), JsonValue::String("pending".into())),
        ("concurrency".into(), JsonValue::Integer(8)),
        ("max_node_count".into(), JsonValue::Integer(4)),
        ("distsql_scan_concurrency".into(), JsonValue::Integer(16)),
        ("index_count".into(), JsonValue::Integer(2)),
        ("column_count".into(), JsonValue::Integer(3)),
        ("file_size".into(), JsonValue::String("2GiB".into())),
        ("data_kv_size".into(), JsonValue::String("1GiB".into())),
        ("index_kv_size".into(), JsonValue::String("512MiB".into())),
        (
            "per_core_speed".into(),
            JsonValue::String("96MiB/core/hour".into()),
        ),
        (
            "overall_speed".into(),
            JsonValue::String("3GiB/hour".into()),
        ),
        ("row_count".into(), JsonValue::Integer(1_024)),
        ("row_length".into(), JsonValue::Integer(2_097_152)),
        (
            "duration".into(),
            JsonValue::Object(vec![
                ("total".into(), JsonValue::String("40m0s".into())),
                ("encode".into(), JsonValue::String("10m0s".into())),
                ("merge_sort".into(), JsonValue::String(String::new())),
                ("ingest".into(), JsonValue::String("30m0s".into())),
                ("collect_conflicts".into(), JsonValue::String(String::new())),
                ("resolve_conflicts".into(), JsonValue::String(String::new())),
                ("post_process".into(), JsonValue::String(String::new())),
            ]),
        ),
    ])
}

pub(super) struct ScheduleStatusRuntime {
    tune_factors: std::sync::Mutex<Option<astersql_server_handler_tikvhandler::TTLTuneFactors>>,
    pause_scale_in: std::sync::Mutex<Option<astersql_server_handler_tikvhandler::TTLFlag>>,
    max_concurrent_task: std::sync::Mutex<i32>,
    task: std::sync::Mutex<Option<astersql_server_handler_tikvhandler::Task>>,
}

impl Default for ScheduleStatusRuntime {
    fn default() -> Self {
        Self {
            tune_factors: std::sync::Mutex::new(None),
            pause_scale_in: std::sync::Mutex::new(None),
            max_concurrent_task: std::sync::Mutex::new(16),
            task: std::sync::Mutex::new(Some(astersql_server_handler_tikvhandler::Task {
                key: "status-test-task".into(),
                required_slots: 8,
                task_type: "ImportInto".into(),
                extra_params: astersql_server_handler_tikvhandler::ExtraParams {
                    max_runtime_slots: 0,
                    target_steps: Vec::new(),
                },
            })),
        }
    }
}

impl astersql_server_handler_tikvhandler::DxfRuntime for ScheduleStatusRuntime {
    fn now_unix_seconds(&self) -> i64 {
        1_000
    }

    fn parse_duration_seconds(
        &self,
        _value: &str,
    ) -> astersql_server_handler_tikvhandler::DxfResult<i64> {
        match _value {
            "10h" => Ok(10 * 60 * 60),
            value => Err(astersql_server_handler_tikvhandler::DxfError::new(format!(
                "invalid duration {value}"
            ))),
        }
    }

    fn validate_history_page_size(
        &self,
        page_size: i32,
    ) -> astersql_server_handler_tikvhandler::DxfResult<()> {
        if page_size == 1024 || (1..=200).contains(&page_size) {
            Ok(())
        } else {
            Err(astersql_server_handler_tikvhandler::DxfError::new(
                "page size is out of range",
            ))
        }
    }

    fn validate_keyspace_name(
        &self,
        keyspace: &str,
    ) -> astersql_server_handler_tikvhandler::DxfResult<()> {
        if keyspace == "ks.1" {
            Err(astersql_server_handler_tikvhandler::DxfError::new(
                "invalid keyspace",
            ))
        } else {
            Ok(())
        }
    }

    fn get_schedule_status(
        &self,
        context: &astersql_server_handler_tikvhandler::DxfContext,
    ) -> astersql_server_handler_tikvhandler::DxfResult<
        astersql_server_handler_tikvhandler::dxf::JsonValue,
    > {
        assert_eq!(context.deadline_unix_seconds, Some(1_010));
        Ok(astersql_server_handler_tikvhandler::dxf::JsonValue::Object(
            vec![(
                "tidb_worker".into(),
                astersql_server_handler_tikvhandler::dxf::JsonValue::Object(vec![(
                    "required_count".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Integer(1),
                )]),
            )],
        ))
    }

    fn get_active_task_summary(
        &self,
        context: &astersql_server_handler_tikvhandler::DxfContext,
    ) -> astersql_server_handler_tikvhandler::DxfResult<
        astersql_server_handler_tikvhandler::dxf::JsonValue,
    > {
        assert_eq!(context.deadline_unix_seconds, Some(1_010));
        Ok(astersql_server_handler_tikvhandler::dxf::JsonValue::Object(
            vec![
                (
                    "total".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Integer(3),
                ),
                (
                    "per_keyspace".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Object(vec![
                        (
                            "SYSTEM".into(),
                            astersql_server_handler_tikvhandler::dxf::JsonValue::Integer(1),
                        ),
                        (
                            "ks1".into(),
                            astersql_server_handler_tikvhandler::dxf::JsonValue::Integer(2),
                        ),
                    ]),
                ),
            ],
        ))
    }

    fn list_history_tasks(
        &self,
        context: &astersql_server_handler_tikvhandler::DxfContext,
        page_size: i32,
        page_token: i64,
        keyspace: &str,
    ) -> astersql_server_handler_tikvhandler::DxfResult<
        astersql_server_handler_tikvhandler::dxf::JsonValue,
    > {
        assert_eq!(context.deadline_unix_seconds, Some(1_010));
        let fixtures = [
            (5, "history-key-5", "ks1"),
            (4, "history-key-4", "ks3"),
            (3, "history-key-3", "ks1"),
            (2, "history-key-2", "ks2"),
            (1, "history-key-1", "ks1"),
        ];
        let matching: Vec<_> = fixtures
            .into_iter()
            .filter(|(_, _, task_keyspace)| keyspace.is_empty() || *task_keyspace == keyspace)
            .collect();
        let approx_total_count = matching.len() as i64;
        let candidates: Vec<_> = matching
            .into_iter()
            .filter(|(id, _, _)| page_token == 0 || *id < page_token)
            .collect();
        let page_size = page_size as usize;
        let has_more = candidates.len() > page_size;
        let page: Vec<_> = candidates.into_iter().take(page_size).collect();
        let next_page_token = if has_more {
            page.last().map(|(id, _, _)| *id).unwrap_or_default()
        } else {
            0
        };
        let items = page
            .into_iter()
            .map(|(id, key, keyspace)| dxf_history_task_json(id, key, keyspace))
            .collect();
        Ok(astersql_server_handler_tikvhandler::dxf::JsonValue::Object(
            vec![
                (
                    "items".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Array(items),
                ),
                (
                    "has_more".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Bool(has_more),
                ),
                (
                    "next_page_token".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Integer(next_page_token),
                ),
                (
                    "approx_total_count".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Integer(
                        approx_total_count,
                    ),
                ),
            ],
        ))
    }

    fn get_import_history_job(
        &self,
        context: &astersql_server_handler_tikvhandler::DxfContext,
        keyspace: &str,
        job_id: i64,
    ) -> astersql_server_handler_tikvhandler::DxfResult<
        astersql_server_handler_tikvhandler::dxf::JsonValue,
    > {
        assert!(context.internal_dist_task);
        if keyspace == "ks1" && job_id == 9527 {
            Ok(dxf_import_history_job_json(job_id, keyspace))
        } else {
            Err(astersql_server_handler_tikvhandler::DxfError {
                kind: astersql_server_handler_tikvhandler::DxfErrorKind::TaskNotFound,
                message: "not found in history".into(),
            })
        }
    }

    fn update_pause_scale_in_flag(
        &self,
        context: &astersql_server_handler_tikvhandler::DxfContext,
        flag: &astersql_server_handler_tikvhandler::TTLFlag,
    ) -> astersql_server_handler_tikvhandler::DxfResult<()> {
        if context.deadline_unix_seconds != Some(1_010) {
            return Err(astersql_server_handler_tikvhandler::DxfError::new(
                "schedule update did not receive the default request timeout",
            ));
        }
        *self.pause_scale_in.lock().expect("pause scale-in lock") = Some(flag.clone());
        Ok(())
    }

    fn load_keyspace_if_supported(
        &self,
        _storage: &astersql_server_handler_tikvhandler::StorageHandle,
        _context: &astersql_server_handler_tikvhandler::DxfContext,
        _keyspace: &str,
    ) -> astersql_server_handler_tikvhandler::DxfResult<()> {
        if _keyspace == "SYSTEM" {
            Ok(())
        } else {
            Err(astersql_server_handler_tikvhandler::DxfError::new(format!(
                "keyspace {_keyspace} is unavailable"
            )))
        }
    }

    fn get_schedule_tune_factors(
        &self,
        _context: &astersql_server_handler_tikvhandler::DxfContext,
        _keyspace: &str,
    ) -> astersql_server_handler_tikvhandler::DxfResult<
        astersql_server_handler_tikvhandler::TTLTuneFactors,
    > {
        self.tune_factors
            .lock()
            .expect("tune factors lock")
            .clone()
            .ok_or_else(|| {
                astersql_server_handler_tikvhandler::DxfError::new("tune factors missing")
            })
    }

    fn set_schedule_tune_factors_in_new_txn(
        &self,
        _storage: &astersql_server_handler_tikvhandler::StorageHandle,
        context: &astersql_server_handler_tikvhandler::DxfContext,
        keyspace: &str,
        factors: &astersql_server_handler_tikvhandler::TTLTuneFactors,
    ) -> astersql_server_handler_tikvhandler::DxfResult<()> {
        if keyspace != "SYSTEM" || !context.internal_dist_task {
            return Err(astersql_server_handler_tikvhandler::DxfError::new(
                "schedule tune updates require the SYSTEM internal-task context",
            ));
        }
        *self.tune_factors.lock().expect("tune factors lock") = Some(factors.clone());
        Ok(())
    }

    fn min_amplify_factor(&self) -> f64 {
        1.0
    }

    fn max_amplify_factor(&self) -> f64 {
        10.0
    }

    fn set_max_concurrent_task(
        &self,
        value: i32,
    ) -> astersql_server_handler_tikvhandler::DxfResult<()> {
        if !(16..=256).contains(&value) {
            return Err(astersql_server_handler_tikvhandler::DxfError::new(format!(
                "value {value} is out of range [16, 256]"
            )));
        }
        *self
            .max_concurrent_task
            .lock()
            .expect("max concurrent task lock") = value;
        Ok(())
    }

    fn get_max_concurrent_task(&self) -> i32 {
        *self
            .max_concurrent_task
            .lock()
            .expect("max concurrent task lock")
    }

    fn get_task_by_id(
        &self,
        context: &astersql_server_handler_tikvhandler::DxfContext,
        task_id: i64,
    ) -> astersql_server_handler_tikvhandler::DxfResult<astersql_server_handler_tikvhandler::Task>
    {
        if task_id != 1 || !context.internal_dist_task {
            return Err(astersql_server_handler_tikvhandler::DxfError::new(
                "task not found",
            ));
        }
        self.task
            .lock()
            .expect("task lock")
            .clone()
            .ok_or_else(|| astersql_server_handler_tikvhandler::DxfError::new("task not found"))
    }

    fn is_valid_business_step(
        &self,
        task_type: &str,
        step: astersql_server_handler_tikvhandler::Step,
    ) -> bool {
        task_type == "ImportInto" && step.0 == 3
    }

    fn step_to_string(
        &self,
        task_type: &str,
        step: astersql_server_handler_tikvhandler::Step,
    ) -> String {
        if task_type == "ImportInto" && step.0 == 3 {
            "encode".into()
        } else {
            String::new()
        }
    }

    fn update_task_extra_params(
        &self,
        context: &astersql_server_handler_tikvhandler::DxfContext,
        task_id: i64,
        extra: astersql_server_handler_tikvhandler::ExtraParams,
    ) -> astersql_server_handler_tikvhandler::DxfResult<()> {
        if task_id != 1 || !context.internal_dist_task {
            return Err(astersql_server_handler_tikvhandler::DxfError::new(
                "task not found",
            ));
        }
        let mut task = self.task.lock().expect("task lock");
        let Some(task) = task.as_mut() else {
            return Err(astersql_server_handler_tikvhandler::DxfError::new(
                "task not found",
            ));
        };
        task.extra_params = extra;
        Ok(())
    }

    fn log_info(&self, _message: &str) {}

    fn log_warning(&self, _message: &str, _error: &astersql_server_handler_tikvhandler::DxfError) {}
}

#[test]
fn dxf_schedule_status_rejects_non_get_and_returns_the_runtime_payload() {
    use std::sync::Arc;

    let handler = astersql_server_handler_tikvhandler::NewDXFScheduleStatusHandler(Arc::new(
        ScheduleStatusRuntime::default(),
    ));
    let mut writer = DxfResponseRecorder::default();
    handler.ServeHTTP(
        &mut writer,
        &astersql_server_handler_tikvhandler::dxf::Request {
            method: "POST".into(),
            ..Default::default()
        },
    );
    assert_eq!(
        writer.error,
        Some((None, "This api only support GET method".into()))
    );
    assert!(writer.data.is_none());

    let mut writer = DxfResponseRecorder::default();
    handler.ServeHTTP(
        &mut writer,
        &astersql_server_handler_tikvhandler::dxf::Request {
            method: "GET".into(),
            ..Default::default()
        },
    );
    assert_eq!(
        writer.data,
        Some(astersql_server_handler_tikvhandler::dxf::JsonValue::Object(
            vec![(
                "tidb_worker".into(),
                astersql_server_handler_tikvhandler::dxf::JsonValue::Object(vec![(
                    "required_count".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Integer(1),
                ),]),
            ),]
        ))
    );
    assert!(writer.error.is_none());

    let suite = super::http_handler_test::create_basic_http_handler_test_suite();
    let post = suite
        .client
        .post_status(
            "/dxf/schedule/status",
            "application/x-www-form-urlencoded",
            &[],
        )
        .expect("POST must reach the DXF status route");
    assert_eq!(post.status, 400);
    assert!(post.text().unwrap().contains("only support GET"));

    let get = suite
        .client
        .fetch_status("/dxf/schedule/status")
        .expect("GET must reach the DXF status route");
    assert_eq!(get.status, 200);
    assert_eq!(
        get.text().unwrap(),
        r#"{"tidb_worker":{"required_count":1}}"#
    );

    let paused = suite
        .client
        .post_status(
            "/dxf/schedule?action=pause_scale_in",
            "application/x-www-form-urlencoded",
            &[],
        )
        .expect("pause action query must be visible through Go FormValue semantics");
    assert_eq!(paused.status, 200);
    assert_eq!(
        paused.text().unwrap(),
        r#"{"enabled":true,"ttl":3600,"expire_time":4600}"#
    );

    let current = suite
        .client
        .fetch_status("/dxf/schedule/max_concurrent_task")
        .expect("GET max concurrent task must reach the DXF route");
    assert_eq!(current.status, 200);
    assert_eq!(
        current.text().unwrap(),
        r#"{"max_concurrent_task":16,"persistence":"memory_only"}"#
    );
    let changed = suite
        .client
        .post_status(
            "/dxf/schedule/max_concurrent_task?value=128",
            "application/x-www-form-urlencoded",
            &[],
        )
        .expect("POST max concurrent task must read the query value");
    assert_eq!(changed.status, 200);
    assert_eq!(
        changed.text().unwrap(),
        r#"{"max_concurrent_task":128,"persistence":"memory_only"}"#
    );

    let history = suite
        .client
        .fetch_status("/dxf/import-into/history/job/ks1/9527")
        .expect("import history must pass route variables to the DXF handler");
    assert_eq!(history.status, 200);
    let history = history.text().unwrap();
    assert!(history.contains(r#""job_id":9527"#));
    assert!(history.contains(r#""task_id":42"#));
    assert!(history.contains(r#""total":"40m0s""#));
    let missing = suite
        .client
        .fetch_status("/dxf/import-into/history/job/ks1/9528")
        .expect("missing import history must return an HTTP response");
    assert_eq!(missing.status, 404);
    assert!(missing.text().unwrap().contains("not found in history"));

    let tuned = suite
        .client
        .post_status(
            "/dxf/schedule/tune",
            "application/x-www-form-urlencoded",
            b"keyspace=SYSTEM&amplify_factor=2&ttl=10h",
        )
        .expect("schedule tune must parse POST form data");
    assert_eq!(tuned.status, 200);
    assert_eq!(
        tuned.text().unwrap(),
        r#"{"ttl":36000,"expire_time":37000,"amplify_factor":2.0}"#
    );

    let slots = suite
        .client
        .post_status(
            "/dxf/task/1/max_runtime_slots",
            "application/x-www-form-urlencoded",
            b"value=6&target_step=3",
        )
        .expect("runtime-slot update must parse route and form parameters");
    assert_eq!(slots.status, 200);
    assert_eq!(
        slots.text().unwrap(),
        r#"{"task_id":1,"task_key":"status-test-task","required_slots":8,"max_runtime_slots":6,"target_steps":["encode"]}"#
    );

    let active = suite
        .client
        .fetch_status("/dxf/task/active")
        .expect("active-task query must use the runtime summary");
    assert_eq!(active.status, 200);
    assert_eq!(
        active.text().unwrap(),
        r#"{"total":3,"per_keyspace":{"SYSTEM":1,"ks1":2}}"#
    );
    let page = suite
        .client
        .fetch_status("/dxf/task/history?page_size=2&page_token=9&keyspace=ks1")
        .expect("history query must preserve all pagination parameters");
    assert_eq!(page.status, 200);
    let page = page.text().unwrap();
    assert!(page.contains(r#""id":5"#));
    assert!(page.contains(r#""id":3"#));
    assert!(page.contains(r#""approx_total_count":3"#));
}

fn dxf_form_request(
    method: &str,
    fields: &[(&str, &str)],
) -> astersql_server_handler_tikvhandler::dxf::Request {
    let form = fields
        .iter()
        .map(|(name, value)| ((*name).into(), vec![(*value).into()]))
        .collect();
    astersql_server_handler_tikvhandler::dxf::Request {
        method: method.into(),
        form,
        ..Default::default()
    }
}

fn dxf_json_field<'a>(
    value: &'a astersql_server_handler_tikvhandler::dxf::JsonValue,
    name: &str,
) -> &'a astersql_server_handler_tikvhandler::dxf::JsonValue {
    let astersql_server_handler_tikvhandler::dxf::JsonValue::Object(fields) = value else {
        panic!("expected JSON object, got {value:?}");
    };
    fields
        .iter()
        .find_map(|(field, value)| (field == name).then_some(value))
        .unwrap_or_else(|| panic!("missing JSON field {name:?} in {value:?}"))
}

fn dxf_json_integer(
    value: &astersql_server_handler_tikvhandler::dxf::JsonValue,
    name: &str,
) -> i64 {
    let value = dxf_json_field(value, name);
    let astersql_server_handler_tikvhandler::dxf::JsonValue::Integer(value) = value else {
        panic!("expected integer field {name:?}, got {value:?}");
    };
    *value
}

fn dxf_json_string<'a>(
    value: &'a astersql_server_handler_tikvhandler::dxf::JsonValue,
    name: &str,
) -> &'a str {
    let value = dxf_json_field(value, name);
    let astersql_server_handler_tikvhandler::dxf::JsonValue::String(value) = value else {
        panic!("expected string field {name:?}, got {value:?}");
    };
    value
}

#[test]
fn dxf_schedule_tune_validates_and_persists_the_go_contract() {
    use std::sync::Arc;

    let runtime: Arc<dyn astersql_server_handler_tikvhandler::DxfRuntime> =
        Arc::new(ScheduleStatusRuntime::default());
    let handler = astersql_server_handler_tikvhandler::NewDXFScheduleTuneHandler(
        astersql_server_handler_tikvhandler::StorageHandle("test-store".into()),
        Arc::clone(&runtime),
    );

    let mut writer = DxfResponseRecorder::default();
    handler.ServeHTTP(&mut writer, &dxf_form_request("GET", &[]));
    assert!(
        writer
            .error
            .as_ref()
            .is_some_and(|(_, message)| message.contains("invalid or empty target keyspace"))
    );

    for amplify_factor in ["0.9", "10.1"] {
        let mut writer = DxfResponseRecorder::default();
        handler.ServeHTTP(
            &mut writer,
            &dxf_form_request(
                "POST",
                &[("keyspace", "SYSTEM"), ("amplify_factor", amplify_factor)],
            ),
        );
        assert!(
            writer
                .error
                .as_ref()
                .is_some_and(|(_, message)| message.contains("is out of range")),
            "amplify factor {amplify_factor} must be rejected"
        );
    }

    let mut writer = DxfResponseRecorder::default();
    handler.ServeHTTP(
        &mut writer,
        &dxf_form_request(
            "POST",
            &[
                ("keyspace", "SYSTEM"),
                ("amplify_factor", "2"),
                ("ttl", "10h"),
            ],
        ),
    );
    assert_eq!(
        writer.data,
        Some(astersql_server_handler_tikvhandler::dxf::JsonValue::Object(
            vec![
                (
                    "ttl".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Integer(36_000),
                ),
                (
                    "expire_time".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Integer(37_000),
                ),
                (
                    "amplify_factor".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Float(2.0),
                ),
            ]
        ))
    );
    assert!(writer.error.is_none());

    let mut writer = DxfResponseRecorder::default();
    handler.ServeHTTP(
        &mut writer,
        &dxf_form_request("GET", &[("keyspace", "SYSTEM")]),
    );
    assert_eq!(
        writer.data,
        Some(astersql_server_handler_tikvhandler::dxf::JsonValue::Object(
            vec![
                (
                    "ttl".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Integer(36_000),
                ),
                (
                    "expire_time".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Integer(37_000),
                ),
                (
                    "amplify_factor".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Float(2.0),
                ),
            ]
        ))
    );
    assert!(writer.error.is_none());
}

#[test]
fn dxf_schedule_pause_and_resume_preserve_go_ttl_semantics() {
    use std::sync::Arc;

    let runtime: Arc<dyn astersql_server_handler_tikvhandler::DxfRuntime> =
        Arc::new(ScheduleStatusRuntime::default());
    let handler = astersql_server_handler_tikvhandler::NewDXFScheduleHandler(runtime);

    let mut writer = DxfResponseRecorder::default();
    handler.ServeHTTP(&mut writer, &dxf_form_request("GET", &[]));
    assert!(
        writer
            .error
            .as_ref()
            .is_some_and(|(_, message)| message.contains("only support POST method"))
    );

    let mut writer = DxfResponseRecorder::default();
    handler.ServeHTTP(
        &mut writer,
        &dxf_form_request("POST", &[("action", "pause_scale_in")]),
    );
    assert_eq!(
        writer.data,
        Some(astersql_server_handler_tikvhandler::dxf::JsonValue::Object(
            vec![
                (
                    "enabled".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Bool(true),
                ),
                (
                    "ttl".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Integer(3_600),
                ),
                (
                    "expire_time".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Integer(4_600),
                ),
            ]
        ))
    );

    let mut writer = DxfResponseRecorder::default();
    handler.ServeHTTP(
        &mut writer,
        &dxf_form_request("POST", &[("action", "resume_scale_in")]),
    );
    assert_eq!(
        writer.data,
        Some(astersql_server_handler_tikvhandler::dxf::JsonValue::Object(
            Vec::new()
        ))
    );
    assert!(writer.error.is_none());
}

#[test]
fn dxf_max_concurrent_task_validates_bounds_and_persists_the_value() {
    use std::sync::Arc;

    let runtime: Arc<dyn astersql_server_handler_tikvhandler::DxfRuntime> =
        Arc::new(ScheduleStatusRuntime::default());
    let handler = astersql_server_handler_tikvhandler::NewDXFTaskMaxConcurrentHandler(runtime);

    let mut writer = DxfResponseRecorder::default();
    handler.ServeHTTP(&mut writer, &dxf_form_request("DELETE", &[]));
    assert_eq!(
        writer.error,
        Some((None, "This api only support GET and POST method".into()))
    );

    for (value, expected_error) in [
        ("", "invalid value"),
        ("aa", "invalid value"),
        ("15", "out of range"),
        ("257", "out of range"),
    ] {
        let mut writer = DxfResponseRecorder::default();
        handler.ServeHTTP(&mut writer, &dxf_form_request("POST", &[("value", value)]));
        assert!(
            writer
                .error
                .as_ref()
                .is_some_and(|(_, message)| message.contains(expected_error)),
            "value {value} must be rejected"
        );
    }

    let mut writer = DxfResponseRecorder::default();
    handler.ServeHTTP(&mut writer, &dxf_form_request("GET", &[]));
    assert_eq!(
        writer.data,
        Some(astersql_server_handler_tikvhandler::dxf::JsonValue::Object(
            vec![
                (
                    "max_concurrent_task".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Integer(16),
                ),
                (
                    "persistence".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::String(
                        "memory_only".into()
                    ),
                ),
            ]
        ))
    );

    let mut writer = DxfResponseRecorder::default();
    handler.ServeHTTP(&mut writer, &dxf_form_request("POST", &[("value", "128")]));
    assert_eq!(
        writer.data,
        Some(astersql_server_handler_tikvhandler::dxf::JsonValue::Object(
            vec![
                (
                    "max_concurrent_task".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Integer(128),
                ),
                (
                    "persistence".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::String(
                        "memory_only".into()
                    ),
                ),
            ]
        ))
    );

    let mut writer = DxfResponseRecorder::default();
    handler.ServeHTTP(&mut writer, &dxf_form_request("GET", &[]));
    assert_eq!(
        dxf_json_integer(writer.data.as_ref().unwrap(), "max_concurrent_task"),
        128
    );
}

#[test]
fn dxf_max_runtime_slots_preserves_go_validation_and_task_update_contract() {
    use std::sync::Arc;

    let runtime = Arc::new(ScheduleStatusRuntime::default());
    *runtime.task.lock().expect("task lock") = Some(astersql_server_handler_tikvhandler::Task {
        key: "key1".into(),
        required_slots: 8,
        task_type: "ImportInto".into(),
        extra_params: astersql_server_handler_tikvhandler::ExtraParams {
            max_runtime_slots: 0,
            target_steps: Vec::new(),
        },
    });
    let handler =
        astersql_server_handler_tikvhandler::NewDXFTaskMaxRuntimeSlotsHandler(runtime.clone());

    let request = |method: &str, task_id: &str, values: &[(&str, &str)]| {
        let mut request = dxf_form_request(method, values);
        request.path.insert("taskID".into(), task_id.into());
        request
    };

    for (request, expected_error) in [
        (request("GET", "1", &[]), "only support POST method"),
        (request("POST", "0", &[("value", "1")]), "invalid task ID"),
        (request("POST", "aa", &[("value", "1")]), "invalid task ID"),
        (request("POST", "1", &[]), "invalid value"),
        (request("POST", "1", &[("value", "aa")]), "invalid value"),
        (request("POST", "1", &[("value", "0")]), "invalid value"),
        (
            request("POST", "1", &[("value", "1"), ("target_step", "a")]),
            "invalid target step",
        ),
        (
            request("POST", "1123123", &[("value", "1"), ("target_step", "1")]),
            "task not found",
        ),
        (
            request("POST", "1", &[("value", "10")]),
            "max runtime slots should be less than required slots(8)",
        ),
        (
            request("POST", "1", &[("value", "6"), ("target_step", "100")]),
            "invalid target step 100 for task type ImportInto",
        ),
    ] {
        let mut writer = DxfResponseRecorder::default();
        handler.ServeHTTP(&mut writer, &request);
        assert!(
            writer
                .error
                .as_ref()
                .is_some_and(|(_, message)| message.contains(expected_error)),
            "{expected_error} must be returned for {request:?}"
        );
    }

    let mut invalid_steps_request = request("POST", "1", &[("value", "1")]);
    invalid_steps_request
        .form
        .insert("target_step".into(), vec!["1".into(), "aa".into()]);
    let mut writer = DxfResponseRecorder::default();
    handler.ServeHTTP(&mut writer, &invalid_steps_request);
    assert!(
        writer
            .error
            .as_ref()
            .is_some_and(|(_, message)| message.contains("invalid target step aa"))
    );

    let mut writer = DxfResponseRecorder::default();
    handler.ServeHTTP(
        &mut writer,
        &request("POST", "1", &[("value", "6"), ("target_step", "3")]),
    );
    assert_eq!(
        writer.data,
        Some(astersql_server_handler_tikvhandler::dxf::JsonValue::Object(
            vec![
                (
                    "task_id".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Integer(1)
                ),
                (
                    "task_key".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::String("key1".into())
                ),
                (
                    "required_slots".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Integer(8)
                ),
                (
                    "max_runtime_slots".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Integer(6)
                ),
                (
                    "target_steps".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Array(vec![
                        astersql_server_handler_tikvhandler::dxf::JsonValue::String(
                            "encode".into()
                        )
                    ])
                ),
            ]
        ))
    );
    let task = runtime
        .task
        .lock()
        .expect("task lock")
        .clone()
        .expect("task exists");
    assert_eq!(task.extra_params.max_runtime_slots, 6);
    assert_eq!(
        task.extra_params.target_steps,
        [astersql_server_handler_tikvhandler::Step(3)]
    );

    let mut writer = DxfResponseRecorder::default();
    handler.ServeHTTP(&mut writer, &request("POST", "1", &[("value", "6")]));
    assert!(matches!(
        writer.data,
        Some(astersql_server_handler_tikvhandler::dxf::JsonValue::Object(ref fields))
            if fields.iter().any(|(name, value)| name == "target_steps" && value == &astersql_server_handler_tikvhandler::dxf::JsonValue::Array(Vec::new()))
    ));
    assert!(
        runtime
            .task
            .lock()
            .expect("task lock")
            .as_ref()
            .expect("task exists")
            .extra_params
            .target_steps
            .is_empty()
    );
}

#[test]
fn dxf_active_task_history_and_import_history_preserve_query_validation() {
    use std::sync::Arc;

    let runtime: Arc<dyn astersql_server_handler_tikvhandler::DxfRuntime> =
        Arc::new(ScheduleStatusRuntime::default());
    let active = astersql_server_handler_tikvhandler::NewDXFActiveTaskHandler(Arc::clone(&runtime));
    let history =
        astersql_server_handler_tikvhandler::NewDXFTaskHistoryHandler(Arc::clone(&runtime));
    let import =
        astersql_server_handler_tikvhandler::NewDXFImportIntoHistoryJobInfoHandler(runtime);

    let mut writer = DxfResponseRecorder::default();
    active.ServeHTTP(&mut writer, &dxf_form_request("POST", &[]));
    assert_eq!(
        writer.error,
        Some((None, "This api only support GET method".into()))
    );
    let mut writer = DxfResponseRecorder::default();
    active.ServeHTTP(&mut writer, &dxf_form_request("GET", &[]));
    assert_eq!(
        writer.data,
        Some(astersql_server_handler_tikvhandler::dxf::JsonValue::Object(
            vec![
                (
                    "total".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Integer(3),
                ),
                (
                    "per_keyspace".into(),
                    astersql_server_handler_tikvhandler::dxf::JsonValue::Object(vec![
                        (
                            "SYSTEM".into(),
                            astersql_server_handler_tikvhandler::dxf::JsonValue::Integer(1),
                        ),
                        (
                            "ks1".into(),
                            astersql_server_handler_tikvhandler::dxf::JsonValue::Integer(2),
                        ),
                    ]),
                ),
            ]
        ))
    );

    for (query, expected_error) in [
        ([("page_size", "0")], "invalid page_size 0"),
        ([("page_size", "201")], "invalid page_size 201"),
        ([("page_size", "aa")], "invalid page_size aa"),
        ([("page_token", "0")], "invalid page_token 0"),
        ([("page_token", "aa")], "invalid page_token aa"),
        ([("keyspace", "ks.1")], "invalid keyspace ks.1"),
    ] {
        let mut request = dxf_form_request("GET", &[]);
        request.query = query
            .into_iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect();
        let mut writer = DxfResponseRecorder::default();
        history.ServeHTTP(&mut writer, &request);
        assert!(
            writer
                .error
                .as_ref()
                .is_some_and(|(_, message)| message == expected_error),
            "{expected_error} must be preserved"
        );
    }
    let mut writer = DxfResponseRecorder::default();
    let mut request = dxf_form_request("GET", &[]);
    request.query = [
        ("page_size".into(), "2".into()),
        ("page_token".into(), "9".into()),
        ("keyspace".into(), "ks1".into()),
    ]
    .into_iter()
    .collect();
    history.ServeHTTP(&mut writer, &request);
    let page = writer.data.expect("valid history query must return a page");
    assert_eq!(dxf_json_integer(&page, "approx_total_count"), 3);
    let astersql_server_handler_tikvhandler::dxf::JsonValue::Array(items) =
        dxf_json_field(&page, "items")
    else {
        panic!("history items must be an array");
    };
    assert_eq!(
        items
            .iter()
            .map(|item| dxf_json_integer(item, "id"))
            .collect::<Vec<_>>(),
        [5, 3]
    );

    let mut writer = DxfResponseRecorder::default();
    import.ServeHTTP(&mut writer, &dxf_form_request("POST", &[]));
    assert_eq!(
        writer.error,
        Some((None, "This api only support GET method".into()))
    );
    let mut request = dxf_form_request("GET", &[]);
    request.path = [
        ("keyspace".into(), "ks1".into()),
        ("job_id".into(), "9527".into()),
    ]
    .into_iter()
    .collect();
    let mut writer = DxfResponseRecorder::default();
    import.ServeHTTP(&mut writer, &request);
    let info = writer
        .data
        .expect("valid import history query must return job info");
    assert_eq!(dxf_json_integer(&info, "job_id"), 9527);
    assert_eq!(dxf_json_integer(&info, "task_id"), 42);
    assert_eq!(dxf_json_string(&info, "keyspace"), "ks1");
    let mut writer = DxfResponseRecorder::default();
    let mut missing = request;
    missing.path.insert("job_id".into(), "9528".into());
    import.ServeHTTP(&mut writer, &missing);
    assert_eq!(
        writer.error,
        Some((Some(404), "not found in history".into()))
    );
}

#[test]
fn dxf_task_history_matches_go_pagination_and_keyspace_filtering() {
    use std::sync::Arc;

    let runtime: Arc<dyn astersql_server_handler_tikvhandler::DxfRuntime> =
        Arc::new(ScheduleStatusRuntime::default());
    let handler = astersql_server_handler_tikvhandler::NewDXFTaskHistoryHandler(runtime);
    let fetch_page = |query: &[(&str, &str)]| {
        let mut request = dxf_form_request("GET", &[]);
        request.query = query
            .iter()
            .map(|(name, value)| ((*name).into(), (*value).into()))
            .collect();
        let mut writer = DxfResponseRecorder::default();
        handler.ServeHTTP(&mut writer, &request);
        assert!(writer.error.is_none(), "history query must succeed");
        writer.data.expect("history query must return a page")
    };
    let assert_page = |page: &astersql_server_handler_tikvhandler::dxf::JsonValue,
                       expected_ids: &[i64],
                       expected_total: i64,
                       expected_has_more: bool,
                       expected_token: i64| {
        let astersql_server_handler_tikvhandler::dxf::JsonValue::Array(items) =
            dxf_json_field(page, "items")
        else {
            panic!("history items must be a JSON array: {page:?}");
        };
        let ids: Vec<_> = items
            .iter()
            .map(|item| dxf_json_integer(item, "id"))
            .collect();
        assert_eq!(ids, expected_ids);
        assert_eq!(dxf_json_integer(page, "approx_total_count"), expected_total);
        assert_eq!(dxf_json_integer(page, "next_page_token"), expected_token);
        assert_eq!(
            dxf_json_field(page, "has_more"),
            &astersql_server_handler_tikvhandler::dxf::JsonValue::Bool(expected_has_more)
        );
    };

    let first = fetch_page(&[("page_size", "2")]);
    assert_page(&first, &[5, 4], 5, true, 4);
    let first_item = match dxf_json_field(&first, "items") {
        astersql_server_handler_tikvhandler::dxf::JsonValue::Array(items) => &items[0],
        value => panic!("history items must be an array, got {value:?}"),
    };
    assert_eq!(dxf_json_string(first_item, "key"), "history-key-5");
    assert_eq!(dxf_json_string(first_item, "state"), "succeed");
    assert!(!dxf_json_string(first_item, "start_time").is_empty());
    assert!(!dxf_json_string(first_item, "state_update_time").is_empty());
    assert!(!dxf_json_string(first_item, "end_time").is_empty());

    let second = fetch_page(&[("page_size", "2"), ("page_token", "4")]);
    assert_page(&second, &[3, 2], 5, true, 2);
    let last = fetch_page(&[("page_size", "2"), ("page_token", "2")]);
    assert_page(&last, &[1], 5, false, 0);

    let filtered = fetch_page(&[("page_size", "2"), ("keyspace", "ks1")]);
    assert_page(&filtered, &[5, 3], 3, true, 3);
    let astersql_server_handler_tikvhandler::dxf::JsonValue::Array(items) =
        dxf_json_field(&filtered, "items")
    else {
        panic!("filtered history items must be an array");
    };
    assert!(
        items
            .iter()
            .all(|item| dxf_json_string(item, "keyspace") == "ks1")
    );
    let filtered_last = fetch_page(&[("page_size", "2"), ("keyspace", "ks1"), ("page_token", "3")]);
    assert_page(&filtered_last, &[1], 3, false, 0);
}

#[test]
fn dxf_import_history_matches_go_aggregated_job_fields() {
    use std::sync::Arc;

    let runtime: Arc<dyn astersql_server_handler_tikvhandler::DxfRuntime> =
        Arc::new(ScheduleStatusRuntime::default());
    let handler =
        astersql_server_handler_tikvhandler::NewDXFImportIntoHistoryJobInfoHandler(runtime);
    let mut request = dxf_form_request("GET", &[]);
    request.path = [
        ("keyspace".into(), "ks1".into()),
        ("job_id".into(), "9527".into()),
    ]
    .into_iter()
    .collect();
    let mut writer = DxfResponseRecorder::default();
    handler.ServeHTTP(&mut writer, &request);
    assert!(writer.error.is_none());
    let info = writer.data.expect("history job must return detailed info");

    assert_eq!(dxf_json_integer(&info, "job_id"), 9527);
    assert_eq!(dxf_json_integer(&info, "task_id"), 42);
    assert_eq!(dxf_json_string(&info, "keyspace"), "ks1");
    assert_eq!(dxf_json_integer(&info, "distsql_scan_concurrency"), 16);
    assert_eq!(dxf_json_integer(&info, "index_count"), 2);
    assert_eq!(dxf_json_integer(&info, "column_count"), 3);
    assert_eq!(dxf_json_integer(&info, "row_count"), 1024);
    let duration = dxf_json_field(&info, "duration");
    assert_eq!(dxf_json_string(duration, "total"), "40m0s");
    assert_eq!(dxf_json_string(duration, "encode"), "10m0s");
    assert_eq!(dxf_json_string(duration, "ingest"), "30m0s");
    assert_eq!(dxf_json_string(duration, "post_process"), "");
}

#[test]
// TestDXFAPI 对应 Go 函数 `func TestDXFAPI(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestDXFAPI 对应 Go 函数 `func TestDXFAPI(t *testing.T) {`。
pub fn test_dxfapi() {
    let suite = super::http_handler_test::create_basic_http_handler_test_suite();
    for (path, expected) in [
        ("/dxf/schedule/status", "only support GET method"),
        ("/dxf/task/active", "only support GET method"),
        ("/dxf/task/history", "only support GET method"),
        (
            "/dxf/import-into/history/job/ks1/9527",
            "only support GET method",
        ),
    ] {
        let response = suite
            .client
            .post_status(path, "application/x-www-form-urlencoded", &[])
            .expect("DXF method rejection must reach the status listener");
        assert_eq!(response.status, 400, "{path}");
        assert!(response.text().unwrap().contains(expected), "{path}");
    }
    let response = suite
        .client
        .fetch_status("/dxf/schedule")
        .expect("DXF schedule method rejection must reach the status listener");
    assert_eq!(response.status, 400);
    assert!(
        response
            .text()
            .unwrap()
            .contains("only support POST method")
    );

    let response = suite
        .client
        .post_status(
            "/dxf/schedule?action=unsupported",
            "application/x-www-form-urlencoded",
            &[],
        )
        .expect("DXF schedule action validation must reach the status listener");
    assert_eq!(response.status, 400);
    assert!(
        response
            .text()
            .unwrap()
            .contains("invalid action unsupported")
    );

    for (path, expected) in [
        ("/dxf/task/history?page_size=0", "invalid page_size 0"),
        ("/dxf/task/history?page_size=201", "invalid page_size 201"),
        ("/dxf/task/history?page_size=aa", "invalid page_size aa"),
        ("/dxf/task/history?page_token=0", "invalid page_token 0"),
        ("/dxf/task/history?page_token=aa", "invalid page_token aa"),
        ("/dxf/task/history?keyspace=ks.1", "invalid keyspace ks.1"),
        (
            "/dxf/import-into/history/job/ks1/invalid",
            "invalid job id invalid",
        ),
        (
            "/dxf/import-into/history/job/ks.1/9527",
            "invalid or empty target keyspace ks.1",
        ),
        (
            "/dxf/schedule/tune?keyspace=unknown",
            "failed to load keyspace unknown",
        ),
    ] {
        let response = suite
            .client
            .fetch_status(path)
            .expect("DXF validation request must reach the status listener");
        assert_eq!(response.status, 400, "{path}");
        assert!(response.text().unwrap().contains(expected), "{path}");
    }
    for (path, expected) in [
        ("/dxf/schedule/max_concurrent_task?value=15", "out of range"),
        (
            "/dxf/schedule/max_concurrent_task?value=aa",
            "invalid value",
        ),
        (
            "/dxf/schedule/tune?keyspace=SYSTEM&amplify_factor=0.9",
            "is out of range",
        ),
        ("/dxf/task/1/max_runtime_slots", "invalid value"),
    ] {
        let response = suite
            .client
            .post_status(path, "application/x-www-form-urlencoded", &[])
            .expect("DXF POST validation must reach the status listener");
        assert_eq!(response.status, 400, "{path}");
        assert!(response.text().unwrap().contains(expected), "{path}");
    }
    let response = suite
        .client
        .fetch_status("/dxf/task/1/max_runtime_slots")
        .expect("runtime-slot method rejection must reach the status listener");
    assert_eq!(response.status, 400);
    assert!(
        response
            .text()
            .unwrap()
            .contains("only support POST method")
    );

    let response = suite
        .client
        .fetch_status("/dxf/schedule/status")
        .expect("DXF schedule status must be available on the status listener");
    assert_eq!(response.status, 200);
    assert_eq!(
        response.text().unwrap(),
        r#"{"tidb_worker":{"required_count":1}}"#
    );

    let response = suite
        .client
        .fetch_status("/dxf/task/active")
        .expect("DXF active-task summary must be available on the status listener");
    assert_eq!(response.status, 200);
    assert_eq!(
        response.text().unwrap(),
        r#"{"total":3,"per_keyspace":{"SYSTEM":1,"ks1":2}}"#
    );

    let response = suite
        .client
        .fetch_status("/dxf/task/history?page_size=2&page_token=9&keyspace=ks1")
        .expect("DXF history must preserve pagination and keyspace parameters");
    assert_eq!(response.status, 200);
    let response = response.text().unwrap();
    assert!(response.contains(r#""id":5"#));
    assert!(response.contains(r#""id":3"#));
    assert!(response.contains(r#""approx_total_count":3"#));

    let response = suite
        .client
        .fetch_status("/dxf/import-into/history/job/ks1/9527")
        .expect("DXF import history must return its persisted job information");
    assert_eq!(response.status, 200);
    let response = response.text().unwrap();
    assert!(response.contains(r#""job_id":9527"#));
    assert!(response.contains(r#""task_id":42"#));
    assert!(response.contains(r#""distsql_scan_concurrency":16"#));

    let response = suite
        .client
        .fetch_status("/dxf/import-into/history/job/ks1/9528")
        .expect("missing DXF import history must produce an HTTP response");
    assert_eq!(response.status, 404);
    assert!(response.text().unwrap().contains("not found in history"));
}

#[test]
// TestDXFScheduleTuneAPI 对应 Go 函数 `func TestDXFScheduleTuneAPI(t *testing.T) {`。
// 这是 HTTP/status 集成测试：通过真实 TCP 与带状态运行时保留请求、断言和清理语义。
/// TestDXFScheduleTuneAPI 对应 Go 函数 `func TestDXFScheduleTuneAPI(t *testing.T) {`。
pub fn test_dxf_schedule_tune_api() {
    let suite = super::http_handler_test::create_basic_http_handler_test_suite();
    let response = suite
        .client
        .fetch_status("/dxf/schedule/tune")
        .expect("empty DXF tune request must produce an HTTP response");
    assert_eq!(response.status, 400);
    assert!(
        response
            .text()
            .unwrap()
            .contains("invalid or empty target keyspace")
    );

    let response = suite
        .client
        .fetch_status("/dxf/schedule/tune?keyspace=aaa")
        .expect("unknown keyspace must produce an HTTP response");
    assert_eq!(response.status, 400);
    assert!(response.text().unwrap().contains("failed to load keyspace"));

    for amplify_factor in ["0.9", "10.1"] {
        let response = suite
            .client
            .post_status(
                &format!("/dxf/schedule/tune?keyspace=SYSTEM&amplify_factor={amplify_factor}"),
                "application/x-www-form-urlencoded",
                &[],
            )
            .expect("out-of-range amplify factor must produce an HTTP response");
        assert_eq!(response.status, 400);
        assert!(response.text().unwrap().contains("is out of range"));
    }

    let response = suite
        .client
        .post_status(
            "/dxf/schedule/tune?keyspace=SYSTEM&amplify_factor=2&ttl=10h",
            "application/x-www-form-urlencoded",
            &[],
        )
        .expect("DXF tune POST must persist TTL and amplification");
    assert_eq!(response.status, 200);
    assert_eq!(
        response.text().unwrap(),
        r#"{"ttl":36000,"expire_time":37000,"amplify_factor":2.0}"#
    );

    let response = suite
        .client
        .fetch_status("/dxf/schedule/tune?keyspace=SYSTEM")
        .expect("DXF tune GET must read the persisted factor");
    assert_eq!(response.status, 200);
    assert!(response.text().unwrap().contains("\"amplify_factor\":2.0"));
}

#[test]
/// dxf default tune factor uses scheduler bounds。
fn dxf_default_tune_factor_uses_scheduler_bounds() {
    use astersql_dxf_framework_schstatus::{
        GetDefaultTuneFactors, MaxAmplifyFactor, MinAmplifyFactor,
    };

    let factors = GetDefaultTuneFactors();
    assert_eq!(factors.AmplifyFactor, MinAmplifyFactor);
    assert!(factors.AmplifyFactor <= MaxAmplifyFactor);
}

#[test]
fn canonical_history_http_preserves_failed_tasks() {
    use astersql_dxf_framework_storage::{self as storage, proto};
    use astersql_server::server::{Server, ServerConfig, ServerDriver, StatusConfig};
    use std::sync::Arc;
    struct Driver;
    impl ServerDriver for Driver {
        fn name(&self) -> &str {
            "history"
        }
    }
    use astersql_store_mockstore_mockstorage::{KVStore, KeyspaceMeta, NewMockStorage};
    let store = Arc::try_unwrap(
        NewMockStorage(
            KVStore::NewMemoryWithWallClockTSO(),
            Some(KeyspaceMeta {
                Name: "SYSTEM".into(),
                ..Default::default()
            }),
        )
        .unwrap(),
    )
    .ok()
    .unwrap();
    let domain = Arc::new(astersql_domain::Domain::new(
        store,
        Arc::new(astersql_domain::canonical_domain::KvInfoSchemaLoader::new()),
        astersql_domain::DomainConfig {
            schema_lease: std::time::Duration::ZERO,
            stats_lease: std::time::Duration::ZERO,
            ..Default::default()
        },
    ));
    domain.init().unwrap();
    let session = astersql_session::runtime::BootstrapCanonicalDomain(domain.clone()).unwrap();

    let manager = session.ImportTaskManager().unwrap();
    manager
        .InitMeta((), "127.0.0.1:4000".into(), "background".into())
        .unwrap();
    let mut ids = Vec::new();
    let mut tasks = Vec::new();
    for (index, keyspace) in ["ks1", "ks2", "ks1", "ks3", "ks1"].iter().enumerate() {
        let id = manager
            .CreateTask(
                (),
                format!("history-key-{}", index + 1),
                proto::ImportInto,
                (*keyspace).into(),
                8,
                "".into(),
                0,
                proto::ExtraParams::default(),
                b"test".to_vec(),
            )
            .unwrap();
        let task = manager.GetTaskByID((), id).unwrap();
        manager
            .SwitchTaskStep((), task, proto::TaskStateRunning, proto::StepOne, vec![])
            .unwrap();
        if index == 4 {
            manager
                .FailTask(
                    (),
                    id,
                    proto::TaskStateRunning,
                    storage::Error::new("history task failed: secret"),
                )
                .unwrap();
        } else {
            manager.SucceedTask((), id).unwrap();
        }
        tasks.push(manager.GetTaskByID((), id).unwrap());
        ids.push(id);
    }
    manager.TransferTasks2History((), tasks).unwrap();
    drop(manager);
    drop(session);
    let server = Server::new(
        ServerConfig {
            host: "127.0.0.1".into(),
            port: 0,
            status: StatusConfig {
                report_status: true,
                host: "127.0.0.1".into(),
                port: 0,
                ..Default::default()
            },
            ..Default::default()
        },
        Arc::new(Driver),
    )
    .unwrap();
    server
        .run(Arc::new(
            astersql_server::runtime::CanonicalServerDomain::new(domain),
        ))
        .unwrap();
    struct Close(Arc<Server>);
    impl Drop for Close {
        fn drop(&mut self) {
            self.0.close();
        }
    }
    let _close = Close(server.clone());
    let address = server.status_listener_addr().unwrap();
    let request = |method: &str, query: &str| {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
            .unwrap();
        write!(
            stream,
            "{method} /dxf/task/history{query} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
        )
        .unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    };
    if !astersql_config_kerneltype::IsNextGen() {
        assert!(request("GET", "?page_size=2").starts_with("HTTP/1.1 404"));
        return;
    }
    for (method, query, message) in [
        ("POST", "", "only support GET method"),
        ("GET", "?page_size=0", "invalid page_size 0"),
        ("GET", "?page_size=201", "invalid page_size 201"),
        ("GET", "?page_size=aa", "invalid page_size aa"),
        ("GET", "?page_token=0", "invalid page_token 0"),
        ("GET", "?page_token=aa", "invalid page_token aa"),
        ("GET", "?keyspace=ks.1", "invalid keyspace ks.1"),
    ] {
        let response = request(method, query);
        assert!(response.starts_with("HTTP/1.1 400"), "{response}");
        assert!(response.contains(message), "{response}");
    }
    let fetch = |query: &str| {
        let response = request("GET", query);
        assert!(response.starts_with("HTTP/1.1 200"), "{response}");
        assert!(!response.contains("secret"));
        serde_json::from_str::<serde_json::Value>(response.split_once("\r\n\r\n").unwrap().1)
            .unwrap()
    };
    let first = fetch("?page_size=2");
    assert_eq!(first["ApproxTotalCount"], 5);
    assert_eq!(first["HasMore"], true);
    assert_eq!(first["NextPageToken"], ids[3]);
    assert_eq!(first["Items"][0]["ID"], ids[4]);
    assert_eq!(first["Items"][0]["State"], "failed");
    assert_eq!(first["Items"][0]["ErrorCategory"], "failed");
    assert_eq!(first["Items"][0]["ErrorCode"], "");
    assert_eq!(first["Items"][1]["ErrorCategory"], "");
    assert!(first["Items"][0].get("Error").is_none());
    for field in ["StartTime", "StateUpdateTime", "EndTime"] {
        assert_ne!(first["Items"][0][field], "0001-01-01T00:00:00Z");
    }
    let second = fetch(&format!("?page_size=2&page_token={}", ids[3]));
    assert_eq!(second["Items"][0]["ID"], ids[2]);
    assert_eq!(second["Items"][1]["ID"], ids[1]);
    assert_eq!(second["HasMore"], true);
    assert_eq!(second["NextPageToken"], ids[1]);
    assert_eq!(second["ApproxTotalCount"], 5);
    let last = fetch(&format!("?page_size=2&page_token={}", ids[1]));
    assert_eq!(last["Items"].as_array().unwrap().len(), 1);
    assert_eq!(last["NextPageToken"], 0);
    assert_eq!(last["HasMore"], false);
    let filtered = fetch("?page_size=2&keyspace=ks1");
    assert_eq!(filtered["ApproxTotalCount"], 3);
    assert_eq!(filtered["Items"][1]["ID"], ids[2]);
    assert_eq!(filtered["HasMore"], true);
    for item in filtered["Items"].as_array().unwrap() {
        assert_eq!(item["Keyspace"], "ks1");
    }
    let filtered_last = fetch(&format!("?page_size=2&keyspace=ks1&page_token={}", ids[2]));
    assert_eq!(filtered_last["Items"].as_array().unwrap().len(), 1);
    assert_eq!(filtered_last["Items"][0]["ID"], ids[0]);
    assert_eq!(filtered_last["Items"][0]["Keyspace"], "ks1");
    assert_eq!(filtered_last["ApproxTotalCount"], 3);
    assert_eq!(filtered_last["HasMore"], false);
    assert_eq!(filtered_last["NextPageToken"], 0);
    let defaults = fetch("");
    assert_eq!(defaults["Items"].as_array().unwrap().len(), 5);
}
