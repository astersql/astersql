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

// TopSQL 测试用 mock 收集器。
//
// 提供可内存注册 SQL/Plan、聚合 CPU 时间、按 SQL 查询与等待 Collect 次数的
// `TopSQLCollector`，实现 `collector::Collector` 与 `stmtstats::Collector`，
// 便于集成测试在无真实 reporter 时断言行为。

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use logutil::log::{BgLogger, LogField, LogLevel};
use parser::digester_impl as parser;

/// mock 收集器内部可变状态：SQL/Plan 元数据与按 digest 聚合的 CPU 统计。
#[derive(Default)]
struct CollectorState {
    // Go strings can contain arbitrary bytes, so digest keys remain bytes in
    // Rust instead of using a lossy UTF-8 conversion.
    // Go 字符串可含任意字节，digest 键保留原始字节，避免有损 UTF-8 转换。
    sql_map: HashMap<Vec<u8>, String>,
    plan_map: HashMap<Vec<u8>, String>,
    sql_stats_map: HashMap<Vec<u8>, collector::SQLCPUTimeRecord>,
}

// TopSQLCollector uses for testing.
/// 测试用 TopSQL 收集器：线程安全地记录 SQL/Plan 与 CPU 统计。
pub struct TopSQLCollector {
    state: Mutex<CollectorState>,
    collect_cnt: AtomicI64,
}

// NewTopSQLCollector uses for testing.
/// 创建空的 mock TopSQLCollector（Arc 包装便于多处共享）。
pub fn NewTopSQLCollector() -> Arc<TopSQLCollector> {
    Arc::new(TopSQLCollector {
        state: Mutex::new(CollectorState::default()),
        collect_cnt: AtomicI64::new(0),
    })
}

impl TopSQLCollector {
    // Start implements TopSQLReporter interface.
    /// 空实现：满足 TopSQLReporter 接口，测试中无需真正启动采样。
    pub fn Start(&self) {}

    // Collect uses for testing.
    /// 按 SQLDigest‖PlanDigest 聚合 CPU 时间；空批次仍增加 CollectCnt。
    pub fn Collect(&self, stats: Vec<collector::SQLCPUTimeRecord>) {
        if stats.is_empty() {
            self.collect_cnt.fetch_add(1, Ordering::SeqCst);
            return;
        }

        {
            let mut state = self
                .state
                .lock()
                .expect("mock TopSQLCollector state lock poisoned");
            for stmt in stats {
                let hash = self.hash(&stmt);
                let accumulated = state.sql_stats_map.entry(hash).or_insert_with(|| {
                    collector::SQLCPUTimeRecord {
                        SQLDigest: stmt.SQLDigest.clone(),
                        PlanDigest: stmt.PlanDigest.clone(),
                        CPUTimeMs: 0,
                    }
                });
                // Go uint32 addition wraps on overflow.
                // 与 Go uint32 一致，溢出时回绕相加。
                accumulated.CPUTimeMs = accumulated.CPUTimeMs.wrapping_add(stmt.CPUTimeMs);

                let sql = state
                    .sql_map
                    .get(stmt.SQLDigest.as_slice())
                    .cloned()
                    .unwrap_or_default();
                let has_plan = state
                    .plan_map
                    .get(stmt.PlanDigest.as_slice())
                    .is_some_and(|plan| !plan.is_empty());
                BgLogger().log(
                    LogLevel::Info,
                    "mock top sql collector collected sql",
                    [
                        LogField::String("sql".to_owned(), sql),
                        LogField::Bool("has-plan".to_owned(), has_plan),
                    ],
                );
            }
        }

        // Match Go's defer order: release the map lock before incrementing.
        // 对齐 Go defer：先释放 map 锁再递增计数。
        self.collect_cnt.fetch_add(1, Ordering::SeqCst);
    }

    // BindProcessCPUTimeUpdater implements TopSQLReporter.
    /// 空实现：绑定进程 CPU 更新器接口占位。
    pub fn BindProcessCPUTimeUpdater(&self, _updater: Arc<dyn collector::ProcessCPUTimeUpdater>) {}

    // BindKeyspaceName implements TopSQLReporter.
    /// 空实现：绑定 keyspace（多租户命名空间）名称占位。
    pub fn BindKeyspaceName(&self, _keyspace_name: &[u8]) {}

    // CollectStmtStatsMap implements stmtstats.Collector.
    /// 空实现：语句级统计（执行次数、耗时等）收集占位。
    pub fn CollectStmtStatsMap(&self, _stats: stmtstats::StatementStatsMap) {}

