// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc. Licensed under Apache-2.0.

//! Global GC safepoint manager ported from `br/pkg/gc/manager_global.go`.
//!
//! 全局（非 keyspace）GC 安全点管理器：经 PD 已弃用的
//! `UpdateServiceGCSafePoint` API 设置/删除服务安全点，保持与旧版 BR 行为兼容。
//! 由 `NewManager(NullspaceID)` 选用；与 `keyspaceManager` 相对，作用域为整集群。
//! 集成测试通过环境变量 failpoint 写信号文件，区分 global / keyspace 保护路径。

use std::sync::Arc;
use std::time::Duration;

use crate::manager::{Manager, PdClient};
use crate::safepoint::{BRServiceSafePoint, Context, SharedError, Trace};

/// globalManager implements Manager using the global GC safepoint mechanism.
/// It uses the deprecated pd.Client.UpdateServiceGCSafePoint API for backward
/// compatibility.
///
/// 持有共享 `PdClient`；所有 GC 读写都走全局服务安全点接口，不按 keyspace 分流。
pub struct globalManager {
    pub(crate) pd_client: Arc<dyn PdClient>,
}

/// newGlobalManager creates a new globalManager instance.
///
/// 工厂函数，对应 Go `newGlobalManager`；仅包装 PD 客户端，无额外状态。
pub fn newGlobalManager(pd_client: Arc<dyn PdClient>) -> globalManager {
    globalManager { pd_client }
}

/// Mirrors Go `failpoint.Inject`: the failpoint value is a signal file path
/// carried by an environment variable named after the failpoint.
///
/// Rust 侧用同名环境变量代替 failpoint 注入：非空路径即视为开启对应探针。
fn failpoint_signal_file(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|path| !path.is_empty())
}

impl Manager for globalManager {
    /// GetGCSafePoint returns the current GC safe point.
    ///
    /// 以 `safe_point=0` 调用 `UpdateGCSafePoint` 做只读查询（与 Go 一致），
    /// 错误经 `Trace` 包装后向上返回。
    fn GetGCSafePoint(&self, ctx: &Context) -> Result<u64, SharedError> {
        match self.pd_client.UpdateGCSafePoint(ctx, 0) {
            Ok(safe_point) => Ok(safe_point),
            Err(err) => Err(Trace(err)),
        }
    }

    /// SetServiceSafePoint sets the global service safe point using the
    /// deprecated API. This maintains backward compatibility with existing BR
    /// behavior.
    ///
    /// 注册服务安全点：PD 目标值为 `BackupTS-1`（wrapping_sub 对齐无符号减法），
    /// 成功后可选写 failpoint 信号并 sleep 3s 供集成测试观测；
    /// 若 PD 返回的 lastSafePoint 高于请求值且 TTL>0，告警 GC 寿命可能不足。
    fn SetServiceSafePoint(
        &self,
        ctx: &Context,
        sp: BRServiceSafePoint,
    ) -> Result<(), SharedError> {
        // Go: log.Debug("update PD safePoint limit with TTL", zap.Object("safePoint", sp))
        let result = self.pd_client.UpdateServiceGCSafePoint(
            ctx,
            &sp.ID,
            sp.TTL,
            sp.BackupTS.wrapping_sub(1),
        );
        if result.is_ok() {
            // Integration tests use this to distinguish global vs keyspace GC protection.
            // 成功路径才触发探针，避免失败时误写信号干扰断言。
            if let Some(sig_file) = failpoint_signal_file("hint-gc-global-set-safepoint") {
                // Write the service ID so the test can match PD output precisely.
                if let Err(write_err) = std::fs::write(&sig_file, sp.ID.as_bytes()) {
                    eprintln!(
                        "[WARN] failed to write failpoint signal file: {write_err}; file: {sig_file}"
                    );
                }
                // Provide a small observation window for test scripts.
                std::thread::sleep(Duration::from_secs(3));
            }
        }
        // PD 可能接受请求但实际保留更高安全点；TTL>0 时才有保护语义，需告警。
        if let Ok(last_safe_point) = &result {
            if *last_safe_point > sp.BackupTS.wrapping_sub(1) && sp.TTL > 0 {
                eprintln!(
                    "[WARN] service GC safe point lost, we may fail to back up if GC lifetime isn't long enough; lastSafePoint: {last_safe_point}, safePoint: {}",
                    sp.MarshalLogObject()
                );
            }
        }
        match result {
            Ok(_) => Ok(()),
            Err(err) => Err(Trace(err)),
        }
    }

    /// DeleteServiceSafePoint removes the service safe point by setting TTL to 0.
    ///
    /// 删除语义：TTL=0、safe_point=0 调用同一弃用 API；成功后可写 delete 探针信号。
    fn DeleteServiceSafePoint(
        &self,
        ctx: &Context,
        sp: BRServiceSafePoint,
    ) -> Result<(), SharedError> {
        // Setting TTL to 0 effectively removes the service safe point.
        let result = self.pd_client.UpdateServiceGCSafePoint(ctx, &sp.ID, 0, 0);
        if result.is_ok() {
            // 与 Set 对称的 delete 探针，供测试确认走的是 global 删除路径。
            if let Some(sig_file) = failpoint_signal_file("hint-gc-global-delete-safepoint") {
                if let Err(write_err) = std::fs::write(&sig_file, sp.ID.as_bytes()) {
                    eprintln!(
                        "[WARN] failed to write failpoint signal file: {write_err}; file: {sig_file}"
                    );
                }
            }
        }
        match result {
            Ok(_) => Ok(()),
            Err(err) => Err(Trace(err)),
        }
    }
}
