// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc. Licensed under Apache-2.0.

//! Test fixtures ported from `br/pkg/gc/mock_test.go`.
//!
//! Go builds an unistore MockPD on BadgerDB. This crate's Darwin/arm64 lane
//! intentionally has no kv/domain/kvproto/grpcio/unistore deps, so the same
//! PD GC call shapes (UpdateServiceGCSafePoint / UpdateGCSafePoint /
//! GetGCStatesClient barriers, plus InternalController advance) are simulated
//! in-process while preserving call order, errors, and data shapes.
//!
//! 进程内 PD/GC 测试替身：复现 Go unistore MockPD 的调用形态与状态语义，
//! 避免本机依赖 Badger/grpcio。全局服务安全点映射到 Nullspace 上的屏障，
//! 供 `requireBarrier`/`getState` 与 Go 测试同一套断言。
//! `mockManager` 在真实 Manager 外包一层，支持调用计数与错误注入（含 DeadlineExceeded）。
//! 本文件只提供夹具与助手，不含业务断言用例；用例见 manager_test / safepoint_test。
//! 状态并发由 `Mutex` 串行化；原子计数用于跨线程观测 Set 次数。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::manager::{
    GCBarrierInfo, GCState, GCStatesClient, KeyspaceID, Manager, NewManager, NullspaceID, PdClient,
};
use crate::safepoint::{BRServiceSafePoint, Context, SharedError};

/// testKeyspaceID is a non-global keyspace ID used for testing.
///
/// 非 Nullspace 的固定测试 keyspace；与 manager_test / safepoint_test 共用。
pub const testKeyspaceID: KeyspaceID = 100;

/// 构造可下传的通用 `SharedError`（io::Error::other），用于参数非法等场景。
fn mock_err(message: &str) -> SharedError {
    Box::new(std::io::Error::other(message.to_string()))
}

/// DeadlineExceeded mirrors Go `context.DeadlineExceeded` for error injection.
///
/// 类型化超时错误，供 Keeper 负向用例做 `error_is` / ErrorIs 风格匹配。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeadlineExceeded;

impl std::fmt::Display for DeadlineExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "context deadline exceeded")
    }
}

impl std::error::Error for DeadlineExceeded {}

/// Per-keyspace GC state mirroring unistore MockPD's gcStatesManager.
///
/// 单 keyspace 内存态：GC/Txn 安全点、服务安全点映射，以及 GC 屏障表。
#[derive(Default)]
struct KeyspaceState {
    /// 当前 GC 安全点；单调不减（仅在更大时更新）。
    gc_safe_point: u64,
    /// 事务安全点上界；GC 推进不得越过该值。
    txn_safe_point: u64,
    /// 对外快照的屏障列表来源；global Update 也会写入此处以统一断言。
    barriers: HashMap<String, GCBarrierInfo>,
}

/// 全 PD 内部状态：按 keyspace ID 分桶；缺省 entry 表示尚未写入。
#[derive(Default)]
struct MockPdInner {
    states: HashMap<u32, KeyspaceState>,
}

/// mockPDClient mirrors Go's `mockPDClient`: a pd.Client adapter over MockPD.
///
/// 线程安全的内存 PD；实现 `PdClient` 的全局安全点与 keyspace GCStatesClient。
#[derive(Default)]
pub struct mockPDClient {
    inner: Mutex<MockPdInner>,
}

impl mockPDClient {
    /// 只读快照指定 keyspace 的 `GCState`；无条目时安全点为 0、屏障为空。
    fn snapshot(&self, keyspace_id: KeyspaceID) -> GCState {
        let inner = self.inner.lock().unwrap();
        let state = inner.states.get(&keyspace_id);
        GCState {
            GCSafePoint: state.map(|s| s.gc_safe_point).unwrap_or(0),
            TxnSafePoint: state.map(|s| s.txn_safe_point).unwrap_or(0),
            GCBarriers: state
                .map(|s| s.barriers.values().cloned().collect())
                .unwrap_or_default(),
        }
    }

