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

// 跨 Keyspace 运行时管理器：按需创建/持有/空闲回收各 keyspace 的 SessionManager。
// Keyspace 是存储与元数据的命名空间隔离单元；跨 KS 访问需通过本 Manager 获取运行时句柄。
// 经典内核（classic kernel）或当前 KS 自身不允许走跨 KS 路径。

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use crate::{AlterTableModeTarget, Cancellation, DdlClient, SchemaCoordinator};

/// 跨 KS 系统会话池建议大小。
pub const CROSS_KEYSPACE_SESSION_POOL_SIZE: usize = 5;
/// 无持有者后，运行时被空闲回收的超时（默认 30 分钟）。
pub const CROSS_KEYSPACE_RUNTIME_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// GC 循环扫描空闲运行时的间隔。
pub const CROSS_KEYSPACE_RUNTIME_SWEEP_INTERVAL: Duration = Duration::from_secs(60);
/// 系统 Keyspace 名称；其 Store 关闭时不随 SessionManager 关闭。
pub const SYSTEM_KEYSPACE: &str = "SYSTEM";

#[derive(Clone, Debug, Eq, PartialEq)]
/// 跨 KS Manager 错误。
pub struct ManagerError(pub String);
/// 存储引擎抽象：提供所属 keyspace 与关闭能力。
pub trait Store: Send + Sync {
    fn keyspace(&self) -> &str;
    fn close(&self) -> Result<(), ManagerError>;
}
/// 会话池抽象：关闭时释放池内会话。
pub trait SessionPool: Send + Sync {
    fn close(&self);
}
/// InfoSchema 缓存桩接口（跨 KS 侧不关心具体实现）。
pub trait InfoCache: Send + Sync {}
/// 随 SessionManager 关闭的可逆生命周期组件。
pub trait Lifecycle: Send + Sync {
    fn close(&self) -> Result<(), ManagerError>;
}
/// 按 keyspace 创建 SessionManager 的工厂。
pub trait RuntimeFactory: Send + Sync {
    fn create(&self, keyspace: &str) -> Result<Arc<SessionManager>, ManagerError>;
}

/// 单个 keyspace 运行时条目：SessionManager、活跃持有者集合、上次全部释放时间。
struct RuntimeEntry {
    session_manager: Arc<SessionManager>,
    active_holders: HashSet<String>,
    last_release_at: Option<Instant>,
}
/// Manager 可变状态：keyspace → RuntimeEntry。
struct ManagerState {
    runtimes: HashMap<String, RuntimeEntry>,
}
/// 跨 Keyspace 运行时管理器（进程内单例风格使用）。
pub struct Manager {
    classic_kernel: bool,
    current_keyspace: String,
    factory: Arc<dyn RuntimeFactory>,
    state: Mutex<ManagerState>,
    closed: AtomicBool,
}

/// 构造 Manager；classic_kernel 为真时禁用跨 KS。
pub fn new_manager(
    classic_kernel: bool,
    current_keyspace: impl Into<String>,
    factory: Arc<dyn RuntimeFactory>,
) -> Arc<Manager> {
    Arc::new(Manager {
        classic_kernel,
        current_keyspace: current_keyspace.into(),
        factory,
        state: Mutex::new(ManagerState {
            runtimes: HashMap::new(),
        }),
        closed: AtomicBool::new(false),
    })
}
impl Manager {
    /// 返回当前已加载的全部 keyspace 名。
    pub fn all_keyspaces(&self) -> Vec<String> {
        self.state
            .lock()
            .expect("crossks mutex poisoned")
            .runtimes
            .keys()
            .cloned()
            .collect()
    }

    /// 若存在则返回指定 keyspace 的 SessionManager。
    /// 对应 Go `export_test.go` 的 `Manager.Get`。
    /// Returns the session manager for the specified keyspace when present.
    /// Corresponds to Go `Manager.Get` from `export_test.go`.
    pub fn get(&self, ks: &str) -> Option<Arc<SessionManager>> {
        self.state
            .lock()
            .expect("crossks mutex poisoned")
            .runtimes
            .get(ks)
            .map(|entry| Arc::clone(&entry.session_manager))
    }

