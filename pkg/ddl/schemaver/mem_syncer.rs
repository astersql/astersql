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

// 内存版 Schema Version Syncer（模式版本同步器）。
//
// 不依赖真实 etcd，用原子变量与内存 HashMap 模拟各实例的 schema 版本
// 上报与全局版本通知，供单元测试验证 `Syncer` 协议行为。
// 开启 MDL（Metadata Lock，元数据锁）时按 job ID 记录版本；
// 关闭时只维护单一的本机 schema 版本。

use astersql_domain_serverinfo as serverinfo;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, RwLock, mpsc};
use std::time::Duration;

use crate::{
    Context, DoneSignal, IsMDLEnabled, MOCK_OWNER_SLOW_JOB, MOCK_UPDATE_MDL_ERROR, SyncError,
    SyncSummary, Syncer, WatchChan, WatchResponse,
};

/// 轮询检查各节点版本是否追上最新值的间隔。
pub const checkVersionsInterval: Duration = Duration::from_millis(2);

/// 内存同步器：用本地状态模拟 etcd 路径上的版本发布与等待。
pub struct MemSyncer {
    /// 非 MDL 模式下本机当前 schema 版本。
    selfSchemaVersion: AtomicI64,
    /// MDL 模式下按 DDL job ID 记录的 schema 版本。
    mdlSchemaVersions: Mutex<HashMap<i64, i64>>,
    /// 向全局版本观察通道发送通知的发送端。
    globalVerSender: RwLock<mpsc::SyncSender<WatchResponse>>,
    /// 保持 Init 替换掉的旧通道处于打开状态，与 Go 的未关闭 channel 一致。
    retiredGlobalVerSenders: Mutex<Vec<mpsc::SyncSender<WatchResponse>>>,
    /// 全局 schema 版本变更的观察通道。
    globalVerCh: RwLock<WatchChan>,
    /// 模拟 etcd session 生命周期的完成信号。
    mockSession: RwLock<DoneSignal>,
}

/// 创建容量为 1 的同步通道，并包装为 `WatchChan`。
fn new_watch_channel() -> (mpsc::SyncSender<WatchResponse>, WatchChan) {
    let (sender, receiver) = mpsc::sync_channel(1);
    (sender, WatchChan::New(receiver))
}

/// 构造默认状态的内存同步器（引用计数包装）。
pub fn NewMemSyncer() -> Arc<MemSyncer> {
    Arc::new(MemSyncer::default())
}

impl Default for MemSyncer {
    fn default() -> Self {
        let (sender, channel) = new_watch_channel();
        Self {
            selfSchemaVersion: AtomicI64::new(0),
            mdlSchemaVersions: Mutex::new(HashMap::new()),
            globalVerSender: RwLock::new(sender),
            retiredGlobalVerSenders: Mutex::new(Vec::new()),
            globalVerCh: RwLock::new(channel),
            mockSession: RwLock::new(DoneSignal::New()),
        }
    }
}

impl MemSyncer {
    /// 清空 MDL 版本表并重建观察通道与 session 信号。
    ///
    /// 与 Go 一致，非 MDL 模式的 `selfSchemaVersion` 在重新初始化时保持不变。
    pub fn Init(&self, _context: Context) -> Result<(), SyncError> {
        self.mdlSchemaVersions
            .lock()
            .expect("MDL versions lock poisoned")
            .clear();
        let (sender, channel) = new_watch_channel();
        let mut global_ver_sender = self
            .globalVerSender
            .write()
            .expect("global version sender lock poisoned");
        self.retiredGlobalVerSenders
            .lock()
            .expect("retired global version senders lock poisoned")
            .push(global_ver_sender.clone());
        *global_ver_sender = sender;
        drop(global_ver_sender);
        *self
            .globalVerCh
            .write()
            .expect("global version channel lock poisoned") = channel;
        *self
            .mockSession
            .write()
            .expect("mock session lock poisoned") = DoneSignal::New();
        Ok(())
    }

    /// 返回全局 schema 版本观察通道的克隆。
    pub fn GlobalVersionCh(&self) -> WatchChan {
        self.globalVerCh
            .read()
            .expect("global version channel lock poisoned")
            .clone()
    }

    /// 内存实现无需真正 watch etcd，此方法为空操作。
    pub fn WatchGlobalSchemaVer(&self, _context: Context) {}

