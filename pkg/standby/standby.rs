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

// Standby（热备）Keyspace 加载控制器：状态机、HTTP 管理面与优雅退出。
//
// 实例先以 standby 待命，收到 `/tidb-pool/activate` 后绑定 keyspace（逻辑命名空间）
// 并等待 Server 就绪；starter 模式下配合 Manager 回收、关闭连接等待与正常重启日志。
// 状态流转固定为 `standby -> activated -> terminating`，中途不允许回退。

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use astersql_config_deploymode as deploymode;
use astersql_server::http_status::{Request, Response, Router, serve_error};
use astersql_server::standby::{StandbyController, StandbyReadyServer, StandbyShutdownServer};
use serde::Deserialize;

/// 状态字符串：待命。
pub const STANDBY_STATE: &str = "standby";
/// 状态字符串：已激活。
pub const ACTIVATED_STATE: &str = "activated";
/// 状态字符串：终止中。
pub const TERMINATING_STATE: &str = "terminating";
/// checkconn 响应：连接因正常重启关闭。
pub const CONNECTION_NORMAL_CLOSED: &str = "normal closed";
/// 管理 HTTP 路径前缀。
pub const HTTP_PATH_PREFIX: &str = "/tidb-pool/";
/// 默认正常重启日志路径（keyspace:原因）。
pub const TIDB_NORMAL_RESTART_LOG_PATH: &str = "/tmp/tidb-normal-restart.log";
/// 优雅退出默认等待连接清零时长（8 小时）。
pub const DEFAULT_CLOSE_CONNECTION_WAIT: Duration = Duration::from_secs(8 * 60 * 60);
/// 允许的最大关闭连接等待（24 小时）。
pub const MAX_CLOSE_CONNECTION_WAIT: Duration = Duration::from_secs(24 * 60 * 60);
/// 通知 Manager free 的最大重试次数。
pub const MANAGER_FREE_MAX_ATTEMPTS: usize = 3;
/// Manager free 重试间隔。
const MANAGER_FREE_RETRY_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize)]
/// 激活请求体：keyspace、导出 ID、空闲上限与 DDL/自动分析开关等。
pub struct ActivateRequest {
    #[serde(default)]
    /// 目标 keyspace 名（必填）。
    /// 已记录的 keyspace。
    pub keyspace_name: String,
    #[serde(default)]
    /// 导出/配额标识；starter 状态接口可回传。
    pub export_id: String,
    #[serde(default)]
    /// 激活后允许的最大空闲秒数。
    pub max_idle_seconds: u64,
    #[serde(default)]
    /// 透传元数据。
    pub metadata: HashMap<String, String>,
    #[serde(default)]
    /// 是否运行自动 ANALYZE。
    pub run_auto_analyze: bool,
    #[serde(default)]
    /// 是否启用 DDL。
    pub tidb_enable_ddl: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 控制器生命周期状态。
pub enum State {
    /// 待命，等待激活。
    Standby,
    /// 已激活，可对外服务。
    Activated,
    /// 正在关闭。
    Terminating,
}

impl State {
    /// 转为稳定的状态字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standby => STANDBY_STATE,
            Self::Activated => ACTIVATED_STATE,
            Self::Terminating => TERMINATING_STATE,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 远端与本地 keyspace 不一致时的对比信息。
pub struct KeyspaceMismatch {
    /// 请求中的 keyspace。
    pub remote: String,
    /// 本机配置的 keyspace。
    pub local: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 状态接口结构化视图（测试/调试用）。
pub struct StatusResponse {
    /// 当前状态。
    pub state: State,
    pub keyspace_name: String,
    /// starter 下可选 export_id。
    pub export_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 请求进程退出的信号类型。
pub enum ExitSignal {
    /// 中断式退出（非 starter 或强制路径常用）。
    Interrupt,
    /// 终止式退出（starter 优雅路径常用）。
    Terminate,
}

/// 向运行时投递退出信号的抽象。
pub trait ExitSignaler: Send + Sync {
    /// 请求以给定信号退出。
    fn request_exit(&self, signal: ExitSignal);
}

#[derive(Default)]
/// 记录最近一次退出信号的测试替身。
pub struct RecordingExitSignaler {
    /// 已请求但尚未 take 的信号。
    signal: Mutex<Option<ExitSignal>>,
}

impl RecordingExitSignaler {
    /// 取出并清空已记录信号。
    pub fn take(&self) -> Option<ExitSignal> {
        self.signal
            .lock()
            .expect("exit signal lock poisoned")
            .take()
    }
}

impl ExitSignaler for RecordingExitSignaler {
    fn request_exit(&self, signal: ExitSignal) {
        *self.signal.lock().expect("exit signal lock poisoned") = Some(signal);
    }
}

/// 连接池 Manager 客户端：实例退出前通知回收。
pub trait ManagerClient: Send + Sync {
    /// 以给定原因调用 Manager free。
    fn free(&self, exit_reason: &str) -> Result<(), String>;
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// `/tidb-pool/exit` 查询参数解析结果。
pub struct ExitOptions {
    /// 是否优雅关闭（等待连接清零）。
    pub graceful: bool,
    /// 等待连接清零的上限。
    pub wait: Duration,
    /// 若本机是 auto_id owner 则跳过退出。
    pub skip_auto_id_owner: bool,
    /// 关闭后是否需要通知 Manager free。
    pub need_manager_free: bool,
}

/// 默认非优雅、不等待、不跳过 owner、不要求 Manager free。
impl Default for ExitOptions {
    fn default() -> Self {
        Self {
            graceful: false,
            wait: Duration::ZERO,
            skip_auto_id_owner: false,
            need_manager_free: false,
        }
    }
}

/// 互斥保护下的可变控制器状态。
struct ControllerState {
    /// 当前生命周期状态。
    state: State,
    /// 最近一次激活请求内容。
    activation: ActivateRequest,
    /// Server 启动/准备结果；None 表示尚未就绪。
    server_start_result: Option<Result<(), String>>,
    /// 等待 Server 就绪超时；0 表示无限等待。
    activation_timeout: Duration,
}

/// 控制器共享内部状态与同步原语。
struct ControllerInner {
    /// 状态与激活信息。
    state: Mutex<ControllerState>,
    /// 从 standby 进入 activated 时唤醒 wait_for_activate。
    activated: Condvar,
    /// Server 启动结果写入时唤醒 activate HTTP。
    server_started: Condvar,
    /// 保证 end_standby 只生效一次。
    end_once: AtomicBool,
    /// 可选 Manager 客户端。
    manager: Option<Arc<dyn ManagerClient>>,
    /// 退出信号投递器。
    exit: Arc<dyn ExitSignaler>,
    /// 关闭连接等待（毫秒）。
    close_connection_wait_millis: AtomicU64,
    /// 上次连接活跃的 Unix 秒。
    last_active: AtomicI64,
    /// 正常重启日志路径。
    restart_log_path: PathBuf,
    /// 启动时读取并删除后保留的上一次正常重启信息。
    previous_restart: Mutex<Option<(String, String)>>,
    /// 本机 keyspace，用于 exit/checkconn 校验。
    local_keyspace: Mutex<String>,
    /// 测试/显式覆盖的 starter 模式开关。
    starter_mode: AtomicBool,
}

#[derive(Clone)]
/// Keyspace 加载控制器：Standby 状态机与管理 HTTP 实现。
pub struct LoadKeyspaceController {
    /// 共享内部实现。
    inner: Arc<ControllerInner>,
}

impl LoadKeyspaceController {
    /// 使用默认 RecordingExitSignaler 构造。
    pub fn new(manager: Option<Arc<dyn ManagerClient>>) -> Self {
        Self::with_exit_signaler(manager, Arc::new(RecordingExitSignaler::default()))
    }