    /// 关闭并移除指定 keyspace 的 SessionManager。
    /// 对应 Go `export_test.go` 的 `Manager.CloseKS`。
    /// Closes and removes the session manager for the specified keyspace.
    /// Corresponds to Go `Manager.CloseKS` from `export_test.go`.
    pub fn close_ks(&self, target_ks: &str) {
        let manager = {
            let mut state = self.state.lock().expect("crossks mutex poisoned");
            state
                .runtimes
                .remove(target_ks)
                .map(|entry| entry.session_manager)
        };
        if let Some(manager) = manager {
            manager.close();
        }
    }

    /// 查询 `holder_id` 是否仍持有 `ks` 的运行时。
    /// Returns whether `holder_id` currently holds the runtime for `ks`.
    pub fn has_active_holder(&self, ks: &str, holder_id: &str) -> bool {
        self.state
            .lock()
            .expect("crossks mutex poisoned")
            .runtimes
            .get(ks)
            .is_some_and(|entry| entry.active_holders.contains(holder_id))
    }

    /// 返回 `ks` 的活跃持有者数量（运行时不存在则为 None）。
    /// Returns the number of active holders for `ks`, if the runtime exists.
    pub fn active_holder_len(&self, ks: &str) -> Option<usize> {
        self.state
            .lock()
            .expect("crossks mutex poisoned")
            .runtimes
            .get(ks)
            .map(|entry| entry.active_holders.len())
    }

    /// 查询是否已记录 `ks` 的 last_release_at。
    /// Returns whether `last_release_at` has been recorded for `ks`.
    pub fn last_release_at_is_some(&self, ks: &str) -> bool {
        self.state
            .lock()
            .expect("crossks mutex poisoned")
            .runtimes
            .get(ks)
            .is_some_and(|entry| entry.last_release_at.is_some())
    }

    /// 覆盖写入 last_release_at，供空闲驱逐单测使用。
    /// Overwrites `last_release_at` for idle-eviction tests.
    pub fn set_last_release_at(&self, ks: &str, at: Instant) {
        if let Some(entry) = self
            .state
            .lock()
            .expect("crossks mutex poisoned")
            .runtimes
            .get_mut(ks)
        {
            entry.last_release_at = Some(at);
        }
    }

