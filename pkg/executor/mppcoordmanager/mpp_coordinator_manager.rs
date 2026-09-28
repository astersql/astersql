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

// MPP 协调器管理器：按查询/gather 注册协调器，并后台清理超时实例。
//
// Gather 是一次 MPP 查询中的汇聚单元；协调器接收 TiFlash 任务状态上报
//（ReportStatus）。后台线程按 `DETECT_FREQUENCY` 扫描已关闭且超过
// `max_lifetime` 的条目并删除。

use std::collections::HashMap;
use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 后台过期检测周期（默认 5 分钟）。
pub const DETECT_FREQUENCY: Duration = Duration::from_secs(5 * 60);

/// MPP 查询全局标识：查询时间戳 + 本地查询号 + 发起 server id。
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct MppQueryId {
    pub query_ts: u64,
    pub local_query_id: u64,
    pub server_id: u64,
}

/// 协调器在管理器中的唯一键：查询 id + gather id。
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct CoordinatorUniqueId {
    pub mpp_query_id: MppQueryId,
    pub gather_id: u64,
}

/// 任务状态上报中的元数据字段。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReportTaskMeta {
    pub query_ts: u64,
    pub local_query_id: u64,
    pub server_id: u64,
    pub gather_id: u64,
    pub task_id: i64,
    pub mpp_version: i64,
}

/// TiFlash → TiDB 的任务状态上报请求。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReportTaskStatusRequest {
    pub meta: ReportTaskMeta,
    pub data: Vec<u8>,
}

/// MPP 协议层错误。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MppError {
    pub mpp_version: i64,
    pub message: String,
}

/// 上报响应；`error` 非空表示协调器侧失败。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ReportTaskStatusResponse {
    pub error: Option<MppError>,
}

/// 管理器/协调器操作失败的字符串错误。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoordinatorError(pub String);

impl Display for CoordinatorError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for CoordinatorError {}

/// 单个 gather 的 MPP 协调器：接收状态上报并报告是否已关闭。
pub trait MppCoordinator: Send + Sync + 'static {
    fn is_closed(&self) -> bool;
    fn report_status(&self, request: &ReportTaskStatusRequest) -> Result<(), CoordinatorError>;
}

/// 注册/活跃/超时计数指标（原子量快照）。
#[derive(Default)]
pub struct CoordinatorMetrics {
    total_registered: AtomicU64,
    active: AtomicU64,
    overtime: AtomicU64,
}

impl CoordinatorMetrics {
    /// 累计注册次数。
    pub fn total_registered(&self) -> u64 {
        self.total_registered.load(Ordering::Acquire)
    }

    /// 当前仍登记的活跃数。
    pub fn active(&self) -> u64 {
        self.active.load(Ordering::Acquire)
    }

    /// 因超时被清理的累计次数。
    pub fn overtime(&self) -> u64 {
        self.overtime.load(Ordering::Acquire)
    }
}

/// 受锁保护的管理器可变状态。
#[derive(Default)]
struct ManagerState {
    server_on: bool,
    server_address: String,
    coordinators: HashMap<CoordinatorUniqueId, Arc<dyn MppCoordinator>>,
}

/// 后台检测线程的停止通道与 JoinHandle。
#[derive(Default)]
struct BackgroundRuntime {
    stop: Option<mpsc::Sender<()>>,
    handle: Option<JoinHandle<()>>,
}

/// 全局/局部 MPP 协调器注册表与生命周期管理。
pub struct MppCoordinatorManager {
    state: Arc<Mutex<ManagerState>>,
    background: Mutex<BackgroundRuntime>,
    metrics: Arc<CoordinatorMetrics>,
    detect_frequency: Duration,
    max_lifetime_nanos: AtomicU64,
}

impl Default for MppCoordinatorManager {
    fn default() -> Self {
        Self::new(DETECT_FREQUENCY)
    }
}

impl MppCoordinatorManager {
    /// 使用给定检测周期构造管理器（尚未启动后台线程）。
    pub fn new(detect_frequency: Duration) -> Self {
        Self {
            state: Arc::new(Mutex::new(ManagerState::default())),
            background: Mutex::new(BackgroundRuntime::default()),
            metrics: Arc::new(CoordinatorMetrics::default()),
            detect_frequency,
            max_lifetime_nanos: AtomicU64::new(0),
        }
    }

