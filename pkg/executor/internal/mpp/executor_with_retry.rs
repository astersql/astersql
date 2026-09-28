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

// 带恢复重试的 MPP 响应包装（ExecutorWithRetry）。
//
// 将底层 `MppCoordinator` 包装为 `kv::Response`：先按容量缓冲结果，
// 遇可恢复错误时关闭旧 gather、分配新 gather_id、经 `CoordinatorFactory`
// 重建协调器并重新 Execute；同时通过 `CoordinatorRegistry` 供 gRPC
// ReportStatus 路由到同一共享实例。

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use astersql_errors as errors;
use astersql_kv as kv;
use astersql_util_memory::tracker::{NewTracker, Tracker};

use crate::recovery_handler::{NewRecoveryHandler, RecoveryHandler, RecoveryInfo};

/// 协调器在注册表中的唯一键：查询 ID + gather ID（一次 gather 一轮派发）。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CoordinatorUniqueId {
    /// MPP 查询标识（QueryTs / LocalQueryID / ServerID）。
    pub query_id: kv::MPPQueryID,
    /// 当前 gather 序号；恢复重建时递增。
    pub gather_id: u64,
}

/// 共享的 MPP 协调器实例（互斥保护）。
pub type SharedMppCoordinator = Arc<Mutex<Box<dyn kv::MppCoordinator>>>;
/// 共享的状态上报器，供 gRPC ReportStatus 回调。
pub type SharedMppStatusReporter = Arc<dyn kv::MppStatusReporter>;

/// gRPC report 路由使用的管理边界；注册的共享协调器与本包装器消费的是同一实例。
///
/// Manager boundary used by the gRPC report router. The shared coordinator is
/// the same owned instance consumed by this response wrapper.
pub trait CoordinatorRegistry: Send + Sync {
    fn Register(
        &self,
        id: CoordinatorUniqueId,
        coordinator: SharedMppCoordinator,
        reporter: SharedMppStatusReporter,
    ) -> Result<(), errors::SharedError>;
    fn Unregister(&self, id: CoordinatorUniqueId);
}

/// 进程内协调器注册表：按 UniqueId 保存协调器与 StatusReporter。
#[derive(Default)]
pub struct MppCoordinatorManager {
    coordinators: Mutex<HashMap<CoordinatorUniqueId, RegisteredCoordinator>>,
}

/// 注册表条目：协调器本体 + 对外状态上报器。
struct RegisteredCoordinator {
    coordinator: SharedMppCoordinator,
    reporter: SharedMppStatusReporter,
}

impl MppCoordinatorManager {
    /// 将 ReportStatus 请求转发给已注册的 StatusReporter。
    pub fn ReportStatus(
        &self,
        id: CoordinatorUniqueId,
        request: kv::ReportStatusRequest,
    ) -> Result<(), errors::SharedError> {
        let reporter = self
            .coordinators
            .lock()
            .map_err(|_| errors::New("MPP coordinator registry lock is poisoned"))?
            .get(&id)
            .map(|registered| registered.reporter.clone())
            .ok_or_else(|| errors::New("MppCoordinator not exists"))?;
        reporter.ReportStatus(request)
    }

    /// 当前已注册的协调器数量。
    pub fn Len(&self) -> usize {
        self.coordinators
            .lock()
            .map(|coordinators| coordinators.len())
            .unwrap_or_default()
    }
}

impl CoordinatorRegistry for MppCoordinatorManager {
    fn Register(
        &self,
        id: CoordinatorUniqueId,
        coordinator: SharedMppCoordinator,
        reporter: SharedMppStatusReporter,
    ) -> Result<(), errors::SharedError> {
        let mut coordinators = self
            .coordinators
            .lock()
            .map_err(|_| errors::New("MPP coordinator registry lock is poisoned"))?;
        if coordinators.contains_key(&id) {
            return Err(errors::New(format!(
                "Mpp coordinator already registered: {} {} {} {}",
                id.query_id.QueryTs, id.query_id.LocalQueryID, id.query_id.ServerID, id.gather_id
            )));
        }
        coordinators.insert(
            id,
            RegisteredCoordinator {
                coordinator,
                reporter,
            },
        );
        Ok(())
    }

    fn Unregister(&self, id: CoordinatorUniqueId) {
        if let Ok(mut coordinators) = self.coordinators.lock() {
            coordinators.remove(&id);
        }
    }
}

