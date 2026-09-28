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

// etcd 驱动的 Owner（所有者）选举管理器。
//
// TiDB 多实例通过 etcd 租约（lease）与 campaign 选出唯一 Owner，保证 DDL 等
// 后台任务只有一个节点执行。本模块实现竞选循环、Watch 退位、强制接管、
// Owner 操作字节编解码，以及基于 etcd lock 的分布式锁。

use std::any::Any;
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use etcd_client::{
    Client, Compare, CompareOp, DeleteOptions, EventType, GetOptions, LeaderKey, LockOptions,
    PutOptions, ResignOptions, SortOrder, SortTarget, Txn, TxnOp, WatchOptions,
};
use thiserror::Error;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::mock::mock_owner_op_value;

/// 取消令牌，对应 Go context.Context 的取消语义。
pub type Context = CancellationToken;

/// etcd key 操作默认超时。
const KEY_OP_DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
/// key 操作失败后的重试间隔。
const KEY_OP_RETRY_INTERVAL: Duration = Duration::from_millis(200);
/// 新建 etcd session（lease）的默认重试次数。
const NEW_SESSION_DEFAULT_RETRY_COUNT: usize = 3;
/// 分布式锁请求的最大尝试次数，对应 Go `maxRetryCnt`。
const DISTRIBUTED_LOCK_MAX_RETRY_COUNT: usize = 10;
/// 分布式锁请求的线性退避基数，对应 Go `util.RetryInterval`。
const DISTRIBUTED_LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(500);

/// 全局 Manager session TTL（秒），可被测试动态改写。
static MANAGER_SESSION_TTL: AtomicI32 = AtomicI32::new(60);
/// ForceToBeOwner 每次尝试前的等待毫秒数。
static WAIT_TIME_ON_FORCE_OWNER_MS: AtomicU64 = AtomicU64::new(5_000);

/// Owner 选举与 etcd 操作相关错误。
#[derive(Debug, Error)]
pub enum OwnerError {
    #[error(transparent)]
    Etcd(#[from] etcd_client::Error),
    #[error("operation was cancelled")]
    Cancelled,
    #[error("owner election has no leader")]
    NoLeader,
    #[error("owner information does not match manager {0}")]
    OwnerInfoNotMatch(String),
    #[error("this node is not an owner and cannot resign")]
    NotOwner,
    #[error("owner manager is closed")]
    Closed,
    #[error("put owner key failed, cmp is false")]
    CompareFailed,
    #[error("watcher is closed, key: {0}")]
    WatcherClosed(String),
    #[error("watch canceled, key: {key}: {reason}")]
    WatchCancelled { key: String, reason: String },
    #[error("background task failed: {0}")]
    Join(String),
}

/// Owner 包统一 Result 别名。
pub type Result<T> = std::result::Result<T, OwnerError>;

/// Receives owner state transitions.
/// 接收成为 Owner / 退位事件的监听器。
pub trait Listener: Send + Sync {
    /// 本节点成为 Owner 时回调。
    fn OnBecomeOwner(&self);
    /// 本节点失去 Owner 时回调。
    fn OnRetireOwner(&self);
}

/// Common interface implemented by the etcd-backed and local owner managers.
/// etcd 实现与本地 Mock 共用的 Owner 管理接口。
#[async_trait]
pub trait Manager: Send + Sync + Any {
    /// 向下转型用的 Any 引用。
    fn as_any(&self) -> &dyn Any;
    /// 本节点在选举中的 ID。
    fn ID(&self) -> String;
    /// 当前是否持有 Owner。
    fn IsOwner(&self) -> bool;
    /// 主动标记退位并通知 Listener。
    async fn RetireOwner(&self);
    /// 查询当前 Owner 的节点 ID。
    async fn GetOwnerID(&self, ctx: &Context) -> Result<String>;
    /// 在 Owner key 值上附加操作字节（如升级同步状态）。
    async fn SetOwnerOpValue(&self, ctx: &Context, op: OpType) -> Result<()>;
    /// 启动后台竞选循环；可选覆盖 session TTL。
    async fn CampaignOwner(&self, with_ttl: &[i32]) -> Result<()>;
    /// 取消竞选并撤销 session。
    async fn CampaignCancel(&self);
    /// 仅打断竞选循环，不撤销 lease。
    async fn BreakCampaignLoop(&self);
    /// 向 etcd 声明放弃领导权（resign）。
    async fn ResignOwner(&self, ctx: &Context) -> Result<()>;
    /// 关闭管理器，幂等。
    async fn Close(&self);
    /// 注册状态变更监听器。
    async fn SetListener(&self, listener: Arc<dyn Listener>);
    /// 强制清除旧 Owner key 并尝试成为 Owner（兼容旧版本路径）。
    async fn ForceToBeOwner(&self, ctx: &Context) -> Result<()>;
}

/// DDL 子系统只需查询是否 Owner 的窄接口。
pub trait DDLOwnerChecker: Send + Sync {
    fn IsOwner(&self) -> bool;
}

/// Operation byte appended to an owner election value.
/// 附加在 Owner 选举 value 末尾的操作字节。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u8)]
pub enum OpType {
    #[default]
    OpNone = 0,
    /// 同步升级状态（sync upgrading state）。
    OpSyncUpgradingState = 1,
}

impl OpType {
    /// 是否处于「同步升级状态」操作。
    pub fn IsSyncedUpgradingState(self) -> bool {
        self == Self::OpSyncUpgradingState
    }
}

impl From<u8> for OpType {
    fn from(value: u8) -> Self {
        match value {
            1 => Self::OpSyncUpgradingState,
            _ => Self::OpNone,
        }
    }
}

impl fmt::Display for OpType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::OpNone => "none",
            Self::OpSyncUpgradingState => "sync upgrading state",
        })
    }
}