    /// AdvanceTxnSafePoint then AdvanceGCSafePoint (Go InternalController order).
    ///
    /// 先抬 txn 再抬 gc；若目标超过 txn 则报错，对齐 unistore 约束。
    pub fn advance_gc_safe_point(
        &self,
        keyspace_id: KeyspaceID,
        ts: u64,
    ) -> Result<(), SharedError> {
        let mut inner = self.inner.lock().unwrap();
        let state = inner.states.entry(keyspace_id).or_default();
        Self::advance_txn_safe_point(state, ts)?;
        Self::advance_gc_safe_point_only(state, ts)?;
        Ok(())
    }

    fn advance_txn_safe_point(state: &mut KeyspaceState, target: u64) -> Result<u64, SharedError> {
        if target < state.txn_safe_point {
            return Err(mock_err(&format!(
                "[PD:gc:ErrDecreasingTxnSafePoint] trying to update txn safe point to a smaller value, current value: {}, given: {target}",
                state.txn_safe_point
            )));
        }

        let min_barrier = state
            .barriers
            .values()
            .map(|barrier| barrier.BarrierTS)
            .min();
        let mut new_txn_safe_point =
            min_barrier.map_or(target, |barrier_ts| target.min(barrier_ts));
        if new_txn_safe_point < state.txn_safe_point {
            new_txn_safe_point = state.txn_safe_point;
        }
        state.txn_safe_point = new_txn_safe_point;
        Ok(new_txn_safe_point)
    }

    fn advance_gc_safe_point_only(
        state: &mut KeyspaceState,
        target: u64,
    ) -> Result<u64, SharedError> {
        if target < state.gc_safe_point {
            return Err(mock_err(&format!(
                "trying to update gc safe point to a smaller value, current value: {}, given: {target}",
                state.gc_safe_point
            )));
        }
        if target > state.txn_safe_point {
            return Err(mock_err(&format!(
                "trying to update GC safe point to a too large value that exceeds the txn safe point, current value: {}, given: {target}, current txn safe point: {}",
                state.gc_safe_point, state.txn_safe_point
            )));
        }
        state.gc_safe_point = target;
        Ok(target)
    }
}

/// 绑定单一 keyspace 的 GCStatesClient 替身；所有读写落到同一 `mockPDClient`。
struct MockGcStatesClient {
    pd: Arc<mockPDClient>,
    keyspace_id: u32,
}

impl GCStatesClient for MockGcStatesClient {
    fn GetGCState(&self, _ctx: &Context) -> Result<GCState, SharedError> {
        Ok(self.pd.snapshot(self.keyspace_id))
    }

    fn SetGCBarrier(
        &self,
        _ctx: &Context,
        barrier_id: &str,
        barrier_ts: u64,
        ttl_seconds: i64,
    ) -> Result<GCBarrierInfo, SharedError> {
        // 空 ID、零 TS 或非正 TTL 视为非法，与 Go mock 校验对齐。
        if barrier_id.is_empty() || barrier_ts == 0 || ttl_seconds <= 0 {
            return Err(mock_err("invalid barrier arguments"));
        }
        let mut inner = self.pd.inner.lock().unwrap();
        let state = inner.states.entry(self.keyspace_id).or_default();
        if barrier_ts < state.txn_safe_point {
            return Err(mock_err(&format!(
                "trying to set a GC barrier on ts {barrier_ts} which is already behind the txn safe point {}",
                state.txn_safe_point
            )));
        }
        let info = GCBarrierInfo {
            BarrierID: barrier_id.to_string(),
            BarrierTS: barrier_ts,
            TTL: i64::MAX,
        };
        // 同 ID 覆盖写入，模拟 Set 既可新建也可更新。
        state.barriers.insert(barrier_id.to_string(), info.clone());
        Ok(info)
    }