    /// 更新本机（或指定 job）的 schema 版本；可注入 mock 错误。
    pub fn UpdateSelfVersion(
        &self,
        _context: Context,
        jobID: i64,
        version: i64,
    ) -> Result<(), SyncError> {
        if MOCK_UPDATE_MDL_ERROR.load(Ordering::Acquire) {
            return Err(SyncError("mock update mdl to etcd error".into()));
        }
        // MDL 开启时按 job ID 写入；否则覆盖单一本机版本。
        if IsMDLEnabled() {
            self.mdlSchemaVersions
                .lock()
                .expect("MDL versions lock poisoned")
                .insert(jobID, version);
        } else {
            self.selfSchemaVersion.store(version, Ordering::Release);
        }
        Ok(())
    }

    /// 返回当前 mock session 的完成信号。
    pub fn Done(&self) -> DoneSignal {
        self.mockSession
            .read()
            .expect("mock session lock poisoned")
            .clone()
    }

    /// 关闭 mock session，使 `Done()` 变为已完成。
    pub fn CloseSession(&self) {
        self.Done().Close();
    }

    /// 重建一个未完成的 mock session，模拟 etcd session 重启。
    pub fn Restart(&self, _context: Context) -> Result<(), SyncError> {
        *self
            .mockSession
            .write()
            .expect("mock session lock poisoned") = DoneSignal::New();
        Ok(())
    }

    /// 模拟 owner 发布全局版本：向观察通道投递一条空响应。
    pub fn OwnerUpdateGlobalVersion(
        &self,
        _context: Context,
        _version: i64,
    ) -> Result<(), SyncError> {
        let _ = self
            .globalVerSender
            .read()
            .expect("global version sender lock poisoned")
            .try_send(WatchResponse::default());
        Ok(())
    }

    /// 轮询等待本机（或指定 job）版本追上 `latestVer`。
    pub fn WaitVersionSynced(
        &self,
        context: Context,
        jobID: i64,
        latestVer: i64,
        _checkAssumedSvr: bool,
    ) -> Result<SyncSummary, SyncError> {
        // 测试注入：对指定 job 人为延迟，模拟 owner 检查缓慢。
        if MOCK_OWNER_SLOW_JOB.load(Ordering::Acquire) == jobID {
            std::thread::sleep(Duration::from_secs(2));
        }
        loop {
            if !context.Wait(checkVersionsInterval) {
                return Err(context.Err().unwrap());
            }
            let synced = if IsMDLEnabled() {
                self.mdlSchemaVersions
                    .lock()
                    .expect("MDL versions lock poisoned")
                    .get(&jobID)
                    .is_some_and(|version| *version >= latestVer)
            } else {
                self.selfSchemaVersion.load(Ordering::Acquire) >= latestVer
            };
            if synced {
                return Ok(SyncSummary {
                    ServerCount: 1,
                    AssumedServerCount: 0,
                });
            }
        }
    }

    /// 内存实现无后台 job 版本同步循环。
    pub fn SyncJobSchemaVerLoop(&self, _context: Context) {}
    /// 内存实现不依赖 serverinfo 同步器。
    pub fn SetServerInfoSyncer(&self, _syncer: Option<Arc<serverinfo::Syncer>>) {}
    /// 关闭同步器（内存实现为空操作）。
    pub fn Close(&self) {}
}

/// 将 `MemSyncer` 的方法转发为 `Syncer` trait 实现。
impl Syncer for MemSyncer {
    fn Init(&self, c: Context) -> Result<(), SyncError> {
        MemSyncer::Init(self, c)
    }
    fn UpdateSelfVersion(&self, c: Context, j: i64, v: i64) -> Result<(), SyncError> {
        MemSyncer::UpdateSelfVersion(self, c, j, v)
    }
    fn OwnerUpdateGlobalVersion(&self, c: Context, v: i64) -> Result<(), SyncError> {
        MemSyncer::OwnerUpdateGlobalVersion(self, c, v)
    }
    fn GlobalVersionCh(&self) -> WatchChan {
        MemSyncer::GlobalVersionCh(self)
    }
    fn WatchGlobalSchemaVer(&self, c: Context) {
        MemSyncer::WatchGlobalSchemaVer(self, c)
    }
    fn Done(&self) -> DoneSignal {
        MemSyncer::Done(self)
    }
    fn Restart(&self, c: Context) -> Result<(), SyncError> {
        MemSyncer::Restart(self, c)
    }
    fn WaitVersionSynced(
        &self,
        c: Context,
        j: i64,
        v: i64,
        a: bool,
    ) -> Result<SyncSummary, SyncError> {
        MemSyncer::WaitVersionSynced(self, c, j, v, a)
    }
    fn SyncJobSchemaVerLoop(&self, c: Context) {
        MemSyncer::SyncJobSchemaVerLoop(self, c)
    }
    fn SetServerInfoSyncer(&self, s: Option<Arc<serverinfo::Syncer>>) {
        MemSyncer::SetServerInfoSyncer(self, s)
    }
    fn Close(&self) {
        MemSyncer::Close(self)
    }
}
