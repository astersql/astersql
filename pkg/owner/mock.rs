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

// 本地存储场景下的 Owner（所有者）竞选模拟实现。
//
// Owner 指集群中某类后台任务（如 DDL、统计信息）在同一时刻仅由一个 TiDB 实例持有的领导权。
// 本模块用进程内状态模拟 etcd 竞选，供单测在无真实 etcd 时验证竞选、卸任与监听回调。
use std::any::Any;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex as StdMutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{Mutex, Notify, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::manager::{Context, Listener, Manager, OpType, OwnerError, Result};
use crate::mock_owner_state::MockGlobalStateEntry;

/// 进程级模拟操作值，对应 Go `mockOwnerOpValue`。
static MOCK_OWNER_OP_VALUE: LazyLock<StdMutex<OpType>> =
    LazyLock::new(|| StdMutex::new(OpType::OpNone));

/// 读取当前模拟操作类型；owner 路径不影响 Go 的进程级共享状态。
pub(crate) fn mock_owner_op_value(_owner_path: &str) -> OpType {
    *MOCK_OWNER_OP_VALUE
        .lock()
        .expect("mock owner op mutex poisoned")
}

/// 写入进程级模拟操作类型。
fn set_mock_owner_op_value(op: OpType) {
    *MOCK_OWNER_OP_VALUE
        .lock()
        .expect("mock owner op mutex poisoned") = op;
}

/// 最小存储身份接口，对应 Go `kv.Storage.UUID()` 分支所需能力。
/// Minimal storage identity required by Go's `kv.Storage.UUID()` branch.
pub trait Storage: Send + Sync {
    fn UUID(&self) -> String;
}

/// 一次竞选后台任务的句柄：取消令牌与异步任务。
struct MockCampaign {
    cancel: CancellationToken,
    task: JoinHandle<()>,
}

/// MockManager 的内部可变状态（经 Arc 共享）。
struct MockManagerInner {
    id: String,
    store_id: String,
    key: String,
    context: Context,
    listener: RwLock<Option<Arc<dyn Listener>>>,
    campaign: Mutex<Option<MockCampaign>>,
    resign: Notify,
    closed: AtomicBool,
    epoch: AtomicU64,
}

/// 本地存储用的 Owner 管理器；同一 store 与 owner key 下各实例仍会互相竞争。
/// Local-store owner manager. Instances still compete per store and owner key.
#[derive(Clone)]
pub struct MockManager {
    inner: Arc<MockManagerInner>,
}

/// 构造模拟 Owner 管理器；`store` 用于取 UUID 区分不同存储实例。
pub fn NewMockManager(
    ctx: Context,
    id: impl Into<String>,
    store: Option<&dyn Storage>,
    owner_key: impl Into<String>,
) -> Arc<dyn Manager> {
    let key = owner_key.into();
    // Make sure the mockOwnerOpValue is initialized before GetOwnerOpValue in bootstrap.
    set_mock_owner_op_value(OpType::OpNone);
    Arc::new(MockManager {
        inner: Arc::new(MockManagerInner {
            id: id.into(),
            store_id: store
                .map(Storage::UUID)
                .unwrap_or_else(|| "mock_store_id".to_owned()),
            key,
            context: ctx,
            listener: RwLock::new(None),
            campaign: Mutex::new(None),
            resign: Notify::new(),
            closed: AtomicBool::new(false),
            epoch: AtomicU64::new(0),
        }),
    })
}

impl MockManager {
    /// 定位到本实例对应的全局模拟 Owner 条目选择器。
    fn selector(&self) -> crate::mock_owner_state::MockGlobalStateSelector<'static> {
        MockGlobalStateEntry.OwnerKey(self.inner.store_id.clone(), self.inner.key.clone())
    }

    /// 尝试将自身登记为 Owner；成功则触发 OnBecomeOwner 监听回调。
    async fn try_become_owner(&self) {
        if self.IsOwner() {
            return;
        }
        // Publish the new epoch before publishing ownership. Old work must
        // never see a reacquired owner paired with the previous tenure.
        self.inner.epoch.fetch_add(1, Ordering::AcqRel);
        if self.selector().SetOwner(self.inner.id.clone()) {
            tracing::info!(owner_key = %self.inner.key, id = %self.inner.id, "mock manager gets owner");
            if let Some(listener) = self.inner.listener.read().await.clone() {
                listener.OnBecomeOwner();
            }
        }
    }

    /// 若当前仍是 Owner 则卸任，并触发 OnRetireOwner。
    async fn retire_owner_if_current(&self) {
        if self.selector().UnsetOwner(&self.inner.id) {
            tracing::info!(owner_key = %self.inner.key, id = %self.inner.id, "mock manager retires owner");
            if let Some(listener) = self.inner.listener.read().await.clone() {
                listener.OnRetireOwner();
            }
        }
    }

    /// 竞选循环：周期性尝试成为 Owner，并响应取消、上下文结束与主动卸任通知。
    async fn campaign_loop(self, cancel: CancellationToken) {
        let mut ticker = tokio::time::interval(Duration::from_secs(1));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // CampaignOwner 在启动循环前已做首次尝试，因此丢弃
        // tokio interval 的立即首 tick，以保持与 Go 相同的约 1 秒节奏。
        // CampaignOwner performs the first attempt before spawning, so discard
        // tokio's immediate first tick and preserve Go's one-second cadence.
        ticker.tick().await;
        // 等待取消、上下文取消、Resign 通知或定时 tick。
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    self.retire_owner_if_current().await;
                    break;
                }
                _ = self.inner.context.cancelled() => {
                    self.retire_owner_if_current().await;
                    break;
                }
                _ = self.inner.resign.notified() => {
                    self.retire_owner_if_current().await;
                    tokio::select! {
                        _ = cancel.cancelled() => break,
                        _ = self.inner.context.cancelled() => break,
                        _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                    }
                }
                _ = ticker.tick() => self.try_become_owner().await,
            }
        }
    }
}