    fn DeleteGCBarrier(
        &self,
        _ctx: &Context,
        barrier_id: &str,
    ) -> Result<Option<GCBarrierInfo>, SharedError> {
        let mut inner = self.pd.inner.lock().unwrap();
        // 返回被删条目（若有），便于调用方观测；缺失则 Ok(None)。
        Ok(inner
            .states
            .entry(self.keyspace_id)
            .or_default()
            .barriers
            .remove(barrier_id))
    }
}

impl PdClient for Arc<mockPDClient> {
    fn UpdateGCSafePoint(&self, _ctx: &Context, safe_point: u64) -> Result<u64, SharedError> {
        let mut inner = self.inner.lock().unwrap();
        // 全局 API 固定写 NullspaceID 桶。
        let state = inner.states.entry(NullspaceID).or_default();
        if safe_point < state.gc_safe_point {
            // MockPD's compatibility wrapper silently ignores decreasing updates.
            return Ok(state.gc_safe_point);
        }
        mockPDClient::advance_gc_safe_point_only(state, safe_point)
    }

    fn UpdateServiceGCSafePoint(
        &self,
        _ctx: &Context,
        service_id: &str,
        ttl: i64,
        safe_point: u64,
    ) -> Result<u64, SharedError> {
        let mut inner = self.inner.lock().unwrap();
        let state = inner.states.entry(NullspaceID).or_default();
        if service_id == "gc_worker" {
            if ttl != i64::MAX {
                return Err(mock_err(
                    "ttl of gc_worker's service safe point must be math.MaxInt64",
                ));
            }
            return mockPDClient::advance_txn_safe_point(state, safe_point);
        }
        // Unistore surfaces service safe points as GC barriers on NullspaceID,
        // which is what requireBarrier/getState assert in Go tests.
        // TTL<=0 删除屏障；TTL>0 时须经过与 GCStates API 相同的校验。
        if ttl <= 0 {
            state.barriers.remove(service_id);
        } else {
            if service_id.is_empty() || safe_point == 0 {
                return Err(mock_err("invalid arguments"));
            }
            if safe_point < state.txn_safe_point {
                return Err(mock_err(&format!(
                    "trying to set a GC barrier on ts {safe_point} which is already behind the txn safe point {}",
                    state.txn_safe_point
                )));
            }
            state.barriers.insert(
                service_id.to_string(),
                GCBarrierInfo {
                    BarrierID: service_id.to_string(),
                    BarrierTS: safe_point,
                    TTL: i64::MAX,
                },
            );
        }
        // Compatibility API returns the Nullspace txn safe point directly.
        Ok(state.txn_safe_point)
    }

    fn GetGCStatesClient(&self, keyspace_id: u32) -> Arc<dyn GCStatesClient> {
        // 每次返回新的绑定客户端；共享同一底层 PD 状态。
        Arc::new(MockGcStatesClient {
            pd: self.clone(),
            keyspace_id,
        })
    }
}

/// newTestMockPD creates a fully configured MockPD wrapper.
/// Cleanup is automatic via Arc drop (no Badger temp dirs on this lane).
///
/// 空状态工厂；无临时目录，生命周期随 Arc 引用计数结束。
pub fn newTestMockPD() -> Arc<mockPDClient> {
    Arc::new(mockPDClient::default())
}

// ============================================================================
// State Query Helper Functions
// ============================================================================

/// findBarrier finds a barrier by ID in the GC state. Returns None if not found.
///
/// 在快照的 `GCBarriers` 中按 ID 线性查找，供断言助手复用。
pub fn findBarrier<'a>(state: &'a GCState, barrier_id: &str) -> Option<&'a GCBarrierInfo> {
    state.GCBarriers.iter().find(|b| b.BarrierID == barrier_id)
}

/// requireBarrier asserts that a barrier exists with the expected TS.
///
/// 必须存在且 BarrierTS 精确匹配；对应 Go require.Equal 风格硬失败。
pub fn requireBarrier(state: &GCState, barrier_id: &str, expected_ts: u64) {
    let barrier = findBarrier(state, barrier_id);
    assert!(barrier.is_some(), "barrier {barrier_id:?} should exist");
    assert_eq!(
        expected_ts,
        barrier.unwrap().BarrierTS,
        "barrier {barrier_id:?} TS mismatch"
    );
}