/// 读取全局 Manager session TTL（秒）。
pub fn ManagerSessionTTL() -> i32 {
    MANAGER_SESSION_TTL.load(Ordering::Relaxed)
}

/// 设置全局 Manager session TTL（秒）。
pub fn SetManagerSessionTTL(ttl: i32) {
    MANAGER_SESSION_TTL.store(ttl, Ordering::Relaxed);
}

/// 读取 ForceToBeOwner 每次尝试前的等待时间。
pub fn WaitTimeOnForceOwner() -> Duration {
    Duration::from_millis(WAIT_TIME_ON_FORCE_OWNER_MS.load(Ordering::Relaxed))
}

/// 设置 ForceToBeOwner 等待时间。
pub fn SetWaitTimeOnForceOwner(wait: Duration) {
    WAIT_TIME_ON_FORCE_OWNER_MS.store(wait.as_millis() as u64, Ordering::Relaxed);
}

/// 单个 etcd session：lease、保活任务与丢失信号。
struct SessionRuntime {
    lease_id: i64,
    cancel: CancellationToken,
    lost: CancellationToken,
    keep_alive: JoinHandle<()>,
}

/// 后台竞选循环的取消令牌与任务句柄。
struct CampaignRuntime {
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

/// OwnerManager 共享内部状态。
struct OwnerManagerInner {
    id: String,
    key: String,
    root_context: Context,
    prompt: String,
    client: Client,
    leader: RwLock<Option<LeaderKey>>,
    session_lease: AtomicI64,
    session: Mutex<Option<SessionRuntime>>,
    campaign: Mutex<Option<CampaignRuntime>>,
    listener: RwLock<Option<Arc<dyn Listener>>>,
    closed: AtomicBool,
}

/// Etcd-backed owner manager. Clones share one election lifecycle.
/// 基于 etcd 的 Owner 管理器；Clone 共享同一选举生命周期。
#[derive(Clone)]
pub struct OwnerManager {
    inner: Arc<OwnerManagerInner>,
}

/// 构造 etcd Owner 管理器并装箱为 `Arc<dyn Manager>`。
pub fn NewOwnerManager(
    ctx: Context,
    client: Client,
    prompt: impl Into<String>,
    id: impl Into<String>,
    key: impl Into<String>,
) -> Arc<dyn Manager> {
    Arc::new(OwnerManager {
        inner: Arc::new(OwnerManagerInner {
            id: id.into(),
            key: key.into(),
            root_context: ctx,
            prompt: prompt.into(),
            client,
            leader: RwLock::new(None),
            session_lease: AtomicI64::new(0),
            session: Mutex::new(None),
            campaign: Mutex::new(None),
            listener: RwLock::new(None),
            closed: AtomicBool::new(false),
        }),
    })
}

impl OwnerManager {
    /// 若已 Close 或根 Context 已取消则返回 Closed。
    fn check_open(&self) -> Result<()> {
        if self.inner.closed.load(Ordering::Acquire) || self.inner.root_context.is_cancelled() {
            Err(OwnerError::Closed)
        } else {
            Ok(())
        }
    }

