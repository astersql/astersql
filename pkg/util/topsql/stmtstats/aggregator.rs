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

// 语句统计聚合器：后台周期合并 StatementStats，并向 Collector / RUCollector 推送。
//
// 先排空 RU 再处理语句统计，避免会话结束注销前丢失尾部 RU 增量。
// 单次聚合 RU key 数有上限，超出部分记入丢弃指标。对应 Go aggregator。

#![allow(non_snake_case)]

use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::reporter_metrics;
use crate::topsql_state;
use crate::{
    DefaultRUVersion, NormalizeRUVersion, RUIncrementMap, RUVersion, RUVersionProvider,
    StatementStats, StatementStatsMap, StatementStatsMapMerge,
};

/// 全局聚合器可注册的 StatementStats 上限。
const MAX_STMT_STATS_SIZE: usize = 1_000_000;
/// 单次 RU 聚合最多保留的 distinct key 数。
const MAX_RU_KEYS_PER_AGGREGATE: usize = 10_000;

/// 接收合并后的语句统计批次。
pub trait Collector: Send + Sync {
    /// 消费一份 `StatementStatsMap`。
    fn CollectStmtStatsMap(&self, stats: StatementStatsMap);
}

/// 接收 RU 增量及 RU 版本切换通知。
pub trait RUCollector: Send + Sync {
    /// 推送本轮 RU 增量与当前 RU 版本。
    fn CollectRUIncrements(&self, increments: RUIncrementMap, version: RUVersion);
    /// RU 版本切换时回调（如 V1→V2）。
    fn OnRUVersionChange(&self, version: RUVersion);
}