/// requireNoBarrier asserts that a barrier does not exist.
///
/// 负向断言：同名屏障不得出现在该 keyspace 快照中。
pub fn requireNoBarrier(state: &GCState, barrier_id: &str) {
    let barrier = findBarrier(state, barrier_id);
    assert!(barrier.is_none(), "barrier {barrier_id:?} should not exist");
}

/// getState returns the GC state for the specified keyspace.
/// Use NullspaceID for global mode.
///
/// 经 GCStatesClient 读快照；失败直接 panic（测试助手约定）。
pub fn getState(ctx: &Context, mock_pd: &Arc<mockPDClient>, keyspace_id: KeyspaceID) -> GCState {
    mock_pd
        .GetGCStatesClient(keyspace_id)
        .GetGCState(ctx)
        .expect("GetGCState")
}

// ============================================================================
// Mock Manager Wrapper for Keeper Tests
// ============================================================================

/// mockManager wraps a real gc.Manager (backed by mockPD) with:
/// - Call counting for verification
/// - Error injection for negative testing
///
/// Keeper 测试用包装：内层为 `NewManager` 真实现，外层可注入 Set/Get 错误并计数。
pub struct mockManager {
    /// 由 `NewManager` 生成的真实实现（global 或 keyspace）。
    inner: Arc<dyn Manager>,
    /// 暴露给测试以推进安全点或读状态快照。
    pub mockPD: Arc<mockPDClient>,
    /// `SetServiceSafePoint` 调用次数（含注入失败前的计数）。
    set_safe_point_calls: AtomicU64,
    /// 非空时 Set 短路返回该错误（DeadlineExceeded 保持类型）。
    set_safe_point_err: Mutex<Option<SharedError>>,
    /// 非空时 GetGCSafePoint 短路返回（string 化后的 mock_err）。
    gc_safe_point_err: Mutex<Option<SharedError>>,
}

/// 按 keyspace 构造包装 Manager；同时暴露底层 `mockPD` 供状态推进与断言。
pub fn newMockManagerWrapper(keyspace_id: KeyspaceID) -> Arc<mockManager> {
    let mock_pd = newTestMockPD();
    let mgr = NewManager(Arc::new(mock_pd.clone()), keyspace_id);
    Arc::new(mockManager {
        inner: mgr,
        mockPD: mock_pd,
        set_safe_point_calls: AtomicU64::new(0),
        set_safe_point_err: Mutex::new(None),
        gc_safe_point_err: Mutex::new(None),
    })
}

impl mockManager {
    /// 读取 Set 调用计数，用于验证 TTL 续约/重试是否触发。
    pub fn getSetSafePointCalls(&self) -> u64 {
        self.set_safe_point_calls.load(Ordering::SeqCst)
    }

    /// 注入下一次（及之后）Set 返回的错误，直到被覆盖。
    pub fn inject_set_safe_point_err(&self, err: SharedError) {
        *self.set_safe_point_err.lock().unwrap() = Some(err);
    }

    /// 注入 GetGCSafePoint 错误，用于 Keeper 读安全点失败路径。
    pub fn inject_gc_safe_point_err(&self, err: SharedError) {
        *self.gc_safe_point_err.lock().unwrap() = Some(err);
    }

    /// setGCSafePoint sets the GC safe point in mockPD for testing.
    /// This first advances txn safe point, then advances GC safe point.
    ///
    /// 测试侧推进 GC：委托 `advance_gc_safe_point`，保持 txn→gc 顺序。
    pub fn setGCSafePoint(
        &self,
        _ctx: &Context,
        keyspace_id: KeyspaceID,
        ts: u64,
    ) -> Result<(), SharedError> {
        self.mockPD.advance_gc_safe_point(keyspace_id, ts)
    }
}

