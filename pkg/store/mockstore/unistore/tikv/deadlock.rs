// Copyright 2026 AsterSQL.
// Copyright 2019-present PingCAP, Inc.
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

// 死锁检测的客户端/服务端薄封装。
//
// 将等待边（txn 等待 wait_for_txn）请求转发给 `Detector`，
// 并在发现死锁时通过 WaiterManager 唤醒等待方。
// Leader/Follower 角色用于模拟分布式检测器的主从切换。

use crate::detector::{DeadlockError, Detector, DiagnosticContext, WaitForEntry};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Follower 角色：非检测主节点。
pub const FOLLOWER: i32 = 0;
/// Leader 角色：负责执行死锁检测。
pub const LEADER: i32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 发给 Detector 的请求类型。
pub enum RequestType {
    /// 检测是否形成等待环（死锁）。
    Detect,
    /// 清理单条等待边。
    CleanUpWaitFor,
    /// 清理某事务的全部等待边。
    CleanUp,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 死锁检测请求：类型 + 等待条目。
pub struct DeadlockRequest {
    /// 请求操作类型。
    pub request_type: RequestType,
    /// 等待边条目（含 txn、wait_for、key 等诊断信息）。
    pub entry: WaitForEntry,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 检测到死锁时返回的响应。
pub struct DeadlockResponse {
    /// 触发检测的等待条目。
    pub entry: WaitForEntry,
    /// 闭环边上的 key hash，用于定位冲突键。
    pub deadlock_key_hash: u64,
    /// 完整等待链（从环上某点沿等待方向排列）。
    pub wait_chain: Vec<WaitForEntry>,
}

/// 持有 Detector 与角色的服务端。
pub struct DetectorServer {
    /// 等待图检测器实例。
    pub detector: Detector,
    /// 当前角色：FOLLOWER 或 LEADER。
    role: AtomicI32,
}
/// DetectorServer 的构造与请求处理。
impl DetectorServer {
    /// 创建默认 TTL/容量配置的检测服务。
    pub fn new() -> Self {
        Self {
            detector: Detector::new(Duration::from_secs(3), 100_000, Duration::from_secs(3600)),
            role: AtomicI32::new(FOLLOWER),
        }
    }
    /// 按请求类型执行 Detect / CleanUpWaitFor / CleanUp。
    /// 仅 Detect 在成环时返回 `Some(DeadlockResponse)`。
    pub fn detect(&self, request: &DeadlockRequest) -> Option<DeadlockResponse> {
        // 按类型分发；清理类请求无响应载荷。
        match request.request_type {
            RequestType::Detect => self
                .detector
                .detect(
                    request.entry.txn,
                    request.entry.wait_for_txn,
                    request.entry.key_hash,
                    DiagnosticContext {
                        key: request.entry.key.clone(),
                        resource_group_tag: request.entry.resource_group_tag.clone(),
                    },
                )
                .map(|error| convert_error(error, &request.entry)),
            RequestType::CleanUpWaitFor => {
                self.detector.clean_up_wait_for(
                    request.entry.txn,
                    request.entry.wait_for_txn,
                    request.entry.key_hash,
                );
                None
            }
            RequestType::CleanUp => {
                self.detector.clean_up(request.entry.txn);
                None
            }
        }
    }
    /// 当前是否为 Leader。
    pub fn is_leader(&self) -> bool {
        self.role.load(Ordering::Acquire) == LEADER
    }
    /// 切换 Leader/Follower 角色。
    pub fn change_role(&self, role: i32) {
        self.role.store(role, Ordering::Release);
    }
}
/// 默认等价于 `new()`。
impl Default for DetectorServer {
    fn default() -> Self {
        Self::new()
    }
}

/// 将 Detector 的 DeadlockError 转为对外响应。
fn convert_error(error: DeadlockError, request_entry: &WaitForEntry) -> DeadlockResponse {
    DeadlockResponse {
        entry: WaitForEntry {
            txn: request_entry.txn,
            wait_for_txn: request_entry.wait_for_txn,
            key_hash: request_entry.key_hash,
            key: Vec::new(),
            resource_group_tag: Vec::new(),
        },
        deadlock_key_hash: error.deadlock_key_hash,
        wait_chain: error.wait_chain,
    }
}

/// 死锁发生时唤醒相关等待者的回调接口。
pub trait DeadlockWaiterManager: Send + Sync {
    /// 根据死锁响应对等待队列执行唤醒。
    fn wake_up_for_deadlock(&self, response: DeadlockResponse);
}

/// 客户端：排队请求并同步调用服务端 detect。
pub struct DetectorClient {
    /// 共享的检测服务端。
    server: Arc<DetectorServer>,
    /// 死锁时的唤醒管理器。
    waiter_manager: Arc<dyn DeadlockWaiterManager>,
    /// 已提交的请求队列（便于测试观察）。
    pending: Mutex<Vec<DeadlockRequest>>,
}
/// DetectorClient 提交与清理 API。
impl DetectorClient {
    /// 绑定服务端与 WaiterManager。
    pub fn new(
        server: Arc<DetectorServer>,
        waiter_manager: Arc<dyn DeadlockWaiterManager>,
    ) -> Self {
        Self {
            server,
            waiter_manager,
            pending: Mutex::new(Vec::new()),
        }
    }
    /// 入队并立即检测；若死锁则回调唤醒。
    pub fn submit(&self, request: DeadlockRequest) {
        self.pending
            .lock()
            .expect("deadlock client queue poisoned")
            .push(request.clone());
        if let Some(response) = self.server.detect(&request) {
            self.waiter_manager.wake_up_for_deadlock(response);
        }
    }
    /// 清理指定 start_ts（事务开始时间戳）的全部等待边。
    pub fn clean_up(&self, start_ts: u64) {
        self.submit(request(
            RequestType::CleanUp,
            start_ts,
            0,
            0,
            Vec::new(),
            Vec::new(),
        ));
    }
    /// 清理 txn → wait_for 在 key_hash 上的单条边。
    pub fn clean_up_wait_for(&self, txn: u64, wait_for: u64, key_hash: u64) {
        self.submit(request(
            RequestType::CleanUpWaitFor,
            txn,
            wait_for,
            key_hash,
            Vec::new(),
            Vec::new(),
        ));
    }
    /// 提交一条 Detect 请求（含诊断用 key / resource_group_tag）。
    pub fn detect(
        &self,
        txn: u64,
        wait_for: u64,
        key_hash: u64,
        key: Vec<u8>,
        resource_group_tag: Vec<u8>,
    ) {
        self.submit(request(
            RequestType::Detect,
            txn,
            wait_for,
            key_hash,
            key,
            resource_group_tag,
        ));
    }
    /// 返回已提交请求数量。
    pub fn pending_count(&self) -> usize {
        self.pending
            .lock()
            .expect("deadlock client queue poisoned")
            .len()
    }
}

/// 构造带 WaitForEntry 的 DeadlockRequest。
fn request(
    request_type: RequestType,
    txn: u64,
    wait_for_txn: u64,
    key_hash: u64,
    key: Vec<u8>,
    resource_group_tag: Vec<u8>,
) -> DeadlockRequest {
    DeadlockRequest {
        request_type,
        entry: WaitForEntry {
            txn,
            wait_for_txn,
            key_hash,
            key,
            resource_group_tag,
        },
    }
}
