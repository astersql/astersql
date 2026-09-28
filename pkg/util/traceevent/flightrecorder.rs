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

// 飞行记录器（Flight Recorder）：按 dump 触发条件决定是否保留并导出追踪事件。
//
// 对应 Go `flightrecorder.go`。会话侧 `Trace` 缓冲事件并用 bitset 记录命中触发器；
// 全局 `HttpFlightRecorder` 编译配置为真值表，在 discard/flush 时判定是否 collect。
// dump_trigger 支持 sampling、suspicious_event、user_command 以及 and/or 组合。

use crate::traceevent::{
    ALL_CATEGORIES, Context, Event, Sink, TraceCategory, parse_trace_category,
};
use rand::random;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::{Arc, OnceLock, RwLock};

/// 配置编译等错误信息（字符串形式，对齐 Go error.Error()）。
pub type Error = String;

/// Trace 内部可变状态：事件缓冲、触发器 bitset、采样随机数。
#[derive(Default)]
struct TraceState {
    events: Vec<Event>,
    bits: u64,
    random: u32,
}

/// 单次追踪会话的事件缓冲，实现 `Sink`；flush 时按飞行记录器策略决定保留或丢弃。
pub struct Trace {
    state: RwLock<TraceState>,
}
impl Trace {
    /// 使用随机数构造空 Trace。
    pub fn new() -> Self {
        Self::new_with_random(random())
    }
    /// 指定 random 后缀（用于生成 trace_id 末 4 字节）。
    pub fn new_with_random(value: u32) -> Self {
        Self {
            state: RwLock::new(TraceState {
                random: value,
                ..Default::default()
            }),
        }
    }
    /// 克隆当前缓冲中的全部事件。
    pub fn events(&self) -> Vec<Event> {
        self.state
            .read()
            .expect("trace lock poisoned")
            .events
            .clone()
    }
    /// 已命中触发器的位图。
    pub fn bits(&self) -> u64 {
        self.state.read().expect("trace lock poisoned").bits
    }
    /// 本 Trace 的随机后缀。
    pub fn random(&self) -> u32 {
        self.state.read().expect("trace lock poisoned").random
    }
    /// 将第 `idx` 个触发器标记为已命中。
    pub fn mark_bits(&self, idx: usize) {
        self.state.write().expect("trace lock poisoned").bits |= 1_u64 << idx;
    }
    /// 若飞行记录器判定应保留则 collect，随后清空 bitset/事件并刷新 random。
    pub fn discard_or_flush(&self, ctx: &Context) {
        if let Some(recorder) = get_flight_recorder() {
            let events = {
                let state = self.state.read().expect("trace lock poisoned");
                recorder
                    .should_keep(state.bits)
                    .then(|| state.events.clone())
            };
            if let Some(events) = events {
                recorder.collect(ctx, events);
            }
        }
        let mut state = self.state.write().expect("trace lock poisoned");
        state.bits = 0;
        // 超大缓冲直接换新 Vec，避免 clear 后容量长期偏大。
        if state.events.len() > MAX_EVENTS {
            state.events = Vec::new();
        } else {
            state.events.clear();
        }
        state.random = random();
    }
}
impl Default for Trace {
    fn default() -> Self {
        Self::new()
    }
}
impl Sink for Trace {
    fn record(&self, _ctx: &Context, event: Event) {
        self.state
            .write()
            .expect("trace lock poisoned")
            .events
            .push(event);
    }
}

/// 用户命令触发条件：按 SQL 正则、digest、语句标签、用户或表匹配。
#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserCommandConfig {
    #[serde(rename = "type", default)]
    pub kind: String,
    pub sql_regexp: String,
    pub sql_digest: String,
    pub plan_digest: String,
    pub stmt_label: String,
    pub by_user: String,
    pub table: String,
}

/// 开发调试类可疑事件子类型配置。
#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DevDebugConfig {
    #[serde(rename = "type", default)]
    pub kind: String,
}
/// 内部执行未携带 Trace 时的调试类型名。
pub const DEV_DEBUG_TYPE_EXECUTE_INTERNAL_TRACE_MISSING: &str = "execute_internal_trace_missing";
/// 发送请求缺少 Trace ID 时的调试类型名。
pub const DEV_DEBUG_TYPE_SEND_REQUEST_TRACE_ID_MISSING: &str = "send_request_trace_id_missing";

/// 可疑事件触发：慢查询、失败、锁解析、Region 错误或内部/dev_debug 子类型。
#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SuspiciousEventConfig {
    #[serde(rename = "type", default)]
    pub kind: String,
    pub is_internal: bool,
    pub dev_debug: Option<DevDebugConfig>,
}

