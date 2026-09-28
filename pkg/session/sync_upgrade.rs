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

// 集群滚动升级时的全局状态同步（Upgrading ↔ NormalRunning）。
//
// 通过 `SyncUpgradeRuntime` 抽象 etcd/DDL owner/分布式任务等副作用，
// 在升级窗口内将集群状态切到 Upgrading，结束后恢复 NormalRunning。

#![allow(dead_code, non_snake_case)]

use std::time::{Duration, Instant};

/// 集群全局运行状态：升级中或正常运行。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerState {
    /// 滚动升级窗口内。
    Upgrading,
    /// 正常对外服务。
    NormalRunning,
}

/// DDL owner 操作结果：是否已同步升级状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OwnerOp {
    /// owner 侧是否已同步到 Upgrading。
    pub synced_upgrading_state: bool,
}

/// 判断截止时间是否已到（对应 Go context Done）。
pub fn isContextDone(deadline: Instant) -> bool {
    Instant::now() >= deadline
}

/// Etcd, DDL and distributed-task effects are required from the host runtime.
/// There are deliberately no default implementations that could report success.
/// 宿主运行时需提供的升级副作用：全局状态、DDL owner、任务并发与日志。
pub trait SyncUpgradeRuntime {
    /// 运行时错误类型。
    type Error;

    /// 将全局状态写入 etcd 等存储，带超时。
    fn update_global_state(
        &mut self,
        state: ServerState,
        timeout: Duration,
    ) -> Result<(), Self::Error>;
    /// 向 DDL owner 发起同步升级状态操作。
    fn owner_operation(&mut self, timeout: Duration) -> Result<OwnerOp, Self::Error>;
    /// 恢复所有因升级暂停的 DDL job；分别返回逐 job 错误与整体调用错误。
    fn resume_all_jobs(&mut self) -> (Vec<Self::Error>, Option<Self::Error>);
    /// 分布式任务管理器是否可用。
    fn task_manager_available(&mut self) -> bool;
    /// 调整任务溢出并发度。
    fn adjust_task_overflow_concurrency(&mut self) -> Result<(), Self::Error>;
    /// 读取当前全局状态。
    fn global_state(&mut self, timeout: Duration) -> Result<ServerState, Self::Error>;
    /// 构造超时错误。
    fn timeout_error(&mut self, timeout: Duration) -> Self::Error;
    /// 记录警告日志。
    fn log_warning(&mut self, message: &str, error: Option<&Self::Error>);
    /// 记录版本与升级状态。
    fn log_state(&mut self, old_version: i64, new_version: i64, upgrading: bool);
    /// 休眠指定时长（轮询间隔）。
    fn sleep(&mut self, duration: Duration);
}

/// 将集群切入 Upgrading，并轮询直到 DDL owner 同步成功或超时。
pub fn SyncUpgradeState<R: SyncUpgradeRuntime>(
    runtime: &mut R,
    timeout: Duration,
) -> Result<(), R::Error> {
    runtime.update_global_state(ServerState::Upgrading, timeout)?;

    let deadline = Instant::now() + timeout;
    let interval = Duration::from_millis(200);
    let mut attempt = 0_u64;
    loop {
        if isContextDone(deadline) {
            return Err(runtime.timeout_error(timeout));
        }

        // 单次 owner 操作超时不超过剩余时间与 3s。
        let remaining = deadline.saturating_duration_since(Instant::now());
        let child_timeout = remaining.min(Duration::from_secs(3));
        match runtime.owner_operation(child_timeout) {
            Ok(op) if op.synced_upgrading_state => return Ok(()),
            Ok(_) => {
                // Go 对未同步的 owner op 与查询错误使用同一条告警。
                if attempt.is_multiple_of(10) {
                    runtime.log_warning("get owner op failed", None);
                }
            }
            Err(error) => {
                if attempt.is_multiple_of(10) {
                    runtime.log_warning("get owner operation failed", Some(&error));
                }
            }
        }
        attempt += 1;
        runtime.sleep(interval);
    }
}

/// 升级结束后恢复暂停的 DDL/任务，并将全局状态切回 NormalRunning。
pub fn SyncNormalRunning<R: SyncUpgradeRuntime>(runtime: &mut R) -> Result<(), R::Error> {
    // 恢复 job：逐条失败只记日志，不中断后续步骤。
    let (job_errors, error) = runtime.resume_all_jobs();
    if let Some(error) = error.as_ref() {
        runtime.log_warning("resume all paused jobs failed", Some(error));
    }
    for error in &job_errors {
        runtime.log_warning("resume the job failed", Some(error));
    }

    if runtime.task_manager_available()
        && let Err(error) = runtime.adjust_task_overflow_concurrency()
    {
        runtime.log_warning("cannot adjust task overflow concurrency", Some(&error));
    }

    runtime.update_global_state(ServerState::NormalRunning, Duration::from_secs(3))
}

/// 查询集群是否处于 Upgrading 状态。
pub fn IsUpgradingClusterState<R: SyncUpgradeRuntime>(runtime: &mut R) -> Result<bool, R::Error> {
    runtime
        .global_state(Duration::from_secs(3))
        .map(|state| state == ServerState::Upgrading)
}

/// 带重试地查询升级状态并记录版本日志；超时后仅打警告并返回。
pub fn isUpgradingClusterStateWithRetry<R: SyncUpgradeRuntime>(
    runtime: &mut R,
    old_version: i64,
    new_version: i64,
    timeout: Duration,
) {
    let deadline = Instant::now() + timeout;
    let interval = Duration::from_millis(200);
    let mut attempt = 0_u64;
    loop {
        match IsUpgradingClusterState(runtime) {
            Ok(upgrading) => {
                runtime.log_state(old_version, new_version, upgrading);
                return;
            }
            Err(error) => {
                if isContextDone(deadline) {
                    runtime.log_warning("get global state timed out", Some(&error));
                    return;
                }
                // 每 25 次失败打一次警告。
                if attempt.is_multiple_of(25) {
                    runtime.log_warning("get global state failed", Some(&error));
                }
            }
        }
        attempt += 1;
        runtime.sleep(interval);
    }
}
