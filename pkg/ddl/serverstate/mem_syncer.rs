// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 内存版集群全局状态同步器（MemSyncer）。
//
// 实现 `Syncer` trait，在进程内用共享变量与 channel 模拟
// etcd 上的 server global state（服务器全局状态）同步，供单测与本地场景使用。
// 全局状态用于协调集群是否处于升级（Upgrading）等运行态。

use crate::syncer::{
    STATE_NORMAL_RUNNING, STATE_UPGRADING, StateInfo, SyncContext, SyncError, Syncer, WatchChannel,
    WatchResponse, Watcher,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, mpsc};

/// 进程内共享的集群全局状态（惰性初始化）。
static CLUSTER_STATE: OnceLock<RwLock<StateInfo>> = OnceLock::new();
/// 测试开关：为 true 时更新状态跳过 watch 通知，仅写本地缓存。
static MOCK_UPGRADING_STATE: AtomicBool = AtomicBool::new(false);

/// 取得（并必要时初始化）集群全局状态的读写锁。
fn cluster_state() -> &'static RwLock<StateInfo> {
    CLUSTER_STATE.get_or_init(|| RwLock::new(StateInfo::new(STATE_NORMAL_RUNNING)))
}

/// 设置是否启用“模拟升级态”行为（跳过 watch 推送）。
pub fn set_mock_upgrading_state(enabled: bool) {
    MOCK_UPGRADING_STATE.store(enabled, Ordering::Release);
}

/// 基于内存 channel 的 `Syncer` 实现。
pub struct MemSyncer {
    /// 向全局 watch 通道发送事件的发送端（`init` 后才有值）。
    global_sender: Mutex<Option<mpsc::SyncSender<WatchResponse>>>,
    /// 全局状态变更的观察者（Watcher）。
    global_watcher: Watcher,
    /// 预留的 mock session 通道，当前主要用于占位初始化。
    mock_session: Mutex<Option<(mpsc::SyncSender<()>, mpsc::Receiver<()>)>>,
}

impl Default for MemSyncer {
    fn default() -> Self {
        Self {
            global_sender: Mutex::new(None),
            global_watcher: Watcher::default(),
            mock_session: Mutex::new(None),
        }
    }
}

/// 构造内存同步器并装箱为 `Arc<dyn Syncer>`。
pub fn new_mem_syncer() -> Arc<dyn Syncer> {
    Arc::new(MemSyncer::default())
}

impl Syncer for MemSyncer {
    /// 初始化 watch 通道与集群状态缓存。
    fn init(&self, _ctx: &SyncContext) -> Result<(), SyncError> {
        let (global_sender, global_receiver) = mpsc::sync_channel(1);
        let (session_sender, session_receiver) = mpsc::sync_channel(1);
        *self.global_sender.lock().unwrap() = Some(global_sender);
        *self.mock_session.lock().unwrap() = Some((session_sender, session_receiver));
        self.global_watcher.replace(global_receiver);
        let _ = cluster_state();
        Ok(())
    }

    /// 更新全局状态；非 mock 模式下先向 watch 通道发事件，再写入共享状态。
    fn update_global_state(
        &self,
        _ctx: &SyncContext,
        state_info: StateInfo,
    ) -> Result<(), SyncError> {
        // 测试用 mock：只更新本地状态，不触发 watch 事件。
        if MOCK_UPGRADING_STATE.load(Ordering::Acquire) {
            *cluster_state().write().unwrap() = state_info;
            return Ok(());
        }
        self.global_sender
            .lock()
            .unwrap()
            .as_ref()
            .ok_or(SyncError::NotInitialized)?
            .send(WatchResponse::default())
            .map_err(|_| SyncError::WatchClosed)?;
        *cluster_state().write().unwrap() = state_info;
        Ok(())
    }

    /// 读取当前缓存的全局状态。
    fn get_global_state(&self, _ctx: &SyncContext) -> Result<StateInfo, SyncError> {
        Ok(cluster_state().read().unwrap().clone())
    }

    /// 判断集群是否处于升级态（`STATE_UPGRADING`）。
    fn is_upgrading_state(&self) -> bool {
        cluster_state().read().unwrap().state == STATE_UPGRADING
    }

    /// 返回可订阅全局状态变更的 watch 通道。
    fn watch_chan(&self) -> WatchChannel {
        self.global_watcher.channel()
    }

    /// 内存实现无需重新订阅远端；空操作以兼容接口。
    fn rewatch(&self, _ctx: &SyncContext) {}
}