    /// 注入自定义退出信号器（测试可断言信号）。
    pub fn with_exit_signaler(
        manager: Option<Arc<dyn ManagerClient>>,
        exit: Arc<dyn ExitSignaler>,
    ) -> Self {
        Self {
            inner: Arc::new(ControllerInner {
                state: Mutex::new(ControllerState {
                    state: State::Standby,
                    activation: ActivateRequest::default(),
                    server_start_result: None,
                    activation_timeout: Duration::ZERO,
                }),
                activated: Condvar::new(),
                server_started: Condvar::new(),
                end_once: AtomicBool::new(false),
                manager,
                exit,
                close_connection_wait_millis: AtomicU64::new(0),
                last_active: AtomicI64::new(0),
                restart_log_path: PathBuf::from(TIDB_NORMAL_RESTART_LOG_PATH),
                previous_restart: Mutex::new(None),
                local_keyspace: Mutex::new(String::new()),
                starter_mode: AtomicBool::new(false),
            }),
        }
    }

    /// 是否 starter：显式开关或部署模式判定。
    fn is_starter(&self) -> bool {
        self.inner.starter_mode.load(Ordering::Acquire) || deploymode::IsStarter()
    }

    /// Seeds activation metadata without flipping standby state (mirrors Go package globals).
    /// 仅写入激活元数据，不改变 standby 状态（对齐 Go 包级全局）。

