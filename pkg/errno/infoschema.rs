// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 错误 / 警告多维统计（infoschema）。
//
// 对应 Go `pkg/errno/infoschema.go`：按错误码在全局、用户、主机三个维度累计
// error / warning 次数及首次/末次出现时间，供 `INFORMATION_SCHEMA` 类视图或
// 诊断接口读取。统计存放在进程级 `OnceLock` + `Mutex` 中；对外查询接口返回
// 深拷贝快照，避免调用方持有锁或看到后续增量。

use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

/// ErrorSummary summarizes errors and warnings.
/// 单个错误码在某一维度上的汇总：错误次数、警告次数、首次/末次出现时间。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ErrorSummary {
    /// 错误累计次数。
    pub ErrorCount: i32,
    /// 警告累计次数。
    pub WarningCount: i32,
    /// 该错误码首次被记录的时间。
    pub FirstSeen: SystemTime,
    /// 该错误码最近一次被记录的时间。
    pub LastSeen: SystemTime,
}

impl Default for ErrorSummary {
    fn default() -> Self {
        Self {
            ErrorCount: 0,
            WarningCount: 0,
            FirstSeen: UNIX_EPOCH,
            LastSeen: UNIX_EPOCH,
        }
    }
}

/// Statistics for one TiDB server instance.
/// 单实例内存中的三维统计容器（全局 / 用户 / 主机）。
#[derive(Default)]
struct InstanceStatistics {
    /// 按错误码聚合的全局计数。
    global: HashMap<u16, Box<ErrorSummary>>,
    /// 用户名 → 错误码 → 汇总。
    users: HashMap<String, HashMap<u16, Box<ErrorSummary>>>,
    /// 主机名 → 错误码 → 汇总。
    hosts: HashMap<String, HashMap<u16, Box<ErrorSummary>>>,
}

/// 进程级统计单例。
fn statistics() -> &'static Mutex<InstanceStatistics> {
    static STATS: OnceLock<Mutex<InstanceStatistics>> = OnceLock::new();
    STATS.get_or_init(|| Mutex::new(InstanceStatistics::default()))
}

/// 加锁读取统计；毒化时恢复内层值以保证测试可继续。
fn lock_statistics() -> MutexGuard<'static, InstanceStatistics> {
    statistics()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Resets errors and warnings across global, user, and host dimensions.
/// 清空全局 / 用户 / 主机三个维度的全部计数。
pub fn FlushStats() {
    *lock_statistics() = InstanceStatistics::default();
}

/// 深拷贝错误码→汇总 map（Box 内 ErrorSummary 一并克隆）。
fn copyMap(oldMap: &HashMap<u16, Box<ErrorSummary>>) -> HashMap<u16, Box<ErrorSummary>> {
    oldMap.clone()
}

/// Returns a deep snapshot across all users and hosts.
/// 返回全局维度统计的深拷贝快照。
pub fn GlobalStats() -> HashMap<u16, Box<ErrorSummary>> {
    copyMap(&lock_statistics().global)
}

/// Returns a deep per-user snapshot.
/// 返回按用户维度的深拷贝快照。
pub fn UserStats() -> HashMap<String, HashMap<u16, Box<ErrorSummary>>> {
    lock_statistics().users.clone()
}

/// Returns a deep per-host snapshot.
/// 返回按主机维度的深拷贝快照。
pub fn HostStats() -> HashMap<String, HashMap<u16, Box<ErrorSummary>>> {
    lock_statistics().hosts.clone()
}

/// 以给定首次出现时间构造空计数汇总。
fn summary(first_seen: SystemTime) -> Box<ErrorSummary> {
    Box::new(ErrorSummary {
        FirstSeen: first_seen,
        ..ErrorSummary::default()
    })
}

/// 确保 global / user / host 三处均已为 `errCode` 建好计数槽。
fn initCounters(errCode: u16, user: &str, host: &str, seen: SystemTime) {
    let mut stats = lock_statistics();
    stats.global.entry(errCode).or_insert_with(|| summary(seen));
    stats
        .users
        .entry(user.to_owned())
        .or_default()
        .entry(errCode)
        .or_insert_with(|| summary(seen));
    stats
        .hosts
        .entry(host.to_owned())
        .or_default()
        .entry(errCode)
        .or_insert_with(|| summary(seen));
}

/// 使用给定时钟在三维统计上做一次增量。
///
/// 与 Go 一致，先采集用于 `LastSeen` 的时间，再由 `initCounters` 使用第二次
/// 时钟采样初始化 `FirstSeen`；两次更新分别持锁。
pub(crate) fn incrementWithClock(
    errCode: u16,
    user: &str,
    host: &str,
    warning: bool,
    mut now: impl FnMut() -> SystemTime,
) {
    let seen = now();
    initCounters(errCode, user, host, now());
    let mut stats = lock_statistics();

    /// 更新单条汇总：按 warning 标志加 ErrorCount 或 WarningCount。
    fn update(summary: &mut ErrorSummary, warning: bool, seen: SystemTime) {
        if warning {
            summary.WarningCount += 1;
        } else {
            summary.ErrorCount += 1;
        }
        summary.LastSeen = seen;
    }

    update(
        stats
            .global
            .get_mut(&errCode)
            .expect("global counter exists"),
        warning,
        seen,
    );
    update(
        stats
            .users
            .get_mut(user)
            .expect("user counter exists")
            .get_mut(&errCode)
            .expect("user error counter exists"),
        warning,
        seen,
    );
    update(
        stats
            .hosts
            .get_mut(host)
            .expect("host counter exists")
            .get_mut(&errCode)
            .expect("host error counter exists"),
        warning,
        seen,
    );
}

/// 在三维统计上对 `errCode` 做一次 error 或 warning 增量，并刷新 LastSeen。
fn increment(errCode: u16, user: &str, host: &str, warning: bool) {
    incrementWithClock(errCode, user, host, warning, SystemTime::now);
}

/// Increments global, user, and host error statistics for `errCode`.
/// 在三维统计上为 `errCode` 增加一次 error。
pub fn IncrementError(errCode: u16, user: impl AsRef<str>, host: impl AsRef<str>) {
    increment(errCode, user.as_ref(), host.as_ref(), false);
}

/// Increments global, user, and host warning statistics for `errCode`.
/// 在三维统计上为 `errCode` 增加一次 warning。
pub fn IncrementWarning(errCode: u16, user: impl AsRef<str>, host: impl AsRef<str>) {
    increment(errCode, user.as_ref(), host.as_ref(), true);
}
