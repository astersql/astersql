// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// Schema 版本同步器核心实现。
//
// 在分布式 TiDB 集群中，DDL owner（作业所有者）推进 schema 变更时，
// 需要把新的全局 schema 版本写入 etcd（分布式协调服务），并等待集群内
// 每个实例把自己已加载的版本号同步上来。本模块提供：
// - 可取消/带超时的 [`Context`] 与会话完成信号 [`DoneSignal`]；
// - 简化的 etcd 客户端抽象 [`EtcdClient`] 与内存实现 [`MemoryEtcdClient`]；
// - [`Syncer`] trait 与基于 etcd 的 [`etcdSyncer`]；
// - 按节点汇总版本的 [`nodeVersions`]，以及 MDL（Metadata Lock，元数据锁）
//   路径下按 job 等待全员追上的逻辑。

use astersql_domain_serverinfo as serverinfo;
use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock, mpsc};
use std::time::{Duration, Instant};

/// 全局 schema 版本的初始字符串值。
pub const InitialVersion: &str = "0";
/// 写入 etcd 时不重试（仅尝试一次）。
pub const putKeyNoRetry: i64 = 1;
/// 写入/删除 etcd 键的默认重试次数。
pub const keyOpDefaultRetryCnt: i64 = 3;
/// 无限重试（直到上下文取消）。
pub const putKeyRetryUnlimited: i64 = i64::MAX;
/// 非 MDL 模式下轮询检查各节点版本的间隔。
pub const checkVersInterval: Duration = Duration::from_millis(20);
/// 日志前缀标识。
pub const ddlPrompt: &str = "ddl-syncer";
/// etcd 上各实例上报自身 schema 版本的路径前缀。
pub const DDLAllSchemaVersions: &str = "/tidb/ddl/all_schema_versions";
/// etcd 上按 DDL job 分路径上报 schema 版本的前缀（MDL 模式）。
pub const DDLAllSchemaVersionsByJob: &str = "/tidb/ddl/all_schema_by_job_versions";
/// etcd 上全局 schema 版本键。
pub const DDLGlobalSchemaVersion: &str = "/tidb/ddl/global_schema_version";
/// etcd session（会话租约）的默认 TTL（秒）。
pub const SessionTTL: i32 = 90;

/// 首次等待版本同步前的休眠毫秒数（可被测试覆盖）。
static CHECK_VERS_FIRST_WAIT_MILLIS: AtomicU64 = AtomicU64::new(50);
/// 是否启用 MDL（元数据锁）路径的版本同步。
static MDL_ENABLED: AtomicBool = AtomicBool::new(false);
/// 是否处于 next-gen（新一代）部署模式。
static NEXT_GEN: AtomicBool = AtomicBool::new(false);
/// 测试注入：模拟 watch 响应发生 compaction（压缩导致历史丢失）。
static MOCK_COMPACTION: AtomicBool = AtomicBool::new(false);
/// 测试注入：查询 Done 时强制关闭 session。
static ERROR_MOCK_SESSION_DONE: AtomicBool = AtomicBool::new(false);
/// 测试注入：指定 job 在等待版本时人为变慢。
pub(crate) static MOCK_OWNER_SLOW_JOB: AtomicI64 = AtomicI64::new(i64::MIN);
/// 测试注入：更新 MDL 版本时返回错误。
pub(crate) static MOCK_UPDATE_MDL_ERROR: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
pub(crate) static TEST_CONFIG_LOCK: Mutex<()> = Mutex::new(());

/// 设置首次等待版本同步的时长。
pub fn SetCheckVersFirstWaitTime(duration: Duration) {
    CHECK_VERS_FIRST_WAIT_MILLIS.store(duration.as_millis() as u64, Ordering::Release);
}
/// 读取首次等待版本同步的时长。
pub fn CheckVersFirstWaitTime() -> Duration {
    Duration::from_millis(CHECK_VERS_FIRST_WAIT_MILLIS.load(Ordering::Acquire))
}
/// 开关 MDL（元数据锁）版本同步路径。
pub fn SetMDLEnabled(enabled: bool) {
    MDL_ENABLED.store(enabled, Ordering::Release);
}
/// 查询是否已启用 MDL。
pub fn IsMDLEnabled() -> bool {
    MDL_ENABLED.load(Ordering::Acquire)
}
/// 开关 next-gen 部署相关过滤逻辑。
pub fn SetNextGen(enabled: bool) {
    NEXT_GEN.store(enabled, Ordering::Release);
}
/// 开关 compaction 模拟。
pub fn SetMockCompaction(enabled: bool) {
    MOCK_COMPACTION.store(enabled, Ordering::Release);
}
/// 开关“Done 时强制关闭 session”的模拟。
pub fn SetErrorMockSessionDone(enabled: bool) {
    ERROR_MOCK_SESSION_DONE.store(enabled, Ordering::Release);
}
/// 设置（或清除）需要人为变慢的 owner 检查 job ID。
pub fn SetMockOwnerCheckAllVersionSlow(job_id: Option<i64>) {
    MOCK_OWNER_SLOW_JOB.store(job_id.unwrap_or(i64::MIN), Ordering::Release);
}
/// 开关更新 MDL 版本时的注入错误。
pub fn SetMockUpdateMDLError(enabled: bool) {
    MOCK_UPDATE_MDL_ERROR.store(enabled, Ordering::Release);
}

/// 同步过程中的错误（包装可展示的字符串消息）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncError(pub String);
impl fmt::Display for SyncError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for SyncError {}

/// 可取消上下文的内部共享状态。
#[derive(Debug)]
struct ContextState {
    cancelled: AtomicBool,
    lock: Mutex<()>,
    wake: Condvar,
}
/// 可取消、可选超时的操作上下文（类似 Go 的 `context.Context`）。
#[derive(Clone, Debug)]
pub struct Context {
    state: Arc<ContextState>,
    deadline: Option<Instant>,
    parent: Option<Box<Context>>,
}
impl Context {
    /// 创建永不超时、未被取消的后台上下文。
    pub fn Background() -> Self {
        Self {
            state: Arc::new(ContextState {
                cancelled: AtomicBool::new(false),
                lock: Mutex::new(()),
                wake: Condvar::new(),
            }),
            deadline: None,
            parent: None,
        }
    }
    /// 独立取消的子上下文，同时继承父上下文的结束信号。
    pub fn Child(&self) -> Self {
        let mut child = Self::Background();
        child.deadline = self.deadline;
        child.parent = Some(Box::new(self.clone()));
        child
    }
    /// 派生一个带超时截止时间的上下文（共享取消状态）。
    pub fn WithTimeout(&self, timeout: Duration) -> Self {
        Self {
            state: Arc::clone(&self.state),
            deadline: Some(self.deadline.map_or(Instant::now() + timeout, |d| {
                d.min(Instant::now() + timeout)
            })),
            parent: self.parent.clone(),
        }
    }
    /// 取消上下文并唤醒所有等待者。
    pub fn Cancel(&self) {
        self.state.cancelled.store(true, Ordering::Release);
        self.state.wake.notify_all();
    }
    /// 是否已取消或已超过截止时间。
    pub fn Done(&self) -> bool {
        self.state.cancelled.load(Ordering::Acquire)
            || self.parent.as_ref().is_some_and(|p| p.Done())
            || self.deadline.is_some_and(|d| Instant::now() >= d)
    }
    /// 若已结束则返回对应错误，否则返回 `None`。
    pub fn Err(&self) -> Option<SyncError> {
        self.Done()
            .then(|| SyncError("context cancelled or deadline exceeded".into()))
    }
    /// 阻塞等待至多 `duration`；若提前取消/超时返回 `false`。
    pub fn Wait(&self, duration: Duration) -> bool {
        if self.Done() {
            return false;
        }
        // 实际等待时长不超过剩余截止时间。
        let wait = self
            .deadline
            .map(|d| d.saturating_duration_since(Instant::now()).min(duration))
            .unwrap_or(duration);
        let guard = self.state.lock.lock().expect("context lock poisoned");
        let _ = self
            .state
            .wake
            .wait_timeout(guard, wait)
            .expect("context lock poisoned");
        !self.Done()
    }
}
impl Default for Context {
    fn default() -> Self {
        Self::Background()
    }
}

