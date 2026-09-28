// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 昂贵查询与长事务巡检。
//
// 后台循环扫描会话进程列表：超过阈值则打昂贵查询/事务日志，并按
// `max_execution_time`、自动 ANALYZE 超时、runaway 规则执行 kill。
// 对应 Go `pkg/util/expensivequery`。

use astersql_statistics_handle_util::GLOBAL_AUTO_ANALYZE_PROCESS_LIST;
use std::fmt::{Display, Formatter};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::time::{Duration, Instant};

/// 查询被视为“昂贵”的耗时阈值（秒），可运行时调整。
pub static EXPENSIVE_QUERY_TIME_THRESHOLD: AtomicU64 = AtomicU64::new(60);
/// 事务被视为“昂贵”的耗时阈值（秒）。
pub static EXPENSIVE_TXN_TIME_THRESHOLD: AtomicU64 = AtomicU64::new(60);
/// 自动 ANALYZE（统计信息收集）最长允许时间（秒）；0 表示不限制。
pub static MAX_AUTO_ANALYZE_TIME: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
/// 日志级别；数值越小越详细，用于门控 warn 输出。
pub enum LogLevel {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}

/// 当前全局日志级别。
static LOG_LEVEL: AtomicU8 = AtomicU8::new(LogLevel::Info as u8);

/// 设置巡检日志级别。
pub fn set_log_level(level: LogLevel) {
    LOG_LEVEL.store(level as u8, Ordering::Release);
}

/// 当前级别是否允许输出 warn。
fn warn_enabled() -> bool {
    LOG_LEVEL.load(Ordering::Acquire) <= LogLevel::Warn as u8
}

/// Runaway 查询检测器：判断是否应按资源组规则杀掉该查询。
pub trait RunawayChecker: Send + Sync {
    /// 返回 (原因描述, 是否应 kill)。
    fn check_rule_kill_action(&self) -> (String, bool);
}

/// 会话进程信息快照，供昂贵查询巡检使用。
pub struct ProcessInfo {
    pub id: u64,
    pub info: String,
    pub started_at: Instant,
    pub current_txn_start_ts: u64,
    pub current_txn_created_at: Instant,
    pub in_restricted_sql: bool,
    pub max_execution_time_millis: u64,
    pub resource_group_name: String,
    pub runaway_checker: Option<Arc<dyn RunawayChecker>>,
    /// 上次打印昂贵查询日志的时间，用于限流。
    expensive_log_time: Mutex<Option<Instant>>,
    /// 上次打印昂贵事务日志的时间。
    expensive_txn_log_time: Mutex<Option<Instant>>,
}

impl ProcessInfo {
    /// 构造进程信息；时间戳取当前时刻。
    pub fn new(id: u64, info: impl Into<String>) -> Self {
        let now = Instant::now();
        Self {
            id,
            info: info.into(),
            started_at: now,
            current_txn_start_ts: 0,
            current_txn_created_at: now,
            in_restricted_sql: false,
            max_execution_time_millis: 0,
            resource_group_name: String::new(),
            runaway_checker: None,
            expensive_log_time: Mutex::new(None),
            expensive_txn_log_time: Mutex::new(None),
        }
    }
}

impl Display for ProcessInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "id={}, sql={}", self.id, self.info)
    }
}

/// 会话管理抽象：列举进程、查询详情、按连接 kill。
pub trait SessionManager: Send + Sync {
    /// 返回当前进程列表。
    fn show_process_list(&self) -> Vec<Arc<ProcessInfo>>;
    /// 按连接 ID 取进程信息。
    fn get_process_info(&self, connection_id: u64) -> Option<Arc<ProcessInfo>>;
    /// 杀掉查询和/或连接；`runaway` 标记来自 runaway 规则。
    fn kill(&self, connection_id: u64, query: bool, connection: bool, runaway: bool);
}

/// 耗时直方图观察接口（内部/普通事务指标）。
pub trait Histogram: Send + Sync {
    /// 记录一次耗时（秒）。
    fn observe(&self, seconds: f64);
}

