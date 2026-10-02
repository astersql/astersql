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

// DXF（Distributed eXecution Framework，分布式执行框架）HTTP 接口。
//
// 提供调度状态查询、暂停缩容、调度调参、活跃/历史任务、并发上限与
// 任务运行时槽位（runtime slots）等运维 API；副作用通过 `DxfRuntime` 注入。

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

/// 暂停缩容（pause scale-in）表单 action。
const PAUSE_SCALE_IN_ACTION: &str = "pause_scale_in";
/// 恢复缩容表单 action。
const RESUME_SCALE_IN_ACTION: &str = "resume_scale_in";
/// DXF 操作默认 TTL（存活时间）秒数：1 小时。
pub const DXF_OPERATION_DEFAULT_TTL_SECONDS: i64 = 60 * 60;
/// 请求上下文默认超时秒数。
const REQUEST_DEFAULT_TIMEOUT_SECONDS: i64 = 10;

#[derive(Clone, Debug, Eq, PartialEq)]
/// DXF 错误类别：通用或任务未找到。
pub enum DxfErrorKind {
    /// 通用错误。
    General,
    /// 任务不存在（可映射为 HTTP 404）。
    TaskNotFound,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// DXF 接口错误，携带类别与消息。
pub struct DxfError {
    /// 错误类别。
    pub kind: DxfErrorKind,
    /// 错误文案。
    pub message: String,
}

impl DxfError {
    /// 构造通用类别错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            kind: DxfErrorKind::General,
            message: message.into(),
        }
    }
}

impl fmt::Display for DxfError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for DxfError {}

/// DXF 结果别名。
pub type DxfResult<T> = Result<T, DxfError>;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// DXF 请求上下文：请求 ID、截止时间与内部任务标记。
pub struct DxfContext {
    /// 请求追踪 ID。
    pub request_id: String,
    /// 可选截止 Unix 秒；超时后运行时应取消。
    pub deadline_unix_seconds: Option<i64>,
    /// 是否标记为内部分布式任务访问。
    pub internal_dist_task: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 解析后的 HTTP 请求视图（方法、query、form、path、上下文）。
pub struct Request {
    /// HTTP 方法。
    pub method: String,
    /// URL 查询参数。
    pub query: HashMap<String, String>,
    /// 表单参数（同名可多值）。
    pub form: HashMap<String, Vec<String>>,
    /// 路径变量。
    pub path: HashMap<String, String>,
    /// 请求级 DXF 上下文。
    pub context: DxfContext,
}

impl Request {
    /// 取查询参数，缺省为空串。
    fn query_value(&self, name: &str) -> &str {
        self.query.get(name).map_or("", String::as_str)
    }

    /// 取表单首个值，缺省为空串。
    fn form_value(&self, name: &str) -> &str {
        self.form
            .get(name)
            .and_then(|values| values.first())
            .map_or("", String::as_str)
    }

    /// 取表单全部值。
    fn form_values(&self, name: &str) -> &[String] {
        self.form.get(name).map_or(&[], Vec::as_slice)
    }