/// 会话生命周期信号：关闭后 `Done()` 为真。
#[derive(Clone)]
pub struct DoneSignal(Arc<AtomicBool>);
impl DoneSignal {
    /// 创建尚未关闭的信号。
    pub fn New() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }
    /// 标记为已关闭。
    pub fn Close(&self) {
        self.0.store(true, Ordering::Release);
    }
    /// 是否已关闭。
    pub fn Done(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}
impl Default for DoneSignal {
    fn default() -> Self {
        Self::New()
    }
}

/// etcd watch 事件类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventType {
    /// 键被写入或更新。
    PUT,
    /// 键被删除。
    DELETE,
}
/// etcd 键值对及其修改修订号（revision）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KeyValue {
    /// 键字节。
    pub Key: Vec<u8>,
    /// 值字节。
    pub Value: Vec<u8>,
    /// 最近一次修改对应的 etcd revision。
    pub ModRevision: i64,
}
/// 单条 watch 事件。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Event {
    /// 事件关联的键值。
    pub Kv: KeyValue,
    /// 事件类型。
    pub Type: EventType,
}
impl Default for EventType {
    fn default() -> Self {
        Self::PUT
    }
}
/// etcd Watch API 的一批响应。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WatchResponse {
    /// 本批事件列表。
    pub Events: Vec<Event>,
    /// 非 0 表示发生了历史压缩，需重新同步。
    pub CompactRevision: i64,
    /// 可选的通道级错误。
    pub Error: Option<SyncError>,
}
/// 接收 `WatchResponse` 的通道包装。
#[derive(Clone)]
pub struct WatchChan {
    receiver: Arc<Mutex<mpsc::Receiver<WatchResponse>>>,
}
impl WatchChan {
    /// 用底层 mpsc 接收端构造观察通道。
    pub(crate) fn New(receiver: mpsc::Receiver<WatchResponse>) -> Self {
        Self {
            receiver: Arc::new(Mutex::new(receiver)),
        }
    }
    /// 带超时地接收一条观察响应。
    pub fn RecvTimeout(
        &self,
        timeout: Duration,
    ) -> std::result::Result<WatchResponse, mpsc::RecvTimeoutError> {
        self.receiver
            .lock()
            .expect("watch channel lock poisoned")
            .recv_timeout(timeout)
    }
    /// 非阻塞尝试接收。
    pub fn TryRecv(&self) -> std::result::Result<WatchResponse, mpsc::TryRecvError> {
        self.receiver
            .lock()
            .expect("watch channel lock poisoned")
            .try_recv()
    }
}
/// 构造一个永远不会收到事件的空观察通道。
fn empty_watch_channel() -> WatchChan {
    let (_tx, rx) = mpsc::channel();
    WatchChan::New(rx)
}

/// etcd Get 操作的响应。
#[derive(Clone, Debug, Default)]
pub struct GetResponse {
    /// 命中的键值列表。
    pub Kvs: Vec<KeyValue>,
    /// 当前集群修订号。
    pub Revision: i64,
}
/// etcd 客户端抽象：同步器依赖的最小 KV / Watch 能力。
pub trait EtcdClient: Send + Sync {
    /// Establish a lease session; network backends override the in-memory default.
    fn NewSession(&self, ctx: &Context, ttl: i32) -> Result<Session, SyncError> {
        if let Some(error) = ctx.Err() {
            return Err(error);
        }
        Ok(Session::New())
    }

    /// 仅在键不存在时写入；返回是否真正写入。
    fn PutIfAbsent(&self, ctx: &Context, key: &str, value: &str) -> Result<bool, SyncError>;
    /// 写入键值，可选绑定 lease（租约）。
    fn Put(
        &self,
        ctx: &Context,
        key: &str,
        value: &str,
        lease: Option<i64>,
    ) -> Result<(), SyncError>;
    /// 单调写入（不带租约），用于 MDL 路径。
    fn PutMono(&self, ctx: &Context, key: &str, value: &str) -> Result<(), SyncError>;
    /// 读取单个键或前缀下的全部键。
    fn Get(&self, ctx: &Context, key: &str, prefix: bool) -> Result<GetResponse, SyncError>;
    /// 删除键。
    fn Delete(&self, ctx: &Context, key: &str) -> Result<(), SyncError>;
    /// 从指定 revision 起观察键或前缀变更。
    fn Watch(&self, ctx: &Context, key: &str, prefix: bool, start_revision: i64) -> WatchChan;
}

