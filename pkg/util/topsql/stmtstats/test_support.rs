// Copyright 2026 AsterSQL.

// stmtstats 单测辅助类型与工具函数。
//
// 提供可记录批次的 Collector / RUCollector mock、可切换的 RU 版本提供者，
// 以及构造/累加 RU 明细、重置 TopSQL/TopRU 全局开关的便捷函数。

#![allow(non_snake_case, non_upper_case_globals, static_mut_refs)]

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::{Arc, Mutex, RwLock};

pub use super::*;

/// 测试用语句统计收集器：把每次上报的 `StatementStatsMap` 存入批次列表。
#[derive(Clone, Default)]
pub struct StatementCollector {
    /// 按上报顺序保存的语句统计批次。
    pub batches: Arc<Mutex<Vec<StatementStatsMap>>>,
}

impl Collector for StatementCollector {
    fn CollectStmtStatsMap(&self, stats: StatementStatsMap) {
        self.batches
            .lock()
            .expect("statement batches lock poisoned")
            .push(stats);
    }
}

/// 测试用 RU 收集器：记录增量批次与版本变更通知。
#[derive(Clone, Default)]
pub struct TestRUCollector {
    /// `(增量映射, RU 版本)` 上报批次。
    pub batches: Arc<Mutex<Vec<(RUIncrementMap, RUVersion)>>>,
    /// `OnRUVersionChange` 收到的版本序列。
    pub changes: Arc<Mutex<Vec<RUVersion>>>,
}

impl RUCollector for TestRUCollector {
    fn CollectRUIncrements(&self, increments: RUIncrementMap, version: RUVersion) {
        self.batches
            .lock()
            .expect("RU batches lock poisoned")
            .push((increments, version));
    }

    fn OnRUVersionChange(&self, version: RUVersion) {
        self.changes
            .lock()
            .expect("RU changes lock poisoned")
            .push(version);
    }
}

/// 可原子切换当前 RU 版本的测试提供者。
pub struct TestRUVersionProvider(pub AtomicI32);

impl TestRUVersionProvider {
    /// 以给定初始版本构造。
    pub fn new(version: RUVersion) -> Self {
        Self(AtomicI32::new(version))
    }

    /// 运行时切换 RU 版本（SeqCst，保证测试可见性）。
    pub fn set(&self, version: RUVersion) {
        self.0.store(version, Ordering::SeqCst);
    }
}

impl RUVersionProvider for TestRUVersionProvider {
    fn GetRUVersion(&self) -> RUVersion {
        self.0.load(Ordering::SeqCst)
    }
}

/// 构造带读写/TiKV v2/TiFlash RU 初值的共享明细。
pub fn ru_details(
    read_ru: f64,
    write_ru: f64,
    tikv_ru_v2: f64,
    tiflash_ru: f64,
) -> SharedRUDetails {
    Arc::new(RwLock::new(execdetails::RUDetails {
        read_ru,
        write_ru,
        tikv_ru_v2,
        tiflash_ru,
        ..Default::default()
    }))
}

/// 向已有共享明细累加各分量 RU。
pub fn add_ru(
    details: &SharedRUDetails,
    read_ru: f64,
    write_ru: f64,
    tikv_ru_v2: f64,
    tiflash_ru: f64,
) {
    let mut details = details.write().expect("RU details lock poisoned");
    details.read_ru += read_ru;
    details.write_ru += write_ru;
    details.tikv_ru_v2 += tikv_ru_v2;
    details.tiflash_ru += tiflash_ru;
}

/// 关闭 TopSQL，并循环关闭 TopRU 直至全局开关全部关闭。
pub fn reset_top_state() {
    topsql_state::DisableTopSQL();
    // TopRU 可能有多层启用计数，需循环 Disable 直到真正关闭。
    while topsql_state::TopRUEnabled() {
        topsql_state::DisableTopRU();
    }
}
