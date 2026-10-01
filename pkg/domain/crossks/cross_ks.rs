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
    /// An independently opened SYSTEM client is owned by this runtime;
    /// shared system clients keep the Go default of remaining open.
    fn close_on_runtime_shutdown(&self) -> bool {
        self.keyspace() != SYSTEM_KEYSPACE
    }
}
/// 会话池抽象：关闭时释放池内会话。
pub trait SessionPool: Send + Sync {
    fn close(&self);
}
/// Shared target InfoSchema cache published by the common schema loader.
pub trait InfoCache: Send + Sync {
    fn schema(&self) -> Option<astersql_infoschema_issyncer::SchemaInfo> {
        None
    }
}
/// 随 SessionManager 关闭的可逆生命周期组件。
pub trait Lifecycle: Send + Sync {
    fn close(&self) -> Result<(), ManagerError>;
}
/// Virtual server registration owned by a cross-keyspace runtime.
pub trait ServerInfoSyncer: Send + Sync {
    fn server_info_id(&self) -> String;
    fn remove_server_info(&self);
    fn revoke_session(&self);
}

impl ServerInfoSyncer for Mutex<astersql_domain_serverinfo::Syncer> {
    fn server_info_id(&self) -> String {
        self.lock()
            .expect("server info syncer mutex poisoned")
            .GetLocalServerInfo()
            .StaticInfo
            .ID
    }

    fn remove_server_info(&self) {
        self.lock()
            .expect("server info syncer mutex poisoned")
            .RemoveServerInfo();
    }

    fn revoke_session(&self) {
        self.lock()
            .expect("server info syncer mutex poisoned")
            .RevokeSession();
    }
}

/// Keeps a newly registered virtual server until bootstrap hands it to the runtime.
pub struct ServerInfoRegistration {
    syncer: Option<Arc<dyn ServerInfoSyncer>>,
}

impl ServerInfoRegistration {
    pub fn new(syncer: Arc<dyn ServerInfoSyncer>) -> Self {
        Self {
            syncer: Some(syncer),
        }
    }

    pub fn into_runtime(mut self, manager: &SessionManager) {
        if let Some(syncer) = self.syncer.take() {
            manager.set_server_info_syncer(syncer);
        }
    }
}

impl Drop for ServerInfoRegistration {
    fn drop(&mut self) {
        if let Some(syncer) = self.syncer.take() {
            syncer.remove_server_info();
            syncer.revoke_session();
        }
    }
}
/// 按 keyspace 创建 SessionManager 的工厂。
pub trait RuntimeFactory: Send + Sync {
    /// Prepare the target Store and pool before publishing virtual server info.
    fn prepare(&self, _keyspace: &str) -> Result<(), ManagerError> {
        Ok(())
    }
    fn create(&self, keyspace: &str) -> Result<Arc<SessionManager>, ManagerError>;
    /// The registration wrapper passes its virtual server ID so schema
    /// version publication uses the same instance identity.
    fn create_with_server_info(
        &self,
        keyspace: &str,
        _server_info_id: &str,
    ) -> Result<Arc<SessionManager>, ManagerError> {
        self.create(keyspace)
    }
    /// Release resources prepared before virtual server registration when
    /// registration itself fails.
    fn registration_failed(&self, _keyspace: &str) {}
}