/// 恢复后用新的 gather ID 重建已调度的协调器。
///
/// Rebuilds the scheduled coordinator with a fresh gather ID after recovery.
pub trait CoordinatorFactory: Send + Sync {
    /// 按 gather_id 构造新的 MppCoordinator。
    fn Build(&self, gather_id: u64) -> Result<Box<dyn kv::MppCoordinator>, errors::SharedError>;
}

/// MPP 恢复开关与结果缓冲容量配置。
#[derive(Clone, Copy, Debug)]
pub struct MppRecoveryConfig {
    /// 是否走 TiFlash auto-scaler 恢复路径。
    pub use_auto_scaler: bool,
    /// 是否启用恢复重试。
    pub enabled: bool,
    /// RecoveryHandler 可缓冲的响应条数上限。
    pub holder_capacity: u64,
}

impl Default for MppRecoveryConfig {
    fn default() -> Self {
        Self {
            use_auto_scaler: false,
            enabled: false,
            holder_capacity: 2,
        }
    }
}

/// 可恢复重试的 MPP 响应执行器：缓冲结果，失败时重建 gather 并继续 Next。
pub struct ExecutorWithRetry<'parent> {
    coordinator: Option<SharedMppCoordinator>,
    factory: Arc<dyn CoordinatorFactory>,
    registry: Arc<dyn CoordinatorRegistry>,
    gather_allocator: Arc<AtomicU64>,
    context: kv::Context,
    mem_tracker: Box<Tracker>,
    recovery: RecoveryHandler,
    /// 本次 Execute 返回的 KV 扫描范围。
    pub KVRanges: Vec<kv::KeyRange>,
    query_id: kv::MPPQueryID,
    gather_id: u64,
    node_count: i32,
    closed: bool,
    parent_lifetime: PhantomData<&'parent mut Tracker>,
}

/// 构造 ExecutorWithRetry：挂接内存 Tracker、创建 RecoveryHandler，并首次 setup 协调器。
#[allow(clippy::too_many_arguments)]
pub fn NewExecutorWithRetry<'parent>(
    context: kv::Context,
    parent_tracker: &'parent mut Tracker,
    query_id: kv::MPPQueryID,
    gather_allocator: Arc<AtomicU64>,
    factory: Arc<dyn CoordinatorFactory>,
    registry: Arc<dyn CoordinatorRegistry>,
    recovery_config: MppRecoveryConfig,
) -> Result<ExecutorWithRetry<'parent>, errors::SharedError> {
    let recovery = NewRecoveryHandler(
        recovery_config.use_auto_scaler,
        recovery_config.holder_capacity,
        recovery_config.enabled,
        parent_tracker,
    );
    let mut mem_tracker = NewTracker(parent_tracker.Label(), 0);
    mem_tracker.AttachTo(parent_tracker as *mut Tracker);
    let mut executor = ExecutorWithRetry {
        coordinator: None,
        factory,
        registry,
        gather_allocator,
        context,
        mem_tracker,
        recovery,
        KVRanges: Vec::new(),
        query_id,
        gather_id: 0,
        node_count: 0,
        closed: false,
        parent_lifetime: PhantomData,
    };
    executor.KVRanges = executor.setupMPPCoordinator(false)?;
    Ok(executor)
}