/// 内存 etcd 中保存的单个键值条目。
#[derive(Clone)]
struct StoredValue {
    value: Vec<u8>,
    mod_revision: i64,
    lease: Option<i64>,
}
/// 一条已注册的 watch 订阅。
struct WatchRegistration {
    key: String,
    prefix: bool,
    start_revision: i64,
    sender: mpsc::Sender<WatchResponse>,
}
/// 进程内模拟的 etcd 客户端，供测试与本地推演使用。
#[derive(Default)]
pub struct MemoryEtcdClient {
    values: RwLock<BTreeMap<String, StoredValue>>,
    revision: AtomicI64,
    history: Mutex<Vec<(i64, Event)>>,
    watchers: Mutex<Vec<WatchRegistration>>,
    get_failures: AtomicUsize,
    put_failures: AtomicUsize,
}
impl MemoryEtcdClient {
    /// 接下来 `count` 次 Get 将注入失败。
    pub fn FailGets(&self, count: usize) {
        self.get_failures.store(count, Ordering::Release);
    }
    /// 接下来 `count` 次 Put 将注入失败。
    pub fn FailPuts(&self, count: usize) {
        self.put_failures.store(count, Ordering::Release);
    }
    /// 若计数器大于 0 则减一并返回 true（表示本次应失败）。
    fn consume(counter: &AtomicUsize) -> bool {
        counter
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| {
                (v > 0).then(|| v - 1)
            })
            .is_ok()
    }
    /// 向匹配的订阅者推送事件；发送失败的订阅会被移除。
    fn notify(&self, event: Event, revision: i64) {
        self.watchers
            .lock()
            .expect("watchers lock poisoned")
            .retain(|watcher| {
                if revision < watcher.start_revision {
                    return true;
                }
                let key = String::from_utf8_lossy(&event.Kv.Key);
                if !(if watcher.prefix {
                    key.starts_with(&watcher.key)
                } else {
                    key == watcher.key
                }) {
                    return true;
                }
                watcher
                    .sender
                    .send(WatchResponse {
                        Events: vec![event.clone()],
                        CompactRevision: 0,
                        Error: None,
                    })
                    .is_ok()
            });
    }
    /// 记录事件并通知订阅者，使带 revision 的后注册 watch 能补收历史事件。
    fn record_and_notify(&self, event: Event, revision: i64) {
        let mut history = self.history.lock().expect("etcd history lock poisoned");
        history.push((revision, event.clone()));
        // 持有 history 锁直到通知完成，避免 Watch 在历史快照与注册之间漏事件。
        self.notify(event, revision);
    }
    /// 递增 revision 写入键值，并通知匹配的 watch 订阅者。
    fn write(
        &self,
        ctx: &Context,
        key: &str,
        value: &str,
        lease: Option<i64>,
    ) -> Result<(), SyncError> {
        if ctx.Done() {
            return Err(ctx.Err().unwrap());
        }
        if Self::consume(&self.put_failures) {
            return Err(SyncError("injected etcd put failure".into()));
        }
        let revision = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
        self.values
            .write()
            .expect("etcd values lock poisoned")
            .insert(
                key.into(),
                StoredValue {
                    value: value.as_bytes().to_vec(),
                    mod_revision: revision,
                    lease,
                },
            );
        self.record_and_notify(
            Event {
                Kv: KeyValue {
                    Key: key.as_bytes().to_vec(),
                    Value: value.as_bytes().to_vec(),
                    ModRevision: revision,
                },
                Type: EventType::PUT,
            },
            revision,
        );
        Ok(())
    }
}
impl EtcdClient for MemoryEtcdClient {
    fn PutIfAbsent(&self, ctx: &Context, key: &str, value: &str) -> Result<bool, SyncError> {
        if ctx.Done() {
            return Err(ctx.Err().unwrap());
        }
        if Self::consume(&self.put_failures) {
            return Err(SyncError("injected etcd put failure".into()));
        }
        // 键已存在则视为 CAS 失败，不覆盖。
        let mut values = self.values.write().expect("etcd values lock poisoned");
        if values.contains_key(key) {
            return Ok(false);
        }
        let revision = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
        values.insert(
            key.into(),
            StoredValue {
                value: value.as_bytes().to_vec(),
                mod_revision: revision,
                lease: None,
            },
        );
        drop(values);
        self.record_and_notify(
            Event {
                Kv: KeyValue {
                    Key: key.as_bytes().to_vec(),
                    Value: value.as_bytes().to_vec(),
                    ModRevision: revision,
                },
                Type: EventType::PUT,
            },
            revision,
        );
        Ok(true)
    }
    fn Put(
        &self,
        ctx: &Context,
        key: &str,
        value: &str,
        lease: Option<i64>,
    ) -> Result<(), SyncError> {
        self.write(ctx, key, value, lease)
    }
    fn PutMono(&self, ctx: &Context, key: &str, value: &str) -> Result<(), SyncError> {
        if ctx.Done() {
            return Err(ctx.Err().unwrap());
        }
        if Self::consume(&self.put_failures) {
            return Err(SyncError("injected etcd put failure".into()));
        }

        // Match Go's PutKVToEtcdMono: read the current modification revision,
        // then commit only if no concurrent writer changed it in the meantime.
        let previous_revision = self
            .values
            .read()
            .expect("etcd values lock poisoned")
            .get(key)
            .map_or(0, |stored| stored.mod_revision);
        std::thread::yield_now();
        let mut values = self.values.write().expect("etcd values lock poisoned");
        let current_revision = values.get(key).map_or(0, |stored| stored.mod_revision);
        if current_revision != previous_revision {
            return Err(SyncError(
                "performing compare-and-swap during PutKVToEtcd failed".into(),
            ));
        }
        let revision = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
        values.insert(
            key.into(),
            StoredValue {
                value: value.as_bytes().to_vec(),
                mod_revision: revision,
                lease: None,
            },
        );
        drop(values);
        self.record_and_notify(
            Event {
                Kv: KeyValue {
                    Key: key.as_bytes().to_vec(),
                    Value: value.as_bytes().to_vec(),
                    ModRevision: revision,
                },
                Type: EventType::PUT,
            },
            revision,
        );
        Ok(())
    }
    fn Get(&self, ctx: &Context, key: &str, prefix: bool) -> Result<GetResponse, SyncError> {
        if ctx.Done() {
            return Err(ctx.Err().unwrap());
        }
        if Self::consume(&self.get_failures) {
            return Err(SyncError("injected etcd get failure".into()));
        }
        let values = self.values.read().expect("etcd values lock poisoned");
        let Kvs = values
            .iter()
            .filter(|(candidate, _)| {
                if prefix {
                    candidate.starts_with(key)
                } else {
                    candidate.as_str() == key
                }
            })
            .map(|(key, value)| KeyValue {
                Key: key.as_bytes().to_vec(),
                Value: value.value.clone(),
                ModRevision: value.mod_revision,
            })
            .collect();
        Ok(GetResponse {
            Kvs,
            Revision: self.revision.load(Ordering::Acquire),
        })
    }
    fn Delete(&self, ctx: &Context, key: &str) -> Result<(), SyncError> {
        if ctx.Done() {
            return Err(ctx.Err().unwrap());
        }
        if self
            .values
            .write()
            .expect("etcd values lock poisoned")
            .remove(key)
            .is_some()
        {
            let revision = self.revision.fetch_add(1, Ordering::AcqRel) + 1;
            self.record_and_notify(
                Event {
                    Kv: KeyValue {
                        Key: key.as_bytes().to_vec(),
                        Value: Vec::new(),
                        ModRevision: revision,
                    },
                    Type: EventType::DELETE,
                },
                revision,
            );
        }
        Ok(())
    }
    fn Watch(&self, _ctx: &Context, key: &str, prefix: bool, start_revision: i64) -> WatchChan {
        let (sender, receiver) = mpsc::channel();
        let history = self.history.lock().expect("etcd history lock poisoned");
        for (_, event) in history.iter().filter(|(revision, event)| {
            // etcd 未指定 WithRev（0）时只发送注册后的新事件。
            if start_revision == 0 || *revision < start_revision {
                return false;
            }
            let event_key = String::from_utf8_lossy(&event.Kv.Key);
            if prefix {
                event_key.starts_with(key)
            } else {
                event_key == key
            }
        }) {
            let _ = sender.send(WatchResponse {
                Events: vec![event.clone()],
                CompactRevision: 0,
                Error: None,
            });
        }
        self.watchers
            .lock()
            .expect("watchers lock poisoned")
            .push(WatchRegistration {
                key: key.into(),
                prefix,
                start_revision,
                sender,
            });
        drop(history);
        WatchChan::New(receiver)
    }
}