    /// 校验目标 KS：非空、非经典内核/非当前 KS、Manager 未关闭。
    fn validate_target_keyspace(&self, keyspace: &str) -> Result<(), ManagerError> {
        if keyspace.is_empty() {
            return Err(ManagerError("target keyspace must not be empty".into()));
        }
        if self.classic_kernel || self.current_keyspace == keyspace {
            return Err(ManagerError(
                "cross keyspace is not available in classic kernel or current keyspace".into(),
            ));
        }
        if self.closed.load(Ordering::Acquire) {
            return Err(ManagerError("cross keyspace manager is closed".into()));
        }
        Ok(())
    }
    /// 在已持有锁的前提下，获取或通过 factory 创建 RuntimeEntry。
    fn get_or_create_locked<'a>(
        &self,
        state: &'a mut ManagerState,
        keyspace: &str,
    ) -> Result<&'a mut RuntimeEntry, ManagerError> {
        if !state.runtimes.contains_key(keyspace) {
            let manager = self.factory.create(keyspace)?;
            state.runtimes.insert(
                keyspace.into(),
                RuntimeEntry {
                    session_manager: manager,
                    active_holders: HashSet::new(),
                    last_release_at: None,
                },
            );
        }
        Ok(state.runtimes.get_mut(keyspace).expect("runtime inserted"))
    }
    /// 校验后获取或创建目标 KS 的 SessionManager（不登记持有者）。
    pub fn get_or_create(&self, keyspace: &str) -> Result<Arc<SessionManager>, ManagerError> {
        self.validate_target_keyspace(keyspace)?;
        let mut state = self.state.lock().expect("crossks mutex poisoned");
        Ok(Arc::clone(
            &self
                .get_or_create_locked(&mut state, keyspace)?
                .session_manager,
        ))
    }
    /// 以 holder_id 独占登记并返回 RuntimeHandle；重复 acquire 同一 holder 会失败。
    pub fn acquire(
        self: &Arc<Self>,
        keyspace: &str,
        holder_id: &str,
    ) -> Result<RuntimeHandle, ManagerError> {
        if holder_id.is_empty() {
            return Err(ManagerError(
                "cross keyspace runtime holderID must not be empty".into(),
            ));
        }
        self.validate_target_keyspace(keyspace)?;
        let mut state = self.state.lock().expect("crossks mutex poisoned");
        let entry = self.get_or_create_locked(&mut state, keyspace)?;
        if !entry.active_holders.insert(holder_id.into()) {
            return Err(ManagerError(format!(
                "cross keyspace runtime for keyspace {keyspace} is already acquired by holderID {holder_id}"
            )));
        }
        Ok(RuntimeHandle {
            manager: Arc::downgrade(self),
            target_keyspace: keyspace.into(),
            holder_id: holder_id.into(),
            session_manager: Arc::clone(&entry.session_manager),
            released: AtomicBool::new(false),
        })
    }
    /// 释放持有者；若无剩余持有者则记录 last_release_at 供空闲回收。
    fn release(&self, keyspace: &str, holder_id: &str) {
        let mut state = self.state.lock().expect("crossks mutex poisoned");
        let Some(entry) = state.runtimes.get_mut(keyspace) else {
            return;
        };
        entry.active_holders.remove(holder_id);
        if entry.active_holders.is_empty() {
            entry.last_release_at = Some(Instant::now());
        }
    }
    /// 驱逐空闲超时的运行时：先在锁内收集，再在锁外 close，避免长时间持锁。
    pub fn sweep_idle_runtimes(&self, idle_timeout: Duration) {
        // 在锁内筛选并移除空闲条目，close 放到锁外执行。
        let evicted = {
            let mut state = self.state.lock().expect("crossks mutex poisoned");
            let keys = state
                .runtimes
                .iter()
                .filter_map(|(key, entry)| {
                    (entry.active_holders.is_empty()
                        && entry
                            .last_release_at
                            .is_some_and(|time| time.elapsed() >= idle_timeout))
                    .then_some(key.clone())
                })
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| {
                    state
                        .runtimes
                        .remove(&key)
                        .map(|entry| entry.session_manager)
                })
                .collect::<Vec<_>>()
        };
        for manager in evicted {
            manager.close();
        }
    }
    /// 系统 KS 上的 GC 循环：按扫描间隔调用 sweep_idle_runtimes，直到取消。
    pub fn run_system_keyspace_gc_loop(&self, cancellation: &Cancellation) {
        let mut waited = Duration::ZERO;
        while !cancellation.is_cancelled() {
            std::thread::sleep(Duration::from_secs(1));
            waited += Duration::from_secs(1);
            if waited >= CROSS_KEYSPACE_RUNTIME_SWEEP_INTERVAL {
                self.sweep_idle_runtimes(CROSS_KEYSPACE_RUNTIME_IDLE_TIMEOUT);
                waited = Duration::ZERO;
            }
        }
    }
    /// 关闭 Manager：标记 closed 并关闭全部运行时。
    pub fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        let runtimes = {
            let mut state = self.state.lock().expect("crossks mutex poisoned");
            state
                .runtimes
                .drain()
                .map(|(_, entry)| entry.session_manager)
                .collect::<Vec<_>>()
        };
        for runtime in runtimes {
            runtime.close();
        }
    }
}