/// 昂贵事件日志输出接口。
pub trait EventLogger: Send + Sync {
    /// 输出 warn 级别昂贵事件。
    fn warn(&self, message: &str, process: &ProcessInfo, cost: Duration, detail: &str);
    /// 输出 info（如 bootstrap 阶段内存超限）。
    fn info(&self, message: &str, connection_id: u64);
}

#[derive(Default)]
/// 空操作直方图，默认占位。
pub struct NoopHistogram;

impl Histogram for NoopHistogram {
    fn observe(&self, _seconds: f64) {}
}

#[derive(Default)]
/// 空操作日志器，默认占位。
pub struct NoopLogger;

impl EventLogger for NoopLogger {
    fn warn(&self, _message: &str, _process: &ProcessInfo, _cost: Duration, _detail: &str) {}
    fn info(&self, _message: &str, _connection_id: u64) {}
}

/// 昂贵查询后台句柄：持有退出通道、会话管理器与指标/日志依赖。
pub struct Handle {
    exit: Mutex<mpsc::Receiver<()>>,
    session_manager: RwLock<Option<Arc<dyn SessionManager>>>,
    internal_histogram: Arc<dyn Histogram>,
    general_histogram: Arc<dyn Histogram>,
    logger: Arc<dyn EventLogger>,
}

impl Handle {
    /// 用退出接收端构造句柄，默认 Noop 指标与日志。
    pub fn new(exit: mpsc::Receiver<()>) -> Self {
        Self {
            exit: Mutex::new(exit),
            session_manager: RwLock::new(None),
            internal_histogram: Arc::new(NoopHistogram),
            general_histogram: Arc::new(NoopHistogram),
            logger: Arc::new(NoopLogger),
        }
    }

    /// 注入直方图与日志实现。
    pub fn with_observers(
        mut self,
        internal: Arc<dyn Histogram>,
        general: Arc<dyn Histogram>,
        logger: Arc<dyn EventLogger>,
    ) -> Self {
        self.internal_histogram = internal;
        self.general_histogram = general;
        self.logger = logger;
        self
    }

    /// 绑定会话管理器；`run` 前必须设置。
    pub fn set_session_manager(&self, manager: Arc<dyn SessionManager>) -> &Self {
        *self.session_manager.write().unwrap() = Some(manager);
        self
    }

    /// 主循环：每 100ms 唤醒，扫描进程并刷新阈值。
    pub fn run(&self) {
        let manager = self
            .session_manager
            .read()
            .unwrap()
            .clone()
            .expect("expensive query session manager is not set");
        let mut query_threshold = EXPENSIVE_QUERY_TIME_THRESHOLD.load(Ordering::Acquire);
        let mut transaction_threshold = EXPENSIVE_TXN_TIME_THRESHOLD.load(Ordering::Acquire);
        let mut last_metric_time = None;
        // 退出信号或通道断开则结束；超时则进入一轮巡检。
        loop {
            match self
                .exit
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_millis(100))
            {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => return,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            let now = Instant::now();
            // 事务直方图最多约每 15 秒采样一次，避免过热。
            let need_metrics = last_metric_time
                .is_none_or(|last| now.duration_since(last) > Duration::from_secs(15));
            if need_metrics {
                last_metric_time = Some(now);
            }
            // 先查长事务，再对有 SQL 文本的进程查查询超时/kill。
            for process in manager.show_process_list() {
                self.inspect_transaction(&process, transaction_threshold, need_metrics, now);
                if process.info.is_empty() {
                    continue;
                }
                self.inspect_query(manager.as_ref(), &process, query_threshold, now);
            }
            query_threshold = EXPENSIVE_QUERY_TIME_THRESHOLD.load(Ordering::Acquire);
            transaction_threshold = EXPENSIVE_TXN_TIME_THRESHOLD.load(Ordering::Acquire);
        }
    }