/// dump 触发器树节点：叶子为 sampling/event/user_command，内部为 and/or。
#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DumpTriggerConfig {
    #[serde(rename = "type", default)]
    pub kind: String,
    pub sampling: i64,
    #[serde(rename = "suspicious_event")]
    pub event: Option<SuspiciousEventConfig>,
    pub user_command: Option<UserCommandConfig>,
    pub and: Vec<DumpTriggerConfig>,
    pub or: Vec<DumpTriggerConfig>,
}

impl UserCommandConfig {
    /// 将本叶子编译进 mapping，返回单 bit 掩码；校验 type 与取值非空。
    fn compile(
        &self,
        name: &mut String,
        mapping: &mut CompiledDumpTriggerConfig,
        config: &DumpTriggerConfig,
    ) -> Result<u64, Error> {
        name.push_str(".user_command");
        let (suffix, value) = match self.kind.as_str() {
            "sql_regexp" => (".sql_regexp", &self.sql_regexp),
            "sql_digest" => (".sql_digest", &self.sql_digest),
            "plan_digest" => (".plan_digest", &self.plan_digest),
            "stmt_label" => (".stmt_label", &self.stmt_label),
            "by_user" => (".by_user", &self.by_user),
            "table" => (".table", &self.table),
            _ => return Err("wrong dump_trigger.user_command.type".into()),
        };
        if value.is_empty() {
            if self.kind == "stmt_label" {
                return Err("dump_trigger.user_command.stmt_label should not be empty, should be something in https://github.com/pingcap/tidb/blob/adf08267939416d1b989e56dba6a6544bf34a8dd/pkg/parser/ast/ast.go#L160".into());
            }
            return Err(format!(
                "dump_trigger.user_command{} should not be empty",
                suffix
            ));
        }
        name.push_str(suffix);
        mapping.add_trigger(name.clone(), config)
    }
}

impl DevDebugConfig {
    /// 校验已知 dev_debug type 并注册触发器。
    fn compile(
        &self,
        name: &mut String,
        mapping: &mut CompiledDumpTriggerConfig,
        config: &DumpTriggerConfig,
    ) -> Result<u64, Error> {
        name.push_str(".dev_debug");
        match self.kind.as_str() {
            DEV_DEBUG_TYPE_EXECUTE_INTERNAL_TRACE_MISSING
            | DEV_DEBUG_TYPE_SEND_REQUEST_TRACE_ID_MISSING => {
                mapping.add_trigger(name.clone(), config)
            }
            _ => Err("wrong dump_trigger.suspicious_event.dev_debug.type".into()),
        }
    }
}

impl SuspiciousEventConfig {
    /// 按 kind 注册可疑事件或下沉到 dev_debug 子配置。
    fn compile(
        &self,
        name: &mut String,
        mapping: &mut CompiledDumpTriggerConfig,
        config: &DumpTriggerConfig,
    ) -> Result<u64, Error> {
        name.push_str(".suspicious_event");
        match self.kind.as_str() {
            "slow_query" | "query_fail" | "resolve_lock" | "region_error" => {
                mapping.add_trigger(name.clone(), config)
            }
            "is_internal" => {
                name.push_str(".is_internal");
                mapping.add_trigger(name.clone(), config)
            }
            "dev_debug" => self
                .dev_debug
                .as_ref()
                .ok_or_else(|| "dump_trigger.suspicious_event.dev_debug missing".to_string())?
                .compile(name, mapping, config),
            _ => Err("wrong dump_trigger.suspicious_event.type".into()),
        }
    }
}

impl DumpTriggerConfig {
    /// 递归编译触发器树：叶子返回单 bit 向量，and/or 用真值表合并。
    pub fn compile(
        &self,
        name: &mut String,
        mapping: &mut CompiledDumpTriggerConfig,
    ) -> Result<Vec<u64>, Error> {
        name.push_str("dump_trigger");
        match self.kind.as_str() {
            "sampling" => {
                if self.sampling <= 0 {
                    return Err("wrong dump_trigger.sampling".into());
                }
                name.push_str(".sampling");
                Ok(vec![mapping.add_trigger(name.clone(), self)?])
            }
            "suspicious_event" => Ok(vec![
                self.event
                    .as_ref()
                    .ok_or_else(|| "dump_trigger.suspicious_event missing".to_string())?
                    .compile(name, mapping, self)?,
            ]),
            "user_command" => Ok(vec![
                self.user_command
                    .as_ref()
                    .ok_or_else(|| "dump_trigger.user_command missing".to_string())?
                    .compile(name, mapping, self)?,
            ]),
            "and" => {
                if self.and.is_empty() {
                    return Err("dump_trigger.and missing".into());
                }
                let mut result = Vec::new();
                for child in &self.and {
                    result =
                        truth_table_for_and(result, child.compile(&mut String::new(), mapping)?);
                }
                Ok(result)
            }
            "or" => {
                if self.or.is_empty() {
                    return Err("dump_trigger.or missing".into());
                }
                let mut result = Vec::new();
                for child in &self.or {
                    result =
                        truth_table_for_or(result, child.compile(&mut String::new(), mapping)?);
                }
                Ok(result)
            }
            _ => Err("wrong dump_trigger.type".into()),
        }
    }
}