/// 聚合多个会话的语句/RU 统计，并分发给已注册收集器。
pub struct Aggregator {
    /// 可选的 RU 版本提供者。
    ru_version_provider: RwLock<Option<Arc<dyn RUVersionProvider>>>,
    /// 已注册的 StatementStats 集合。
    stats: Mutex<Vec<Arc<StatementStats>>>,
    /// 语句统计收集器。
    collectors: Mutex<Vec<Arc<dyn Collector>>>,
    /// RU 收集器。
    ru_collectors: Mutex<Vec<Arc<dyn RUCollector>>>,
    /// 后台 worker 是否在跑。
    running: AtomicBool,
    /// 上次观察到的 RU 版本（用于切换检测）。
    last_ru_version: AtomicI32,
    /// 关闭信号发送端。
    shutdown: Mutex<Option<Sender<()>>>,
    /// 聚合后台线程句柄。
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl Aggregator {
    /// 创建空聚合器（尚未启动 worker）。
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            ru_version_provider: RwLock::new(None),
            stats: Mutex::new(Vec::new()),
            collectors: Mutex::new(Vec::new()),
            ru_collectors: Mutex::new(Vec::new()),
            running: AtomicBool::new(false),
            last_ru_version: AtomicI32::new(DefaultRUVersion()),
            shutdown: Mutex::new(None),
            worker: Mutex::new(None),
        })
    }

    /// 绑定或清除 RU 版本提供者。
    pub fn set_ru_version_provider(&self, provider: Option<Arc<dyn RUVersionProvider>>) {
        *self
            .ru_version_provider
            .write()
            .expect("RU version provider lock poisoned") = provider;
    }

    /// 读取并归一化当前 RU 版本；无提供者时用默认版本。
    pub fn current_ru_version(&self) -> RUVersion {
        self.ru_version_provider
            .read()
            .expect("RU version provider lock poisoned")
            .as_ref()
            .map_or_else(DefaultRUVersion, |provider| {
                NormalizeRUVersion(provider.GetRUVersion())
            })
    }

    /// 启动 1s 周期聚合线程；已在运行则直接返回。
    pub fn start(self: &Arc<Self>) {
        if self
            .running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return;
        }
        self.last_ru_version
            .store(self.current_ru_version(), Ordering::SeqCst);
        let (shutdown_tx, shutdown_rx) = mpsc::channel();
        *self.shutdown.lock().expect("shutdown lock poisoned") = Some(shutdown_tx);
        let aggregator = Arc::clone(self);
        // 超时则聚合一轮；收到关闭或断开则退出。
        let worker = thread::spawn(move || {
            loop {
                match shutdown_rx.recv_timeout(Duration::from_secs(1)) {
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                    Err(RecvTimeoutError::Timeout) => aggregator.aggregate_all(),
                }
            }
        });
        *self.worker.lock().expect("worker lock poisoned") = Some(worker);
    }

    /// 停止 worker 并等待其退出；幂等。
    pub fn close(&self) {
        if !self.running.swap(false, Ordering::SeqCst) {
            return;
        }
        if let Some(shutdown) = self.shutdown.lock().expect("shutdown lock poisoned").take() {
            let _ = shutdown.send(());
        }
        if let Some(worker) = self.worker.lock().expect("worker lock poisoned").take() {
            worker.join().expect("aggregator worker panicked");
        }
    }

    /// 是否已关闭（未运行）。
    pub fn closed(&self) -> bool {
        !self.running.load(Ordering::SeqCst)
    }

    /// 注册 StatementStats；达到容量即拒绝。
    pub fn register(&self, stats: Arc<StatementStats>) {
        self.register_with_limit(stats, MAX_STMT_STATS_SIZE);
    }

    pub(crate) fn register_with_limit(&self, stats: Arc<StatementStats>, limit: usize) {
        let mut registered = self.stats.lock().expect("stats set lock poisoned");
        if registered.len() >= limit {
            return;
        }
        if !registered.iter().any(|item| Arc::ptr_eq(item, &stats)) {
            registered.push(stats);
        }
    }

    /// 按指针相等注销 StatementStats。
    pub fn unregister(&self, stats: &Arc<StatementStats>) {
        self.stats
            .lock()
            .expect("stats set lock poisoned")
            .retain(|item| !Arc::ptr_eq(item, stats));
    }

    /// 当前已注册 StatementStats 数量。
    pub fn stats_len(&self) -> usize {
        self.stats.lock().expect("stats set lock poisoned").len()
    }

    /// 注册语句统计收集器（去重）。
    pub fn register_collector(&self, collector: Arc<dyn Collector>) {
        let mut collectors = self.collectors.lock().expect("collector lock poisoned");
        if !collectors.iter().any(|item| Arc::ptr_eq(item, &collector)) {
            collectors.push(collector);
        }
    }

    /// 注销语句统计收集器。
    pub fn unregister_collector(&self, collector: &Arc<dyn Collector>) {
        self.collectors
            .lock()
            .expect("collector lock poisoned")
            .retain(|item| !Arc::ptr_eq(item, collector));
    }

    /// 注册 RU 收集器（去重）。
    pub fn register_ru_collector(&self, collector: Arc<dyn RUCollector>) {
        let mut collectors = self
            .ru_collectors
            .lock()
            .expect("RU collector lock poisoned");
        if !collectors.iter().any(|item| Arc::ptr_eq(item, &collector)) {
            collectors.push(collector);
        }
    }

    /// 注销 RU 收集器。
    pub fn unregister_ru_collector(&self, collector: &Arc<dyn RUCollector>) {
        self.ru_collectors
            .lock()
            .expect("RU collector lock poisoned")
            .retain(|item| !Arc::ptr_eq(item, collector));
    }

    /// 完整一轮：先 RU 再语句，保证 finished 会话尾部 RU 不被漏采。
    pub fn aggregate_all(&self) {
        // Go deliberately drains RU first so a finished session is not
        // unregistered before its tail RU increment is observed.
        self.drain_and_push_ru();
        self.drain_and_push_stmt_stats();
    }

    /// 合并并 Take 各 StatementStats，推送给 Collector；已 Finished 的顺带注销。
    pub fn drain_and_push_stmt_stats(&self) {
        let stats = self.stats.lock().expect("stats set lock poisoned").clone();
        let mut total = StatementStatsMap::new();
        for statement_stats in stats {
            if statement_stats.Finished() {
                self.unregister(&statement_stats);
            }
            total.Merge(statement_stats.Take());
        }
        if total.is_empty() || !topsql_state::TopSQLEnabled() {
            return;
        }
        let collectors = self
            .collectors
            .lock()
            .expect("collector lock poisoned")
            .clone();
        for collector in collectors {
            collector.CollectStmtStatsMap(total.clone());
        }
    }

    /// 检测 RU 版本切换或合并 RU 增量并推送；超限 key 计入丢弃指标。
    pub fn drain_and_push_ru(&self) {
        let current_version = self.current_ru_version();
        let last_version = self.last_ru_version.load(Ordering::SeqCst);
        let stats = self.stats.lock().expect("stats set lock poisoned").clone();
        // 版本切换：重置各会话 RU 状态并通知收集器，本轮不推增量。
        if current_version != last_version {
            for statement_stats in stats {
                statement_stats.ResetRUStateOnVersionChange(current_version);
            }
            let collectors = self
                .ru_collectors
                .lock()
                .expect("RU collector lock poisoned")
                .clone();
            for collector in collectors {
                collector.OnRUVersionChange(current_version);
            }
            self.last_ru_version
                .store(current_version, Ordering::SeqCst);
            return;
        }

        let mut total = RUIncrementMap::with_capacity(MAX_RU_KEYS_PER_AGGREGATE);
        let mut dropped_keys = 0_u64;
        let mut dropped_ru = 0.0;
        for statement_stats in stats {
            for (key, increment) in statement_stats.MergeRUInto() {
                if let Some(existing) = total.get_mut(&key) {
                    existing.Merge(&increment);
                } else if total.len() >= MAX_RU_KEYS_PER_AGGREGATE {
                    // 超出上限的新 key 丢弃并累计指标。
                    dropped_keys += 1;
                    dropped_ru += increment.TotalRU;
                } else {
                    total.insert(key, increment);
                }
            }
        }
        record_dropped_ru(dropped_keys, dropped_ru);

        if total.is_empty() || !topsql_state::TopRUEnabled() {
            return;
        }
        let collectors = self
            .ru_collectors
            .lock()
            .expect("RU collector lock poisoned")
            .clone();
        for collector in collectors {
            collector.CollectRUIncrements(total.clone(), current_version);
        }
    }
}

