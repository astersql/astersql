// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// DXF（Distributed eXecution Framework）HTTP 解析逻辑的单元测试。
//
// 使用 `MockRuntime` 注入时间与时长解析，覆盖 `parsePauseScaleInFlag`
// 对非法 action、非法 ttl、默认 TTL 与显式 TTL 的分支。

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::dxf::{
    DXF_OPERATION_DEFAULT_TTL_SECONDS, DxfContext, DxfError, DxfResult, DxfRuntime, ExtraParams,
    Flag, JsonValue, Request, StorageHandle, TTLFlag, TTLTuneFactors, Task, parsePauseScaleInFlag,
    ttl_flag_json, tune_factors_json,
};

/// 测试用运行时：固定“当前时间”，其余 DXF 副作用返回空/未使用。
struct MockRuntime {
    /// 模拟的 Unix 秒时间戳。
    now: i64,
}

impl MockRuntime {
    /// 以真实墙钟秒数初始化，便于与过期时间窗口断言对齐。
    fn new() -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_secs() as i64;
        Self { now }
    }
}

impl DxfRuntime for MockRuntime {
    fn now_unix_seconds(&self) -> i64 {
        self.now
    }

    fn parse_duration_seconds(&self, value: &str) -> DxfResult<i64> {
        parse_go_duration_seconds(value)
    }

    fn validate_history_page_size(&self, _page_size: i32) -> DxfResult<()> {
        Ok(())
    }

    fn validate_keyspace_name(&self, _keyspace: &str) -> DxfResult<()> {
        Ok(())
    }

    fn get_schedule_status(&self, _context: &DxfContext) -> DxfResult<JsonValue> {
        Ok(JsonValue::Null)
    }

    fn get_active_task_summary(&self, _context: &DxfContext) -> DxfResult<JsonValue> {
        Ok(JsonValue::Null)
    }

    fn list_history_tasks(
        &self,
        _context: &DxfContext,
        _page_size: i32,
        _page_token: i64,
        _keyspace: &str,
    ) -> DxfResult<JsonValue> {
        Ok(JsonValue::Null)
    }

    fn get_import_history_job(
        &self,
        _context: &DxfContext,
        _keyspace: &str,
        _job_id: i64,
    ) -> DxfResult<JsonValue> {
        Ok(JsonValue::Null)
    }

    fn update_pause_scale_in_flag(&self, _context: &DxfContext, _flag: &TTLFlag) -> DxfResult<()> {
        Ok(())
    }

    fn load_keyspace_if_supported(
        &self,
        _storage: &StorageHandle,
        _context: &DxfContext,
        _keyspace: &str,
    ) -> DxfResult<()> {
        Ok(())
    }

    fn get_schedule_tune_factors(
        &self,
        _context: &DxfContext,
        _keyspace: &str,
    ) -> DxfResult<TTLTuneFactors> {
        Err(DxfError::new("unused"))
    }

    fn set_schedule_tune_factors_in_new_txn(
        &self,
        _storage: &StorageHandle,
        _context: &DxfContext,
        _keyspace: &str,
        _factors: &TTLTuneFactors,
    ) -> DxfResult<()> {
        Ok(())
    }

    fn min_amplify_factor(&self) -> f64 {
        0.0
    }

    fn max_amplify_factor(&self) -> f64 {
        1.0
    }

    fn set_max_concurrent_task(&self, _value: i32) -> DxfResult<()> {
        Ok(())
    }

    fn get_max_concurrent_task(&self) -> i32 {
        0
    }

    fn get_task_by_id(&self, _context: &DxfContext, _task_id: i64) -> DxfResult<Task> {
        Err(DxfError::new("unused"))
    }

    fn is_valid_business_step(&self, _task_type: &str, _step: crate::dxf::Step) -> bool {
        false
    }

    fn step_to_string(&self, _task_type: &str, _step: crate::dxf::Step) -> String {
        String::new()
    }

    fn update_task_extra_params(
        &self,
        _context: &DxfContext,
        _task_id: i64,
        _extra: ExtraParams,
    ) -> DxfResult<()> {
        Ok(())
    }

    fn log_info(&self, _message: &str) {}

    fn log_warning(&self, _message: &str, _error: &DxfError) {}
}

/// 简化的 Go duration 解析：末位为单位（s/m/h/d），前缀为整数。
fn parse_go_duration_seconds(value: &str) -> DxfResult<i64> {
    if value.is_empty() {
        return Err(DxfError::new("empty duration"));
    }
    let (digits, unit) = value.split_at(value.len().saturating_sub(1));
    let amount: i64 = digits
        .parse()
        .map_err(|_| DxfError::new(format!("invalid duration {value}")))?;
    let seconds = match unit {
        "s" => amount,
        "m" => amount.saturating_mul(60),
        "h" => amount.saturating_mul(60 * 60),
        "d" => amount.saturating_mul(24 * 60 * 60),
        _ => return Err(DxfError::new(format!("invalid duration {value}"))),
    };
    Ok(seconds)
}

/// 由表单键值对构造最小 `Request`，供 parse 测试使用。
fn form_request(entries: &[(&str, &str)]) -> Request {
    let mut form = HashMap::new();
    for (name, value) in entries {
        form.insert((*name).to_owned(), vec![(*value).to_owned()]);
    }
    Request {
        form,
        ..Default::default()
    }
}