/// Runs the existing server-info lease recovery loop and joins it before cleanup.
struct RegisteredServerInfo {
    id: String,
    syncer: Arc<Mutex<astersql_domain_serverinfo::Syncer>>,
    worker: Mutex<Option<(std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>)>>,
}
impl RegisteredServerInfo {
    fn new(syncer: astersql_domain_serverinfo::Syncer) -> Arc<Self> {
        let id = syncer.GetLocalServerInfo().StaticInfo.ID;
        Arc::new(Self {
            id,
            syncer: Arc::new(Mutex::new(syncer)),
            worker: Mutex::new(None),
        })
    }
    fn start(&self, store: Arc<dyn Store>) -> Result<(), ManagerError> {
        let (exit, receive) = std::sync::mpsc::channel();
        let syncer = self.syncer.clone();
        let join = std::thread::Builder::new()
            .name("keyspace-server-info".into())
            .spawn(move || {
                syncer.lock().unwrap().ServerInfoSyncLoop(&store, receive);
            })
            .map_err(|e| ManagerError(e.to_string()))?;
        *self.worker.lock().unwrap() = Some((exit, join));
        Ok(())
    }
    fn stop(&self) {
        if let Some((exit, join)) = self.worker.lock().unwrap().take() {
            let _ = exit.send(());
            let _ = join.join();
        }
    }
}
impl ServerInfoSyncer for RegisteredServerInfo {
    fn server_info_id(&self) -> String {
        self.id.clone()
    }
    fn remove_server_info(&self) {
        self.stop();
        self.syncer.lock().unwrap().RemoveServerInfo();
    }
    fn revoke_session(&self) {
        self.stop();
        self.syncer.lock().unwrap().RevokeSession();
    }
}
impl Drop for RegisteredServerInfo {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Wraps the runtime bootstrap with the same virtual ServerInfo registration
/// and failure cleanup used by Go `createSessionManager`.
struct RegisteredRuntimeFactory {
    inner: Arc<dyn RuntimeFactory>,
    etcd: Arc<
        dyn Fn(
                &str,
            )
                -> Result<Option<Arc<dyn astersql_domain_serverinfo::EtcdClient>>, ManagerError>
            + Send
            + Sync,
    >,
    reporter: Arc<dyn astersql_domain_serverinfo::MinStartTSReporter>,
}

impl RuntimeFactory for RegisteredRuntimeFactory {
    fn create(&self, keyspace: &str) -> Result<Arc<SessionManager>, ManagerError> {
        self.inner.prepare(keyspace)?;
        let etcd = match (self.etcd)(keyspace) {
            Ok(etcd) => etcd,
            Err(error) => {
                self.inner.registration_failed(keyspace);
                return Err(error);
            }
        };
        let mut syncer = astersql_domain_serverinfo::NewCrossKSSyncer(
            uuid::Uuid::new_v4().to_string(),
            Arc::new(|| 0),
            etcd,
            Arc::clone(&self.reporter),
            keyspace.to_owned(),
        );
        if let Err(error) =
            syncer.NewSessionAndStoreServerInfo(astersql_domain_serverinfo::Context::Background())
        {
            syncer.RemoveServerInfo();
            syncer.RevokeSession();
            self.inner.registration_failed(keyspace);
            return Err(ManagerError(format!(
                "register cross-keyspace server info: {error}"
            )));
        }
        let registered = RegisteredServerInfo::new(*syncer);
        let registration = ServerInfoRegistration::new(registered.clone());
        let server_info_id = registration
            .syncer
            .as_ref()
            .expect("new registration")
            .server_info_id();
        let manager = self
            .inner
            .create_with_server_info(keyspace, &server_info_id)?;
        if let Err(error) = registered.start(manager.store()) {
            manager.close();
            return Err(error);
        }
        registration.into_runtime(&manager);
        Ok(manager)
    }
}

/// Build a cross-keyspace manager that registers a virtual server before each
/// runtime bootstrap and transfers cleanup ownership on success.
pub fn new_manager_with_server_info(
    classic_kernel: bool,
    current_keyspace: impl Into<String>,
    factory: Arc<dyn RuntimeFactory>,
    etcd: Option<Arc<dyn astersql_domain_serverinfo::EtcdClient>>,
    reporter: Arc<dyn astersql_domain_serverinfo::MinStartTSReporter>,
) -> Arc<Manager> {
    new_manager_with_server_info_provider(
        classic_kernel,
        current_keyspace,
        factory,
        Arc::new(move |_| Ok(etcd.clone())),
        reporter,
    )
}

/// Register each virtual server through the target keyspace's own etcd
/// namespace. The provider resolves the numeric PD keyspace ID before any
/// target runtime or virtual server is created.
pub fn new_manager_with_server_info_provider(
    classic_kernel: bool,
    current_keyspace: impl Into<String>,
    factory: Arc<dyn RuntimeFactory>,
    etcd: Arc<
        dyn Fn(
                &str,
            )
                -> Result<Option<Arc<dyn astersql_domain_serverinfo::EtcdClient>>, ManagerError>
            + Send
            + Sync,
    >,
    reporter: Arc<dyn astersql_domain_serverinfo::MinStartTSReporter>,
) -> Arc<Manager> {
    new_manager(
        classic_kernel,
        current_keyspace,
        Arc::new(RegisteredRuntimeFactory {
            inner: factory,
            etcd,
            reporter,
        }),
    )
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
    idle_gc: Mutex<Option<(std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>)>>,
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
        idle_gc: Mutex::new(None),
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
    /// Start the idle sweep worker owned by this manager.
    pub fn start_idle_gc(self: &Arc<Self>) -> Result<(), ManagerError> {
        let mut worker = self.idle_gc.lock().expect("crossks GC mutex poisoned");
        if worker.is_some() || self.closed.load(Ordering::Acquire) {
            return Ok(());
        }
        let manager = Arc::downgrade(self);
        let (stop, receiver) = std::sync::mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("crossks-idle-gc".into())
            .spawn(move || {
                loop {
                    match receiver.recv_timeout(CROSS_KEYSPACE_RUNTIME_SWEEP_INTERVAL) {
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            let Some(manager) = manager.upgrade() else {
                                break;
                            };
                            if manager.closed.load(Ordering::Acquire) {
                                break;
                            }
                            manager.sweep_idle_runtimes(CROSS_KEYSPACE_RUNTIME_IDLE_TIMEOUT);
                        }
                        _ => break,
                    }
                }
            })
            .map_err(|error| ManagerError(format!("start crossks idle GC: {error}")))?;
        *worker = Some((stop, thread));
        Ok(())
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
        if let Some((stop, thread)) = self
            .idle_gc
            .lock()
            .expect("crossks GC mutex poisoned")
            .take()
        {
            let _ = stop.send(());
            // The worker may release the last manager reference after a sweep.
            if thread.thread().id() != std::thread::current().id() {
                let _ = thread.join();
            }
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
    server_info_syncer: Mutex<Option<Arc<dyn ServerInfoSyncer>>>,
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
            server_info_syncer: Mutex::new(None),
            closed: AtomicBool::new(false),
        }
    }
    pub fn store(&self) -> Arc<dyn Store> {
        Arc::clone(&self.store)
    }
    /// Test-facing virtual server ID, matching Go `ServerInfoID`.
    pub fn server_info_id(&self) -> Option<String> {
        self.server_info_syncer
            .lock()
            .expect("server info syncer mutex poisoned")
            .as_ref()
            .map(|syncer| syncer.server_info_id())
    }
    /// Takes responsibility for cleaning up the virtual server registration.
    fn set_server_info_syncer(&self, syncer: Arc<dyn ServerInfoSyncer>) {
        let cleanup = {
            let mut slot = self
                .server_info_syncer
                .lock()
                .expect("server info syncer mutex poisoned");
            if self.closed.load(Ordering::Acquire) {
                Some(syncer)
            } else {
                slot.replace(syncer)
            }
        };
        if let Some(cleanup) = cleanup {
            cleanup.remove_server_info();
            cleanup.revoke_session();
        }
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
    /// 关闭 SessionManager：先停止使用会话池的后台组件，再关闭池和 Store。
    pub fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.session_pool.close();
        for lifecycle in self.lifecycles.iter().rev() {
            let _ = lifecycle.close();
        }
        if let Some(syncer) = self
            .server_info_syncer
            .lock()
            .expect("server info syncer mutex poisoned")
            .take()
        {
            syncer.remove_server_info();
            syncer.revoke_session();
        }
        if self.store.close_on_runtime_shutdown() {
            let _ = self.store.close();
        }
    }
    /// 是否已关闭。
    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}