impl ExecutorWithRetry<'_> {
    /// 当前查询 + gather 组成的注册键。
    fn unique_id(&self) -> CoordinatorUniqueId {
        CoordinatorUniqueId {
            query_id: self.query_id,
            gather_id: self.gather_id,
        }
    }

    /// 取得当前协调器的互斥锁守卫。
    fn coordinator(
        &self,
    ) -> Result<MutexGuard<'_, Box<dyn kv::MppCoordinator>>, errors::SharedError> {
        self.coordinator
            .as_ref()
            .ok_or_else(|| errors::New("MPP coordinator is not initialized"))?
            .lock()
            .map_err(|_| errors::New("MPP coordinator lock is poisoned"))
    }

    /// 构建（或恢复时重建）MPP 协调器：分配 gather、注册、Execute，校验 TiFlash 节点数。
    fn setupMPPCoordinator(
        &mut self,
        recovering: bool,
    ) -> Result<Vec<kv::KeyRange>, errors::SharedError> {
        // 恢复路径：关闭旧协调器并从注册表注销。
        if recovering {
            let old_id = self.unique_id();
            let old = self
                .coordinator
                .take()
                .ok_or_else(|| errors::New("MPP coordinator is missing during recovery"))?;
            let _ = old
                .lock()
                .map_err(|_| errors::New("MPP coordinator lock is poisoned"))?
                .Close();
            self.registry.Unregister(old_id);
        }

        // 分配新 gather_id，经 Factory 重建并注册。
        self.gather_id = self.gather_allocator.fetch_add(1, Ordering::SeqCst) + 1;
        let coordinator = Arc::new(Mutex::new(self.factory.Build(self.gather_id)?));
        let id = self.unique_id();
        let reporter = coordinator
            .lock()
            .map_err(|_| errors::New("MPP coordinator lock is poisoned"))?
            .StatusReporter();
        self.registry.Register(id, coordinator.clone(), reporter)?;
        self.coordinator = Some(coordinator.clone());

        let ranges = match coordinator
            .lock()
            .map_err(|_| errors::New("MPP coordinator lock is poisoned"))?
            .Execute(&self.context)
        {
            Ok(ranges) => ranges,
            Err(error) => {
                self.registry.Unregister(id);
                self.coordinator = None;
                return Err(error);
            }
        };
        self.node_count = coordinator
            .lock()
            .map_err(|_| errors::New("MPP coordinator lock is poisoned"))?
            .GetNodeCnt();
        if self.node_count <= 0 {
            self.registry.Unregister(id);
            self.coordinator = None;
            return Err(errors::New(format!(
                "tiflash node count should be greater than zero: {}",
                self.node_count
            )));
        }
        Ok(ranges)
    }

    /// 在启用恢复时预拉并缓冲响应；遇错则 Recovery + 重建协调器后清空缓冲。
    fn nextWithRecovery(&mut self, context: &kv::Context) -> Result<(), errors::SharedError> {
        if !self.recovery.Enabled() {
            return Ok(());
        }
        // 缓冲至 holder 满或流结束；错误则尝试恢复并 setup(true)。
        while self.recovery.CanHoldResult() {
            let response = self.coordinator()?.Next(context);
            match response {
                Ok(Some(response)) => self.recovery.HoldResult(response),
                Ok(None) => break,
                Err(mpp_error) => {
                    if self
                        .recovery
                        .Recovery(Some(&RecoveryInfo {
                            MPPErr: Some(mpp_error.clone()),
                            NodeCnt: self.node_count,
                        }))
                        .is_err()
                    {
                        return Err(mpp_error);
                    }
                    // Go preserves the MPP failure as the externally visible error when
                    // rebuilding the coordinator also fails: the rebuild error is only
                    // diagnostic context and must not replace the cause that triggered retry.
                    if self.setupMPPCoordinator(true).is_err() {
                        return Err(mpp_error);
                    }
                    self.recovery.ResetHolder();
                }
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn recovery_handler_mut(&mut self) -> &mut RecoveryHandler {
        &mut self.recovery
    }

    #[cfg(test)]
    pub(crate) fn gather_id(&self) -> u64 {
        self.gather_id
    }
}

impl kv::Response for ExecutorWithRetry<'_> {
    /// 先走恢复预拉；若有缓冲则 FIFO 弹出，否则直接向协调器 Next。
    fn Next(
        &mut self,
        context: &kv::Context,
    ) -> Result<Option<Box<dyn kv::ResultSubset>>, errors::SharedError> {
        self.nextWithRecovery(context)?;
        if self.recovery.NumHoldResp() != 0 {
            return self.recovery.PopFrontResp().map(Some);
        }
        self.coordinator()?.Next(context)
    }

    /// 幂等关闭：清空缓冲、Detach Tracker、关闭协调器并注销。
    fn Close(&mut self) -> Result<(), errors::SharedError> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.recovery.ResetHolder();
        self.mem_tracker.Detach();
        let id = self.unique_id();
        let result = match self.coordinator.take() {
            Some(coordinator) => match coordinator.lock() {
                Ok(mut coordinator) => coordinator.Close(),
                Err(_) => Err(errors::New("MPP coordinator lock is poisoned")),
            },
            None => Ok(()),
        };
        self.registry.Unregister(id);
        result
    }
}

impl Drop for ExecutorWithRetry<'_> {
    fn drop(&mut self) {
        let _ = kv::Response::Close(self);
    }
}