impl Manager for mockManager {
    fn GetGCSafePoint(&self, ctx: &Context) -> Result<u64, SharedError> {
        // 注入优先：有错误则不再访问内层 Manager。
        if let Some(err) = self.gc_safe_point_err.lock().unwrap().as_ref() {
            return Err(mock_err(&err.to_string()));
        }
        self.inner.GetGCSafePoint(ctx)
    }

    fn SetServiceSafePoint(
        &self,
        ctx: &Context,
        sp: BRServiceSafePoint,
    ) -> Result<(), SharedError> {
        // 无论成败都先计数，方便断言「尝试过调用」。
        self.set_safe_point_calls.fetch_add(1, Ordering::SeqCst);
        if let Some(err) = self.set_safe_point_err.lock().unwrap().as_ref() {
            // Preserve typed DeadlineExceeded for ErrorIs-style checks.
            // DeadlineExceeded 需保持具体类型，避免被 string 化后丢失 ErrorIs。
            if err.downcast_ref::<DeadlineExceeded>().is_some() {
                return Err(Box::new(DeadlineExceeded));
            }
            return Err(mock_err(&err.to_string()));
        }
        self.inner.SetServiceSafePoint(ctx, sp)
    }

    fn DeleteServiceSafePoint(
        &self,
        ctx: &Context,
        sp: BRServiceSafePoint,
    ) -> Result<(), SharedError> {
        // 删除路径不做错误注入，直接转发内层实现。
        self.inner.DeleteServiceSafePoint(ctx, sp)
    }
}

/// error_is walks the source chain looking for `DeadlineExceeded` (Go ErrorIs).
///
/// 沿 `source` 链查找类型化超时，对齐 Go `errors.Is(..., context.DeadlineExceeded)`。
pub fn error_is_deadline_exceeded(err: &SharedError) -> bool {
    // 从最外层错误开始，沿 cause 链向下匹配类型。
    let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(err.as_ref());
    while let Some(e) = cur {
        if e.downcast_ref::<DeadlineExceeded>().is_some() {
            return true;
        }
        // 无 source 时结束；不把 Display 文本当匹配条件。
        cur = e.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_pd_matches_unistore_gc_state_constraints() {
        let ctx = Context::Background();
        let mock_pd = newTestMockPD();
        let states = mock_pd.GetGCStatesClient(testKeyspaceID);

        mock_pd
            .advance_gc_safe_point(testKeyspaceID, 100)
            .expect("advance initial GC safe point");

        let err = states
            .SetGCBarrier(&ctx, "behind-txn", 99, 30)
            .expect_err("barrier behind txn safe point must fail");
        assert!(
            err.to_string().contains("behind the txn safe point"),
            "unexpected error: {err}"
        );

        let barrier = states
            .SetGCBarrier(&ctx, "blocks-txn", 120, 30)
            .expect("set barrier");
        assert_eq!(barrier.TTL, i64::MAX, "unistore reports TTLNeverExpire");

        let err = mock_pd
            .advance_gc_safe_point(testKeyspaceID, 150)
            .expect_err("barrier must prevent GC from advancing beyond it");
        assert!(
            err.to_string().contains("exceeds the txn safe point"),
            "unexpected error: {err}"
        );
        let state = getState(&ctx, &mock_pd, testKeyspaceID);
        assert_eq!(state.TxnSafePoint, 120);
        assert_eq!(state.GCSafePoint, 100);
    }

    #[test]
    fn mock_pd_matches_unistore_global_compatibility_api() {
        let ctx = Context::Background();
        let mock_pd = newTestMockPD();

        mock_pd
            .advance_gc_safe_point(NullspaceID, 80)
            .expect("advance nullspace GC safe point");
        let returned = mock_pd
            .UpdateServiceGCSafePoint(&ctx, "br-test", 30, 100)
            .expect("set global service safe point");
        assert_eq!(returned, 80, "compatibility API returns txn safe point");

        let err = mock_pd
            .UpdateGCSafePoint(&ctx, 81)
            .expect_err("GC safe point cannot exceed txn safe point");
        assert!(
            err.to_string().contains("exceeds the txn safe point"),
            "unexpected error: {err}"
        );
    }
}