/// 模拟 etcd session：持有 lease ID 与完成信号。
#[derive(Clone)]
pub struct Session {
    lease: i64,
    done: DoneSignal,
    cleanup: Option<Arc<SessionCleanup>>,
}
impl Session {
    /// 分配新的单调递增 lease 并创建未关闭的完成信号。
    pub fn New() -> Self {
        static NEXT: AtomicI64 = AtomicI64::new(1);
        Self {
            lease: NEXT.fetch_add(1, Ordering::Relaxed),
            done: DoneSignal::New(),
            cleanup: None,
        }
    }
    /// Bind the lease completion signal and idempotent backend cleanup.
    pub fn WithLease(
        lease: i64,
        done: DoneSignal,
        cleanup: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            lease,
            done,
            cleanup: Some(Arc::new(SessionCleanup {
                closed: AtomicBool::new(false),
                callback: Box::new(cleanup),
            })),
        }
    }
    /// 返回本会话的 lease ID。
    pub fn Lease(&self) -> i64 {
        self.lease
    }
    /// 返回完成信号的克隆。
    pub fn Done(&self) -> DoneSignal {
        self.done.clone()
    }
    /// 关闭会话完成信号。
    pub fn Close(&self) {
        self.done.Close();
        if let Some(cleanup) = &self.cleanup {
            cleanup.close();
        }
    }
}
struct SessionCleanup {
    closed: AtomicBool,
    callback: Box<dyn Fn() + Send + Sync>,
}
impl SessionCleanup {
    fn close(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            (self.callback)();
        }
    }
}
impl Drop for SessionCleanup {
    fn drop(&mut self) {
        self.close();
    }
}
impl Default for Session {
    fn default() -> Self {
        Self::New()
    }
}

/// 全局 schema 版本键的观察器，可重新订阅。
pub struct Watcher {
    channel: RwLock<WatchChan>,
    context: Mutex<Option<Context>>,
}
impl Watcher {
    /// 创建尚未订阅任何键的观察器。
    pub fn New() -> Self {
        Self {
            channel: RwLock::new(empty_watch_channel()),
            context: Mutex::new(None),
        }
    }
    /// 订阅指定键的变更，替换当前通道。
    pub fn Watch(&self, ctx: &Context, client: &dyn EtcdClient, key: &str) {
        let child = ctx.Child();
        if let Some(old) = self.context.lock().unwrap().replace(child.clone()) {
            old.Cancel();
        }
        *self.channel.write().expect("watcher lock poisoned") = client.Watch(&child, key, false, 0);
    }
    /// 重新订阅（断线或 compaction 后恢复观察）。
    pub fn Rewatch(&self, ctx: &Context, client: &dyn EtcdClient, key: &str) {
        self.Watch(ctx, client, key);
    }
    /// 返回当前观察通道。
    pub fn WatchChan(&self) -> WatchChan {
        self.channel.read().expect("watcher lock poisoned").clone()
    }
}
impl Drop for Watcher {
    fn drop(&mut self) {
        if let Some(ctx) = self.context.lock().unwrap().take() {
            ctx.Cancel();
        }
    }
}
impl Default for Watcher {
    fn default() -> Self {
        Self::New()
    }
}

/// 等待版本同步完成时汇总的服务器计数信息。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SyncSummary {
    /// 参与同步的服务器实例数。
    pub ServerCount: i32,
    /// 其中被标记为 assumed（假定/临时）的实例数。
    pub AssumedServerCount: i32,
}
impl SyncSummary {
    /// 格式化为可读摘要字符串。
    pub fn String(&self) -> String {
        if self.AssumedServerCount > 0 {
            format!(
                "server count: {}, assumed server count: {}",
                self.ServerCount, self.AssumedServerCount
            )
        } else {
            format!("server count: {}", self.ServerCount)
        }
    }
}
impl fmt::Display for SyncSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.String())
    }
}

/// Schema 版本同步器协议：实例上报版本、owner 发布全局版本并等待追上。
pub trait Syncer: Send + Sync {
    /// 初始化：确保全局版本键存在，并注册本机版本路径。
    fn Init(&self, ctx: Context) -> Result<(), SyncError>;
    /// 上报本机（或指定 job）已加载的 schema 版本。
    fn UpdateSelfVersion(&self, ctx: Context, jobID: i64, version: i64) -> Result<(), SyncError>;
    /// Owner 将全局 schema 版本推进到 `version`。
    fn OwnerUpdateGlobalVersion(&self, ctx: Context, version: i64) -> Result<(), SyncError>;
    /// 返回全局版本变更的观察通道。
    fn GlobalVersionCh(&self) -> WatchChan;
    /// 重新订阅全局 schema 版本键。
    fn WatchGlobalSchemaVer(&self, ctx: Context);
    /// 返回当前 etcd session 的完成信号。
    fn Done(&self) -> DoneSignal;
    /// 在 session 失效后重新建立会话并写回本机版本路径。
    fn Restart(&self, ctx: Context) -> Result<(), SyncError>;
    /// 阻塞直到相关实例的 schema 版本均不低于 `latestVer`。
    fn WaitVersionSynced(
        &self,
        ctx: Context,
        jobID: i64,
        latestVer: i64,
        checkAssumedSvr: bool,
    ) -> Result<SyncSummary, SyncError>;
    /// 后台循环：持续同步按 job 分路径的节点版本。
    fn SyncJobSchemaVerLoop(&self, ctx: Context);
    /// 注入/替换用于枚举在线实例的 serverinfo 同步器。
    fn SetServerInfoSyncer(&self, syncer: Option<Arc<serverinfo::Syncer>>);
    /// 关闭并清理本机在 etcd 上的版本路径。
    fn Close(&self);
}

/// 一次性匹配回调：当节点版本集合满足条件时返回 true 并被消费掉。
type MatchFn = Box<dyn FnMut(&HashMap<String, i64>) -> bool + Send>;
/// `nodeVersions` 的内部可变状态。
struct NodeVersionsInner {
    versions: HashMap<String, i64>,
    once_match: Option<MatchFn>,
}
/// 跟踪某 DDL job 下各节点已上报的 schema 版本，并可挂接一次性匹配回调。
pub struct nodeVersions {
    inner: Mutex<NodeVersionsInner>,
}
/// 构造带初始容量与可选一次性匹配回调的节点版本集合。
pub fn newNodeVersions(initialCap: usize, once_match: Option<MatchFn>) -> Arc<nodeVersions> {
    Arc::new(nodeVersions {
        inner: Mutex::new(NodeVersionsInner {
            versions: HashMap::with_capacity(initialCap),
            once_match,
        }),
    })
}
impl nodeVersions {
    /// 记录/更新某节点版本；若存在一次性回调且匹配成功则消费掉回调。
    pub fn add(&self, nodeID: String, ver: i64) {
        let mut inner = self.inner.lock().expect("node versions lock poisoned");
        inner.versions.insert(nodeID, ver);
        if let Some(mut callback) = inner.once_match.take() {
            if !callback(&inner.versions) {
                inner.once_match = Some(callback);
            }
        }
    }
    /// 移除某节点的版本记录。
    pub fn del(&self, nodeID: &str) {
        self.inner
            .lock()
            .expect("node versions lock poisoned")
            .versions
            .remove(nodeID);
    }
    /// 当前记录的节点数。
    pub fn len(&self) -> usize {
        self.inner
            .lock()
            .expect("node versions lock poisoned")
            .versions
            .len()
    }
    /// 立即尝试匹配；未匹配则保存回调待后续 `add` 触发。
    pub fn matchOrSet(&self, mut callback: MatchFn) {
        let mut inner = self.inner.lock().expect("node versions lock poisoned");
        if !callback(&inner.versions) {
            inner.once_match = Some(callback);
        }
    }
    /// 清空全部节点版本数据。
    pub fn clearData(&self) {
        self.inner
            .lock()
            .expect("node versions lock poisoned")
            .versions
            .clear();
    }
    /// 丢弃挂起的一次性匹配回调。
    pub fn clearMatchFn(&self) {
        self.inner
            .lock()
            .expect("node versions lock poisoned")
            .once_match = None;
    }
    /// 无版本数据且无挂起回调时视为可回收。
    pub fn emptyAndNotUsed(&self) -> bool {
        let inner = self.inner.lock().expect("node versions lock poisoned");
        inner.versions.is_empty() && inner.once_match.is_none()
    }
    /// 是否仍挂有一次性匹配回调（测试用）。
    pub fn getMatchFn(&self) -> bool {
        self.inner
            .lock()
            .expect("node versions lock poisoned")
            .once_match
            .is_some()
    }
}