/// 编译后的 dump 触发器：名称→位索引、原配置引用、满足条件的 bit 组合表。
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct CompiledDumpTriggerConfig {
    pub name_mapping: HashMap<String, usize>,
    pub config_ref: Vec<DumpTriggerConfig>,
    pub truth_table: Vec<u64>,
}
impl CompiledDumpTriggerConfig {
    /// 注册新触发器名，分配 bit；最多 64 个，禁止重名。
    fn add_trigger(&mut self, name: String, config: &DumpTriggerConfig) -> Result<u64, Error> {
        if self.name_mapping.contains_key(&name) {
            return Err(format!("duplicate trigger name: {}", name));
        }
        let idx = self.name_mapping.len();
        if idx >= 64 {
            return Err("too many triggers".into());
        }
        self.name_mapping.insert(name, idx);
        self.config_ref.push(config.clone());
        Ok(1_u64 << idx)
    }
}

/// AND 真值表：两侧组合的笛卡尔积按位或（空左侧直接取右侧）。
pub fn truth_table_for_and(x: Vec<u64>, y: Vec<u64>) -> Vec<u64> {
    if x.is_empty() {
        return y;
    }
    let mut result = Vec::with_capacity(x.len() * y.len());
    for left in x.iter().copied() {
        result.extend(y.iter().map(|right| left | right));
    }
    result
}
/// 单 bit 与一组掩码做 AND（每位 | x）。
pub fn truth_table_for_and_one(x: u64, xs: Vec<u64>) -> Vec<u64> {
    xs.into_iter().map(|value| value | x).collect()
}
/// OR 真值表：两侧向量拼接。
pub fn truth_table_for_or(mut x: Vec<u64>, y: Vec<u64>) -> Vec<u64> {
    x.extend(y);
    x
}
/// 若 `bits` 完全覆盖表中任一项掩码则命中。
pub fn check_truth_table(bits: u64, table: &[u64]) -> bool {
    table.iter().any(|value| bits & value == *value)
}

/// 飞行记录器运行时配置：启用的类别列表与 dump 触发器树。
#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FlightRecorderConfig {
    pub enabled_categories: Vec<String>,
    pub dump_trigger: DumpTriggerConfig,
}
impl FlightRecorderConfig {
    /// 填入默认类别（含 `-` 减法前缀语义）与 sampling=1 触发器。
    pub fn initialize(&mut self) {
        self.enabled_categories = vec![
            "-".into(),
            "tikv_write_details".into(),
            "tikv_read_details".into(),
            "dev_debug".into(),
        ];
        self.dump_trigger.kind = "sampling".into();
        self.dump_trigger.sampling = 1;
    }
    /// 编译 dump_trigger，产出带 truth_table 的 CompiledDumpTriggerConfig。
    pub fn compile(&self) -> Result<CompiledDumpTriggerConfig, Error> {
        let mut result = CompiledDumpTriggerConfig::default();
        result.truth_table = self.dump_trigger.compile(&mut String::new(), &mut result)?;
        Ok(result)
    }
}

/// 解析类别名列表：`*` 全开；`-` 后跟名为从全集减去；否则按位或。
pub fn parse_categories(categories: &[String]) -> TraceCategory {
    let mut result = TraceCategory::default();
    let mut subtract = false;
    for value in categories {
        if value == "*" {
            result = ALL_CATEGORIES;
            break;
        }
        if value == "-" {
            result = ALL_CATEGORIES;
            subtract = true;
            continue;
        }
        if subtract {
            result &= !parse_trace_category(value);
        } else {
            result |= parse_trace_category(value);
        }
    }
    result
}

/// HTTP/日志飞行记录器：持有启用类别、编译配置与可选事件发送 channel。
pub struct HttpFlightRecorder {
    channel: Option<SyncSender<Vec<Event>>>,
    enabled_categories: TraceCategory,
    counter: AtomicI64,
    pub config: FlightRecorderConfig,
    pub compiled: CompiledDumpTriggerConfig,
}
impl HttpFlightRecorder {
    /// 当前启用的 TraceCategory 位图。
    pub fn enabled_categories(&self) -> TraceCategory {
        self.enabled_categories
    }
    /// 用编译真值表判断会话 bitset 是否应保留事件。
    pub fn should_keep(&self, bits: u64) -> bool {
        check_truth_table(bits, &self.compiled.truth_table)
    }
    /// 有 channel 则 try_send；否则逐条写入日志 sink。
    pub fn collect(&self, ctx: &Context, events: Vec<Event>) {
        if let Some(channel) = &self.channel {
            let _ = channel.try_send(events);
        } else {
            for event in events {
                crate::traceevent::log_event(ctx, event);
            }
        }
    }
    /// 采样计数：每 `config.sampling` 次返回 true 并清零计数器。
    pub fn check_sampling(&self, config: &DumpTriggerConfig) -> bool {
        let value = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        if value >= config.sampling {
            self.counter.store(0, Ordering::SeqCst);
            true
        } else {
            false
        }
    }
    /// 关闭并清除全局飞行记录器实例。
    pub fn close(&self) {
        close_flight_recorder();
    }
}

