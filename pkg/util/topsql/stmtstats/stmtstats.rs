// Copyright 2021 PingCAP, Inc.
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

// 语句级统计（Statement Stats）与 TopRU 增量累计。
//
// 在语句开始/结束时按 SQL digest + Plan digest 聚合执行次数、耗时、网络字节与
// KV 目标计数；若启用 TopRU，则同步维护执行上下文并按 RU 协议版本采样增量。

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::{
    ExecutionContext, NormalizeRUVersion, RU_VERSION_V2, RUIncrement, RUIncrementMap, RUKey,
    RUVersion, SharedRUDetails, global_aggregator,
};

/// 有符号时长（纳秒），对齐 Go 的 `time.Duration` 语义。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SignedDuration(i64);

impl SignedDuration {
    /// 由纳秒构造。
    pub const fn from_nanos(nanos: i64) -> Self {
        Self(nanos)
    }

    /// 取纳秒值。
    pub const fn Nanoseconds(self) -> i64 {
        self.0
    }
}

/// 语句执行起止观察者：TopSQL/TopRU 通过该回调挂钩统计。
pub trait StatementObserver: Send + Sync {
    /// 语句开始时回调。
    fn OnExecutionBegin(&self, sql_digest: &[u8], plan_digest: &[u8], info: Option<&ExecBeginInfo>);

    /// 语句结束时回调。
    fn OnExecutionFinished(
        &self,
        sql_digest: &[u8],
        plan_digest: &[u8],
        info: Option<&ExecFinishInfo>,
    );
}

/// 语句开始时传入的上下文快照（RU、用户、入站流量、版本等）。
#[derive(Default)]
pub struct ExecBeginInfo {
    /// Rust callers pass the RUDetails handle directly; this is the value cached
    /// from Go's context at statement begin.
    /// 语句开始时缓存的 RU 明细句柄（对应 Go context 中的值）。
    pub RUDetails: Option<SharedRUDetails>,
    /// 执行用户。
    pub User: String,
    /// 入站网络字节数。
    pub InNetworkBytes: u64,
    /// RU 协议版本。
    pub RUVersion: RUVersion,
    /// 是否启用 TopRU 采样。
    pub TopRUEnabled: bool,
}

/// 语句结束时传入的结果快照（出站流量、耗时等）。
#[derive(Default)]
pub struct ExecFinishInfo {
    /// 结束时的 RU 明细句柄。
    pub RUDetails: Option<SharedRUDetails>,
    /// 语句结束时已经计算完成的 RU v2 总量。
    pub TotalRUV2: f64,
    /// 执行用户。
    pub User: String,
    /// 出站网络字节数。
    pub OutNetworkBytes: u64,
    /// 执行耗时。
    pub ExecDuration: SignedDuration,
    /// 是否启用 TopRU 采样。
    pub TopRUEnabled: bool,
}

/// `StatementStats` 的内部可变状态。
#[derive(Default)]
struct StatementStatsInner {
    /// SQL+Plan → 语句统计项。
    data: StatementStatsMap,
    /// 已完成语句的 RU 增量缓冲。
    finished_ru_buffer: RUIncrementMap,
    /// 当前活跃语句的 RU 执行上下文（未结束时保留以便采样 delta）。
    exec_ctx: Option<ExecutionContext>,
}

/// 线程安全的语句统计容器；可注册到全局聚合器。
pub struct StatementStats {
    /// 受互斥锁保护的内部状态。
    inner: Mutex<StatementStatsInner>,
    /// 是否已标记完成（不再参与聚合）。
    finished: AtomicBool,
}

impl Default for StatementStats {
    fn default() -> Self {
        Self::new()
    }
}

