// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc. Licensed under Apache-2.0.

//! GC Manager 抽象，移植自 Go `br/pkg/gc/manager.go`。
//!
//! 统一 global（Nullspace）与 keyspace 两套实现的工厂与 trait；
//! 具体 PD/GCStates 调用分别在 `manager_global` / `manager_keyspace`。
//! 本文件只定义 BR 使用的 PD GC 子集类型与 `NewManager` 分派，不直接 RPC。

use std::sync::Arc;

use crate::manager_global::newGlobalManager;
use crate::manager_keyspace::newKeyspaceManager;
use crate::safepoint::{BRServiceSafePoint, Context, SharedError};

/// KeyspaceID mirrors `tikv.KeyspaceID`.
/// 键空间 ID，对齐 TiKV `KeyspaceID`。
pub type KeyspaceID = u32;

/// NullspaceID mirrors `tikv.NullspaceID` (the global / null keyspace).
/// 全局/空键空间哨兵值；传入 `NewManager` 时走 global 实现。
pub const NullspaceID: KeyspaceID = 0xffff_ffff;

/// GCState mirrors `pd/client/clients/gc.GCState` (the fields BR uses).
/// PD GC 状态快照：safepoint、事务 safepoint 与 barrier 列表（BR 使用字段子集）。
#[derive(Clone, Debug, Default)]
pub struct GCState {
    pub GCSafePoint: u64,
    pub TxnSafePoint: u64,
    pub GCBarriers: Vec<GCBarrierInfo>,
}

/// GCBarrierInfo mirrors `pd/client/clients/gc.GCBarrierInfo`.
/// 单个 GC barrier：ID、阻挡时间戳与 TTL（秒）。
#[derive(Clone, Debug)]
pub struct GCBarrierInfo {
    pub BarrierID: String,
    pub BarrierTS: u64,
    /// TTL in seconds; `i64::MAX` means never expire.
    /// TTL 单位秒；`i64::MAX` 表示永不过期。
    pub TTL: i64,
}

/// GCStatesClient mirrors `pd/client/clients/gc.GCStatesClient`, the
/// keyspace-scoped GC states API surface used by the keyspace manager.
/// 键空间作用域的 GCStates API，供 keyspace Manager 读写 barrier/状态。
pub trait GCStatesClient: Send + Sync {
    fn GetGCState(&self, ctx: &Context) -> Result<GCState, SharedError>;
    fn SetGCBarrier(
        &self,
        ctx: &Context,
        barrier_id: &str,
        barrier_ts: u64,
        ttl_seconds: i64,
    ) -> Result<GCBarrierInfo, SharedError>;
    fn DeleteGCBarrier(
        &self,
        ctx: &Context,
        barrier_id: &str,
    ) -> Result<Option<GCBarrierInfo>, SharedError>;
}

/// PdClient mirrors the subset of `pd.Client` that the GC managers use.
/// PD 客户端子集：global 路径更新 safepoint；keyspace 路径取 GCStatesClient。
pub trait PdClient: Send + Sync {
    /// UpdateGCSafePoint mirrors `pd.Client.UpdateGCSafePoint`.
    /// 更新集群 GC safepoint（global 管理器使用）。
    fn UpdateGCSafePoint(&self, ctx: &Context, safe_point: u64) -> Result<u64, SharedError>;

    /// UpdateServiceGCSafePoint mirrors the deprecated
    /// `pd.Client.UpdateServiceGCSafePoint`.
    /// 更新/注册服务级 GC safepoint（旧 API，仍被 global 路径使用）。
    fn UpdateServiceGCSafePoint(
        &self,
        ctx: &Context,
        service_id: &str,
        ttl: i64,
        safe_point: u64,
    ) -> Result<u64, SharedError>;

    /// GetGCStatesClient mirrors `pd.Client.GetGCStatesClient`.
    /// 按 keyspace 取得 GCStatesClient，供 keyspace Manager 使用。
    fn GetGCStatesClient(&self, keyspace_id: u32) -> Arc<dyn GCStatesClient>;
}

/// Manager abstracts GC operations, supporting both global and keyspace-level GC.
/// BR 侧统一 GC 操作面：查询 safepoint、设置/删除服务 safepoint（含 TTL）。
pub trait Manager: Send + Sync {
    /// GetGCSafePoint returns the current GC safe point.
    /// 返回当前可见的 GC safepoint。
    fn GetGCSafePoint(&self, ctx: &Context) -> Result<u64, SharedError>;

    /// SetServiceSafePoint sets the service safe point with TTL.
    /// If TTL <= 0, it removes the service safe point.
    /// 设置服务 safepoint；TTL<=0 时等价删除（与 Go 语义一致）。
    fn SetServiceSafePoint(&self, ctx: &Context, sp: BRServiceSafePoint)
    -> Result<(), SharedError>;

    /// DeleteServiceSafePoint removes the service safe point.
    /// 显式删除服务 safepoint。
    fn DeleteServiceSafePoint(
        &self,
        ctx: &Context,
        sp: BRServiceSafePoint,
    ) -> Result<(), SharedError>;
}

/// NewManager creates a GC Manager.
/// Pass keyspaceID = NullspaceID for global mode, or actual keyspaceID for keyspace mode.
/// 工厂：`NullspaceID` → global Manager，否则 → keyspace Manager。
pub fn NewManager(pd_client: Arc<dyn PdClient>, keyspace_id: KeyspaceID) -> Arc<dyn Manager> {
    // 与 Go NewManager 分支一致：仅按 keyspace_id 选择实现，不在此发起 RPC。
    if keyspace_id == NullspaceID {
        return Arc::new(newGlobalManager(pd_client));
    }
    Arc::new(newKeyspaceManager(pd_client, keyspace_id))
}