    /// 检查长事务：记指标并限流打 expensive_txn 日志。
    fn inspect_transaction(
        &self,
        process: &ProcessInfo,
        threshold: u64,
        need_metrics: bool,
        now: Instant,
    ) {
        // start_ts 为 0 表示当前无活跃事务。
        if process.current_txn_start_ts == 0 {
            return;
        }
        let cost = now.duration_since(process.current_txn_created_at);
        if cost < Duration::from_secs(threshold) {
            return;
        }
        // 受限 SQL（内部）与普通用户事务分直方图。
        if need_metrics {
            if process.in_restricted_sql {
                self.internal_histogram.observe(cost.as_secs_f64());
            } else {
                self.general_histogram.observe(cost.as_secs_f64());
            }
        }
        // 昂贵事务日志至少间隔 600 秒，防止刷屏。
        let mut last = process.expensive_txn_log_time.lock().unwrap();
        if last.is_none_or(|last| now.duration_since(last) > Duration::from_secs(600))
            && warn_enabled()
        {
            self.log_expensive_query(cost, process, "expensive_txn");
            *last = Some(now);
        }
    }

    /// 检查昂贵查询、执行超时、自动 ANALYZE 超时与 runaway kill。
    fn inspect_query(
        &self,
        manager: &dyn SessionManager,
        process: &ProcessInfo,
        threshold: u64,
        now: Instant,
    ) {
        let cost = now.duration_since(process.started_at);
        // 昂贵查询日志至少间隔 60 秒。
        let mut last = process.expensive_log_time.lock().unwrap();
        if last.is_none_or(|last| now.duration_since(last) > Duration::from_secs(60))
            && cost >= Duration::from_secs(threshold)
            && warn_enabled()
        {
            self.log_expensive_query(cost, process, "expensive_query");
            *last = Some(now);
        }
        drop(last);
        // 超过会话 max_execution_time 则同时 kill 查询与连接。
        if process.max_execution_time_millis > 0
            && cost > Duration::from_millis(process.max_execution_time_millis)
        {
            self.logger.warn(
                "execution timeout, kill it",
                process,
                cost,
                "max execution time",
            );
            manager.kill(process.id, true, true, false);
        }
        // 自动 ANALYZE 进程受全局最长时长约束。
        if GLOBAL_AUTO_ANALYZE_PROCESS_LIST.contains(process.id) {
            let maximum = MAX_AUTO_ANALYZE_TIME.load(Ordering::Acquire);
            if maximum > 0 && cost > Duration::from_secs(maximum) {
                self.logger.warn(
                    "auto analyze timeout, kill it",
                    process,
                    cost,
                    "max auto analyze time",
                );
                manager.kill(process.id, true, false, false);
            }
        }
        // Runaway：资源组规则触发时 kill 查询并标记 runaway。
        if let Some(checker) = &process.runaway_checker {
            let (cause, kill) = checker.check_rule_kill_action();
            if kill {
                self.logger
                    .warn("runaway query timeout", process, cost, &cause);
                manager.kill(process.id, true, false, true);
            }
        }
    }

    /// 查询内存超配额时主动打昂贵日志（可在 bootstrap 阶段降级为 info）。
    pub fn log_on_query_exceed_mem_quota(&self, connection_id: u64) {
        if !warn_enabled() {
            return;
        }
        let Some(manager) = self.session_manager.read().unwrap().clone() else {
            self.logger
                .info("expensive_query during bootstrap phase", connection_id);
            return;
        };
        let Some(process) = manager.get_process_info(connection_id) else {
            return;
        };
        self.log_expensive_query(
            Instant::now().duration_since(process.started_at),
            &process,
            "memory exceeds quota",
        );
    }

    /// 公共昂贵日志入口；即使 SQL 文本为空也要保留日志事件。
    fn log_expensive_query(&self, cost: Duration, process: &ProcessInfo, message: &str) {
        self.logger.warn(message, process, cost, "");
    }
}

/// 构造昂贵查询句柄（Go `NewExpensiveQueryHandle` 对应入口）。
pub fn new_expensive_query_handle(exit: mpsc::Receiver<()>) -> Handle {
    Handle::new(exit)
}