impl StatementStats {
    /// 创建空的语句统计实例。
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(StatementStatsInner::default()),
            finished: AtomicBool::new(false),
        }
    }

    /// 语句开始：累加执行次数与入站流量；TopRU 开启时建立 RU 执行上下文。
    pub fn OnExecutionBegin(
        &self,
        sql_digest: &[u8],
        plan_digest: &[u8],
        info: Option<&ExecBeginInfo>,
    ) {
        let mut inner = self.inner.lock().expect("StatementStats mutex poisoned");
        {
            let item = inner.get_or_create_statement_stats_item(sql_digest, plan_digest);
            item.ExecCount += 1;
            if let Some(info) = info {
                item.NetworkInBytes += info.InNetworkBytes;
            }
        }
        // 仅在 TopRU 开启时挂接 RU 采样上下文。
        if let Some(info) = info.filter(|info| info.TopRUEnabled) {
            inner.add_ru_on_begin(info, sql_digest, plan_digest);
        }
    }

    /// 语句结束：累加耗时与出站流量；TopRU 开启时结算 RU 增量。
    pub fn OnExecutionFinished(
        &self,
        sql_digest: &[u8],
        plan_digest: &[u8],
        info: Option<&ExecFinishInfo>,
    ) {
        let Some(info) = info else {
            return;
        };
        let nanos = info.ExecDuration.Nanoseconds();
        let mut inner = self.inner.lock().expect("StatementStats mutex poisoned");
        // 负时长视为无效，清空 RU 上下文后直接返回。
        if nanos < 0 {
            inner.exec_ctx = None;
            return;
        }
        {
            let item = inner.get_or_create_statement_stats_item(sql_digest, plan_digest);
            item.SumDurationNs += nanos as u64;
            item.DurationCount += 1;
            item.NetworkOutBytes += info.OutNetworkBytes;
        }
        if info.TopRUEnabled {
            inner.add_ru_on_finish(info, sql_digest, plan_digest);
        } else {
            inner.exec_ctx = None;
        }
    }

    /// 获取或创建指定 SQL/Plan 的统计项，并以守卫形式持有锁。
    pub fn GetOrCreateStatementStatsItem(
        &self,
        sql_digest: &[u8],
        plan_digest: &[u8],
    ) -> StatementStatsItemGuard<'_> {
        let key = SQLPlanDigest::new(sql_digest, plan_digest);
        let mut inner = self.inner.lock().expect("StatementStats mutex poisoned");
        inner
            .data
            .entry(key.clone())
            .or_insert_with(NewStatementStatsItem);
        StatementStatsItemGuard { inner, key }
    }

    /// 向指定语句的 KV 目标执行计数累加。
    pub(crate) fn add_kv_exec_count(
        &self,
        sql_digest: &[u8],
        plan_digest: &[u8],
        target: &str,
        count: u64,
    ) {
        let mut inner = self.inner.lock().expect("StatementStats mutex poisoned");
        *inner
            .get_or_create_statement_stats_item(sql_digest, plan_digest)
            .KvStatsItem
            .KvExecCount
            .get_or_insert_with(HashMap::new)
            .entry(target.to_owned())
            .or_insert(0) += count;
    }

    /// 取出并清空当前语句统计映射（供聚合器上报）。
    pub fn Take(&self) -> StatementStatsMap {
        std::mem::take(
            &mut self
                .inner
                .lock()
                .expect("StatementStats mutex poisoned")
                .data,
        )
    }

    /// 标记本实例已完成，聚合器可停止收集。
    pub fn SetFinished(&self) {
        self.finished.store(true, Ordering::SeqCst);
    }

    /// 是否已标记完成。
    pub fn Finished(&self) -> bool {
        self.finished.load(Ordering::SeqCst)
    }

    /// 取出已完成 RU 缓冲，并采样当前活跃语句的 RU delta。
    pub fn MergeRUInto(&self) -> RUIncrementMap {
        let mut inner = self.inner.lock().expect("StatementStats mutex poisoned");
        let result = std::mem::take(&mut inner.finished_ru_buffer);
        inner.sample_active_ru_delta(result)
    }

    /// RU 协议版本变更时：清空已完成缓冲；若活跃上下文版本不一致则丢弃。
    pub fn ResetRUStateOnVersionChange(&self, current_ru_version: RUVersion) {
        let mut inner = self.inner.lock().expect("StatementStats mutex poisoned");
        inner.finished_ru_buffer.clear();
        let should_clear = inner.exec_ctx.as_ref().is_some_and(|ctx| {
            NormalizeRUVersion(ctx.RUVersion) != NormalizeRUVersion(current_ru_version)
        });
        if should_clear {
            inner.exec_ctx = None;
        }
    }

    /// 强制清空当前 RU 执行上下文。
    pub fn ClearRUExecContext(&self) {
        self.inner
            .lock()
            .expect("StatementStats mutex poisoned")
            .exec_ctx = None;
    }
}

impl StatementObserver for StatementStats {
    fn OnExecutionBegin(
        &self,
        sql_digest: &[u8],
        plan_digest: &[u8],
        info: Option<&ExecBeginInfo>,
    ) {
        StatementStats::OnExecutionBegin(self, sql_digest, plan_digest, info);
    }