    pub fn set_activation_request(&self, request: ActivateRequest) {
        self.inner
            .state
            .lock()
            .expect("standby state lock poisoned")
            .activation = request;
    }

    /// 在共享前配置重启日志路径（须仍为唯一 Arc 持有者）。
    pub fn with_restart_log_path(mut self, path: impl Into<PathBuf>) -> Self {
        Arc::get_mut(&mut self.inner)
            .expect("restart path must be configured before sharing controller")
            .restart_log_path = path.into();
        self
    }

    /// 设置等待 Server 就绪的超时。
    pub fn set_activation_timeout(&self, timeout: Duration) {
        self.inner
            .state
            .lock()
            .expect("standby state lock poisoned")
            .activation_timeout = timeout;
    }

    /// 设置本机 keyspace。
    pub fn set_local_keyspace(&self, keyspace: impl Into<String>) {
        *self
            .inner
            .local_keyspace
            .lock()
            .expect("keyspace lock poisoned") = keyspace.into();
    }

    /// 显式设置 starter 模式（便于经典构建跑 nextgen 路径）。
    pub fn set_starter_mode(&self, starter: bool) {
        self.inner.starter_mode.store(starter, Ordering::Release);
    }

    /// 读取当前状态。
    pub fn state(&self) -> State {
        self.inner
            .state
            .lock()
            .expect("standby state lock poisoned")
            .state
    }

    /// 克隆当前激活请求。
    pub fn activation_request(&self) -> ActivateRequest {
        self.inner
            .state
            .lock()
            .expect("standby state lock poisoned")
            .activation
            .clone()
    }

    /// 非空元数据则返回其克隆。
    pub fn activation_metadata(&self) -> Option<HashMap<String, String>> {
        let metadata = self.activation_request().metadata;
        (!metadata.is_empty()).then_some(metadata)
    }

    /// 执行激活：standby→activated；同名重复激活成功；terminating/冲突则失败。
    pub fn activate(&self, request: ActivateRequest) -> Result<(), String> {
        if request.keyspace_name.is_empty() {
            return Err("keyspace_name is required".into());
        }
        let mut state = self
            .inner
            .state
            .lock()
            .expect("standby state lock poisoned");
        // 仅 standby 可首次激活；已激活同 keyspace 幂等。
        match state.state {
            State::Standby => {
                state.state = State::Activated;
                state.activation = request;
                self.inner.activated.notify_all();
                Ok(())
            }
            State::Terminating => Err("server is going to shutdown".into()),
            State::Activated if state.activation.keyspace_name == request.keyspace_name => Ok(()),
            State::Activated => Err("server is not in standby mode".into()),
        }
    }