    /// 取路径变量，缺省为空串。
    fn path_value(&self, name: &str) -> &str {
        self.path.get(name).map_or("", String::as_str)
    }
}

#[derive(Clone, Debug, PartialEq)]
/// 简化 JSON 值树，用于写出 DXF 响应。
pub enum JsonValue {
    Null,
    Bool(bool),
    Integer(i64),
    Float(f64),
    String(String),
    Array(Vec<JsonValue>),
    Object(Vec<(String, JsonValue)>),
}

/// HTTP 响应写出：数据、错误或带状态码的错误。
pub trait ResponseWriter {
    /// 写出成功 JSON 数据。
    fn write_data(&mut self, value: JsonValue);
    /// 写出默认错误响应。
    fn write_error(&mut self, error: DxfError);
    /// 写出指定 HTTP 状态码的错误。
    fn write_error_with_code(&mut self, status: u16, error: DxfError);
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 存储句柄占位（对应 kv.Storage / store）。
pub struct StorageHandle(pub String);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// DXF 调度相关标志位。
pub enum Flag {
    /// 暂停缩容标志。
    PauseScaleIn,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// TTL（存活时间）信息：时长与过期 Unix 秒。
pub struct TTLInfo {
    /// TTL 秒数。
    pub ttl_seconds: i64,
    /// 过期时刻（Unix 秒）。
    pub expire_unix_seconds: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 带可选 TTL 的开关标志（如 pause scale-in）。
pub struct TTLFlag {
    /// 是否启用。
    pub enabled: bool,
    /// 启用时的 TTL 详情。
    pub ttl_info: Option<TTLInfo>,
}

#[derive(Clone, Debug, PartialEq)]
/// 调度调参因子：TTL 与 amplify_factor（放大系数）。
pub struct TTLTuneFactors {
    /// 调参生效的 TTL。
    pub ttl_info: TTLInfo,
    /// 资源放大系数。
    pub amplify_factor: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 业务步骤编号（任务状态机中的 step）。
pub struct Step(pub i32);

#[derive(Clone, Debug, Eq, PartialEq)]
/// 任务额外参数：最大运行时槽位与目标步骤。
pub struct ExtraParams {
    /// 最大运行时槽位数（须小于 required_slots）。
    pub max_runtime_slots: i32,
    /// 目标业务步骤列表。
    pub target_steps: Vec<Step>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// DXF 任务摘要。
pub struct Task {
    /// 任务业务键。
    pub key: String,
    /// 任务所需槽位数。
    pub required_slots: i32,
    /// 任务类型名。
    pub task_type: String,
    /// 可调额外参数。
    pub extra_params: ExtraParams,
}

/// All DXF/PD/storage effects are mandatory production boundaries.
/// All DXF/PD/storage effects are mandatory production boundaries.
///
/// DXF/PD/存储等全部副作用的生产边界；HTTP 层不得直接访问底层。
pub trait DxfRuntime: Send + Sync {
    fn now_unix_seconds(&self) -> i64;
    fn parse_duration_seconds(&self, value: &str) -> DxfResult<i64>;
    fn validate_history_page_size(&self, page_size: i32) -> DxfResult<()>;
    fn validate_keyspace_name(&self, keyspace: &str) -> DxfResult<()>;
    fn get_schedule_status(&self, context: &DxfContext) -> DxfResult<JsonValue>;
    fn get_active_task_summary(&self, context: &DxfContext) -> DxfResult<JsonValue>;
    fn list_history_tasks(
        &self,
        context: &DxfContext,
        page_size: i32,
        page_token: i64,
        keyspace: &str,
    ) -> DxfResult<JsonValue>;
    fn get_import_history_job(
        &self,
        context: &DxfContext,
        keyspace: &str,
        job_id: i64,
    ) -> DxfResult<JsonValue>;
    fn update_pause_scale_in_flag(&self, context: &DxfContext, flag: &TTLFlag) -> DxfResult<()>;
    fn load_keyspace_if_supported(
        &self,
        storage: &StorageHandle,
        context: &DxfContext,
        keyspace: &str,
    ) -> DxfResult<()>;
    fn get_schedule_tune_factors(
        &self,
        context: &DxfContext,
        keyspace: &str,
    ) -> DxfResult<TTLTuneFactors>;
    fn set_schedule_tune_factors_in_new_txn(
        &self,
        storage: &StorageHandle,
        context: &DxfContext,
        keyspace: &str,
        factors: &TTLTuneFactors,
    ) -> DxfResult<()>;
    fn min_amplify_factor(&self) -> f64;
    fn max_amplify_factor(&self) -> f64;
    fn set_max_concurrent_task(&self, value: i32) -> DxfResult<()>;
    fn get_max_concurrent_task(&self) -> i32;
    fn get_task_by_id(&self, context: &DxfContext, task_id: i64) -> DxfResult<Task>;
    fn is_valid_business_step(&self, task_type: &str, step: Step) -> bool;
    fn step_to_string(&self, task_type: &str, step: Step) -> String;
    fn update_task_extra_params(
        &self,
        context: &DxfContext,
        task_id: i64,
        extra: ExtraParams,
    ) -> DxfResult<()>;
    fn log_info(&self, message: &str);
    fn log_warning(&self, message: &str, error: &DxfError);
}

/// 在基准上下文上附加默认请求超时截止时间。
fn timed_context(runtime: &dyn DxfRuntime, base: DxfContext) -> DxfContext {
    DxfContext {
        deadline_unix_seconds: Some(
            runtime
                .now_unix_seconds()
                .saturating_add(REQUEST_DEFAULT_TIMEOUT_SECONDS),
        ),
        ..base
    }
}

/// 使用默认上下文再附加超时。
fn background_timed_context(runtime: &dyn DxfRuntime) -> DxfContext {
    timed_context(runtime, DxfContext::default())
}

/// 生成仅持有 `Arc<dyn DxfRuntime>` 的 handler 结构体与构造函数。
macro_rules! global_handler {
    ($name:ident, $constructor:ident) => {
        pub struct $name {
            runtime: Arc<dyn DxfRuntime>,
        }

        pub fn $constructor(runtime: Arc<dyn DxfRuntime>) -> $name {
            $name { runtime }
        }
    };
}

global_handler!(DXFScheduleStatusHandler, NewDXFScheduleStatusHandler);
global_handler!(DXFActiveTaskHandler, NewDXFActiveTaskHandler);
global_handler!(DXFTaskHistoryHandler, NewDXFTaskHistoryHandler);
global_handler!(
    DXFImportIntoHistoryJobInfoHandler,
    NewDXFImportIntoHistoryJobInfoHandler
);
global_handler!(DXFScheduleHandler, NewDXFScheduleHandler);
global_handler!(DXFTaskMaxConcurrentHandler, NewDXFTaskMaxConcurrentHandler);
global_handler!(
    DXFTaskMaxRuntimeSlotsHandler,
    NewDXFTaskMaxRuntimeSlotsHandler
);

/// 查询 DXF 调度状态（仅 GET）。
impl DXFScheduleStatusHandler {
    pub fn ServeHTTP(&self, writer: &mut dyn ResponseWriter, request: &Request) {
        // 仅允许 GET。
        if request.method != "GET" {
            writer.write_error(DxfError::new("This api only support GET method"));
            return;
        }
        let context = background_timed_context(self.runtime.as_ref());
        match self.runtime.get_schedule_status(&context) {
            Ok(status) => {
                self.runtime
                    .log_info(&format!("current DXF schedule status: {status:?}"));
                writer.write_data(status);
            }
            Err(error) => {
                self.runtime
                    .log_warning("failed to get DXF schedule status", &error);
                writer.write_error_with_code(500, error);
            }
        }
    }
}

/// 查询活跃任务摘要（仅 GET）。
impl DXFActiveTaskHandler {
    pub fn ServeHTTP(&self, writer: &mut dyn ResponseWriter, request: &Request) {
        if request.method != "GET" {
            writer.write_error(DxfError::new("This api only support GET method"));
            return;
        }
        let context = background_timed_context(self.runtime.as_ref());
        match self.runtime.get_active_task_summary(&context) {
            Ok(summary) => writer.write_data(summary),
            Err(error) => {
                self.runtime
                    .log_warning("failed to get DXF active task summary", &error);
                writer.write_error_with_code(500, error);
            }
        }
    }
}

/// 分页查询历史任务（仅 GET）。
impl DXFTaskHistoryHandler {
    pub fn ServeHTTP(&self, writer: &mut dyn ResponseWriter, request: &Request) {
        if request.method != "GET" {
            writer.write_error(DxfError::new("This api only support GET method"));
            return;
        }
        // 先解析并校验分页与 keyspace 查询参数。
        let (page_size, page_token, keyspace) =
            match parseTaskHistoryQuery(request, self.runtime.as_ref()) {
                Ok(query) => query,
                Err(error) => {
                    writer.write_error(error);
                    return;
                }
            };
        let context = timed_context(self.runtime.as_ref(), request.context.clone());
        match self
            .runtime
            .list_history_tasks(&context, page_size, page_token, &keyspace)
        {
            Ok(page) => writer.write_data(page),
            Err(error) => {
                self.runtime
                    .log_warning("failed to list DXF history tasks", &error);
                writer.write_error_with_code(500, error);
            }
        }
    }
}

/// Parse history page size (default 20), token and keyspace.
pub fn parseTaskHistoryQuery(
    request: &Request,
    runtime: &dyn DxfRuntime,
) -> DxfResult<(i32, i64, String)> {
    parse_task_history_query(
        request,
        |size| runtime.validate_history_page_size(size),
        |name| runtime.validate_keyspace_name(name),
    )
}

/// Validate a history request against the production storage and naming contracts.
pub fn parseStoredTaskHistoryQuery(request: &Request) -> DxfResult<(i32, i64, String)> {
    parse_task_history_query(
        request,
        |size| {
            astersql_dxf_framework_storage::ValidateHistoryTaskPageSize(size)
                .map_err(|error| DxfError::new(error.to_string()))
        },
        |name| astersql_util_naming::CheckKeyspaceName(name).map_err(DxfError::new),
    )
}

fn parse_task_history_query(
    request: &Request,
    validate_size: impl FnOnce(i32) -> DxfResult<()>,
    validate_keyspace: impl FnOnce(&str) -> DxfResult<()>,
) -> DxfResult<(i32, i64, String)> {
    let page_size_text = request.query_value("page_size");
    // Share the default with the storage query.
    let page_size = if page_size_text.is_empty() {
        astersql_dxf_framework_storage::DefaultHistoryTaskPageSize
    } else {
        page_size_text
            .parse::<i32>()
            .map_err(|_| DxfError::new(format!("invalid page_size {page_size_text}")))?
    };
    validate_size(page_size)
        .map_err(|_| DxfError::new(format!("invalid page_size {page_size}")))?;

    let token_text = request.query_value("page_token");
    let page_token = if token_text.is_empty() {
        0
    } else {
        token_text
            .parse::<i64>()
            .ok()
            .filter(|token| *token > 0)
            .ok_or_else(|| DxfError::new(format!("invalid page_token {token_text}")))?
    };
    let keyspace = request.query_value("keyspace").to_owned();
    if !keyspace.is_empty() && validate_keyspace(&keyspace).is_err() {
        return Err(DxfError::new(format!("invalid keyspace {keyspace}")));
    }
    Ok((page_size, page_token, keyspace))
}

/// 查询 import-into 历史 job 信息（仅 GET）；任务不存在返回 404。
impl DXFImportIntoHistoryJobInfoHandler {
    pub fn ServeHTTP(&self, writer: &mut dyn ResponseWriter, request: &Request) {
        if request.method != "GET" {
            writer.write_error(DxfError::new("This api only support GET method"));
            return;
        }
        let keyspace = request.path_value("keyspace");
        if keyspace.is_empty() || self.runtime.validate_keyspace_name(keyspace).is_err() {
            writer.write_error(DxfError::new(format!(
                "invalid or empty target keyspace {keyspace}"
            )));
            return;
        }
        let job_text = request.path_value("job_id");
        let job_id = match job_text.parse::<i64>() {
            Ok(job_id) if job_id > 0 => job_id,
            _ => {
                writer.write_error(DxfError::new(format!("invalid job id {job_text}")));
                return;
            }
        };
        // import-into 历史查询走内部分布式任务通道。
        let mut context = background_timed_context(self.runtime.as_ref());
        context.internal_dist_task = true;
        match self
            .runtime
            .get_import_history_job(&context, keyspace, job_id)
        {
            Ok(info) => writer.write_data(info),
            Err(error) if error.kind == DxfErrorKind::TaskNotFound => {
                writer.write_error_with_code(404, error);
            }
            Err(error) => writer.write_error(error),
        }
    }
}

/// 更新 pause/resume scale-in 调度标志（仅 POST）。
impl DXFScheduleHandler {
    pub fn ServeHTTP(&self, writer: &mut dyn ResponseWriter, request: &Request) {
        if request.method != "POST" {
            writer.write_error(DxfError::new("This api only support POST method"));
            return;
        }
        let (_, flag) = match parsePauseScaleInFlag(request, self.runtime.as_ref()) {
            Ok(flag) => flag,
            Err(error) => {
                writer.write_error(error);
                return;
            }
        };
        self.runtime
            .log_info(&format!("DXF schedule flag: {flag:?}"));
        let context = background_timed_context(self.runtime.as_ref());
        match self.runtime.update_pause_scale_in_flag(&context, &flag) {
            Ok(()) => writer.write_data(ttl_flag_json(&flag)),
            Err(error) => writer.write_error_with_code(
                500,
                DxfError::new(format!(
                    "failed to update pause scale-in flag, error {error}"
                )),
            ),
        }
    }
}

/// 解析 pause/resume scale-in 表单：action 与可选 ttl。
pub fn parsePauseScaleInFlag(
    request: &Request,
    runtime: &dyn DxfRuntime,
) -> DxfResult<(Flag, TTLFlag)> {
    let action = request.form_value("action");
    if action != PAUSE_SCALE_IN_ACTION && action != RESUME_SCALE_IN_ACTION {
        return Err(DxfError::new(format!("invalid action {action}")));
    }
    // pause 启用标志并解析 TTL；resume 仅关闭，不带 TTL。
    let enabled = action == PAUSE_SCALE_IN_ACTION;
    let ttl_info = if enabled {
        Some(parseTTLInfo(request, runtime)?)
    } else {
        None
    };
    Ok((Flag::PauseScaleIn, TTLFlag { enabled, ttl_info }))
}

/// 解析表单 ttl；空则用默认 TTL，过期时间为 now+ttl。
pub fn parseTTLInfo(request: &Request, runtime: &dyn DxfRuntime) -> DxfResult<TTLInfo> {
    let ttl_text = request.form_value("ttl");
    let ttl_seconds = if ttl_text.is_empty() {
        DXF_OPERATION_DEFAULT_TTL_SECONDS
    } else {
        runtime
            .parse_duration_seconds(ttl_text)
            .map_err(|error| DxfError::new(format!("invalid ttl {ttl_text}, error {error}")))?
    };
    Ok(TTLInfo {
        ttl_seconds,
        expire_unix_seconds: runtime.now_unix_seconds().saturating_add(ttl_seconds),
    })
}

/// 调度调参 handler：需存储句柄以在事务中写入 tune factors。
pub struct DXFScheduleTuneHandler {
    /// 运行时依赖。
    runtime: Arc<dyn DxfRuntime>,
    /// 存储句柄。
    store: StorageHandle,
}

/// 构造调度调参 handler。
pub fn NewDXFScheduleTuneHandler(
    storage: StorageHandle,
    runtime: Arc<dyn DxfRuntime>,
) -> DXFScheduleTuneHandler {
    DXFScheduleTuneHandler {
        runtime,
        store: storage,
    }
}

/// GET 读取调参；POST 校验 amplify_factor 范围后写入。
impl DXFScheduleTuneHandler {
    pub fn ServeHTTP(&self, writer: &mut dyn ResponseWriter, request: &Request) {
        let keyspace = request.form_value("keyspace");
        if keyspace.is_empty() || self.runtime.validate_keyspace_name(keyspace).is_err() {
            writer.write_error(DxfError::new(format!(
                "invalid or empty target keyspace {keyspace}"
            )));
            return;
        }
        let mut context = background_timed_context(self.runtime.as_ref());
        if let Err(error) = self
            .runtime
            .load_keyspace_if_supported(&self.store, &context, keyspace)
        {
            self.runtime
                .log_warning("failed to load keyspace from PD", &error);
            writer.write_error(DxfError::new(format!(
                "failed to load keyspace {keyspace} from PD: {error}"
            )));
            return;
        }
        // GET 读、POST 写；其他方法拒绝。
        match request.method.as_str() {
            "GET" => match self.runtime.get_schedule_tune_factors(&context, keyspace) {
                Ok(factors) => writer.write_data(tune_factors_json(&factors)),
                Err(error) => {
                    self.runtime
                        .log_warning("failed to get DXF schedule tune factors", &error);
                    writer.write_error_with_code(500, error);
                }
            },
            "POST" => {
                let ttl_info = match parseTTLInfo(request, self.runtime.as_ref()) {
                    Ok(info) => info,
                    Err(error) => {
                        writer.write_error(error);
                        return;
                    }
                };
                let factor_text = request.form_value("amplify_factor");
                let factor = match factor_text.parse::<f64>() {
                    Ok(factor) => factor,
                    Err(error) => {
                        writer.write_error(DxfError::new(format!(
                            "invalid amplify_factor {factor_text}, error {error}"
                        )));
                        return;
                    }
                };
                let minimum = self.runtime.min_amplify_factor();
                let maximum = self.runtime.max_amplify_factor();
                if factor < minimum || factor > maximum {
                    writer.write_error(DxfError::new(format!(
                        "amplify_factor {factor} is out of range [{minimum}, {maximum}]"
                    )));
                    return;
                }
                let factors = TTLTuneFactors {
                    ttl_info,
                    amplify_factor: factor,
                };
                context.internal_dist_task = true;
                match self.runtime.set_schedule_tune_factors_in_new_txn(
                    &self.store,
                    &context,
                    keyspace,
                    &factors,
                ) {
                    Ok(()) => {
                        self.runtime.log_info(&format!(
                            "set DXF schedule tune factors: keyspace={keyspace}, factors={factors:?}"
                        ));
                        writer.write_data(tune_factors_json(&factors));
                    }
                    Err(error) => {
                        self.runtime
                            .log_warning("failed to set DXF schedule tune factors", &error);
                        writer.write_error_with_code(500, error);
                    }
                }
            }
            _ => writer.write_error(DxfError::new("This api only support GET and POST method")),
        }
    }
}

/// 查询/设置内存态最大并发任务数。
impl DXFTaskMaxConcurrentHandler {
    pub fn ServeHTTP(&self, writer: &mut dyn ResponseWriter, request: &Request) {
        match request.method.as_str() {
            "GET" => writeMaxConcurrentTask(writer, self.runtime.as_ref()),
            "POST" => {
                let value_text = request.form_value("value");
                let value = match value_text.parse::<i32>() {
                    Ok(value) => value,
                    Err(error) => {
                        writer.write_error(DxfError::new(format!(
                            "invalid value {value_text}, error {error}"
                        )));
                        return;
                    }
                };
                if let Err(error) = self.runtime.set_max_concurrent_task(value) {
                    writer.write_error(error);
                    return;
                }
                self.runtime
                    .log_info(&format!("set in-memory DXF max concurrent task: {value}"));
                writeMaxConcurrentTask(writer, self.runtime.as_ref());
            }
            _ => writer.write_error(DxfError::new("This api only support GET and POST method")),
        }
    }
}

/// 写出当前 max_concurrent_task 及 persistence=memory_only。
pub fn writeMaxConcurrentTask(writer: &mut dyn ResponseWriter, runtime: &dyn DxfRuntime) {
    writer.write_data(JsonValue::Object(vec![
        (
            "max_concurrent_task".to_owned(),
            JsonValue::Integer(runtime.get_max_concurrent_task().into()),
        ),
        (
            "persistence".to_owned(),
            JsonValue::String("memory_only".to_owned()),
        ),
    ]));
}

/// 设置指定任务的最大运行时槽位与目标步骤（仅 POST）。
impl DXFTaskMaxRuntimeSlotsHandler {
    pub fn ServeHTTP(&self, writer: &mut dyn ResponseWriter, request: &Request) {
        if request.method != "POST" {
            writer.write_error(DxfError::new("This api only support POST method"));
            return;
        }
        let task_text = request.path_value("taskID");
        let task_id = match task_text.parse::<i64>() {
            Ok(id) if id > 0 => id,
            Ok(_) => {
                writer.write_error(DxfError::new("invalid task ID"));
                return;
            }
            Err(error) => {
                writer.write_error(DxfError::new(format!(
                    "invalid task ID {task_text}, error {error}"
                )));
                return;
            }
        };
        let value_text = request.form_value("value");
        let max_runtime_slots = match value_text.parse::<i32>() {
            Ok(value) if value > 0 => value,
            Ok(value) => {
                writer.write_error(DxfError::new(format!("invalid value {value}")));
                return;
            }
            Err(error) => {
                writer.write_error(DxfError::new(format!(
                    "invalid value {value_text}, error {error}"
                )));
                return;
            }
        };
        let mut steps = Vec::new();
        for text in request.form_values("target_step") {
            match text.parse::<i32>() {
                Ok(step) => steps.push(Step(step)),
                Err(error) => {
                    writer.write_error(DxfError::new(format!(
                        "invalid target step {text}, error {error}"
                    )));
                    return;
                }
            }
        }
        let mut context = background_timed_context(self.runtime.as_ref());
        context.internal_dist_task = true;
        let mut task = match self.runtime.get_task_by_id(&context, task_id) {
            Ok(task) => task,
            Err(error) => {
                writer.write_error(error);
                return;
            }
        };
        // max_runtime_slots 必须严格小于 required_slots。
        if max_runtime_slots >= task.required_slots {
            writer.write_error(DxfError::new(format!(
                "max runtime slots should be less than required slots({})",
                task.required_slots
            )));
            return;
        }
        let mut step_strings = Vec::with_capacity(steps.len());
        for step in &steps {
            if !self.runtime.is_valid_business_step(&task.task_type, *step) {
                writer.write_error(DxfError::new(format!(
                    "invalid target step {} for task type {}",
                    step.0, task.task_type
                )));
                return;
            }
            step_strings.push(self.runtime.step_to_string(&task.task_type, *step));
        }
        task.extra_params.max_runtime_slots = max_runtime_slots;
        task.extra_params.target_steps = steps;
        if let Err(error) =
            self.runtime
                .update_task_extra_params(&context, task_id, task.extra_params.clone())
        {
            writer.write_error_with_code(500, error);
            return;
        }
        self.runtime.log_info(&format!(
            "set DXF task max runtime slots: taskID={task_id}, taskKey={}",
            task.key
        ));
        writer.write_data(JsonValue::Object(vec![
            ("task_id".to_owned(), JsonValue::Integer(task_id)),
            ("task_key".to_owned(), JsonValue::String(task.key)),
            (
                "required_slots".to_owned(),
                JsonValue::Integer(task.required_slots.into()),
            ),
            (
                "max_runtime_slots".to_owned(),
                JsonValue::Integer(max_runtime_slots.into()),
            ),
            (
                "target_steps".to_owned(),
                JsonValue::Array(step_strings.into_iter().map(JsonValue::String).collect()),
            ),
        ]));
    }
}

/// 将 TTLFlag 序列化为 JSON 对象。
pub(crate) fn ttl_flag_json(flag: &TTLFlag) -> JsonValue {
    let mut fields = Vec::new();
    if flag.enabled {
        fields.push(("enabled".to_owned(), JsonValue::Bool(true)));
    }
    if let Some(info) = flag.ttl_info {
        fields.push(("ttl".to_owned(), JsonValue::Integer(info.ttl_seconds)));
        fields.push((
            "expire_time".to_owned(),
            JsonValue::Integer(info.expire_unix_seconds),
        ));
    }
    JsonValue::Object(fields)
}

/// 将 TTLInfo 序列化为 JSON 对象。
fn ttl_info_json(info: TTLInfo) -> JsonValue {
    JsonValue::Object(vec![
        ("ttl".to_owned(), JsonValue::Integer(info.ttl_seconds)),
        (
            "expire_time".to_owned(),
            JsonValue::Integer(info.expire_unix_seconds),
        ),
    ])
}

/// 将调参因子序列化为 JSON 对象。
pub(crate) fn tune_factors_json(factors: &TTLTuneFactors) -> JsonValue {
    JsonValue::Object(vec![
        (
            "ttl".to_owned(),
            JsonValue::Integer(factors.ttl_info.ttl_seconds),
        ),
        (
            "expire_time".to_owned(),
            JsonValue::Integer(factors.ttl_info.expire_unix_seconds),
        ),
        (
            "amplify_factor".to_owned(),
            JsonValue::Float(factors.amplify_factor),
        ),
    ])
}
