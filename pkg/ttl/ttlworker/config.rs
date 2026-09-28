// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// TTL worker 调度与缓存刷新的默认时间间隔配置。
//
// 提供 Job/Task Manager 循环、心跳、InfoSchema（表结构元数据）缓存、
// TTL 状态缓存、worker 扩缩容与 GC 的默认周期；`IntervalOverrides` 允许测试或运行时覆盖。

use std::time::Duration;

/// Job Manager 主循环 ticker 默认间隔（检查待调度 job）。
pub const JOB_MANAGER_LOOP_TICKER_INTERVAL: Duration = Duration::from_secs(10);
/// InfoSchema 缓存刷新默认间隔。
pub const UPDATE_INFO_SCHEMA_CACHE_INTERVAL: Duration = Duration::from_secs(120);
/// TTL 表状态缓存刷新默认间隔。
pub const UPDATE_TTL_TABLE_STATUS_CACHE_INTERVAL: Duration = Duration::from_secs(120);
/// TTL 内部 SQL（元数据读写）的执行超时。
pub const TTL_INTERNAL_SQL_TIMEOUT: Duration = Duration::from_secs(30);
/// 按负载调整 scan/delete worker 数量的默认间隔。
pub const RESIZE_WORKERS_INTERVAL: Duration = Duration::from_secs(30);
/// 将一张物理表的扫描拆成的子任务份数下限。
pub const SPLIT_SCAN_COUNT: usize = 64;
/// 单个 TTL job 的最长存活时间（超时后视为失败并回收）。
pub const TTL_JOB_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);
/// Task Manager 主循环 ticker 默认间隔。
pub const TASK_MANAGER_LOOP_TICKER_INTERVAL: Duration = Duration::from_secs(60);
/// TTL task 心跳上报默认间隔，用于检测失联 worker。
pub const TTL_TASK_HEARTBEAT_TICKER_INTERVAL: Duration = Duration::from_secs(60);
/// 清理过期 TTL 元数据（GC）的默认间隔。
pub const TTL_GC_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// Go 版 `getJobManagerLoopSyncTimerInterval` 的默认值。
pub const JOB_MANAGER_SYNC_TIMER_INTERVAL: Duration = Duration::from_secs(1);
/// Go 版 `getTaskManagerLoopCheckTaskInterval` 的默认值。
pub const TASK_MANAGER_CHECK_TASK_INTERVAL: Duration = Duration::from_secs(5);
/// Go 版 `getCheckJobTriggeredInterval` 的默认值。
pub const CHECK_TRIGGERED_JOB_INTERVAL: Duration = Duration::from_secs(2);

/// 各调度间隔的可选覆盖值；`None` 表示使用对应模块常量默认值。
#[derive(Clone, Debug, Default)]
pub struct IntervalOverrides {
    /// 覆盖 Job Manager 检查 job 的间隔。
    pub check_job: Option<Duration>,
    /// 覆盖心跳间隔。
    pub heartbeat: Option<Duration>,
    /// 覆盖 timer 同步间隔。
    pub sync_timer: Option<Duration>,
    /// 覆盖 InfoSchema 缓存刷新间隔。
    pub update_info_schema: Option<Duration>,
    /// 覆盖 TTL 表状态缓存刷新间隔。
    pub update_table_status: Option<Duration>,
    /// 覆盖 worker 扩缩容间隔。
    pub resize_workers: Option<Duration>,
    /// 覆盖 Task Manager 检查 task 的间隔。
    pub check_task: Option<Duration>,
    /// 覆盖 Task Manager 主循环间隔。
    pub task_loop: Option<Duration>,
    /// 覆盖 task 心跳间隔。
    pub task_heartbeat: Option<Duration>,
    /// 覆盖检查已触发 job 的间隔。
    pub check_triggered_job: Option<Duration>,
    /// 覆盖 GC 间隔。
    pub gc: Option<Duration>,
}

impl IntervalOverrides {
    /// 返回检查 job 的有效间隔。
    pub fn check_job(&self) -> Duration {
        self.check_job.unwrap_or(JOB_MANAGER_LOOP_TICKER_INTERVAL)
    }
    /// 返回心跳的有效间隔。
    pub fn heartbeat(&self) -> Duration {
        // The Go implementation deliberately uses the job-manager ticker for
        // this failpoint-controlled interval, rather than the task heartbeat
        // ticker.  Keep that distinction visible in the Rust port.
        self.heartbeat.unwrap_or(JOB_MANAGER_LOOP_TICKER_INTERVAL)
    }
    /// 返回 timer 同步的有效间隔。
    pub fn sync_timer(&self) -> Duration {
        self.sync_timer.unwrap_or(JOB_MANAGER_SYNC_TIMER_INTERVAL)
    }
    /// 返回 InfoSchema 缓存刷新的有效间隔。
    pub fn update_info_schema(&self) -> Duration {
        self.update_info_schema
            .unwrap_or(UPDATE_INFO_SCHEMA_CACHE_INTERVAL)
    }
    /// 返回 TTL 表状态缓存刷新的有效间隔。
    pub fn update_table_status(&self) -> Duration {
        self.update_table_status
            .unwrap_or(UPDATE_TTL_TABLE_STATUS_CACHE_INTERVAL)
    }
    /// 返回 worker 扩缩容的有效间隔。
    pub fn resize_workers(&self) -> Duration {
        self.resize_workers.unwrap_or(RESIZE_WORKERS_INTERVAL)
    }
    /// 返回检查 task 的有效间隔。
    pub fn check_task(&self) -> Duration {
        self.check_task.unwrap_or(TASK_MANAGER_CHECK_TASK_INTERVAL)
    }
    /// 返回 Task Manager 主循环的有效间隔。
    pub fn task_loop(&self) -> Duration {
        self.task_loop.unwrap_or(TASK_MANAGER_LOOP_TICKER_INTERVAL)
    }
    /// 返回 task 心跳的有效间隔。
    pub fn task_heartbeat(&self) -> Duration {
        self.task_heartbeat
            .unwrap_or(TTL_TASK_HEARTBEAT_TICKER_INTERVAL)
    }
    /// 返回检查已触发 job 的有效间隔。
    pub fn check_triggered_job(&self) -> Duration {
        self.check_triggered_job
            .unwrap_or(CHECK_TRIGGERED_JOB_INTERVAL)
    }
    /// 返回 GC 的有效间隔。
    pub fn gc(&self) -> Duration {
        self.gc.unwrap_or(TTL_GC_INTERVAL)
    }
}

/// 计算扫描拆分数：TiKV 部署时至少为 store 数量，否则为 `SPLIT_SCAN_COUNT`。
pub fn scan_split_count(is_tikv: bool, tikv_store_count: usize) -> usize {
    if is_tikv {
        // TiKV 多 store 时提高并行度，避免拆分过粗导致热点。
        SPLIT_SCAN_COUNT.max(tikv_store_count)
    } else {
        SPLIT_SCAN_COUNT
    }
}