    /// 停止旧 session 后按重试次数申请 lease 并启动保活。
    async fn start_session(&self, ttl: i32) -> Result<()> {
        self.stop_session().await;
        let mut last_error = None;
        for attempt in 0..NEW_SESSION_DEFAULT_RETRY_COUNT {
            if self.inner.root_context.is_cancelled() {
                return Err(OwnerError::Cancelled);
            }
            let mut client = self.inner.client.clone();
            match client.lease_grant(ttl as i64, None).await {
                Ok(grant) => {
                    let lease_id = grant.id();
                    let (mut keeper, mut stream) = client.lease_keep_alive(lease_id).await?;
                    let cancel = CancellationToken::new();
                    let cancelled = cancel.clone();
                    let lost = CancellationToken::new();
                    let lost_signal = lost.clone();
                    // 按 TTL/3 周期 keep-alive，失败则触发 session lost。
                    let interval = Duration::from_secs((ttl.max(1) as u64 / 3).max(1));
                    let keep_alive = tokio::spawn(async move {
                        loop {
                            tokio::select! {
                                _ = cancelled.cancelled() => break,
                                _ = tokio::time::sleep(interval) => {
                                    if keeper.keep_alive().await.is_err() {
                                        lost_signal.cancel();
                                        break;
                                    }
                                    match stream.message().await {
                                        Ok(Some(response)) if response.ttl() > 0 => {}
                                        _ => {
                                            lost_signal.cancel();
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    });
                    self.inner.session_lease.store(lease_id, Ordering::Release);
                    *self.inner.session.lock().await = Some(SessionRuntime {
                        lease_id,
                        cancel,
                        lost,
                        keep_alive,
                    });
                    return Ok(());
                }
                Err(error) => {
                    last_error = Some(error);
                    if attempt + 1 < NEW_SESSION_DEFAULT_RETRY_COUNT {
                        tokio::time::sleep(KEY_OP_RETRY_INTERVAL).await;
                    }
                }
            }
        }
        Err(last_error.expect("session retry records an error").into())
    }

    /// 若尚无 session 则创建；已有则直接成功。
    async fn ensure_session(&self, ttl: i32) -> Result<()> {
        if self.inner.session.lock().await.is_some() {
            Ok(())
        } else {
            self.start_session(ttl).await
        }
    }

    /// 取消保活任务并 revoke lease。
    async fn stop_session(&self) {
        let runtime = self.inner.session.lock().await.take();
        if let Some(runtime) = runtime {
            runtime.cancel.cancel();
            let _ = runtime.keep_alive.await;
            let mut client = self.inner.client.clone();
            if let Err(error) = client.lease_revoke(runtime.lease_id).await {
                tracing::warn!(
                    ?error,
                    lease_id = runtime.lease_id,
                    "failed to revoke owner lease"
                );
            }
        }
        self.inner.session_lease.store(0, Ordering::Release);
    }

    /// 返回当前 lease_id 与 session 丢失令牌快照。
    async fn session_snapshot(&self) -> Result<(i64, CancellationToken)> {
        self.inner
            .session
            .lock()
            .await
            .as_ref()
            .map(|session| (session.lease_id, session.lost.clone()))
            .ok_or(OwnerError::Cancelled)
    }

    /// 记录 LeaderKey、打日志并回调 OnBecomeOwner。
    async fn become_owner(&self, leader: LeaderKey) {
        *self.inner.leader.write().await = Some(leader);
        tracing::info!(prompt = %self.inner.prompt, id = %self.inner.id, "become owner");
        if let Some(listener) = self.inner.listener.read().await.clone() {
            listener.OnBecomeOwner();
        }
    }

    /// 若当前持有领导权则清除并回调 OnRetireOwner。
    async fn retire_if_owner(&self) {
        if self.inner.leader.write().await.take().is_some() {
            tracing::info!(prompt = %self.inner.prompt, id = %self.inner.id, "retire owner");
            if let Some(listener) = self.inner.listener.read().await.clone() {
                listener.OnRetireOwner();
            }
        }
    }

    /// 竞选主循环：campaign → watch → 失联重建 session，直至取消。
    async fn campaign_loop(self, cancel: CancellationToken, ttl: i32) {
        loop {
            let (lease_id, session_lost) = match self.session_snapshot().await {
                Ok(snapshot) => snapshot,
                Err(_) => break,
            };
            let mut client = self.inner.client.clone();
            // 与取消 / session 丢失竞争，等待 etcd campaign 结果。
            let campaign = tokio::select! {
                _ = cancel.cancelled() => break,
                _ = session_lost.cancelled() => Err(OwnerError::Cancelled),
                response = client.campaign(self.inner.key.clone(), self.inner.id.clone(), lease_id) => {
                    response.map_err(OwnerError::from)
                }
            };

            let mut response = match campaign {
                Ok(response) => response,
                Err(error) => {
                    if cancel.is_cancelled() {
                        break;
                    }
                    tracing::warn!(?error, "campaign owner failed");
                    if session_lost.is_cancelled() && self.start_session(ttl).await.is_err() {
                        break;
                    }
                    tokio::time::sleep(KEY_OP_RETRY_INTERVAL).await;
                    continue;
                }
            };
            let Some(leader) = response.take_leader() else {
                tracing::warn!("campaign succeeded without leader key");
                continue;
            };
            let key = leader.key().to_vec();
            let revision = leader.rev();
            self.become_owner(leader).await;
            // Watch 本节点 Owner key；删除或取消后退位并重新竞选。
            let watch_result = self
                .watch_owner(&cancel, &session_lost, key, revision)
                .await;
            self.retire_if_owner().await;

            if cancel.is_cancelled() {
                break;
            }
            if session_lost.is_cancelled() && self.start_session(ttl).await.is_err() {
                break;
            }
            if let Err(error) = watch_result {
                tracing::warn!(?error, "watch owner failed");
                tokio::time::sleep(KEY_OP_RETRY_INTERVAL).await;
            }
        }
        self.retire_if_owner().await;
    }

    /// 从 revision+1 起 Watch Owner key；遇 Delete / 取消 / session 丢失则返回。
    async fn watch_owner(
        &self,
        cancel: &CancellationToken,
        session_lost: &CancellationToken,
        key: Vec<u8>,
        revision: i64,
    ) -> Result<()> {
        let key_text = String::from_utf8_lossy(&key).into_owned();
        let mut client = self.inner.client.clone();
        let mut watch = client
            .watch(
                key,
                Some(WatchOptions::new().with_start_revision(revision + 1)),
            )
            .await?;
        loop {
            let response = tokio::select! {
                _ = cancel.cancelled() => return Ok(()),
                _ = session_lost.cancelled() => return Ok(()),
                response = watch.message() => response?,
            };
            let Some(response) = response else {
                return Err(OwnerError::WatcherClosed(key_text));
            };
            if response.canceled() {
                return Err(OwnerError::WatchCancelled {
                    key: key_text,
                    reason: response.cancel_reason().to_owned(),
                });
            }
            // Owner key 被删除意味着本节点失去领导权。
            if response
                .events()
                .iter()
                .any(|event| event.event_type() == EventType::Delete)
            {
                return Ok(());
            }
        }
    }

    /// ForceToBeOwner 单次尝试：删掉前缀下其他 key，再 campaign。
    async fn try_to_be_owner_once(&self) -> Result<()> {
        let lease_id = self.inner.session_lease.load(Ordering::Acquire);
        let key_prefix = format!("{}/", self.inner.key.trim_end_matches('/'));
        let campaign_key = format!("{key_prefix}{lease_id:x}");
        let mut client = self.inner.client.clone();
        let response = client
            .get(key_prefix.clone(), Some(GetOptions::new().with_prefix()))
            .await?;
        // 事务：删除非本 lease 的竞选 key，再 put 本节点 key。
        let mut operations = Vec::with_capacity(response.kvs().len() + 1);
        for kv in response.kvs() {
            if kv.key() != campaign_key.as_bytes() {
                operations.push(TxnOp::delete(kv.key().to_vec(), None));
            }
        }
        operations.push(TxnOp::put(
            campaign_key,
            self.inner.id.clone(),
            Some(PutOptions::new().with_lease(lease_id)),
        ));
        client.txn(Txn::new().and_then(operations)).await?;
        let campaign = tokio::time::timeout(
            KEY_OP_DEFAULT_TIMEOUT,
            client.campaign(self.inner.key.clone(), self.inner.id.clone(), lease_id),
        )
        .await
        .map_err(|_| OwnerError::Cancelled)??;
        if campaign.leader().is_none() {
            return Err(OwnerError::NoLeader);
        }
        Ok(())
    }
}

#[async_trait]
impl Manager for OwnerManager {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn ID(&self) -> String {
        self.inner.id.clone()
    }

    fn IsOwner(&self) -> bool {
        // try_read 失败时保守视为非 Owner，避免阻塞。
        self.inner
            .leader
            .try_read()
            .map(|leader| leader.is_some())
            .unwrap_or(false)
    }

    async fn RetireOwner(&self) {
        *self.inner.leader.write().await = None;
        if let Some(listener) = self.inner.listener.read().await.clone() {
            listener.OnRetireOwner();
        }
    }

    async fn GetOwnerID(&self, ctx: &Context) -> Result<String> {
        let info = get_owner_info(ctx, self.inner.client.clone(), &self.inner.key).await?;
        Ok(String::from_utf8_lossy(&info.owner_id).into_owned())
    }

    async fn SetOwnerOpValue(&self, ctx: &Context, op: OpType) -> Result<()> {
        let info = get_owner_info(ctx, self.inner.client.clone(), &self.inner.key).await?;
        if info.op == op {
            return Ok(());
        }
        if info.owner_id != self.inner.id.as_bytes() {
            return Err(OwnerError::OwnerInfoNotMatch(self.inner.id.clone()));
        }
        // 用 mod_revision 做 CAS，避免并发覆盖其他节点的更新。
        let value = join_owner_values(&[&info.owner_id, &[op as u8]]);
        let transaction = Txn::new()
            .when([Compare::mod_revision(
                info.owner_key.clone(),
                CompareOp::Equal,
                info.mod_revision,
            )])
            .and_then([TxnOp::put(
                info.owner_key,
                value,
                Some(
                    PutOptions::new().with_lease(self.inner.session_lease.load(Ordering::Acquire)),
                ),
            )]);
        let mut client = self.inner.client.clone();
        let response = run_with_context(ctx, client.txn(transaction)).await??;
        if response.succeeded() {
            Ok(())
        } else {
            Err(OwnerError::CompareFailed)
        }
    }

    async fn CampaignOwner(&self, with_ttl: &[i32]) -> Result<()> {
        self.check_open()?;
        let ttl = with_ttl.first().copied().unwrap_or_else(ManagerSessionTTL);
        self.ensure_session(ttl).await?;
        let mut campaign = self.inner.campaign.lock().await;
        // 已有未结束的竞选任务则直接返回，避免重复 spawn。
        if campaign
            .as_ref()
            .is_some_and(|runtime| !runtime.task.is_finished())
        {
            return Ok(());
        }
        let cancel = self.inner.root_context.child_token();
        let manager = self.clone();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move { manager.campaign_loop(task_cancel, ttl).await });
        *campaign = Some(CampaignRuntime { cancel, task });
        Ok(())
    }

    async fn CampaignCancel(&self) {
        self.BreakCampaignLoop().await;
        self.stop_session().await;
    }

    async fn BreakCampaignLoop(&self) {
        let campaign = self.inner.campaign.lock().await.take();
        if let Some(campaign) = campaign {
            campaign.cancel.cancel();
            if let Err(error) = campaign.task.await {
                tracing::warn!(?error, "owner campaign task failed while stopping");
            }
        }
    }

    async fn ResignOwner(&self, ctx: &Context) -> Result<()> {
        let leader = self
            .inner
            .leader
            .read()
            .await
            .clone()
            .ok_or(OwnerError::NotOwner)?;
        let mut client = self.inner.client.clone();
        run_with_context(
            ctx,
            tokio::time::timeout(
                KEY_OP_DEFAULT_TIMEOUT,
                client.resign(Some(ResignOptions::new().with_leader(leader))),
            ),
        )
        .await?
        .map_err(|_| OwnerError::Cancelled)??;
        Ok(())
    }

    async fn Close(&self) {
        // swap 保证只执行一次 CampaignCancel。
        if !self.inner.closed.swap(true, Ordering::AcqRel) {
            self.CampaignCancel().await;
        }
    }

    async fn SetListener(&self, listener: Arc<dyn Listener>) {
        *self.inner.listener.write().await = Some(listener);
    }

    async fn ForceToBeOwner(&self, _ctx: &Context) -> Result<()> {
        self.check_open()?;
        self.start_session(ManagerSessionTTL()).await?;
        // 最多尝试三次，兼容旧版本残留 Owner key。
        for _ in 0..3 {
            tokio::time::sleep(WaitTimeOnForceOwner()).await;
            match self.try_to_be_owner_once().await {
                Ok(()) => break,
                Err(error) => tracing::warn!(?error, "failed to retire owner on older version"),
            }
        }
        Ok(())
    }
}

impl DDLOwnerChecker for OwnerManager {
    fn IsOwner(&self) -> bool {
        Manager::IsOwner(self)
    }
}

/// 在 Context 取消与 future 完成之间竞争；取消则返回 Cancelled。
async fn run_with_context<F, T>(ctx: &Context, future: F) -> Result<T>
where
    F: std::future::Future<Output = T>,
{
    tokio::select! {
        _ = ctx.cancelled() => Err(OwnerError::Cancelled),
        value = future => Ok(value),
    }
}

/// 从 etcd 读到的当前 Owner key 元信息。
struct OwnerInfo {
    owner_key: Vec<u8>,
    owner_id: Vec<u8>,
    op: OpType,
    current_revision: i64,
    mod_revision: i64,
}

/// 按创建时间取前缀下首个 key，解析 Owner ID 与 OpType；带有限重试。
async fn get_owner_info(ctx: &Context, mut client: Client, owner_path: &str) -> Result<OwnerInfo> {
    let options = GetOptions::new()
        .with_prefix()
        .with_sort(SortTarget::Create, SortOrder::Ascend)
        .with_limit(1);
    let mut last_error = None;
    for _ in 0..3 {
        if ctx.is_cancelled() {
            return Err(OwnerError::Cancelled);
        }
        match run_with_context(
            ctx,
            tokio::time::timeout(
                KEY_OP_DEFAULT_TIMEOUT,
                client.get(owner_path, Some(options.clone())),
            ),
        )
        .await?
        {
            Ok(Ok(response)) => {
                let Some(kv) = response.kvs().first() else {
                    return Err(OwnerError::NoLeader);
                };
                let (owner_id, op) = split_owner_values(kv.value());
                return Ok(OwnerInfo {
                    owner_key: kv.key().to_vec(),
                    owner_id,
                    op,
                    current_revision: response
                        .header()
                        .map(|header| header.revision())
                        .unwrap_or(0),
                    mod_revision: kv.mod_revision(),
                });
            }
            Ok(Err(error)) => last_error = Some(error),
            Err(_) => last_error = None,
        }
        tokio::time::sleep(KEY_OP_RETRY_INTERVAL).await;
    }
    match last_error {
        Some(error) => Err(error.into()),
        None => Err(OwnerError::Cancelled),
    }
}

/// Go-compatible owner value decoder. Only exactly two underscore-separated
/// fields carry an operation byte; additional fields leave the operation unset.
/// 与 Go 兼容的 Owner value 解码：仅恰好两段 `_` 分隔时解析操作字节。
pub fn split_owner_values(value: &[u8]) -> (Vec<u8>, OpType) {
    let parts: Vec<&[u8]> = value.split(|byte| *byte == b'_').collect();
    let op = if parts.len() == 2 {
        parts[1]
            .first()
            .copied()
            .map(OpType::from)
            .unwrap_or_default()
    } else {
        OpType::OpNone
    };
    (parts.first().copied().unwrap_or_default().to_vec(), op)
}

/// 用 `_` 拼接多段字节，生成 Owner value。
pub fn join_owner_values(values: &[&[u8]]) -> Vec<u8> {
    values.join(&b'_')
}

/// 按节点 ID 删除前缀下匹配的 Owner key（找不到则静默返回）。
pub async fn DeleteOwnerKeyByID(ctx: &Context, mut client: Client, owner_path: &str, id: &str) {
    let prefix = format!("{}/", owner_path.trim_end_matches('/'));
    let response = run_with_context(
        ctx,
        client.get(prefix, Some(GetOptions::new().with_prefix())),
    )
    .await;
    let Ok(Ok(response)) = response else {
        return;
    };
    for kv in response.kvs() {
        if split_owner_values(kv.value()).0 == id.as_bytes() {
            let _ = run_with_context(ctx, client.delete(kv.key(), None)).await;
            return;
        }
    }
}

/// 校验当前 Owner 为指定 id，返回其 etcd key 与集群 revision。
pub async fn GetOwnerKeyInfo(
    ctx: &Context,
    client: Client,
    owner_path: &str,
    id: &str,
) -> Result<(String, i64)> {
    let info = get_owner_info(ctx, client, owner_path).await?;
    if info.owner_id != id.as_bytes() {
        return Err(OwnerError::OwnerInfoNotMatch(id.to_owned()));
    }
    Ok((
        String::from_utf8_lossy(&info.owner_key).into_owned(),
        info.current_revision,
    ))
}

/// 读取 Owner 操作字节；无 etcd client 时走 Mock 路径。
pub async fn GetOwnerOpValue(
    ctx: &Context,
    client: Option<Client>,
    owner_path: &str,
) -> Result<OpType> {
    match client {
        Some(client) => Ok(get_owner_info(ctx, client, owner_path).await?.op),
        None => Ok(mock_owner_op_value(owner_path)),
    }
}

/// 测试辅助：对真实 OwnerManager 调用 watch_owner。
pub async fn WatchOwnerForTest(
    ctx: &Context,
    manager: &dyn Manager,
    key: impl Into<Vec<u8>>,
    current_revision: i64,
) -> Result<()> {
    let Some(manager) = manager.as_any().downcast_ref::<OwnerManager>() else {
        return Ok(());
    };
    let (_, session_lost) = manager.session_snapshot().await?;
    manager
        .watch_owner(ctx, &session_lost, key.into(), current_revision)
        .await
}

/// Held distributed lock. Call `release` to unlock and revoke its lease.
/// 持有中的分布式锁；调用 `release` 解锁并 revoke lease。
pub struct DistributedLock {
    client: Client,
    key: Vec<u8>,
    lease_id: i64,
    keep_alive_cancel: CancellationToken,
    keep_alive_task: JoinHandle<()>,
}

/// 执行可重试的锁操作；第 n 次失败后等待 `base_delay * n`。
pub(crate) async fn retry_lock_operation<T, E, F, Fut>(
    retry_count: usize,
    base_delay: Duration,
    mut operation: F,
) -> std::result::Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, E>>,
{
    assert!(retry_count > 0, "lock retry count must be positive");
    for attempt in 1..=retry_count {
        match operation().await {
            Ok(value) => return Ok(value),
            Err(error) if attempt == retry_count => return Err(error),
            Err(_) => tokio::time::sleep(base_delay * attempt as u32).await,
        }
    }
    unreachable!("positive retry count always returns from the loop")
}

impl DistributedLock {
    /// 解锁、停止保活并 revoke lease。
    pub async fn release(mut self) -> Result<()> {
        let unlock = self.client.unlock(self.key.clone()).await;
        self.keep_alive_cancel.cancel();
        let _ = (&mut self.keep_alive_task).await;
        let revoke = self.client.lease_revoke(self.lease_id).await;
        unlock?;
        revoke?;
        Ok(())
    }
}

impl Drop for DistributedLock {
    fn drop(&mut self) {
        // Dropping a Tokio JoinHandle detaches its task. Explicitly cancel and
        // abort here so an abandoned Go-style client cannot keep the lease alive.
        // 丢弃时取消/abort 保活，避免遗弃的 Go 风格 client 继续保活。
        self.keep_alive_cancel.cancel();
        self.keep_alive_task.abort();
    }
}

/// 申请带 TTL 的 etcd 分布式锁；失败时清理已创建的 lease。
pub async fn AcquireDistributedLock(
    ctx: &Context,
    mut client: Client,
    key: impl Into<Vec<u8>>,
    ttl_in_seconds: i32,
) -> Result<DistributedLock> {
    let grant = client.lease_grant(ttl_in_seconds as i64, None).await?;
    let lease_id = grant.id();
    let (mut keeper, mut stream) = client.lease_keep_alive(lease_id).await?;
    let cancel = CancellationToken::new();
    let cancelled = cancel.clone();
    let keep_alive_task = tokio::spawn(async move {
        let interval = Duration::from_secs((ttl_in_seconds.max(1) as u64 / 3).max(1));
        loop {
            tokio::select! {
                _ = cancelled.cancelled() => break,
                _ = tokio::time::sleep(interval) => {
                    if keeper.keep_alive().await.is_err() || stream.message().await.is_err() {
                        break;
                    }
                }
            }
        }
    });
    let lock_key = key.into();
    let lock_result = retry_lock_operation(
        DISTRIBUTED_LOCK_MAX_RETRY_COUNT,
        DISTRIBUTED_LOCK_RETRY_INTERVAL,
        || {
            let mut lock_client = client.clone();
            let lock_key = lock_key.clone();
            async move {
                run_with_context(
                    ctx,
                    lock_client.lock(
                        lock_key.clone(),
                        Some(LockOptions::new().with_lease(lease_id)),
                    ),
                )
                .await?
                .map_err(OwnerError::from)
            }
        },
    )
    .await;
    match lock_result {
        Ok(response) => Ok(DistributedLock {
            client,
            key: response.key().to_vec(),
            lease_id,
            keep_alive_cancel: cancel,
            keep_alive_task,
        }),
        Err(error) => {
            // lock 失败时取消保活并 revoke，避免泄漏 lease。
            cancel.cancel();
            let _ = keep_alive_task.await;
            let _ = client.lease_revoke(lease_id).await;
            Err(error)
        }
    }
}

/// 将多个 Listener 按输入顺序广播事件。
pub struct ListenersWrapper {
    listeners: Vec<Arc<dyn Listener>>,
}

impl Listener for ListenersWrapper {
    fn OnBecomeOwner(&self) {
        for listener in &self.listeners {
            listener.OnBecomeOwner();
        }
    }

    fn OnRetireOwner(&self) {
        for listener in &self.listeners {
            listener.OnRetireOwner();
        }
    }
}

/// 构造按序广播的 Listener 包装器。
pub fn NewListenersWrapper(listeners: Vec<Arc<dyn Listener>>) -> ListenersWrapper {
    ListenersWrapper { listeners }
}

/// 前缀删除选项构造（当前未在热路径使用，保留对齐 Go）。
#[allow(dead_code)]
fn delete_options_for_prefix() -> DeleteOptions {
    DeleteOptions::new().with_prefix()
}
