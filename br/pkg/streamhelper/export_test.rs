// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! Go-equivalent helpers from `export_test.go` (package streamhelper).
//!
//! 测试导出层：把未公开的配置/解析锁边界算法暴露给同 crate 测试，
//! 并提供与 Go `export_test.go` 同名的辅助函数。
//! 精简移植下外部存储工厂仍为 API 形状占位，恢复闭包为空操作。

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use astersql_br_pkg_streamhelper_config::{AdvancerCheckPointLagLimitNanos, CommandConfig};
use astersql_br_pkg_streamhelper_spans::Valued;

use crate::advancer::{
    CheckpointAdvancer, NewCommandCheckpointAdvancer, NewTiDBCheckpointAdvancer,
    lowerResolveLockMaxVersion, newCheckpointWithSpan, resolveLockRetryLowerBound,
    resolveLockTargetUpperBound,
};
use crate::advancer_env::Env;

/// Go `NewCheckpointAdvancer` random choice — Rust picks TiDB/Command by parity of nanos.
/// 用纳秒奇偶在 TiDB/Command 两条构造路径间伪随机选择，覆盖两套配置源。
pub fn NewCheckpointAdvancerForTest(env: Arc<dyn Env>) -> CheckpointAdvancer {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    if nanos & 1 == 0 {
        NewTiDBCheckpointAdvancer(env)
    } else {
        NewCommandCheckpointAdvancer(env)
    }
}

/// 测试专用扩展：改配置、读阈值、刷新 flush 间隔等。
pub trait CheckpointAdvancerTestExt {
    fn UpdateConfigWith(&self, f: impl FnOnce(&mut CommandConfig));
    fn UpdateCheckPointLagLimit(&self, limit: Duration);
    fn TESTSetLastCheckpointToCurrentMin(&self);
    fn TESTResolveLockInterval(&self) -> Duration;
    fn TESTDefaultStartPollThreshold(&self) -> Duration;
    fn TESTSubscriberErrorStartPollThreshold(&self) -> Duration;
    fn TESTRefreshLogBackupFlushInterval(&self);
}

impl CheckpointAdvancerTestExt for CheckpointAdvancer {
    /// 在当前 CommandConfig（含 TiDB 内嵌配置）上做局部改写。
    fn UpdateConfigWith(&self, f: impl FnOnce(&mut CommandConfig)) {
        self.updateConfigWithForTest(f);
    }

    fn UpdateCheckPointLagLimit(&self, limit: Duration) {
        // Command path reads the current config; TiDB path reads the global atomic.
        // Updating both mirrors the branch-specific Go behavior without changing mode.
        self.updateConfigWithForTest(|cfg| cfg.CheckPointLagLimit = limit);
        AdvancerCheckPointLagLimitNanos
            .store(limit.as_nanos() as u64, std::sync::atomic::Ordering::SeqCst);
    }

    /// 把 last checkpoint 设为当前 spans 最小值，方便滞后相关断言。
    fn TESTSetLastCheckpointToCurrentMin(&self) {
        let p = self.WithCheckpoints(|vsf| vsf.Min().map(|v| newCheckpointWithSpan(v)));
        if let Some(Some(p)) = p {
            self.UpdateLastCheckpoint(p);
        }
    }

    /// 读取当前 resolve-lock 轮询间隔。
    fn TESTResolveLockInterval(&self) -> Duration {
        self.getResolveLockInterval()
    }

    /// 正常启动轮询阈值。
    fn TESTDefaultStartPollThreshold(&self) -> Duration {
        self.getDefaultStartPollThreshold()
    }

    /// 订阅出错后的启动轮询阈值（通常更激进）。
    fn TESTSubscriberErrorStartPollThreshold(&self) -> Duration {
        self.getSubscriberErrorStartPollThreshold()
    }

    /// 触发从 TiKV 配置刷新 flush 间隔。
    fn TESTRefreshLogBackupFlushInterval(&self) {
        let _ = self.refreshLogBackupFlushInterval();
    }
}

/// 用墙钟毫秒左移构造伪 PD ts，再算 resolve-lock 目标上界。
pub fn TESTResolveLockTargetUpperBound(
    checkpointTS: u64,
    resolveLockInterval: Duration,
    now: SystemTime,
) -> u64 {
    let ms = now
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    // TiDB 物理时间戳习惯：毫秒 << 18。
    let current_ts = ms << 18;
    resolveLockTargetUpperBound(checkpointTS, resolveLockInterval, current_ts)
}

/// 暴露 `resolveLockRetryLowerBound` 供边界单测。
pub fn TESTResolveLockRetryLowerBound(checkpointTS: u64, maxVersion: u64) -> (u64, bool) {
    resolveLockRetryLowerBound(checkpointTS, maxVersion)
}

/// 暴露 `lowerResolveLockMaxVersion` 供边界单测。
pub fn TESTLowerResolveLockMaxVersion(maxVersion: u64, lowerBound: u64) -> (u64, bool) {
    lowerResolveLockMaxVersion(maxVersion, lowerBound)
}

/// 临时覆盖 metadata watch 的 progress 请求间隔与空闲超时，并返回恢复闭包。
pub fn SetMetadataWatchProgressForTest(interval: Duration, timeout: Duration) -> impl FnOnce() {
    let (old_interval, old_timeout) =
        crate::advancer_cliext::setMetadataWatchProgressForTest(interval, timeout);
    move || {
        crate::advancer_cliext::setMetadataWatchProgressForTest(old_interval, old_timeout);
    }
}

/// Global checkpoint storage factory override — slim port has no external storage; restore is no-op.
/// 无外部存储工厂时的占位，避免集成测试因缺少注入点编译失败。
pub fn SetGlobalCheckpointStorageFactoryForTest(
    _factory: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
) -> impl FnOnce() {
    || {}
}

/// 把 Valued span 包装成 Checkpoint，便于断言最小检查点。
pub fn valued_min_checkpoint(v: Valued) -> crate::advancer::Checkpoint {
    newCheckpointWithSpan(v)
}