impl Drop for Aggregator {
    /// 析构时尽量发送关闭信号，避免 worker 悬挂。
    fn drop(&mut self) {
        if let Some(shutdown) = self
            .shutdown
            .get_mut()
            .expect("shutdown lock poisoned")
            .take()
        {
            let _ = shutdown.send(());
        }
    }
}

/// 将丢弃的 RU key/总量写入 reporter 指标（句柄未就绪时容忍）。
fn record_dropped_ru(dropped_keys: u64, dropped_ru: f64) {
    if dropped_keys == 0 {
        return;
    }
    // These handles are initialized by the reporter metrics module. Missing
    // handles are tolerated during early startup, as the data-path cap still applies.
    unsafe {
        if let Some(counter) = reporter_metrics::IgnoreExceedRUKeysCounter.as_ref() {
            counter.inc_by(dropped_keys as f64);
        }
        if let Some(counter) = reporter_metrics::IgnoreExceedRUTotalCounter.as_ref() {
            counter.inc_by(dropped_ru);
        }
    }
}

/// 进程级全局聚合器。
static GLOBAL_AGGREGATOR: LazyLock<Arc<Aggregator>> = LazyLock::new(Aggregator::new);

/// 取得全局聚合器引用。
pub fn global_aggregator() -> &'static Arc<Aggregator> {
    &GLOBAL_AGGREGATOR
}

/// 启动全局聚合器后台循环。
pub fn SetupAggregator() {
    global_aggregator().start();
}

/// 为全局聚合器绑定 RU 版本提供者。
pub fn BindRUVersionProvider(provider: Option<Arc<dyn RUVersionProvider>>) {
    global_aggregator().set_ru_version_provider(provider);
}

/// 关闭全局聚合器。
pub fn CloseAggregator() {
    global_aggregator().close();
}

/// 向全局聚合器注册语句收集器。
pub fn RegisterCollector(collector: Arc<dyn Collector>) {
    global_aggregator().register_collector(collector);
}

/// 从全局聚合器注销语句收集器。
pub fn UnregisterCollector(collector: &Arc<dyn Collector>) {
    global_aggregator().unregister_collector(collector);
}

/// 向全局聚合器注册 RU 收集器。
pub fn RegisterRUCollector(collector: Arc<dyn RUCollector>) {
    global_aggregator().register_ru_collector(collector);
}

/// 从全局聚合器注销 RU 收集器。
pub fn UnregisterRUCollector(collector: &Arc<dyn RUCollector>) {
    global_aggregator().unregister_ru_collector(collector);
}