#[async_trait]
/// 实现 Manager trait：与真实 etcd Owner 管理器相同的对外契约。
impl Manager for MockManager {
    fn as_any(&self) -> &dyn Any {
        self
    }

    /// 返回本管理器实例 ID。
    fn ID(&self) -> String {
        self.inner.id.clone()
    }

    /// 判断本实例是否为当前 Owner。
    fn IsOwner(&self) -> bool {
        self.selector().IsOwner(&self.inner.id)
    }

    fn OwnerEpoch(&self) -> u64 {
        self.inner.epoch.load(Ordering::Acquire)
    }

    /// 卸任（若当前持有 Owner）。
    async fn RetireOwner(&self) {
        self.retire_owner_if_current().await;
    }

    /// 获取 Owner ID；仅当本实例是 Owner 时返回，否则 NoLeader。
    async fn GetOwnerID(&self, _ctx: &Context) -> Result<String> {
        if self.IsOwner() {
            Ok(self.ID())
        } else {
            Err(OwnerError::NoLeader)
        }
    }

    /// 设置该 owner key 上的模拟操作值。
    async fn SetOwnerOpValue(&self, _ctx: &Context, op: OpType) -> Result<()> {
        set_mock_owner_op_value(op);
        Ok(())
    }

    /// 启动竞选：若已关闭则报错；若已有未结束任务则幂等返回；否则立即尝试并启动循环。
    async fn CampaignOwner(&self, _with_ttl: &[i32]) -> Result<()> {
        if self.inner.closed.load(Ordering::Acquire) || self.inner.context.is_cancelled() {
            return Err(OwnerError::Closed);
        }
        let mut campaign = self.inner.campaign.lock().await;
        if campaign
            .as_ref()
            .is_some_and(|runtime| !runtime.task.is_finished())
        {
            return Ok(());
        }
        self.try_become_owner().await;
        let cancel = self.inner.context.child_token();
        let manager = self.clone();
        let task_cancel = cancel.clone();
        let task = tokio::spawn(async move { manager.campaign_loop(task_cancel).await });
        *campaign = Some(MockCampaign { cancel, task });
        Ok(())
    }

    /// 取消进行中的竞选任务并等待其结束。
    async fn CampaignCancel(&self) {
        let campaign = self.inner.campaign.lock().await.take();
        if let Some(campaign) = campaign {
            campaign.cancel.cancel();
            let _ = campaign.task.await;
        }
    }

    /// 打断竞选循环；Go 的 mock 故意委托给 Close，与 etcd 实现不同。
    async fn BreakCampaignLoop(&self) {
        // Go's mock intentionally delegates this to Close, unlike the etcd manager.
        self.Close().await;
    }

    /// 主动卸任并通知竞选循环，使其在短暂等待后可再次参选。
    async fn ResignOwner(&self, _ctx: &Context) -> Result<()> {
        self.retire_owner_if_current().await;
        self.inner.resign.notify_one();
        Ok(())
    }

    /// 关闭管理器：取消上下文、停止竞选并卸任（仅首次生效）。
    async fn Close(&self) {
        if !self.inner.closed.swap(true, Ordering::AcqRel) {
            self.inner.context.cancel();
            self.CampaignCancel().await;
            self.retire_owner_if_current().await;
        }
    }

    /// 注册 Owner 状态变化监听器。
    async fn SetListener(&self, listener: Arc<dyn Listener>) {
        *self.inner.listener.write().await = Some(listener);
    }

    /// 强制成为 Owner；本地 mock 为空操作，与 Go 对齐。
    async fn ForceToBeOwner(&self, _ctx: &Context) -> Result<()> {
        Ok(())
    }
}