    /// 当前 UNIX 纳秒时间戳。
    fn now_nanos() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .min(u64::MAX as u128) as u64
    }

    /// Starts one background detector. A repeated call while it is running is a no-op.
    ///
    /// 启动后台过期检测线程；已在运行则直接返回。
    pub fn run(&self) {
        let mut runtime = self
            .background
            .lock()
            .expect("MPP coordinator background lock poisoned");
        if runtime.handle.is_some() {
            return;
        }
        // 最大存活 = TiFlash 超长读超时 + 检测周期。
        let maximum = astersql_store_copr::TI_FLASH_READ_TIMEOUT_ULTRA_LONG
            .saturating_add(self.detect_frequency)
            .as_nanos()
            .min(u64::MAX as u128) as u64;
        self.max_lifetime_nanos.store(maximum, Ordering::Release);

        let (stop_sender, stop_receiver) = mpsc::channel();
        let state = Arc::clone(&self.state);
        let metrics = Arc::clone(&self.metrics);
        let frequency = self.detect_frequency;
        runtime.stop = Some(stop_sender);
        runtime.handle = Some(thread::spawn(move || {
            // recv_timeout 超时（Err）表示到了检测周期，执行清理；收到 stop 则退出。
            while stop_receiver.recv_timeout(frequency).is_err() {
                Self::detect_and_delete_shared(&state, &metrics, maximum, Self::now_nanos());
            }
        }));
    }

    /// 删除已关闭且 `query_ts + max_lifetime < now` 的协调器，返回被删 id。
    fn detect_and_delete_shared(
        state: &Mutex<ManagerState>,
        metrics: &CoordinatorMetrics,
        maximum_lifetime: u64,
        now_timestamp: u64,
    ) -> Vec<CoordinatorUniqueId> {
        let mut state = state.lock().expect("MPP coordinator state lock poisoned");
        let expired: Vec<_> = state
            .coordinators
            .iter()
            .filter_map(|(id, coordinator)| {
                // Go's uint64 addition wraps, including at the timestamp boundary.
                let deadline = id.mpp_query_id.query_ts.wrapping_add(maximum_lifetime);
                (now_timestamp > deadline && coordinator.is_closed()).then_some(*id)
            })
            .collect();
        for id in &expired {
            state.coordinators.remove(id);
        }
        drop(state);
        metrics
            .overtime
            .fetch_add(expired.len() as u64, Ordering::AcqRel);
        expired
    }

    /// 使用当前配置的 `max_lifetime` 执行一次过期清理（供测试注入 now）。
    pub fn detect_and_delete(&self, now_timestamp: u64) -> Vec<CoordinatorUniqueId> {
        Self::detect_and_delete_shared(
            &self.state,
            &self.metrics,
            self.max_lifetime_nanos.load(Ordering::Acquire),
            now_timestamp,
        )
    }

    /// Sets `maxLifeTime` in nanoseconds. Matches Go field assignment used by tests and `Run`.
    ///
    /// 设置最大存活时间（纳秒），供测试与 `Run` 使用。
    pub fn set_max_lifetime_nanos(&self, nanos: u64) {
        self.max_lifetime_nanos.store(nanos, Ordering::Release);
    }

    /// Returns `maxLifeTime` in nanoseconds.
    ///
    /// 返回最大存活时间（纳秒）。
    pub fn max_lifetime_nanos(&self) -> u64 {
        self.max_lifetime_nanos.load(Ordering::Acquire)
    }

    /// Snapshot of registered coordinator IDs (for tests and diagnostics).
    ///
    /// 返回当前已注册协调器 id 快照（测试/诊断）。
    pub fn coordinator_ids(&self) -> Vec<CoordinatorUniqueId> {
        self.state
            .lock()
            .expect("MPP coordinator state lock poisoned")
            .coordinators
            .keys()
            .copied()
            .collect()
    }

    /// 停止后台检测线程并等待退出。
    pub fn stop(&self) {
        let (stop, handle) = {
            let mut runtime = self
                .background
                .lock()
                .expect("MPP coordinator background lock poisoned");
            (runtime.stop.take(), runtime.handle.take())
        };
        if let Some(stop) = stop {
            let _ = stop.send(());
        }
        if let Some(handle) = handle {
            let _ = handle.join();
        }
    }

    /// 记录本节点是否作为 MPP 服务端及对外地址。
    pub fn init_server_address(&self, server_on: bool, server_address: String) {
        let mut state = self
            .state
            .lock()
            .expect("MPP coordinator state lock poisoned");
        state.server_on = server_on;
        if server_on {
            state.server_address = server_address;
        }
    }

    /// 返回 `(server_on, server_address)`。
    pub fn server_address(&self) -> (bool, String) {
        let state = self
            .state
            .lock()
            .expect("MPP coordinator state lock poisoned");
        (state.server_on, state.server_address.clone())
    }

    /// 注册协调器；同一 id 重复注册返回错误。
    pub fn register(
        &self,
        id: CoordinatorUniqueId,
        coordinator: Arc<dyn MppCoordinator>,
    ) -> Result<(), CoordinatorError> {
        let mut state = self
            .state
            .lock()
            .expect("MPP coordinator state lock poisoned");
        if state.coordinators.contains_key(&id) {
            return Err(CoordinatorError(format!(
                "Mpp coordinator already registered: {} {} {} {}",
                id.mpp_query_id.query_ts,
                id.mpp_query_id.local_query_id,
                id.mpp_query_id.server_id,
                id.gather_id
            )));
        }
        state.coordinators.insert(id, coordinator);
        self.metrics.total_registered.fetch_add(1, Ordering::AcqRel);
        self.metrics.active.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    /// 注销协调器；若原先存在则活跃计数减一。
    pub fn unregister(&self, id: CoordinatorUniqueId) {
        let existed = self
            .state
            .lock()
            .expect("MPP coordinator state lock poisoned")
            .coordinators
            .remove(&id)
            .is_some();
        if existed {
            self.metrics.active.fetch_sub(1, Ordering::AcqRel);
        }
    }

    /// 当前登记的协调器数量。
    pub fn coordinator_count(&self) -> usize {
        self.state
            .lock()
            .expect("MPP coordinator state lock poisoned")
            .coordinators
            .len()
    }

    /// 按 meta 中的查询/gather 定位协调器并转发状态上报。
    pub fn report_status(&self, request: &ReportTaskStatusRequest) -> ReportTaskStatusResponse {
        let id = CoordinatorUniqueId {
            mpp_query_id: MppQueryId {
                query_ts: request.meta.query_ts,
                local_query_id: request.meta.local_query_id,
                server_id: request.meta.server_id,
            },
            gather_id: request.meta.gather_id,
        };
        // Clone the Arc under the manager lock, then invoke user coordinator code
        // without the lock exactly as the Go implementation requires.
        let coordinator = self
            .state
            .lock()
            .expect("MPP coordinator state lock poisoned")
            .coordinators
            .get(&id)
            .cloned();
        let Some(coordinator) = coordinator else {
            return ReportTaskStatusResponse {
                error: Some(MppError {
                    mpp_version: request.meta.mpp_version,
                    message: "MppCoordinator not exists".to_owned(),
                }),
            };
        };
        match coordinator.report_status(request) {
            Ok(()) => ReportTaskStatusResponse::default(),
            Err(error) => ReportTaskStatusResponse {
                error: Some(MppError {
                    mpp_version: request.meta.mpp_version,
                    message: error.to_string(),
                }),
            },
        }
    }

    /// 共享指标句柄。
    pub fn metrics(&self) -> Arc<CoordinatorMetrics> {
        Arc::clone(&self.metrics)
    }

    /// Go 风格别名：`Run`。
    #[allow(non_snake_case)]
    pub fn Run(&self) {
        self.run();
    }

    /// Go 风格别名：`Stop`。
    #[allow(non_snake_case)]
    pub fn Stop(&self) {
        self.stop();
    }

    /// Go 风格别名：`InitServerAddr`。
    #[allow(non_snake_case)]
    pub fn InitServerAddr(&self, server_on: bool, server_address: String) {
        self.init_server_address(server_on, server_address);
    }

    /// Go 风格别名：`GetServerAddr`。
    #[allow(non_snake_case)]
    pub fn GetServerAddr(&self) -> (bool, String) {
        self.server_address()
    }

    /// Go 风格别名：`Register`。
    #[allow(non_snake_case)]
    pub fn Register(
        &self,
        id: CoordinatorUniqueId,
        coordinator: Arc<dyn MppCoordinator>,
    ) -> Result<(), CoordinatorError> {
        self.register(id, coordinator)
    }

    /// Go 风格别名：`Unregister`。
    #[allow(non_snake_case)]
    pub fn Unregister(&self, id: CoordinatorUniqueId) {
        self.unregister(id);
    }

    /// Go 风格别名：`GetCoordCount`。
    #[allow(non_snake_case)]
    pub fn GetCoordCount(&self) -> usize {
        self.coordinator_count()
    }

    /// Go 风格别名：`ReportStatus`。
    #[allow(non_snake_case)]
    pub fn ReportStatus(&self, request: &ReportTaskStatusRequest) -> ReportTaskStatusResponse {
        self.report_status(request)
    }
}

impl Drop for MppCoordinatorManager {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Go 风格常量别名。
#[allow(non_upper_case_globals)]
pub const detectFrequency: Duration = DETECT_FREQUENCY;

/// Go 风格构造函数别名（保留原拼写 Manger）。
#[allow(non_snake_case)]
pub fn newMPPCoordinatorManger() -> MppCoordinatorManager {
    MppCoordinatorManager::default()
}

/// 进程级单例管理器。
#[allow(non_upper_case_globals)]
pub static InstanceMPPCoordinatorManager: LazyLock<MppCoordinatorManager> =
    LazyLock::new(newMPPCoordinatorManger);

/// Go 类型名别名。
pub type CoordinatorUniqueID = CoordinatorUniqueId;
/// Go 类型名别名。
pub type MPPCoordinatorManager = MppCoordinatorManager;