    /// 阻塞直到 end_standby 写入启动结果，或超时。
    fn wait_server_started(&self) -> Result<(), String> {
        let mut state = self
            .inner
            .state
            .lock()
            .expect("standby state lock poisoned");
        let timeout = state.activation_timeout;
        // 超时为 0：无限等待条件变量。
        if timeout.is_zero() {
            while state.server_start_result.is_none() {
                state = self
                    .inner
                    .server_started
                    .wait(state)
                    .expect("standby state lock poisoned");
            }
        } else {
            let (next, wait) = self
                .inner
                .server_started
                .wait_timeout_while(state, timeout, |state| state.server_start_result.is_none())
                .expect("standby state lock poisoned");
            state = next;
            if wait.timed_out() && state.server_start_result.is_none() {
                return Err("timeout waiting for activation".into());
            }
        }
        state.server_start_result.clone().unwrap_or(Ok(()))
    }

    /// 设置优雅关闭等待连接清零的时长。
    pub fn set_close_connection_wait(&self, wait: Duration) {
        self.inner.close_connection_wait_millis.store(
            wait.as_millis().min(u64::MAX as u128) as u64,
            Ordering::Release,
        );
    }

    /// 读取关闭连接等待时长。
    pub fn close_connection_wait(&self) -> Duration {
        Duration::from_millis(
            self.inner
                .close_connection_wait_millis
                .load(Ordering::Acquire),
        )
    }

    /// 将 last_active 推进到当前 Unix 秒（单调不回退）。
    pub fn on_connection_active_now(&self) {
        let now = unix_seconds();
        let _ = self
            .inner
            .last_active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |last| {
                (last < now).then_some(now)
            });
    }

    /// 读取上次活跃 Unix 秒。
    pub fn last_active(&self) -> i64 {
        self.inner.last_active.load(Ordering::Acquire)
    }

    /// 写入 `keyspace:message` 正常重启日志；本地 keyspace 为空则跳过。
    pub fn save_normal_restart_info(&self, message: &str) -> Result<(), String> {
        let keyspace = self
            .inner
            .local_keyspace
            .lock()
            .expect("keyspace lock poisoned")
            .clone();
        if keyspace.is_empty() {
            return Ok(());
        }
        fs::write(
            &self.inner.restart_log_path,
            format!("{keyspace}:{message}"),
        )
        .map_err(|error| format!("write restart log: {error}"))
    }

    /// 读取并删除重启日志。
    pub fn load_normal_restart_info_and_remove(&self) -> Result<Option<(String, String)>, String> {
        load_restart_info(&self.inner.restart_log_path, true)
    }

    /// 若不删除地读取日志且 keyspace 匹配，返回原因消息。
    pub fn is_previous_normal_restart(&self, keyspace: &str) -> Result<Option<String>, String> {
        if let Some(message) = self
            .inner
            .previous_restart
            .lock()
            .expect("previous restart lock poisoned")
            .as_ref()
            .filter(|(previous, _)| !keyspace.is_empty() && previous == keyspace)
            .map(|(_, message)| message.clone())
        {
            return Ok(Some(message));
        }
        Ok(load_restart_info(&self.inner.restart_log_path, false)?
            .filter(|(previous, _)| !keyspace.is_empty() && previous == keyspace)
            .map(|(_, message)| message))
    }

    /// 通过 ExitSignaler 请求退出。
    pub fn request_exit(&self, signal: ExitSignal) {
        self.inner.exit.request_exit(signal);
    }

    /// 带重试地调用 Manager.free；无 Manager 或全部失败返回 false。
    pub fn report_manager_free(&self, reason: &str) -> bool {
        let Some(manager) = &self.inner.manager else {
            return false;
        };
        // 最多 MANAGER_FREE_MAX_ATTEMPTS 次，间隔 MANAGER_FREE_RETRY_INTERVAL。
        for attempt in 1..=MANAGER_FREE_MAX_ATTEMPTS {
            if manager.free(reason).is_ok() {
                return true;
            }
            if attempt < MANAGER_FREE_MAX_ATTEMPTS {
                thread::sleep(MANAGER_FREE_RETRY_INTERVAL);
            }
        }
        false
    }

