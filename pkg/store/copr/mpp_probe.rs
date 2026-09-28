// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// MPP 失败 store 探活与 server 信息缓存。
//
// 当某个 TiFlash/MPP store 被认为不可用时，后台周期探测其是否恢复；
// 恢复超过 TTL 或长期无人查询则从失败列表移除。另维护 MPP server
// 信息的 LRU（最近最少使用）缓存。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// 默认探测周期：两次探测的最小间隔。
pub const DETECT_PERIOD: Duration = Duration::from_secs(3);
/// 单次探活 RPC 超时上限。
pub const DETECT_TIMEOUT_LIMIT: Duration = Duration::from_secs(2);
/// 恢复后保留在失败列表中的最长时间（超时则清理）。
pub const MAX_RECOVERY_TIME_LIMIT: Duration = Duration::from_secs(15 * 60);
/// 未恢复且无人查询时的过期清理时间。
pub const MAX_OBSOLETE_TIME_LIMIT: Duration = Duration::from_secs(60 * 60);
/// MPP server 信息 LRU 缓存容量。
pub const MPP_SERVER_INFO_MANAGER_CACHE_SIZE: usize = 1_000;

/// 探活客户端：判断指定地址的 store 是否存活。
pub trait MppAliveClient: Send + Sync + 'static {
    fn is_alive(&self, address: &str, timeout: Duration) -> bool;
}

#[derive(Clone, Debug)]
/// 单个失败 store 的时间戳状态。
struct StoreTiming {
    /// 最近一次探测到存活的时间；None 表示仍失败。
    recovery_time: Option<Instant>,
    /// 最近一次被业务查询是否恢复的时间。
    last_lookup_time: Instant,
    /// 最近一次实际发起探测的时间。
    last_detect_time: Option<Instant>,
}

/// 单个失败 store 的探活状态与客户端。
pub struct MppStoreState {
    /// store 地址。
    pub address: String,
    /// 用于探活的客户端。
    client: Arc<dyn MppAliveClient>,
    /// 探测与恢复相关时间戳。
    timing: Mutex<StoreTiming>,
}

impl MppStoreState {
    /// 创建初始为未恢复的 store 状态。
    fn new(address: String, client: Arc<dyn MppAliveClient>) -> Self {
        Self {
            address,
            client,
            timing: Mutex::new(StoreTiming {
                recovery_time: None,
                last_lookup_time: Instant::now(),
                last_detect_time: None,
            }),
        }
    }

    /// 若距上次探测已超过周期，则发起一次探活并更新 recovery_time。
    pub fn detect(&self, detect_period: Duration, detect_timeout: Duration) {
        let Ok(mut timing) = self.timing.try_lock() else {
            return;
        };
        let now = Instant::now();
        if timing
            .last_detect_time
            .is_some_and(|last| now.duration_since(last) < detect_period)
        {
            return;
        }
        let alive = self.client.is_alive(&self.address, detect_timeout);
        let completed_at = Instant::now();
        timing.last_detect_time = Some(completed_at);
        if alive {
            timing.recovery_time.get_or_insert(completed_at);
        } else {
            timing.recovery_time = None;
        }
    }

    /// 是否已恢复超过 recovery_ttl；同时刷新 last_lookup_time。
    pub fn is_recovered(&self, recovery_ttl: Duration) -> bool {
        let Ok(mut timing) = self.timing.try_lock() else {
            return false;
        };
        let now = Instant::now();
        timing.last_lookup_time = now;
        timing
            .recovery_time
            .is_some_and(|recovered| now.duration_since(recovered) > recovery_ttl)
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// MPP server 的地址、逻辑 CPU 数与启动时间戳。
pub struct MppServerInfo {
    /// server 地址。
    pub address: String,
    /// 逻辑 CPU 数量。
    pub logical_cpu_count: u64,
    /// 进程启动时间戳。
    pub start_timestamp: i64,
}

#[derive(Default)]
/// LRU 内部状态：地址到信息的映射与访问顺序。
struct ServerInfoState {
    /// 地址 -> server 信息。
    values: HashMap<String, MppServerInfo>,
    /// 从旧到新的访问顺序（队首最旧）。
    order: VecDeque<String>,
}

/// 有容量上限的 MPP server 信息管理器（LRU）。
pub struct MppServerInfoManager {
    /// 最大缓存条目数。
    capacity: usize,
    /// 受保护的缓存状态。
    state: Mutex<ServerInfoState>,
}

impl Default for MppServerInfoManager {
    fn default() -> Self {
        Self::new(MPP_SERVER_INFO_MANAGER_CACHE_SIZE)
    }
}

impl MppServerInfoManager {
    /// 按指定容量创建管理器。
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            state: Mutex::new(ServerInfoState::default()),
        }
    }

