// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.

//! 将 `CheckpointAdvancer` 适配为日志备份 Owner 守护进程回调。
//! 与 Go 侧 `advancer_daemon.go` 对齐：tick / 成为 Owner / 停止时的生命周期钩子。

use std::sync::atomic::{AtomicBool, Ordering};

use uuid::Uuid;

use crate::advancer::CheckpointAdvancer;

/// etcd Owner 竞选提示串，标识日志备份推进器角色。
pub const ownerPrompt: &str = "log-backup";
/// Owner 会话在 etcd 上的路径前缀。
pub const ownerPath: &str = "/tidb/br-stream/owner";

static ADVANCER_OWNER: AtomicBool = AtomicBool::new(false);

/// Whether this process currently owns the log-backup advancer role.
pub fn IsAdvancerOwner() -> bool {
    ADVANCER_OWNER.load(Ordering::SeqCst)
}

impl CheckpointAdvancer {
    /// Owner 周期 tick：推进 region 检查点并上传全局检查点。
    pub fn OnTick(&self) -> Result<(), String> {
        // Refreshing on every daemon tick preserves Go's continuously updated
        // runtime contract without requiring an uncancellable detached thread.
        let _ = self.refreshLogBackupFlushInterval();
        self.tick()
    }

    pub fn OnStart(&self) {
        // Go retries listener setup in the background. The synchronous Rust Env
        // supplies its initial/live batch here; daemon ticks remain available if
        // setup fails, matching Go's non-fatal OnStart contract.
        let _ = self.StartTaskListener();
    }

    /// 成为 Owner 后启动 flush 订阅，并刷新 TiKV flush 间隔配置。
    pub fn OnBecomeOwner(&self) {
        ADVANCER_OWNER.store(true, Ordering::SeqCst);
        self.SpawnSubscriptionHandler();
        let _ = self.refreshLogBackupFlushInterval();
    }

    /// 守护进程展示名，对应 Go OwnerManager 的 Name。
    pub fn Name(&self) -> &'static str {
        "LogBackup::Advancer"
    }

    /// 失去 Owner 或停止时关闭外部存储与订阅，避免泄漏。
    pub fn OnStop(&self) {
        ADVANCER_OWNER.store(false, Ordering::SeqCst);
        self.closeGlobalCheckpointStorage();
        self.stopSubscriber();
    }
}

/// 生成日志备份 Owner 管理器实例 ID（UUID）。
pub fn OwnerManagerForLogBackupId() -> String {
    Uuid::new_v4().to_string()
}

/// 返回 etcd Owner 路径常量。
pub fn OwnerManagerPath() -> &'static str {
    ownerPath
}

/// 返回 etcd Owner 提示串常量。
pub fn OwnerManagerPrompt() -> &'static str {
    ownerPrompt
}
