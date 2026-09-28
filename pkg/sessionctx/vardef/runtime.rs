// Copyright 2025 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 运行时租约（lease）与 NextGen 只读变量判定。
//
// Schema lease：DDL 等待 schema 变更生效的默认租约；Stats lease：统计信息表重载周期；
// PlanReplayer GC lease：执行计划回放器垃圾回收周期。均以原子变量保存纳秒，对齐 Go 包级状态。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use crate::{
    TiDBDDLDiskQuota, TiDBDDLEnableFastReorg, TiDBDDLReorgMaxWriteSpeed, TiDBEnableDistTask,
    TiDBEnableMDL, TiDBMaxDistTaskNodes,
};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

// 这些全局租约对应 Go 包级状态；使用原子变量保留并发读写语义。
static SCHEMA_LEASE: AtomicI64 = AtomicI64::new(1_000_000_000);
static STATS_LEASE: AtomicI64 = AtomicI64::new(3_000_000_000);
static PLAN_REPLAYER_GC_LEASE: AtomicI64 = AtomicI64::new(600_000_000_000);

// SetSchemaLease 修改 DDL 默认 schema lease；Go 明确警告该函数危险，不能随意调用。
/// 设置 DDL 默认 schema lease（危险：影响全局 schema 变更等待时间）。
///
/// Schema lease 是 DDL Owner 与其它实例同步表结构版本的时间窗口。
pub fn SetSchemaLease(lease: Duration) {
    SCHEMA_LEASE.store(lease.as_nanos() as i64, Ordering::SeqCst);
}

// GetSchemaLease 以原子读取返回 schema lease。
/// 原子读取当前 schema lease。
pub fn GetSchemaLease() -> Duration {
    Duration::from_nanos(SCHEMA_LEASE.load(Ordering::SeqCst) as u64)
}

// SetStatsLease/ GetStatsLease 对应 stats 表重载周期的原子 setter/getter。
/// 设置统计信息（stats）表重载周期。
pub fn SetStatsLease(lease: Duration) {
    STATS_LEASE.store(lease.as_nanos() as i64, Ordering::SeqCst);
}
/// 原子读取当前 stats lease。
pub fn GetStatsLease() -> Duration {
    Duration::from_nanos(STATS_LEASE.load(Ordering::SeqCst) as u64)
}

// SetPlanReplayerGCLease/ GetPlanReplayerGCLease 保留 plan replayer GC 周期。
/// 设置 plan replayer（执行计划回放）GC 周期。
pub fn SetPlanReplayerGCLease(lease: Duration) {
    PLAN_REPLAYER_GC_LEASE.store(lease.as_nanos() as i64, Ordering::SeqCst);
}
/// 原子读取当前 plan replayer GC lease。
pub fn GetPlanReplayerGCLease() -> Duration {
    Duration::from_nanos(PLAN_REPLAYER_GC_LEASE.load(Ordering::SeqCst) as u64)
}

// IsReadOnlyVarInNextGen 对变量名做不区分大小写的判断；Go 的 strings.ToLower 与 switch 语义在此显式保留。
/// 判断变量名在 NextGen 内核中是否只读（不区分大小写）。
///
/// NextGen 下部分 DDL/分布式任务相关变量强制只读，防止破坏新一代部署约束。
pub fn IsReadOnlyVarInNextGen(name: &str) -> bool {
    // 显式保留 Go strings.ToLower + switch 的匹配语义。
    match name.to_lowercase().as_str() {
        TiDBEnableMDL
        | TiDBMaxDistTaskNodes
        | TiDBDDLReorgMaxWriteSpeed
        | TiDBDDLDiskQuota
        | TiDBEnableDistTask
        | TiDBDDLEnableFastReorg => true,
        _ => false,
    }
}