    /// 插入或更新条目；超出容量时淘汰最久未用项。
    pub fn add(&self, server: MppServerInfo) {
        let mut state = self.state.lock().expect("MPP server info lock poisoned");
        state.order.retain(|address| address != &server.address);
        state.order.push_back(server.address.clone());
        state.values.insert(server.address.clone(), server);
        while state.values.len() > self.capacity {
            if let Some(address) = state.order.pop_front() {
                state.values.remove(&address);
            }
        }
    }

    /// 按地址删除缓存项。
    pub fn delete(&self, address: &str) {
        let mut state = self.state.lock().expect("MPP server info lock poisoned");
        state.values.remove(address);
        state.order.retain(|item| item != address);
    }

    /// 查询并提升为最近使用。
    pub fn get(&self, address: &str) -> Option<MppServerInfo> {
        let mut state = self.state.lock().expect("MPP server info lock poisoned");
        let value = state.values.get(address).cloned()?;
        state.order.retain(|item| item != address);
        state.order.push_back(address.to_owned());
        Some(value)
    }
}

/// 失败 MPP store 集合的探活器，可后台周期 scan。
pub struct MppFailedStoreProber {
    /// 地址 -> 失败 store 状态。
    stores: Arc<Mutex<HashMap<String, Arc<MppStoreState>>>>,
    /// 后台 worker 是否在跑。
    running: Arc<AtomicBool>,
    /// 请求停止后台循环。
    stop: Arc<AtomicBool>,
    /// 后台线程句柄。
    worker: Mutex<Option<JoinHandle<()>>>,
    /// 探测周期。
    pub detect_period: Duration,
    /// 探活超时。
    pub detect_timeout_limit: Duration,
    /// 恢复后最大保留时间。
    pub max_recovery_time_limit: Duration,
    /// 未恢复条目的过期时间。
    pub max_obsolete_time_limit: Duration,
}

impl Default for MppFailedStoreProber {
    fn default() -> Self {
        Self {
            stores: Arc::new(Mutex::new(HashMap::new())),
            running: Arc::new(AtomicBool::new(false)),
            stop: Arc::new(AtomicBool::new(true)),
            worker: Mutex::new(None),
            detect_period: DETECT_PERIOD,
            detect_timeout_limit: DETECT_TIMEOUT_LIMIT,
            max_recovery_time_limit: MAX_RECOVERY_TIME_LIMIT,
            max_obsolete_time_limit: MAX_OBSOLETE_TIME_LIMIT,
        }
    }
}

impl MppFailedStoreProber {
    /// 将地址加入失败 store 列表。
    pub fn add(&self, address: String, client: Arc<dyn MppAliveClient>) {
        self.stores
            .lock()
            .expect("MPP failed-store map poisoned")
            .insert(
                address.clone(),
                Arc::new(MppStoreState::new(address, client)),
            );
    }

    /// 不存在于失败列表视为已恢复；否则委托 MppStoreState。
    pub fn is_recovered(&self, address: &str, recovery_ttl: Duration) -> bool {
        let state = self
            .stores
            .lock()
            .expect("MPP failed-store map poisoned")
            .get(address)
            .cloned();
        state.is_none_or(|state| state.is_recovered(recovery_ttl))
    }

    /// 从失败列表移除；返回是否原先存在。
    pub fn delete(&self, address: &str) -> bool {
        self.stores
            .lock()
            .expect("MPP failed-store map poisoned")
            .remove(address)
            .is_some()
    }

    /// 对当前失败列表执行一轮探测与过期清理。
    pub fn scan(&self) {
        Self::scan_shared(
            &self.stores,
            self.detect_period,
            self.detect_timeout_limit,
            self.max_recovery_time_limit,
            self.max_obsolete_time_limit,
        );
    }