    /// 无 Server 句柄时构建路由（等价 handler(None)）。
    pub fn router(&self) -> Router {
        self.handler(None)
    }

    /// Builds the standby HTTP mux. `server` mirrors Go's `Handler(svr *server.Server)`.
    /// 构建 standby HTTP 多路复用；`server` 对齐 Go Handler(svr)。

    pub fn handler(&self, server: Option<Arc<dyn StandbyShutdownServer>>) -> Router {
        let router = Router::default();

        // 注册 status / activate / exit / checkconn。
        let status_controller = self.clone();
        router.add(
            "/tidb-pool/status",
            Arc::new(move |_| status_controller.status_http_response()),
        );

        let activate_controller = self.clone();
        let activate_server = server.clone();
        router.add(
            "/tidb-pool/activate",
            Arc::new(move |request| {
                activate_controller.activate_http(request, activate_server.as_ref())
            }),
        );

        let exit_controller = self.clone();
        let exit_server = server.clone();
        router.add(
            "/tidb-pool/exit",
            Arc::new(move |request| exit_controller.exit_http(request, exit_server.as_ref())),
        );

        let check_controller = self.clone();
        let check_server = server.clone();
        router.add(
            "/tidb-pool/checkconn",
            Arc::new(move |request| {
                check_controller.check_connection_http(request, check_server.as_ref())
            }),
        );
        router
    }

    /// 组装 JSON 状态；starter 且有 export_id 时附加该字段。
    fn status_http_response(&self) -> Response {
        let state = self
            .inner
            .state
            .lock()
            .expect("standby state lock poisoned");
        let mut body = format!(
            "{{\"state\":\"{}\",\"keyspace_name\":\"{}\"",
            state.state.as_str(),
            json_escape(&state.activation.keyspace_name),
        );
        // starter 才对外暴露 export_id。
        if self.is_starter() && !state.activation.export_id.is_empty() {
            body.push_str(&format!(
                ",\"export_id\":\"{}\"",
                json_escape(&state.activation.export_id)
            ));
        }
        body.push('}');
        body.push('\n');
        Response::json(200, body)
    }

    /// POST activate：解析体、激活并等待 Server 就绪，映射 HTTP 状态码。
    fn activate_http(
        &self,
        request: &Request,
        server: Option<&Arc<dyn StandbyShutdownServer>>,
    ) -> Response {
        let activation = match parse_activation_request(&request.body) {
            Ok(request) => request,
            Err(_) => return Response::new(400, Vec::new()),
        };
        if self.state() != State::Standby && server.is_some_and(|server| !server.health()) {
            return Response::text(503, "server is going to shutdown");
        }
        match self.activate(activation) {
            Ok(()) => match self.wait_server_started() {
                Ok(()) => self.status_http_response(),
                Err(error) if error.contains("timeout") => Response::text(408, error),
                Err(error) => Response::text(500, error),
            },
            Err(error) if error.contains("shutdown") => Response::text(503, error),
            Err(error) => Response::text(412, error),
        }
    }