    fn OnExecutionFinished(
        &self,
        sql_digest: &[u8],
        plan_digest: &[u8],
        info: Option<&ExecFinishInfo>,
    ) {
        StatementStats::OnExecutionFinished(self, sql_digest, plan_digest, info);
    }
}

impl StatementStatsInner {
    /// 按 SQL/Plan digest 获取或新建统计项。
    fn get_or_create_statement_stats_item(
        &mut self,
        sql_digest: &[u8],
        plan_digest: &[u8],
    ) -> &mut StatementStatsItem {
        self.data
            .entry(SQLPlanDigest::new(sql_digest, plan_digest))
            .or_insert_with(NewStatementStatsItem)
    }

    /// 按 RUKey 获取或新建已完成增量槽位。
    fn get_or_create_ru_increment(&mut self, key: RUKey) -> &mut RUIncrement {
        self.finished_ru_buffer.entry(key).or_default()
    }

    /// 语句开始时建立 RU 执行上下文，并累加 ExecCount。
    fn add_ru_on_begin(&mut self, info: &ExecBeginInfo, sql_digest: &[u8], plan_digest: &[u8]) {
        let key = RUKey::new(info.User.clone(), sql_digest, plan_digest);
        self.exec_ctx = Some(ExecutionContext {
            RUDetails: info.RUDetails.clone(),
            Key: key.clone(),
            LastRUTotal: 0.0,
            RUVersion: NormalizeRUVersion(info.RUVersion),
        });
        self.get_or_create_ru_increment(key).ExecCount += 1;
    }

    /// 语句结束时按 delta 结算 RU；键不匹配则忽略。
    fn add_ru_on_finish(&mut self, info: &ExecFinishInfo, sql_digest: &[u8], plan_digest: &[u8]) {
        let key = RUKey::new(info.User.clone(), sql_digest, plan_digest);
        let Some(exec_ctx) = self.exec_ctx.as_ref() else {
            return;
        };
        // begin/finish 的聚合键必须一致，否则丢弃本次结算。
        if exec_ctx.Key != key {
            return;
        }

        let current_total = if NormalizeRUVersion(exec_ctx.RUVersion) == RU_VERSION_V2 {
            info.TotalRUV2
        } else {
            current_ru_total(exec_ctx, info.RUDetails.as_ref())
        };
        let last_total = exec_ctx.LastRUTotal;
        if current_total > 0.0 {
            let delta = current_total - last_total;
            if delta > 0.0 {
                let increment = self.get_or_create_ru_increment(key);
                increment.TotalRU += delta;
                increment.ExecDuration += info.ExecDuration.Nanoseconds() as u64;
            }
        }
        self.exec_ctx = None;
    }

    /// 对仍在执行的语句采样 RU delta，并推进 LastRUTotal。
    fn sample_active_ru_delta(&mut self, mut result: RUIncrementMap) -> RUIncrementMap {
        let Some(exec_ctx) = self.exec_ctx.as_ref() else {
            return result;
        };
        let current_total = current_ru_total(exec_ctx, exec_ctx.RUDetails.as_ref());
        let delta = current_total - exec_ctx.LastRUTotal;
        let key = exec_ctx.Key.clone();
        if delta > 0.0 {
            result.entry(key).or_default().TotalRU += delta;
        }
        // 推进水位，避免下次采样重复计入同一段 RU。
        self.exec_ctx
            .as_mut()
            .expect("active execution context")
            .LastRUTotal = current_total;
        result
    }
}

/// 在执行期间只采样 v1 的 RRU+WRU；v2 总量在语句结束时提供。
fn current_ru_total(exec_ctx: &ExecutionContext, ru_details: Option<&SharedRUDetails>) -> f64 {
    let details = ru_details.map(|details| details.read().expect("RUDetails lock poisoned"));
    if NormalizeRUVersion(exec_ctx.RUVersion) == RU_VERSION_V2 {
        return 0.0;
    }
    details
        .as_ref()
        .map_or(0.0, |details| details.RRU() + details.WRU())
}

/// 创建语句统计并注册到全局聚合器。
pub fn CreateStatementStats() -> Arc<StatementStats> {
    let stats = Arc::new(StatementStats::new());
    global_aggregator().register(stats.clone());
    stats
}

/// 持有内部锁的统计项守卫，Deref 到对应 `StatementStatsItem`。
pub struct StatementStatsItemGuard<'a> {
    inner: MutexGuard<'a, StatementStatsInner>,
    key: SQLPlanDigest,
}