/// 跨 KS 运行时句柄：持有 SessionManager 弱引用回 Manager，Drop 时自动 release。
pub struct RuntimeHandle {
    manager: Weak<Manager>,
    target_keyspace: String,
    holder_id: String,
    session_manager: Arc<SessionManager>,
    released: AtomicBool,
}
impl RuntimeHandle {
    /// 返回目标 KS 的 Store。
    pub fn store(&self) -> Arc<dyn Store> {
        self.session_manager.store()
    }
    /// 返回系统会话池。
    pub fn system_session_pool(&self) -> Arc<dyn SessionPool> {
        self.session_manager.system_session_pool()
    }
    /// 代理到底层 SessionManager/DdlClient 执行 Alter Table Mode。
    pub fn alter_table_mode(
        &self,
        cancellation: &Cancellation,
        target: AlterTableModeTarget,
    ) -> Result<(), crate::ddl_submit::Error> {
        self.session_manager.alter_table_mode(cancellation, target)
    }
    /// 幂等释放持有；仅首次生效。
    pub fn release(&self) {
        if !self.released.swap(true, Ordering::AcqRel) {
            if let Some(manager) = self.manager.upgrade() {
                manager.release(&self.target_keyspace, &self.holder_id);
            }
        }
    }
}
impl Drop for RuntimeHandle {
    // Drop 时确保持有者被释放，防止泄漏导致无法空闲回收。
    fn drop(&mut self) {
        self.release();
    }
}

/// 单个 keyspace 的会话与组件聚合：Store、InfoCache、会话池、协调器、DDL 客户端与生命周期钩子。
pub struct SessionManager {
    store: Arc<dyn Store>,
    info_cache: Arc<dyn InfoCache>,
    session_pool: Arc<dyn SessionPool>,
    coordinator: Arc<SchemaCoordinator>,
    ddl_client: Arc<DdlClient>,
    lifecycles: Vec<Arc<dyn Lifecycle>>,
    closed: AtomicBool,
}
impl SessionManager {
    /// 组装 SessionManager 各依赖。
    pub fn new(
        store: Arc<dyn Store>,
        info_cache: Arc<dyn InfoCache>,
        session_pool: Arc<dyn SessionPool>,
        coordinator: Arc<SchemaCoordinator>,
        ddl_client: Arc<DdlClient>,
        lifecycles: Vec<Arc<dyn Lifecycle>>,
    ) -> Self {
        Self {
            store,
            info_cache,
            session_pool,
            coordinator,
            ddl_client,
            lifecycles,
            closed: AtomicBool::new(false),
        }
    }
    pub fn store(&self) -> Arc<dyn Store> {
        Arc::clone(&self.store)
    }
    /// 返回 InfoSchema 缓存。
    pub fn info_cache(&self) -> Arc<dyn InfoCache> {
        Arc::clone(&self.info_cache)
    }
    /// 返回系统会话池。
    pub fn system_session_pool(&self) -> Arc<dyn SessionPool> {
        Arc::clone(&self.session_pool)
    }
    /// 返回 Schema 协调器。
    pub fn coordinator(&self) -> Arc<SchemaCoordinator> {
        Arc::clone(&self.coordinator)
    }
    /// 代理到底层 SessionManager/DdlClient 执行 Alter Table Mode。
    pub fn alter_table_mode(
        &self,
        cancellation: &Cancellation,
        target: AlterTableModeTarget,
    ) -> Result<(), crate::ddl_submit::Error> {
        self.ddl_client.alter_table_mode(cancellation, target)
    }
    /// 关闭 SessionManager：关闭会话池、逆序关闭 lifecycle，非 SYSTEM KS 时关闭 Store。
    pub fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.session_pool.close();
        for lifecycle in self.lifecycles.iter().rev() {
            let _ = lifecycle.close();
        }
        if self.store.keyspace() != SYSTEM_KEYSPACE {
            let _ = self.store.close();
        }
    }
    /// 是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}