    /// GET/处理 exit：校验 keyspace、解析选项，starter 下处理强制/优雅关闭。
    fn exit_http(
        &self,
        request: &Request,
        server: Option<&Arc<dyn StandbyShutdownServer>>,
    ) -> Response {
        // keyspace 不匹配返回 412 与双边名称。
        if let Err(mismatch) = self.validate_keyspace(request) {
            return Response::json(
                412,
                format!(
                    "{{\"remote\":\"{}\",\"local\":\"{}\"}}",
                    json_escape(&mismatch.remote),
                    json_escape(&mismatch.local)
                ),
            );
        }
        let options = match parse_exit_options(&request.query) {
            Ok(options) => options,
            Err(error) => return http_error(400, &error),
        };
        if let Some(svr) = server {
            if self.is_starter() {
                if options.need_manager_free && self.inner.manager.is_none() {
                    return http_error(503, "manager notifier is unavailable");
                }
                if options.skip_auto_id_owner && svr.is_auto_id_owner() {
                    return Response::text(304, "auto id service is owner");
                }
                // 强制退出：设 force_shutdown、记日志、Interrupt。
                if !options.graceful {
                    svr.set_force_shutdown();
                    let _ = self.save_normal_restart_info("received force exit request");
                    self.request_exit(ExitSignal::Interrupt);
                    return Response::text(200, "OK");
                }
                // 优雅退出：应用等待时长，并按需标记 Manager free。
                let wait = if options.wait.is_zero() {
                    DEFAULT_CLOSE_CONNECTION_WAIT
                } else {
                    options.wait
                };
                self.set_close_connection_wait(wait);
                if options.need_manager_free {
                    svr.set_need_request_manager_free();
                }
            }
            let _ = self.save_normal_restart_info("received exit request");
        }
        // starter 发 Terminate，否则 Interrupt。
        if self.is_starter() {
            self.request_exit(ExitSignal::Terminate);
        } else {
            self.request_exit(ExitSignal::Interrupt);
        }
        Response::text(200, "OK")
    }

    /// checkconn：根据正常重启日志判断连接是否“正常关闭”。
    fn check_connection_http(
        &self,
        request: &Request,
        server: Option<&Arc<dyn StandbyShutdownServer>>,
    ) -> Response {
        let keyspace = request
            .query
            .get("keyspace_name")
            .cloned()
            .unwrap_or_default();
        let connection = request.query.get("conn_id").cloned().unwrap_or_default();
        if keyspace.is_empty() || connection.is_empty() {
            return Response::text(400, "keyspace_name or conn_id is empty");
        }
        if server
            .and_then(|server| server.normal_closed_connection(&keyspace, &connection))
            .is_some()
        {
            return Response::text(200, CONNECTION_NORMAL_CLOSED);
        }
        match self.is_previous_normal_restart(&keyspace) {
            Ok(Some(_)) => Response::text(200, CONNECTION_NORMAL_CLOSED),
            Ok(None) => Response::text(200, "unconfirmed"),
            Err(error) => serve_error(500, &error),
        }
    }

    /// 比较 query keyspace 与本地配置。
    fn validate_keyspace(&self, request: &Request) -> Result<(), KeyspaceMismatch> {
        let remote = request.query.get("keyspace").cloned().unwrap_or_default();
        let local = self
            .inner
            .local_keyspace
            .lock()
            .expect("keyspace lock poisoned")
            .clone();
        if remote == local {
            Ok(())
        } else {
            Err(KeyspaceMismatch { remote, local })
        }
    }

    /// 在配置的等待上限内等待连接数为零；等待为 0 视为成功。
    fn wait_zero_conn(&self, server: &dyn StandbyShutdownServer) -> bool {
        let max_wait = self.close_connection_wait();
        if max_wait.is_zero() {
            return true;
        }
        server.wait_zero_connections_timeout(max_wait)
    }
}

/// 对接 Server 生命周期钩子的 StandbyController 实现。
impl StandbyController for LoadKeyspaceController {
    /// 启动时清理旧重启日志，并阻塞直到离开 standby。
    fn wait_for_activate(&self) {
        if let Ok(previous) = load_restart_info(&self.inner.restart_log_path, false) {
            *self
                .inner
                .previous_restart
                .lock()
                .expect("previous restart lock poisoned") = previous;
            let _ = fs::remove_file(&self.inner.restart_log_path);
        }
        let mut state = self
            .inner
            .state
            .lock()
            .expect("standby state lock poisoned");
        while state.state == State::Standby {
            state = self
                .inner
                .activated
                .wait(state)
                .expect("standby state lock poisoned");
        }
    }

    /// 写入 Server 启动结果并唤醒等待激活的 HTTP 请求（仅一次）。
    fn end_standby(&self, result: Result<(), String>) {
        // 已结束过则忽略后续调用。
        if self.inner.end_once.swap(true, Ordering::AcqRel) {
            return;
        }
        self.inner
            .state
            .lock()
            .expect("standby state lock poisoned")
            .server_start_result = Some(result);
        self.inner.server_started.notify_all();
    }

