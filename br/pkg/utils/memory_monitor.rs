// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Memory monitor ported from `br/pkg/utils/memory_monitor.go`.
//!
//! BR 进程内存告警配置与启动入口：写入全局内存上限并挂载 `ConfigProvider`。
//! 启动真实 alarm handle，并在 Context 取消时关闭退出通道。

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::thread;

use crate::stubs::context::Context;
use astersql_br_pkg_logutil::{Field, log};
use astersql_errors::SharedError;
use astersql_util_memory::tracker::ServerMemoryLimit;
use astersql_util_memoryusagealarm::{ConfigProvider, NewMemoryUsageAlarmHandle};

/// 未指定 dump 目录时的默认 profile 路径，对齐 Go `DefaultProfilesDir`。
pub const DefaultProfilesDir: &str = "/tmp/profiles";
/// 告警阈值相对进程内存上限的默认比例（0.8）。
const defaultMemoryUsageAlarmRatio: f64 = 0.8;
/// 保留历史告警记录条数默认值。
const defaultMemoryUsageAlarmKeepRecordNum: i64 = 3;

/// BR 侧 `ConfigProvider`：以 IEEE-754 位模式原子存储 ratio，等价于 Go `atomic.Float64`。
pub struct BRConfigProvider {
    ratio: AtomicU64,
    keepNum: AtomicI64,
    logDir: String,
}

impl BRConfigProvider {
    /// 以位模式保存 ratio，既支持无锁读取，也不会损失浮点精度。
    pub fn new(ratio: f64, keep_num: i64, log_dir: impl Into<String>) -> Self {
        Self {
            ratio: AtomicU64::new(ratio.to_bits()),
            keepNum: AtomicI64::new(keep_num),
            logDir: log_dir.into(),
        }
    }

    /// 运行期更新 dump 目录（例如 CLI 覆盖默认路径）。
    pub fn set_log_dir(&mut self, log_dir: impl Into<String>) {
        self.logDir = log_dir.into();
    }

    fn ratio_f64(&self) -> f64 {
        f64::from_bits(self.ratio.load(Ordering::Relaxed))
    }
}

impl ConfigProvider for BRConfigProvider {
    /// 告警触发比例；读路径无锁，供告警句柄高频轮询。
    fn GetMemoryUsageAlarmRatio(&self) -> f64 {
        self.ratio_f64()
    }

    /// 历史告警保留条数，限制磁盘上的 profile 数量。
    fn GetMemoryUsageAlarmKeepRecordNum(&self) -> i64 {
        self.keepNum.load(Ordering::Relaxed)
    }

    fn GetLogDir(&self) -> String {
        // 空字符串回退到 DefaultProfilesDir，与 Go getter 一致。
        if self.logDir.is_empty() {
            DefaultProfilesDir.to_string()
        } else {
            self.logDir.clone()
        }
    }

    /// 组件名写入告警元数据，便于多组件共用告警框架时区分来源。
    fn GetComponentName(&self) -> String {
        // 组件名固定为 br，供告警日志/指标打标签。
        "br".to_string()
    }
}

/// Starts memory limit wiring and the memory-usage alarm handle.
///
/// 空 dump_dir 回退默认目录；`memory_limit>0` 时写入 `ServerMemoryLimit`。
/// 后台运行告警句柄，并在上下文取消时关闭其退出通道。
pub fn RunMemoryMonitor(
    ctx: Context,
    dump_dir: impl Into<String>,
    memory_limit: u64,
) -> Result<(), SharedError> {
    let mut dump_dir = dump_dir.into();
    if dump_dir.is_empty() {
        dump_dir = DefaultProfilesDir.to_string();
    }

    // 与 Go 一致：仅正数上限才覆盖全局配置。
    if memory_limit > 0 {
        ServerMemoryLimit.Store(memory_limit);
    }

    let temp_dir = std::env::temp_dir();
    let provider = Arc::new(BRConfigProvider::new(
        defaultMemoryUsageAlarmRatio,
        defaultMemoryUsageAlarmKeepRecordNum,
        dump_dir.clone(),
    ));
    // 记录是否落在系统临时目录，便于排查权限/清理问题。
    log::L().Info(
        "Memory monitor starting",
        [
            Field::string("dump_dir", &dump_dir),
            Field::bool("using_temp_dir", Path::new(&dump_dir) == temp_dir.as_path()),
            Field::string(
                "memory_usage_alarm_ratio",
                &provider.GetMemoryUsageAlarmRatio().to_string(),
            ),
            Field::int("memory_limit_mb", (memory_limit / 1024 / 1024) as i64),
        ],
    );

    spawn_memory_alarm(ctx, provider);
    Ok(())
}

/// 启动告警循环，并在 Context 取消后关闭其退出通道。
/// 返回 join handle 仅供同 crate 测试验证资源收尾；生产入口保持 fire-and-forget。
pub(crate) fn spawn_memory_alarm(
    ctx: Context,
    provider: Arc<dyn ConfigProvider>,
) -> thread::JoinHandle<()> {
    let (exit_tx, exit_rx) = crossbeam_channel::bounded(1);
    let handle = NewMemoryUsageAlarmHandle(exit_rx, provider);
    thread::spawn(move || {
        let alarm_thread = thread::spawn(move || handle.Run());
        while !ctx.wait_cancelled_timeout(std::time::Duration::from_secs(60)) {}
        let _ = exit_tx.send(());
        let _ = alarm_thread.join();
    })
}

pub use astersql_util_memoryusagealarm::Handle as MemoryUsageAlarmHandle;