/// 基于 etcd 的 schema 版本同步器实现。
pub struct etcdSyncer {
    /// 本机在 etcd 上的 schema 版本完整路径。
    pub selfSchemaVerPath: String,
    /// etcd 客户端。
    pub etcdCli: Arc<dyn EtcdClient>,
    session: RwLock<Arc<Session>>,
    globalVerWatcher: Watcher,
    /// 本 DDL 实例的唯一 ID。
    pub ddlID: String,
    jobNodeVersions: Mutex<HashMap<i64, Arc<nodeVersions>>>,
    /// 按 job 上报版本的 etcd 键前缀。
    pub jobNodeVerPrefix: String,
    svrInfoSyncer: RwLock<Option<Arc<serverinfo::Syncer>>>,
}
/// 用给定 etcd 客户端与实例 ID 构造同步器。
pub fn NewEtcdSyncer(etcdCli: Arc<dyn EtcdClient>, id: impl Into<String>) -> Arc<etcdSyncer> {
    let id = id.into();
    Arc::new(etcdSyncer {
        etcdCli,
        selfSchemaVerPath: format!("{DDLAllSchemaVersions}/{id}"),
        session: RwLock::new(Arc::new(Session::New())),
        globalVerWatcher: Watcher::New(),
        ddlID: id,
        jobNodeVersions: Mutex::new(HashMap::new()),
        jobNodeVerPrefix: format!("{DDLAllSchemaVersionsByJob}/"),
        svrInfoSyncer: RwLock::new(None),
    })
}
impl etcdSyncer {
    /// 读取当前 session。
    fn loadSession(&self) -> Arc<Session> {
        self.session.read().expect("session lock poisoned").clone()
    }
    /// 替换当前 session。
    fn storeSession(&self, session: Arc<Session>) {
        *self.session.write().expect("session lock poisoned") = session;
    }
    fn newSession(&self, ctx: &Context, retries: i64) -> Result<Arc<Session>, SyncError> {
        let mut attempts = 0;
        loop {
            if let Some(error) = ctx.Err() {
                return Err(error);
            }
            attempts += 1;
            match self.etcdCli.NewSession(ctx, SessionTTL) {
                Ok(session) => return Ok(Arc::new(session)),
                Err(error) if attempts >= retries => return Err(error),
                Err(_) => {
                    if !ctx.Wait(Duration::from_millis(200)) {
                        return Err(ctx.Err().unwrap());
                    }
                }
            }
        }
    }
    /// 带重试的 etcd Put / PutMono。
    fn putRetry(
        &self,
        ctx: &Context,
        retry_count: i64,
        key: &str,
        value: &str,
        lease: Option<i64>,
        mono: bool,
    ) -> Result<(), SyncError> {
        let mut attempts = 0_i64;
        loop {
            if ctx.Done() {
                return Err(ctx.Err().unwrap());
            }
            attempts += 1;
            let result = if mono {
                self.etcdCli.PutMono(ctx, key, value)
            } else {
                self.etcdCli.Put(ctx, key, value, lease)
            };
            if result.is_ok() {
                return result;
            }
            if retry_count != putKeyRetryUnlimited && attempts >= retry_count {
                return result;
            }
            if !ctx.Wait(Duration::from_millis(20)) {
                return Err(ctx.Err().unwrap());
            }
        }
    }
    /// 初始化全局版本键、本机会话与全局版本观察。
    pub fn Init(&self, ctx: Context) -> Result<(), SyncError> {
        self.etcdCli
            .PutIfAbsent(&ctx, DDLGlobalSchemaVersion, InitialVersion)?;
        let session = self.newSession(&ctx, keyOpDefaultRetryCnt)?;
        self.storeSession(session.clone());
        self.globalVerWatcher
            .Watch(&ctx, self.etcdCli.as_ref(), DDLGlobalSchemaVersion);
        self.putRetry(
            &ctx,
            keyOpDefaultRetryCnt,
            &self.selfSchemaVerPath,
            InitialVersion,
            Some(session.Lease()),
            false,
        )
    }
    /// 返回当前 session 的完成信号；可注入强制关闭。
    pub fn Done(&self) -> DoneSignal {
        let session = self.loadSession();
        if ERROR_MOCK_SESSION_DONE.load(Ordering::Acquire) {
            session.Close();
        }
        session.Done()
    }
    /// 重建 session 并以无限重试写回本机初始版本路径。
    pub fn Restart(&self, ctx: Context) -> Result<(), SyncError> {
        let session = self.newSession(&ctx, putKeyRetryUnlimited)?;
        self.storeSession(session.clone());
        self.putRetry(
            &ctx.WithTimeout(Duration::from_secs(1)),
            putKeyRetryUnlimited,
            &self.selfSchemaVerPath,
            InitialVersion,
            Some(session.Lease()),
            false,
        )
    }
    /// 返回全局版本观察通道。
    pub fn GlobalVersionCh(&self) -> WatchChan {
        self.globalVerWatcher.WatchChan()
    }
    /// 重新订阅全局 schema 版本键。
    pub fn WatchGlobalSchemaVer(&self, ctx: Context) {
        self.globalVerWatcher
            .Rewatch(&ctx, self.etcdCli.as_ref(), DDLGlobalSchemaVersion);
    }
    /// 上报本机或按 job 路径的 schema 版本。
    pub fn UpdateSelfVersion(
        &self,
        ctx: Context,
        jobID: i64,
        version: i64,
    ) -> Result<(), SyncError> {
        let value = version.to_string();
        // MDL 开启时写入按 job 分路径的键；jobID 为 0 时跳过。
        if IsMDLEnabled() {
            if jobID == 0 {
                return Ok(());
            }
            self.putRetry(
                &ctx,
                keyOpDefaultRetryCnt,
                &format!("{DDLAllSchemaVersionsByJob}/{jobID}/{}", self.ddlID),
                &value,
                None,
                true,
            )
        } else {
            self.putRetry(
                &ctx,
                putKeyRetryUnlimited,
                &self.selfSchemaVerPath,
                &value,
                Some(self.loadSession().Lease()),
                false,
            )
        }
    }
    /// Owner 推进全局 schema 版本。
    pub fn OwnerUpdateGlobalVersion(&self, ctx: Context, version: i64) -> Result<(), SyncError> {
        self.putRetry(
            &ctx,
            putKeyRetryUnlimited,
            DDLGlobalSchemaVersion,
            &version.to_string(),
            None,
            false,
        )
    }
    /// 删除本机在 etcd 上的版本路径（带有限重试）。
    pub fn removeSelfVersionPath(&self) -> Result<(), SyncError> {
        let ctx = Context::Background().WithTimeout(Duration::from_secs(1));
        let mut last = Ok(());
        for _ in 0..keyOpDefaultRetryCnt {
            last = self.etcdCli.Delete(&ctx, &self.selfSchemaVerPath);
            if last.is_ok() {
                break;
            }
        }
        last
    }
    /// 等待集群内相关实例的 schema 版本追上 `latestVer`。
    pub fn WaitVersionSynced(
        &self,
        ctx: Context,
        jobID: i64,
        latestVer: i64,
        checkAssumedSvr: bool,
    ) -> Result<SyncSummary, SyncError> {
        // 非 MDL 模式先短暂等待，给各实例一点上报时间。
        if !IsMDLEnabled() && !ctx.Wait(CheckVersFirstWaitTime()) {
            return Err(ctx.Err().unwrap());
        }
        let mut not_match = 0;
        let interval_count =
            (Duration::from_secs(1).as_millis() / checkVersInterval.as_millis()) as i32;
        let mut updated = HashMap::new();
        loop {
            if ctx.Done() {
                return Err(ctx.Err().unwrap());
            }
            if IsMDLEnabled() {
                let (summary, synced) =
                    self.waitVersionSyncedWithMDL(ctx.clone(), jobID, latestVer, checkAssumedSvr)?;
                if synced {
                    return Ok(summary.unwrap());
                }
            } else {
                // 非 MDL：扫描所有实例版本路径，累计已追上的节点。
                match self.etcdCli.Get(&ctx, DDLAllSchemaVersions, true) {
                    Ok(response) => {
                        let mut success = true;
                        for kv in response.Kvs {
                            let key = String::from_utf8_lossy(&kv.Key).into_owned();
                            if updated.contains_key(&key) {
                                continue;
                            }
                            if !isUpdatedLatestVersion(
                                &key,
                                &String::from_utf8_lossy(&kv.Value),
                                latestVer,
                                not_match,
                                interval_count,
                            ) {
                                success = false;
                                break;
                            }
                            updated.insert(key, ());
                        }
                        if success {
                            return Ok(SyncSummary {
                                ServerCount: updated.len() as i32,
                                AssumedServerCount: 0,
                            });
                        }
                    }
                    Err(_) => {}
                }
                if !ctx.Wait(checkVersInterval) {
                    return Err(ctx.Err().unwrap());
                }
                not_match += 1;
            }
        }
    }
    /// MDL 路径：枚举在线实例，挂接一次性匹配回调等待全员追上。
    pub fn waitVersionSyncedWithMDL(
        &self,
        ctx: Context,
        jobID: i64,
        latestVer: i64,
        checkAssumedSvr: bool,
    ) -> Result<(Option<SyncSummary>, bool), SyncError> {
        let servers = self.getServersForISSync(ctx.clone(), checkAssumedSvr)?;
        let (updated, summary) = calculateUpdatedMap(servers);
        let (notify_tx, notify_rx) = mpsc::sync_channel(1);
        let unmatched = Arc::new(Mutex::new(None::<String>));
        let unmatched_copy = unmatched.clone();
        // 回调在所有目标实例版本均 >= latestVer 时通知成功。
        let callback: MatchFn = Box::new(move |versions| {
            if versions.is_empty() {
                return false;
            }
            for (id, info) in &updated {
                if versions.get(id).is_none_or(|version| *version < latestVer) {
                    *unmatched_copy.lock().expect("unmatched lock poisoned") = Some(info.clone());
                    return false;
                }
            }
            let _ = notify_tx.try_send(());
            true
        });
        let item = self.jobSchemaVerMatchOrSet(jobID, callback);
        let deadline = Instant::now() + Duration::from_secs(1);
        loop {
            if notify_rx.try_recv().is_ok() {
                return Ok((Some(summary), true));
            }
            if ctx.Done() {
                item.clearMatchFn();
                return Err(ctx.Err().unwrap());
            }
            if Instant::now() >= deadline {
                item.clearMatchFn();
                return Ok((None, false));
            }
            ctx.Wait(Duration::from_millis(10));
        }
    }
    /// 从 serverinfo 同步器获取参与 info schema 同步的实例集合。
    pub fn getServersForISSync(
        &self,
        _ctx: Context,
        checkAssumedSvr: bool,
    ) -> Result<HashMap<String, serverinfo::ServerInfo>, SyncError> {
        let syncer = self
            .svrInfoSyncer
            .read()
            .expect("server syncer lock poisoned")
            .clone()
            .ok_or_else(|| SyncError("server info syncer is not set".into()))?;
        let mut servers = syncer
            .GetAllServerInfo(serverinfo::Context::Background())
            .map_err(|e| SyncError(e.to_string()))?;
        // next-gen 且不检查 assumed 时，过滤掉假定实例。
        if NEXT_GEN.load(Ordering::Acquire) && !checkAssumedSvr {
            servers.retain(|_, info| !info.StaticInfo.IsAssumed());
        }
        Ok(servers)
    }
    /// 周期性运行 `syncJobSchemaVer`，直到上下文结束。
    pub fn SyncJobSchemaVerLoop(&self, ctx: Context) {
        loop {
            self.syncJobSchemaVer(ctx.clone());
            if !ctx.Wait(Duration::from_secs(1)) {
                return;
            }
        }
    }
    /// 拉取并持续 watch 按 job 分路径的节点版本键。
    pub fn syncJobSchemaVer(&self, ctx: Context) {
        let Ok(response) = self.etcdCli.Get(&ctx, &self.jobNodeVerPrefix, true) else {
            return;
        };
        {
            // 全量刷新前先清空旧数据，并回收已无用的 job 条目。
            let mut jobs = self
                .jobNodeVersions
                .lock()
                .expect("job versions lock poisoned");
            for item in jobs.values() {
                item.clearData();
            }
            jobs.retain(|_, item| !item.emptyAndNotUsed());
        }
        for kv in response.Kvs {
            self.handleJobSchemaVerKV(&kv, EventType::PUT);
        }
        let watch = self
            .etcdCli
            .Watch(&ctx, &self.jobNodeVerPrefix, true, response.Revision + 1);
        loop {
            if ctx.Done() {
                return;
            }
            match watch.RecvTimeout(Duration::from_millis(100)) {
                Ok(mut response) => {
                    if MOCK_COMPACTION.swap(false, Ordering::AcqRel) {
                        response.CompactRevision = 123;
                    }
                    // 出错或发生 compaction 时退出，由外层循环重建 watch。
                    if response.Error.is_some() || response.CompactRevision != 0 {
                        return;
                    }
                    for event in response.Events {
                        self.handleJobSchemaVerKV(&event.Kv, event.Type);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    }
    /// 解码单条 job 版本 KV 事件并更新内存中的节点版本表。
    pub fn handleJobSchemaVerKV(&self, kv: &KeyValue, event_type: EventType) {
        let (job_id, node_id, version, valid) =
            decodeJobVersionEvent(kv, event_type, &self.jobNodeVerPrefix);
        if !valid {
            return;
        }
        let mut jobs = self
            .jobNodeVersions
            .lock()
            .expect("job versions lock poisoned");
        if event_type == EventType::PUT {
            jobs.entry(job_id)
                .or_insert_with(|| newNodeVersions(1, None))
                .add(node_id, version);
        } else if let Some(item) = jobs.get(&job_id).cloned() {
            item.del(&node_id);
            if item.len() == 0 {
                jobs.remove(&job_id);
            }
        }
    }
    /// 为指定 job 挂接或立即执行匹配回调，并返回对应的节点版本集合。
    pub fn jobSchemaVerMatchOrSet(&self, jobID: i64, callback: MatchFn) -> Arc<nodeVersions> {
        let mut jobs = self
            .jobNodeVersions
            .lock()
            .expect("job versions lock poisoned");
        let item = jobs
            .entry(jobID)
            .or_insert_with(|| newNodeVersions(1, None))
            .clone();
        item.matchOrSet(callback);
        item
    }
    /// 设置用于枚举在线实例的 serverinfo 同步器。
    pub fn SetServerInfoSyncer(&self, syncer: Option<Arc<serverinfo::Syncer>>) {
        *self
            .svrInfoSyncer
            .write()
            .expect("server syncer lock poisoned") = syncer;
    }
    /// 关闭同步器：删除本机版本路径。
    pub fn Close(&self) {
        let _ = self.removeSelfVersionPath();
        self.loadSession().Close();
        if let Some(ctx) = self.globalVerWatcher.context.lock().unwrap().take() {
            ctx.Cancel();
        }
    }
}
/// 将 `etcdSyncer` 方法转发为 `Syncer` trait 实现。
impl Syncer for etcdSyncer {
    fn Init(&self, c: Context) -> Result<(), SyncError> {
        etcdSyncer::Init(self, c)
    }
    fn UpdateSelfVersion(&self, c: Context, j: i64, v: i64) -> Result<(), SyncError> {
        etcdSyncer::UpdateSelfVersion(self, c, j, v)
    }
    fn OwnerUpdateGlobalVersion(&self, c: Context, v: i64) -> Result<(), SyncError> {
        etcdSyncer::OwnerUpdateGlobalVersion(self, c, v)
    }
    fn GlobalVersionCh(&self) -> WatchChan {
        etcdSyncer::GlobalVersionCh(self)
    }
    fn WatchGlobalSchemaVer(&self, c: Context) {
        etcdSyncer::WatchGlobalSchemaVer(self, c)
    }
    fn Done(&self) -> DoneSignal {
        etcdSyncer::Done(self)
    }
    fn Restart(&self, c: Context) -> Result<(), SyncError> {
        etcdSyncer::Restart(self, c)
    }
    fn WaitVersionSynced(
        &self,
        c: Context,
        j: i64,
        v: i64,
        a: bool,
    ) -> Result<SyncSummary, SyncError> {
        etcdSyncer::WaitVersionSynced(self, c, j, v, a)
    }
    fn SyncJobSchemaVerLoop(&self, c: Context) {
        etcdSyncer::SyncJobSchemaVerLoop(self, c)
    }
    fn SetServerInfoSyncer(&self, s: Option<Arc<serverinfo::Syncer>>) {
        etcdSyncer::SetServerInfoSyncer(self, s)
    }
    fn Close(&self) {
        etcdSyncer::Close(self)
    }
}

/// 从 etcd 键值解码 job ID、节点 ID 与版本号；无效时第四个返回值为 false。
pub fn decodeJobVersionEvent(
    kv: &KeyValue,
    event_type: EventType,
    prefix: &str,
) -> (i64, String, i64, bool) {
    let key = String::from_utf8_lossy(&kv.Key);
    let left = key.strip_prefix(prefix).unwrap_or(&key);
    // 键格式：`{prefix}{jobID}/{nodeID}`。
    let parts: Vec<_> = left.split('/').collect();
    if parts.len() != 2 {
        return (0, String::new(), 0, false);
    }
    let Ok(job_id) = parts[0].parse() else {
        return (0, String::new(), 0, false);
    };
    let version = if event_type == EventType::PUT {
        let Ok(v) = String::from_utf8_lossy(&kv.Value).parse() else {
            return (0, String::new(), 0, false);
        };
        v
    } else {
        0
    };
    (job_id, parts[1].into(), version, true)
}
/// 判断某实例上报的版本字符串是否已达到 `latest`。
pub fn isUpdatedLatestVersion(
    _key: &str,
    value: &str,
    latest: i64,
    _not_match: i32,
    interval: i32,
) -> bool {
    let Ok(version) = value.parse::<i64>() else {
        return false;
    };
    if version < latest {
        let _ = interval.max(1);
        return false;
    }
    true
}
/// 生成实例执行 ID：`ip:port`（IPv6 地址会加方括号）。
fn generateExecID(info: &serverinfo::ServerInfo) -> String {
    let host = &info.StaticInfo.IP;
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{}", info.StaticInfo.Port)
    } else {
        format!("{host}:{}", info.StaticInfo.Port)
    }
}
/// 将在线实例去重为「每个 ip:port 保留最新启动时间」的更新映射与汇总。
pub fn calculateUpdatedMap(
    serverInfos: HashMap<String, serverinfo::ServerInfo>,
) -> (HashMap<String, String>, SyncSummary) {
    let mut updated = HashMap::new();
    let mut instances: HashMap<String, String> = HashMap::new();
    let mut assumed = 0;
    for info in serverInfos.values() {
        let instance = generateExecID(info);
        // 同一实例地址出现多个 ID 时，只保留启动时间更新的那个。
        if let Some(old_id) = instances.get(&instance).cloned() {
            if info.StaticInfo.StartTimestamp <= serverInfos[&old_id].StaticInfo.StartTimestamp {
                continue;
            }
            updated.remove(&old_id);
            if serverInfos[&old_id].StaticInfo.IsAssumed() {
                assumed -= 1;
            }
        }
        updated.insert(info.StaticInfo.ID.clone(), getSvrInfoForLog(info));
        instances.insert(instance, info.StaticInfo.ID.clone());
        if info.StaticInfo.IsAssumed() {
            assumed += 1;
        }
    }
    let count = instances.len() as i32;
    (
        updated,
        SyncSummary {
            ServerCount: count,
            AssumedServerCount: assumed,
        },
    )
}
/// 将服务器静态信息格式化为日志友好字符串。
pub fn getSvrInfoForLog(info: &serverinfo::ServerInfo) -> String {
    let static_info = &info.StaticInfo;
    if static_info.IsAssumed() {
        format!(
            "instance ip {}, port {}, id {}, origin keyspace {}",
            static_info.IP, static_info.Port, static_info.ID, static_info.Keyspace
        )
    } else {
        format!(
            "instance ip {}, port {}, id {}",
            static_info.IP, static_info.Port, static_info.ID
        )
    }
}

/// Production schema protocol over a connected, namespaced etcd client.
/// The shared server-info connection retains TLS/endpoint policy; schema leases
/// and watch streams belong to this adapter.
pub struct RealEtcdClient {
    client: etcd_client::Client,
    namespace: String,
    runtime: Arc<tokio::runtime::Runtime>,
    _connection: Arc<serverinfo::RealEtcdClient>,
}
impl RealEtcdClient {
    pub fn new(connection: Arc<serverinfo::RealEtcdClient>) -> Result<Self, SyncError> {
        Ok(Self {
            client: connection.raw_client(),
            namespace: connection.namespace().to_owned(),
            _connection: connection,
            runtime: Arc::new(
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .map_err(|e| SyncError(e.to_string()))?,
            ),
        })
    }
    fn key(&self, key: &str) -> String {
        format!("{}{key}", self.namespace)
    }
    fn run<T>(
        &self,
        ctx: &Context,
        future: impl std::future::Future<Output = Result<T, etcd_client::Error>>,
    ) -> Result<T, SyncError> {
        let bounded = ctx.WithTimeout(Duration::from_secs(1));
        self.runtime.block_on(async {
            tokio::pin!(future);
            loop {
                if let Some(error) = bounded.Err() {
                    return Err(error);
                }
                tokio::select! {
                    result = &mut future => return result.map_err(|e| SyncError(e.to_string())),
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {}
                }
            }
        })
    }
}
impl EtcdClient for RealEtcdClient {
    fn NewSession(&self, ctx: &Context, ttl: i32) -> Result<Session, SyncError> {
        let mut client = self.client.clone();
        let lease = self
            .run(ctx, client.lease_grant(i64::from(ttl), None))?
            .id();
        let (mut keeper, mut stream) = match self.run(ctx, client.lease_keep_alive(lease)) {
            Ok(pair) => pair,
            Err(error) => {
                let _ = self.run(
                    &Context::Background().WithTimeout(Duration::from_secs(1)),
                    client.lease_revoke(lease),
                );
                return Err(error);
            }
        };
        let done = DoneSignal::New();
        let signal = done.clone();
        let context = ctx.clone();
        let task = self.runtime.spawn(async move {
            let interval = Duration::from_secs((ttl.max(1) as u64 / 3).max(1));
            let mut ticker = tokio::time::interval(interval);
            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        if keeper.keep_alive().await.is_err() { break; }
                    }
                    response = stream.message() => {
                        if !matches!(response, Ok(Some(ref response)) if response.ttl() > 0) { break; }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {
                        if context.Done() || signal.Done() { break; }
                    }
                }
            }
            signal.Close();
        });
        let abort = task.abort_handle();
        let client = self.client.clone();
        let runtime = self.runtime.clone();
        let connection = self._connection.clone();
        Ok(Session::WithLease(lease, done, move || {
            abort.abort();
            let mut client = client.clone();
            // Retain the original connection reactor until lease cleanup completes.
            let _connection = &connection;
            runtime.block_on(async move {
                let _ =
                    tokio::time::timeout(Duration::from_secs(1), client.lease_revoke(lease)).await;
            });
        }))
    }

    fn PutIfAbsent(&self, ctx: &Context, key: &str, value: &str) -> Result<bool, SyncError> {
        use etcd_client::{Compare, CompareOp, Txn, TxnOp};
        let key = self.key(key);
        let txn = Txn::new()
            .when([Compare::create_revision(key.clone(), CompareOp::Equal, 0)])
            .and_then([TxnOp::put(key, value, None)]);
        self.run(ctx, self.client.clone().txn(txn))
            .map(|r| r.succeeded())
    }
    fn Put(
        &self,
        ctx: &Context,
        key: &str,
        value: &str,
        lease: Option<i64>,
    ) -> Result<(), SyncError> {
        self.run(
            ctx,
            self.client.clone().put(
                self.key(key),
                value,
                lease.map(|l| etcd_client::PutOptions::new().with_lease(l)),
            ),
        )
        .map(|_| ())
    }
    fn PutMono(&self, ctx: &Context, key: &str, value: &str) -> Result<(), SyncError> {
        use etcd_client::{Compare, CompareOp, Txn, TxnOp};
        // One CAS attempt; putRetry applies the Go retry budget on contention.
        let child = ctx.WithTimeout(Duration::from_secs(1));
        let response = self.Get(&child, key, false)?;
        let revision = response.Kvs.first().map_or(0, |kv| kv.ModRevision);
        let key = self.key(key);
        let txn = Txn::new()
            .when([Compare::mod_revision(
                key.clone(),
                CompareOp::Equal,
                revision,
            )])
            .and_then([TxnOp::put(key, value, None)]);
        if self.run(&child, self.client.clone().txn(txn))?.succeeded() {
            Ok(())
        } else {
            Err(SyncError(
                "performing compare-and-swap during PutKVToEtcd failed".into(),
            ))
        }
    }

    fn Get(&self, ctx: &Context, key: &str, prefix: bool) -> Result<GetResponse, SyncError> {
        let response = self.run(
            ctx,
            self.client.clone().get(
                self.key(key),
                prefix.then(|| etcd_client::GetOptions::new().with_prefix()),
            ),
        )?;
        Ok(GetResponse {
            Revision: response.header().map_or(0, |h| h.revision()),
            Kvs: response
                .kvs()
                .iter()
                .map(|kv| KeyValue {
                    Key: kv
                        .key()
                        .strip_prefix(self.namespace.as_bytes())
                        .unwrap_or(kv.key())
                        .to_vec(),
                    Value: kv.value().to_vec(),
                    ModRevision: kv.mod_revision(),
                })
                .collect(),
        })
    }
    fn Delete(&self, ctx: &Context, key: &str) -> Result<(), SyncError> {
        self.run(ctx, self.client.clone().delete(self.key(key), None))
            .map(|_| ())
    }
    fn Watch(&self, ctx: &Context, key: &str, prefix: bool, start_revision: i64) -> WatchChan {
        let (sender, receiver) = mpsc::channel();
        let key = self.key(key);
        let namespace = self.namespace.clone();
        let context = ctx.clone();
        let mut client = self.client.clone();
        self.runtime.spawn(async move {
            let mut options = etcd_client::WatchOptions::new().with_start_revision(start_revision);
            if prefix { options = options.with_prefix(); }
            let opening = client.watch(key, Some(options));
            tokio::pin!(opening);
            let mut stream = loop {
                tokio::select! {
                    result = &mut opening => {
                        match result {
                            Ok(stream) => break stream,
                            Err(error) => {
                                let _ = sender.send(WatchResponse { Error: Some(SyncError(error.to_string())), ..Default::default() });
                                return;
                            }
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {
                        if context.Done() { return; }
                    }
                }
            };
            loop {
                tokio::select! {
                    response = stream.message() => {
                        match response {
                            Ok(Some(response)) => {
                                let events = response.events().iter().filter_map(|event| {
                                    event.kv().map(|kv| Event {
                                        Type: if event.event_type() == etcd_client::EventType::Delete { EventType::DELETE } else { EventType::PUT },
                                        Kv: KeyValue {
                                            Key: kv.key().strip_prefix(namespace.as_bytes()).unwrap_or(kv.key()).to_vec(),
                                            Value: kv.value().to_vec(),
                                            ModRevision: kv.mod_revision(),
                                        },
                                    })
                                }).collect();
                                let notification = WatchResponse {
                                    Events: events,
                                    CompactRevision: response.compact_revision(),
                                    Error: response.canceled().then(|| SyncError(response.cancel_reason().to_owned())),
                                };
                                if sender.send(notification).is_err() || response.canceled() { break; }
                            }
                            Ok(None) => break,
                            Err(error) => {
                                let _ = sender.send(WatchResponse { Error: Some(SyncError(error.to_string())), ..Default::default() });
                                break;
                            }
                        }
                    }
                    _ = tokio::time::sleep(Duration::from_millis(20)) => {
                        if context.Done() { break; }
                    }
                }
            }
        });
        WatchChan::New(receiver)
    }
}