    /// 返回路径前缀与路由表。
    fn handler(&self, server: Arc<dyn StandbyShutdownServer>) -> Option<(String, Router)> {
        Some((HTTP_PATH_PREFIX.into(), self.handler(Some(server))))
    }

    /// 连接活跃回调：刷新 last_active。
    fn on_connection_active(&self) {
        self.on_connection_active_now();
    }

    /// 初始化监听并 end_standby 汇报结果。
    fn prepare_for_activation(&self, server: &dyn StandbyReadyServer) -> Result<(), String> {
        let result = server.init_tidb_listener();
        self.end_standby(result.clone());
        result
    }

    /// Server 创建后记一次活跃，避免误判空闲。
    fn on_server_created(&self, _server: &dyn StandbyReadyServer) {
        self.on_connection_active_now();
    }

    /// starter 关闭：置 terminating、关 auto_id、等待连接清零后可选 Manager free。
    fn on_server_shutdown(&self, server: &dyn StandbyShutdownServer) {
        // 非 starter 不处理 Manager free / terminating 路径。
        if !self.is_starter() {
            return;
        }
        self.inner
            .state
            .lock()
            .expect("standby state lock poisoned")
            .state = State::Terminating;
        server.auto_id_service_close();
        // 强制关闭则不等待连接。
        if server.force_shutdown() {
            return;
        }
        // 等待超时则不报告 free。
        if !self.wait_zero_conn(server) {
            return;
        }
        // 读取重启日志内容作为 free 原因。
        if server.need_request_manager_free() {
            let reason = fs::read_to_string(&self.inner.restart_log_path)
                .unwrap_or_else(|error| format!("failed to load normal restart log: {error}"));
            self.report_manager_free(&reason);
        }
    }
}

/// 从 query map 解析 ExitOptions。
pub fn parse_exit_options(values: &HashMap<String, String>) -> Result<ExitOptions, String> {
    Ok(ExitOptions {
        graceful: parse_bool(values.get("graceful"), "graceful")?,
        wait: parse_exit_wait(values.get("wait").map(String::as_str).unwrap_or(""))?,
        skip_auto_id_owner: parse_bool(values.get("skip_auto_id_owner"), "skip_auto_id_owner")?,
        need_manager_free: parse_bool(values.get("need_mgr_free"), "need_mgr_free")?,
    })
}

/// 解析 Go 风格布尔字面量；缺省为 false。
fn parse_bool(value: Option<&String>, name: &str) -> Result<bool, String> {
    let Some(value) = value.filter(|value| !value.is_empty()) else {
        return Ok(false);
    };
    match value.as_str() {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Ok(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Ok(false),
        _ => Err(format!("invalid {name}")),
    }
}

/// parseExitWait mirrors Go's parseExitWait (duration or legacy seconds).
/// 解析退出等待：支持 Go duration 或遗留纯秒数字符串。

pub fn parse_exit_wait(value: &str) -> Result<Duration, String> {
    if value.is_empty() {
        return Ok(Duration::ZERO);
    }
    // 先按 Go duration 解析，失败再按整秒遗留格式。
    let wait = match parse_go_duration(value) {
        Ok(wait) => wait,
        Err(_) => {
            let wait_seconds: i64 = value.parse().map_err(|_| "invalid wait".to_string())?;
            if wait_seconds < 0 || wait_seconds > (MAX_CLOSE_CONNECTION_WAIT.as_secs() as i64) {
                return Err("invalid wait".into());
            }
            Duration::from_secs(wait_seconds as u64)
        }
    };
    if wait > MAX_CLOSE_CONNECTION_WAIT {
        return Err("invalid wait".into());
    }
    Ok(wait)
}

/// 解析形如 `1h30m`/`500ms` 的 Go duration 字符串。
fn parse_go_duration(value: &str) -> Result<Duration, String> {
    if value.is_empty() {
        return Ok(Duration::ZERO);
    }
    let mut total = Duration::ZERO;
    let mut number = String::new();
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch.is_ascii_digit() || ch == '.' {
            number.push(ch);
            continue;
        }
        if number.is_empty() {
            return Err("invalid wait".into());
        }
        let amount: f64 = number.parse().map_err(|_| "invalid wait".to_string())?;
        number.clear();
        // 识别 ns/us/µs/ms/s/m/h 单位后缀。
        let unit = match ch {
            'n' if chars.peek() == Some(&'s') => {
                chars.next();
                1e-9
            }
            'u' if chars.peek() == Some(&'s') => {
                chars.next();
                1e-6
            }
            'µ' if chars.peek() == Some(&'s') => {
                chars.next();
                1e-6
            }
            'm' if chars.peek() == Some(&'s') => {
                chars.next();
                1e-3
            }
            's' => 1.0,
            'm' => 60.0,
            'h' => 3600.0,
            _ => return Err("invalid wait".into()),
        };
        total += Duration::from_secs_f64(amount * unit);
    }
    if !number.is_empty() {
        return Err("invalid wait".into());
    }
    Ok(total)
}

/// 对齐 Go http.Error：正文末尾追加换行。
fn http_error(status: u16, text: &str) -> Response {
    // `serve_error` already mirrors Go's `http.Error` by appending one newline.
    serve_error(status, text)
}

/// 读取 `keyspace:message` 重启日志；可选删除文件。
fn load_restart_info(path: &Path, remove: bool) -> Result<Option<(String, String)>, String> {
    let data = match fs::read_to_string(path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read restart log: {error}")),
    };
    let (keyspace, message) = data
        .split_once(':')
        .ok_or_else(|| "invalid normal restart log".to_string())?;
    if remove {
        fs::remove_file(path).map_err(|error| format!("remove restart log: {error}"))?;
    }
    Ok(Some((keyspace.into(), message.into())))
}

/// 反序列化激活 JSON，并校验 keyspace_name 非空。
fn parse_activation_request(body: &[u8]) -> Result<ActivateRequest, String> {
    let request: ActivateRequest =
        serde_json::from_slice(body).map_err(|error| error.to_string())?;
    if request.keyspace_name.is_empty() {
        return Err("keyspace_name is required".into());
    }
    Ok(request)
}

/// 返回 JSON 字符串字面量的内部内容，包括控制字符转义。
fn json_escape(value: &str) -> String {
    let encoded = serde_json::to_string(value).expect("serializing a string cannot fail");
    encoded[1..encoded.len() - 1].to_string()
}

/// 当前 Unix 秒；时钟异常时为 0。
fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64)
}

/// 进程级缓存的上一次正常重启信息。
static PREVIOUS_RESTART: OnceLock<Mutex<Option<(String, String)>>> = OnceLock::new();

/// 向指定路径写入正常重启信息。
pub fn save_tidb_normal_restart_info(
    path: &Path,
    keyspace: &str,
    message: &str,
) -> Result<(), String> {
    if keyspace.is_empty() {
        return Ok(());
    }
    fs::write(path, format!("{keyspace}:{message}"))
        .map_err(|error| format!("write restart log: {error}"))
}

/// 加载并删除重启日志，写入进程级 PREVIOUS_RESTART。
pub fn load_previous_restart(path: &Path) -> Result<(), String> {
    let value = load_restart_info(path, true)?;
    *PREVIOUS_RESTART
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("previous restart lock poisoned") = value;
    Ok(())
}

/// 若缓存中上一次重启属于给定 keyspace，返回原因消息。
pub fn is_previous_tidb_normal_restart(keyspace: &str) -> Option<String> {
    PREVIOUS_RESTART
        .get_or_init(|| Mutex::new(None))
        .lock()
        .expect("previous restart lock poisoned")
        .as_ref()
        .filter(|(previous, _)| !keyspace.is_empty() && previous == keyspace)
        .map(|(_, message)| message.clone())
}