// test_parse_pause_scale_in_flag 对应 Go 的 TestParsePauseScaleInFlag。
// 它覆盖非法 action、非法 ttl、默认 TTL 和显式 5h TTL 四个分支。
#[test]
fn test_parse_pause_scale_in_flag() {
    let runtime = MockRuntime::new();

    // 缺省 action 应报 invalid action。
    let err = parsePauseScaleInFlag(&Request::default(), &runtime).unwrap_err();
    assert!(
        err.message.contains("invalid action"),
        "unexpected error: {}",
        err.message
    );

    // ttl 无法解析时应报 invalid ttl。
    let err = parsePauseScaleInFlag(
        &form_request(&[("action", "pause_scale_in"), ("ttl", "invalid")]),
        &runtime,
    )
    .unwrap_err();
    assert!(
        err.message.contains("invalid ttl"),
        "unexpected error: {}",
        err.message
    );

    // 未给 ttl 时使用 DXF 默认 TTL，并校验过期时间窗口。
    let (flag, ttl_flag) =
        parsePauseScaleInFlag(&form_request(&[("action", "pause_scale_in")]), &runtime)
            .expect("default ttl");
    assert_eq!(Flag::PauseScaleIn, flag);
    assert!(ttl_flag.enabled);
    let ttl_info = ttl_flag.ttl_info.expect("ttl info");
    assert_eq!(DXF_OPERATION_DEFAULT_TTL_SECONDS, ttl_info.ttl_seconds);
    // Go 使用 time.Now().Add(ttlFlag.TTL-time.Minute) 到 time.Now().Add(ttlFlag.TTL) 检查过期时间窗口。
    let lower = runtime.now + ttl_info.ttl_seconds - 60;
    let upper = runtime.now + ttl_info.ttl_seconds;
    assert!(
        ttl_info.expire_unix_seconds >= lower && ttl_info.expire_unix_seconds <= upper,
        "expire {} not in [{lower}, {upper}]",
        ttl_info.expire_unix_seconds
    );

    // 显式 ttl=5h 应换算为 5*3600 秒。
    let (flag, ttl_flag) = parsePauseScaleInFlag(
        &form_request(&[("action", "pause_scale_in"), ("ttl", "5h")]),
        &runtime,
    )
    .expect("explicit ttl");
    assert_eq!(Flag::PauseScaleIn, flag);
    assert!(ttl_flag.enabled);
    let ttl_info = ttl_flag.ttl_info.expect("ttl info");
    assert_eq!(5 * 60 * 60, ttl_info.ttl_seconds);
    let lower = runtime.now + ttl_info.ttl_seconds - 60;
    let upper = runtime.now + ttl_info.ttl_seconds;
    assert!(
        ttl_info.expire_unix_seconds >= lower && ttl_info.expire_unix_seconds <= upper,
        "expire {} not in [{lower}, {upper}]",
        ttl_info.expire_unix_seconds
    );
}

#[test]
fn dxf_response_values_flatten_go_embedded_ttl_fields() {
    assert_eq!(
        ttl_flag_json(&TTLFlag {
            enabled: false,
            ttl_info: None,
        }),
        JsonValue::Object(Vec::new())
    );
    assert_eq!(
        ttl_flag_json(&TTLFlag {
            enabled: true,
            ttl_info: Some(crate::dxf::TTLInfo {
                ttl_seconds: 300,
                expire_unix_seconds: 1_300,
            }),
        }),
        JsonValue::Object(vec![
            ("enabled".to_owned(), JsonValue::Bool(true)),
            ("ttl".to_owned(), JsonValue::Integer(300)),
            ("expire_time".to_owned(), JsonValue::Integer(1_300)),
        ])
    );
    assert_eq!(
        tune_factors_json(&TTLTuneFactors {
            ttl_info: crate::dxf::TTLInfo {
                ttl_seconds: 300,
                expire_unix_seconds: 1_300,
            },
            amplify_factor: 2.0,
        }),
        JsonValue::Object(vec![
            ("ttl".to_owned(), JsonValue::Integer(300)),
            ("expire_time".to_owned(), JsonValue::Integer(1_300)),
            ("amplify_factor".to_owned(), JsonValue::Float(2.0)),
        ])
    );
}

#[test]
fn cleanup_batch_handler_rejects_unsupported_methods_without_changing_value() {
    use crate::dxf::{NewDXFTaskCleanupBatchSizeHandler, ResponseWriter};
    #[derive(Default)]
    struct Writer {
        error: Option<DxfError>,
    }
    impl ResponseWriter for Writer {
        fn write_data(&mut self, _: JsonValue) {
            panic!("unexpected success");
        }
        fn write_error(&mut self, error: DxfError) {
            self.error = Some(error);
        }
        fn write_error_with_code(&mut self, _: u16, error: DxfError) {
            self.error = Some(error);
        }
    }
    let before = astersql_dxf_framework_proto::GetTaskCleanupBatchSize();
    for method in ["PUT", "DELETE"] {
        let mut writer = Writer::default();
        NewDXFTaskCleanupBatchSizeHandler().ServeHTTP(
            &mut writer,
            &Request {
                method: method.into(),
                ..Default::default()
            },
        );
        assert_eq!(
            writer.error.unwrap().message,
            "This api only support GET and POST method"
        );
        assert_eq!(
            astersql_dxf_framework_proto::GetTaskCleanupBatchSize(),
            before
        );
    }
}