impl Deref for StatementStatsItemGuard<'_> {
    type Target = StatementStatsItem;

    fn deref(&self) -> &Self::Target {
        self.inner
            .data
            .get(&self.key)
            .expect("statement stats item must exist while guard is held")
    }
}

impl DerefMut for StatementStatsItemGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.inner
            .data
            .get_mut(&self.key)
            .expect("statement stats item must exist while guard is held")
    }
}

/// 二进制 digest 包装（规范化 SQL/计划的哈希字节）。
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct BinaryDigest(pub Vec<u8>);

impl BinaryDigest {
    /// 以字节切片视图访问。
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl From<&[u8]> for BinaryDigest {
    fn from(value: &[u8]) -> Self {
        Self(value.to_vec())
    }
}

/// SQL digest 与 Plan digest 组成的聚合键。
#[derive(Clone, Debug, Default, Eq, Hash, PartialEq)]
pub struct SQLPlanDigest {
    /// 规范化 SQL 的 digest。
    pub SQLDigest: BinaryDigest,
    /// 执行计划的 digest。
    pub PlanDigest: BinaryDigest,
}

impl SQLPlanDigest {
    /// 由原始 digest 字节构造。
    pub fn new(sql_digest: &[u8], plan_digest: &[u8]) -> Self {
        Self {
            SQLDigest: BinaryDigest::from(sql_digest),
            PlanDigest: BinaryDigest::from(plan_digest),
        }
    }
}

/// 语句统计映射：SQLPlanDigest → StatementStatsItem。
pub type StatementStatsMap = HashMap<SQLPlanDigest, StatementStatsItem>;

/// 为 `StatementStatsMap` 提供按键合并。
pub trait StatementStatsMapMerge {
    /// 合并另一张表：同键字段累加，异键插入。
    fn Merge(&mut self, other: StatementStatsMap);
}

impl StatementStatsMapMerge for StatementStatsMap {
    fn Merge(&mut self, other: StatementStatsMap) {
        for (digest, new_item) in other {
            if let Some(item) = self.get_mut(&digest) {
                item.Merge(Some(&new_item));
            } else {
                self.insert(digest, new_item);
            }
        }
    }
}

/// 单条语句聚合统计：执行次数、耗时、网络与 KV 侧计数。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatementStatsItem {
    /// KV 层目标执行计数。
    pub KvStatsItem: KvStatementStatsItem,
    /// 执行次数。
    pub ExecCount: u64,
    /// 耗时总和（纳秒）。
    pub SumDurationNs: u64,
    /// 计入耗时的次数。
    pub DurationCount: u64,
    /// 入站网络字节。
    pub NetworkInBytes: u64,
    /// 出站网络字节。
    pub NetworkOutBytes: u64,
}

/// 构造带空 KV 计数字典的统计项。
pub fn NewStatementStatsItem() -> StatementStatsItem {
    StatementStatsItem {
        KvStatsItem: NewKvStatementStatsItem(),
        ..StatementStatsItem::default()
    }
}

impl StatementStatsItem {
    /// 合并另一统计项；`None` 为 no-op。
    pub fn Merge(&mut self, other: Option<&StatementStatsItem>) {
        let Some(other) = other else {
            return;
        };
        self.ExecCount += other.ExecCount;
        self.SumDurationNs += other.SumDurationNs;
        self.DurationCount += other.DurationCount;
        self.NetworkInBytes += other.NetworkInBytes;
        self.NetworkOutBytes += other.NetworkOutBytes;
        self.KvStatsItem.Merge(other.KvStatsItem.clone());
    }
}

/// KV 语句统计：按 store/地址目标累计执行次数。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KvStatementStatsItem {
    /// 目标地址 → 执行次数；`None` 表示尚未初始化。
    pub KvExecCount: Option<HashMap<String, u64>>,
}

/// 构造已初始化空 `KvExecCount` 的 KV 统计项。
pub fn NewKvStatementStatsItem() -> KvStatementStatsItem {
    KvStatementStatsItem {
        KvExecCount: Some(HashMap::new()),
    }
}

impl KvStatementStatsItem {
    /// 合并 KV 计数；自身为 `None` 时直接接管对方映射。
    pub fn Merge(&mut self, other: KvStatementStatsItem) {
        if self.KvExecCount.is_none() {
            self.KvExecCount = other.KvExecCount;
            return;
        }
        if let Some(other_counts) = other.KvExecCount {
            let counts = self.KvExecCount.get_or_insert_with(HashMap::new);
            for (target, count) in other_counts {
                *counts.entry(target).or_insert(0) += count;
            }
        }
    }
}
