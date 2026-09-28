// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc. Licensed under Apache-2.0.

//! Keyspace GC barrier manager ported from `br/pkg/gc/manager_keyspace.go`.
//!
//! 按 keyspace 作用域的 GC 屏障管理器：经 `GetGCStatesClient(keyspaceID)` 的
//! `SetGCBarrier` / `DeleteGCBarrier` 新 API，替代全局弃用服务安全点接口。
//! 由 `NewManager(非 NullspaceID)` 选用；TTL<=0 时 Set 直接转 Delete，与统一 Manager 语义对齐。
//! 信号文件内容含 keyspaceID，便于集成测试校验作用域。

use std::sync::Arc;

use crate::manager::{GCStatesClient, KeyspaceID, Manager, PdClient};
use crate::safepoint::{BRServiceSafePoint, Context, SharedError, Trace};

/// keyspaceManager implements Manager using the per-keyspace GC barrier
/// mechanism. It uses the new pd.Client.GetGCStatesClient(keyspaceID)
/// .SetGCBarrier API.
///
/// `gc_client` 在构造时绑定 keyspace；后续读写无需再传 ID，与 Go 字段布局一致。
pub struct keyspaceManager {
    /// PD 客户端；构造后主要用于拿 GCStatesClient，运行期读写走 `gc_client`。
    pub(crate) pd_client: Arc<dyn PdClient>,
    /// 本管理器绑定的 keyspace；探针信号与诊断日志会回写该值。
    pub(crate) keyspace_id: KeyspaceID,
    pub(crate) gc_client: Arc<dyn GCStatesClient>,
}

/// newKeyspaceManager creates a new keyspaceManager instance.
///
/// 从 PD 取绑定该 keyspace 的 GCStatesClient；`pd_client`/`keyspace_id` 保留供探针与诊断。
pub fn newKeyspaceManager(
    pd_client: Arc<dyn PdClient>,
    keyspace_id: KeyspaceID,
) -> keyspaceManager {
    // Get keyspace-specific GC states client.
    // KeyspaceID is bound to this client, all operations will automatically
    // target this keyspace.
    let gc_client = pd_client.GetGCStatesClient(keyspace_id);
    keyspaceManager {
        pd_client,
        keyspace_id,
        gc_client,
    }
}

/// 与 global 侧相同：环境变量名即 failpoint 名，值为信号文件路径。
fn failpoint_signal_file(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|path| !path.is_empty())
}

impl Manager for keyspaceManager {
    /// GetGCSafePoint returns the current GC safe point for this keyspace.
    ///
    /// 读 `GetGCState` 的 `GCSafePoint` 字段，而非全局 `UpdateGCSafePoint(0)`。
    fn GetGCSafePoint(&self, ctx: &Context) -> Result<u64, SharedError> {
        match self.gc_client.GetGCState(ctx) {
            Ok(state) => Ok(state.GCSafePoint),
            Err(err) => Err(Trace(err)),
        }
    }

    /// SetServiceSafePoint sets the keyspace GC barrier using SetGCBarrier API.
    /// If sp.TTL <= 0, it calls DeleteGCBarrier to remove the barrier (same as
    /// unified manager behavior).
    ///
    /// barrierTS 取 `BackupTS-1`，与 global 的 UpdateServiceGCSafePoint 约定一致；
    /// 成功后写 keyspace 探针并 sleep 3s。`barrier_info` 字段仅对应 Go 调试日志消费。
    fn SetServiceSafePoint(
        &self,
        ctx: &Context,
        sp: BRServiceSafePoint,
    ) -> Result<(), SharedError> {
        // Go: log.Debug("set keyspace GC barrier", ...)

        // Handle deletion case (TTL <= 0), same as unified manager behavior.
        // TTL 非正视为删除请求，避免对 PD 发出无效 Set。
        if sp.TTL <= 0 {
            return self.DeleteServiceSafePoint(ctx, sp);
        }

        // Set or update the barrier.
        // barrierTS = BackupTS - 1 (same as UpdateServiceGCSafePoint).
        let barrier_info =
            match self
                .gc_client
                .SetGCBarrier(ctx, &sp.ID, sp.BackupTS.wrapping_sub(1), sp.TTL)
            {
                Ok(info) => info,
                Err(err) => return Err(Trace(err)),
            };

        // Integration tests use this to distinguish global vs keyspace GC protection.
        if let Some(sig_file) = failpoint_signal_file("hint-gc-keyspace-set-barrier") {
            // Include keyspaceID so the test can sanity-check scope if needed.
            // 格式固定为两行 keyspace=/id=，与 Go fmt.Sprintf 对齐。
            let content = format!("keyspace={}\nid={}\n", self.keyspace_id, sp.ID);
            if let Err(write_err) = std::fs::write(&sig_file, content.as_bytes()) {
                eprintln!(
                    "[WARN] failed to write failpoint signal file: {write_err}; file: {sig_file}"
                );
            }
            // Provide a small observation window for test scripts.
            std::thread::sleep(std::time::Duration::from_secs(3));
        }

        // Go: log.Debug("set keyspace GC barrier succeeded", ...)
        // 保留对返回字段的引用，避免「未使用」警告，语义等同 Go 的 Debug 字段读取。
        let _ = (
            &barrier_info.BarrierID,
            barrier_info.BarrierTS,
            barrier_info.TTL,
        );

        Ok(())
    }

    /// DeleteServiceSafePoint removes the keyspace GC barrier.
    ///
    /// 按服务 ID 删屏障；成功后写 delete 探针（含 keyspace 与 id）。
    fn DeleteServiceSafePoint(
        &self,
        ctx: &Context,
        sp: BRServiceSafePoint,
    ) -> Result<(), SharedError> {
        if let Err(err) = self.gc_client.DeleteGCBarrier(ctx, &sp.ID) {
            return Err(Trace(err));
        }
        if let Some(sig_file) = failpoint_signal_file("hint-gc-keyspace-delete-barrier") {
            let content = format!("keyspace={}\nid={}\n", self.keyspace_id, sp.ID);
            if let Err(write_err) = std::fs::write(&sig_file, content.as_bytes()) {
                eprintln!(
                    "[WARN] failed to write failpoint signal file: {write_err}; file: {sig_file}"
                );
            }
        }
        // Go: log.Debug("deleted keyspace GC barrier", ...)
        Ok(())
    }
}