static GLOBAL_FLIGHT_RECORDER: OnceLock<RwLock<Option<Arc<HttpFlightRecorder>>>> = OnceLock::new();
fn global_recorder() -> &'static RwLock<Option<Arc<HttpFlightRecorder>>> {
    GLOBAL_FLIGHT_RECORDER.get_or_init(|| RwLock::new(None))
}

/// 编译配置、构造记录器并安装为全局单例。
fn new_flight_recorder(
    config: FlightRecorderConfig,
    channel: Option<SyncSender<Vec<Event>>>,
) -> Result<Arc<HttpFlightRecorder>, Error> {
    let compiled = config.compile()?;
    let recorder = Arc::new(HttpFlightRecorder {
        channel,
        enabled_categories: parse_categories(&config.enabled_categories),
        counter: AtomicI64::new(0),
        config,
        compiled,
    });
    *global_recorder()
        .write()
        .expect("global flight recorder lock poisoned") = Some(recorder.clone());
    Ok(recorder)
}

/// 启动带 HTTP/channel 导出的飞行记录器。
pub fn start_http_flight_recorder(
    channel: SyncSender<Vec<Event>>,
    config: FlightRecorderConfig,
) -> Result<Arc<HttpFlightRecorder>, Error> {
    new_flight_recorder(config, Some(channel))
}
/// 启动仅写日志（无 channel）的飞行记录器。
pub fn start_log_flight_recorder(config: FlightRecorderConfig) -> Result<(), Error> {
    new_flight_recorder(config, None).map(|_| ())
}
/// 取得当前全局飞行记录器（若已启动）。
pub fn get_flight_recorder() -> Option<Arc<HttpFlightRecorder>> {
    global_recorder()
        .read()
        .expect("global flight recorder lock poisoned")
        .clone()
}
/// 清除全局飞行记录器。
pub fn close_flight_recorder() {
    *global_recorder()
        .write()
        .expect("global flight recorder lock poisoned") = None;
}

/// 按触发器名查找配置；若 `check` 通过则在 Context 的 Trace 上 mark_bits。
pub fn check_flight_recorder_dump_trigger<F>(ctx: &Context, name: &str, check: F)
where
    F: FnOnce(&DumpTriggerConfig) -> bool,
{
    let Some(recorder) = get_flight_recorder() else {
        return;
    };
    let Some(trace) = ctx.sink() else {
        return;
    };
    let Some(&idx) = recorder.compiled.name_mapping.get(name) else {
        return;
    };
    if let Some(config) = recorder.compiled.config_ref.get(idx) {
        if check(config) {
            trace.mark_bits(idx);
        }
    }
}

/// 单 Trace 事件缓冲上限；超出后 discard 时换新 Vec。
pub const MAX_EVENTS: usize = 4096;
/// Go 风格类型别名：HTTPFlightRecorder。
pub type HTTPFlightRecorder = HttpFlightRecorder;
/// Go 风格常量别名。
pub const DevDebugTypeExecuteInternalTraceMissing: &str =
    DEV_DEBUG_TYPE_EXECUTE_INTERNAL_TRACE_MISSING;
pub const DevDebugTypeSendRequestTraceIDMissing: &str =
    DEV_DEBUG_TYPE_SEND_REQUEST_TRACE_ID_MISSING;
pub const maxEvents: usize = MAX_EVENTS;
pub use check_flight_recorder_dump_trigger as CheckFlightRecorderDumpTrigger;
pub use check_truth_table as checkTruthTable;
pub use get_flight_recorder as GetFlightRecorder;
pub use start_http_flight_recorder as StartHTTPFlightRecorder;
pub use start_log_flight_recorder as StartLogFlightRecorder;
pub use truth_table_for_and as truthTableForAnd;
pub use truth_table_for_and_one as truthTableForAnd1;
pub use truth_table_for_or as truthTableForOr;

/// Go `NewTrace`：返回 Arc 包装的空 Trace。
pub fn NewTrace() -> Arc<Trace> {
    Arc::new(Trace::new())
}