    // GetSQLStatsBySQLWithRetry uses for testing.
    /// 轮询等待直至出现匹配 SQL 的统计，或超时返回空向量。
    pub fn GetSQLStatsBySQLWithRetry(
        &self,
        sql: &str,
        planIsNotNull: bool,
    ) -> Vec<collector::SQLCPUTimeRecord> {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if Instant::now() >= deadline {
                return Vec::new();
            }
            let stats = self.GetSQLStatsBySQL(sql, planIsNotNull);
            if !stats.is_empty() {
                return stats;
            }
            self.WaitCollectCnt(1);
        }
    }

    // GetSQLStatsBySQL uses for testing.
    /// 按规范化 SQL 的 digest 查找统计；`planIsNotNull` 时仅返回已注册非空 plan 的项。
    pub fn GetSQLStatsBySQL(
        &self,
        sql: &str,
        planIsNotNull: bool,
    ) -> Vec<collector::SQLCPUTimeRecord> {
        let sql_digest = GenSQLDigest(sql);
        let state = self
            .state
            .lock()
            .expect("mock TopSQLCollector state lock poisoned");
        let mut stats = Vec::with_capacity(2);
        for stmt in state.sql_stats_map.values() {
            if stmt.SQLDigest.as_slice() != sql_digest.Bytes() {
                continue;
            }
            if planIsNotNull {
                let has_plan = state
                    .plan_map
                    .get(stmt.PlanDigest.as_slice())
                    .is_some_and(|plan| !plan.is_empty());
                if has_plan {
                    stats.push(stmt.clone());
                }
            } else {
                stats.push(stmt.clone());
            }
        }
        stats
    }

    // GetSQLCPUTimeBySQL uses for testing.
    /// 汇总指定 SQL 在所有 plan 上的 CPU 毫秒数（溢出回绕）。
    pub fn GetSQLCPUTimeBySQL(&self, sql: &str) -> u32 {
        let sql_digest = GenSQLDigest(sql);
        let state = self
            .state
            .lock()
            .expect("mock TopSQLCollector state lock poisoned");
        state
            .sql_stats_map
            .values()
            .filter(|stmt| stmt.SQLDigest.as_slice() == sql_digest.Bytes())
            .fold(0_u32, |total, stmt| total.wrapping_add(stmt.CPUTimeMs))
    }

    // GetSQL uses for testing.
    /// 按 SQL digest 取已注册的规范化 SQL 文本；未注册返回空串。
    pub fn GetSQL(&self, sqlDigest: &[u8]) -> String {
        self.state
            .lock()
            .expect("mock TopSQLCollector state lock poisoned")
            .sql_map
            .get(sqlDigest)
            .cloned()
            .unwrap_or_default()
    }

    // GetPlan uses for testing.
    /// 按 plan digest 取已注册的规范化执行计划文本；未注册返回空串。
    pub fn GetPlan(&self, planDigest: &[u8]) -> String {
        self.state
            .lock()
            .expect("mock TopSQLCollector state lock poisoned")
            .plan_map
            .get(planDigest)
            .cloned()
            .unwrap_or_default()
    }

    // RegisterSQL uses for testing.
    /// 注册 SQL digest → 规范化文本；已存在则保留首次写入（与 Go 一致）。
    pub fn RegisterSQL(&self, sqlDigest: &[u8], normalizedSQL: String, _is_internal: bool) {
        self.state
            .lock()
            .expect("mock TopSQLCollector state lock poisoned")
            .sql_map
            .entry(sqlDigest.to_vec())
            .or_insert(normalizedSQL);
    }

    // RegisterPlan uses for testing.
    /// 注册 plan digest → 规范化计划；`isLarge` 为真时直接丢弃。
    pub fn RegisterPlan(&self, planDigest: &[u8], normalizedPlan: String, isLarge: bool) {
        if isLarge {
            return;
        }
        self.state
            .lock()
            .expect("mock TopSQLCollector state lock poisoned")
            .plan_map
            .entry(planDigest.to_vec())
            .or_insert(normalizedPlan);
    }

    // WaitCollectCnt uses for testing.
    /// 阻塞等待 CollectCnt 至少再增加 `count`，最长约 10 秒。
    pub fn WaitCollectCnt(&self, count: i64) {
        let deadline = Instant::now() + Duration::from_secs(10);
        let end = self.collect_cnt.load(Ordering::SeqCst).wrapping_add(count);
        loop {
            if self.collect_cnt.load(Ordering::SeqCst) >= end {
                return;
            }
            if Instant::now() >= deadline {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    // Reset cleans all collected data.
    /// 清空全部已收集的 SQL/Plan/统计，并将 CollectCnt 归零。
    pub fn Reset(&self) {
        let mut state = self
            .state
            .lock()
            .expect("mock TopSQLCollector state lock poisoned");
        *state = CollectorState::default();
        self.collect_cnt.store(0, Ordering::SeqCst);
    }

    // CollectCnt uses for testing.
    /// 返回已调用 Collect 的次数。
    pub fn CollectCnt(&self) -> i64 {
        self.collect_cnt.load(Ordering::SeqCst)
    }

    // Close implements the interface.
    /// 空实现：关闭接口占位。
    pub fn Close(&self) {}

    /// 生成与 Go `string(SQLDigest)+string(PlanDigest)` 等价的聚合键。
    fn hash(&self, stat: &collector::SQLCPUTimeRecord) -> Vec<u8> {
        let mut key = Vec::with_capacity(stat.SQLDigest.len() + stat.PlanDigest.len());
        key.extend_from_slice(&stat.SQLDigest);
        key.extend_from_slice(&stat.PlanDigest);
        key
    }
}

impl collector::Collector for TopSQLCollector {
    fn Collect(&self, stats: Vec<collector::SQLCPUTimeRecord>) {
        TopSQLCollector::Collect(self, stats);
    }
}

impl stmtstats::Collector for TopSQLCollector {
    fn CollectStmtStatsMap(&self, stats: stmtstats::StatementStatsMap) {
        TopSQLCollector::CollectStmtStatsMap(self, stats);
    }
}

// GenSQLDigest uses for testing.
/// 对 SQL 做规范化并返回 digest，与 parser::NormalizeDigest 一致。
pub fn GenSQLDigest(sql: &str) -> parser::Digest {
    let (_normalized_sql, digest) = parser::NormalizeDigest(sql);
    digest
}