    /// 共享扫描逻辑：探测后按恢复超时或过期条件批量移除。
    fn scan_shared(
        stores: &Mutex<HashMap<String, Arc<MppStoreState>>>,
        detect_period: Duration,
        detect_timeout: Duration,
        max_recovery: Duration,
        max_obsolete: Duration,
    ) {
        let snapshot: Vec<_> = stores
            .lock()
            .expect("MPP failed-store map poisoned")
            .values()
            .cloned()
            .collect();
        let mut remove = Vec::new();
        for state in snapshot {
            state.detect(detect_period, detect_timeout);
            let Ok(timing) = state.timing.try_lock() else {
                continue;
            };
            let now = Instant::now();
            // 恢复超过上限，或长期无人查询且仍失败，则移出列表。
            let restored = timing
                .recovery_time
                .is_some_and(|time| now.duration_since(time) > max_recovery);
            let obsolete = timing.recovery_time.is_none()
                && now.duration_since(timing.last_lookup_time) > max_obsolete;
            if restored || obsolete {
                remove.push(state.address.clone());
            }
        }
        if !remove.is_empty() {
            let mut stores = stores.lock().expect("MPP failed-store map poisoned");
            for address in remove {
                stores.remove(&address);
            }
        }
    }

    /// 启动唯一后台 worker，周期调用 scan_shared。
    pub fn run(&self) {
        if self
            .running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        self.stop.store(false, Ordering::Release);
        let stores = Arc::clone(&self.stores);
        let stop = Arc::clone(&self.stop);
        let running = Arc::clone(&self.running);
        let detect_period = self.detect_period;
        let detect_timeout = self.detect_timeout_limit;
        let max_recovery = self.max_recovery_time_limit;
        let max_obsolete = self.max_obsolete_time_limit;
        let worker = thread::spawn(move || {
            while !stop.load(Ordering::Acquire) {
                Self::scan_shared(
                    &stores,
                    detect_period,
                    detect_timeout,
                    max_recovery,
                    max_obsolete,
                );
                for _ in 0..10 {
                    if stop.load(Ordering::Acquire) {
                        break;
                    }
                    thread::sleep(Duration::from_millis(100));
                }
            }
            running.store(false, Ordering::Release);
        });
        *self.worker.lock().expect("MPP probe worker lock poisoned") = Some(worker);
    }

    /// 停止后台 worker 并 join。
    pub fn stop(&self) {
        if !self.running.load(Ordering::Acquire) {
            return;
        }
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self
            .worker
            .lock()
            .expect("MPP probe worker lock poisoned")
            .take()
        {
            let _ = worker.join();
        }
    }
}

impl Drop for MppFailedStoreProber {
    fn drop(&mut self) {
        self.stop();
    }
}

/// 进程内全局失败 store 探活器。
static GLOBAL_PROBER: OnceLock<MppFailedStoreProber> = OnceLock::new();
/// 进程内全局 MPP server 信息管理器。
static GLOBAL_SERVER_INFO: OnceLock<MppServerInfoManager> = OnceLock::new();

/// 获取全局失败 store 探活器。
pub fn global_mpp_failed_store_prober() -> &'static MppFailedStoreProber {
    GLOBAL_PROBER.get_or_init(MppFailedStoreProber::default)
}

/// 获取全局 MPP server 信息管理器。
pub fn global_mpp_server_info_manager() -> &'static MppServerInfoManager {
    GLOBAL_SERVER_INFO.get_or_init(MppServerInfoManager::default)
}

/// 对单个地址发起一次探活。
pub fn detect_mpp_store(client: &dyn MppAliveClient, address: &str, timeout: Duration) -> bool {
    client.is_alive(address, timeout)
}

#[allow(non_upper_case_globals)]
/// Go 风格常量别名。
pub const DetectPeriod: Duration = DETECT_PERIOD;
#[allow(non_upper_case_globals)]
pub const DetectTimeoutLimit: Duration = DETECT_TIMEOUT_LIMIT;
#[allow(non_upper_case_globals)]
pub const MaxRecoveryTimeLimit: Duration = MAX_RECOVERY_TIME_LIMIT;
#[allow(non_upper_case_globals)]
pub const MaxObsoletTimeLimit: Duration = MAX_OBSOLETE_TIME_LIMIT;

/// Go 风格类型别名。
pub type MPPStoreState = MppStoreState;
pub type MPPFailedStoreProber = MppFailedStoreProber;
pub type MPPServerInfo = MppServerInfo;
