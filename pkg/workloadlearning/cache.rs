// Copyright 2025 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// 工作负载学习（Workload Learning）的表读代价缓存。
//
// 从 `WorkloadStore` 拉取按版本存储的 `TableReadCostMetrics`，
// 缓存在内存中供后续查询；仅当存储中的版本号更新时才刷新。

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::{TableReadCostMetrics, WorkloadStore};

/// 表读代价缓存快照：按表 ID 映射指标，并记录对应的存储版本号。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TableReadCostCache {
    /// 表 ID → 表读代价指标。
    pub TableReadCostMetrics: HashMap<i64, TableReadCostMetrics>,
    /// 该快照对应的存储版本；用于判断是否需要刷新。
    pub Version: u64,
}

/// 缓存工作器：持有 `WorkloadStore` 与受读写锁保护的表读代价缓存。
pub struct WLCacheWorker {
    store: Arc<dyn WorkloadStore>,
    tableReadCostCache: RwLock<TableReadCostCache>,
}

/// 构造空缓存的 `WLCacheWorker`。
pub fn NewWLCacheWorker(store: Arc<dyn WorkloadStore>) -> WLCacheWorker {
    WLCacheWorker {
        store,
        tableReadCostCache: RwLock::new(TableReadCostCache::default()),
    }
}

impl WLCacheWorker {
    /// 若存储版本新于本地缓存，则加载并替换缓存；返回是否实际发生了更新。
    pub fn UpdateTableReadCostCache(&self) -> Result<bool, String> {
        let latestVersionInStorage = self.store.latest_version()?;
        // 版本未前进则无需刷新。
        if latestVersionInStorage <= self.tableReadCostCache.read().unwrap().Version {
            return Ok(false);
        }
        let rows = self.store.load_metrics(latestVersionInStorage)?;
        let mut newMetrics = HashMap::new();
        // 反序列化 JSON 指标；解析失败的条目直接跳过。
        for (tableID, value) in rows {
            if let Ok(metric) = serde_json::from_str::<TableReadCostMetrics>(&value) {
                newMetrics.insert(tableID, metric);
            }
        }
        self.updateTableReadCostCacheWithMetrics(newMetrics, latestVersionInStorage);
        Ok(true)
    }

    /// 用给定指标与版本整体覆盖本地缓存。
    pub fn updateTableReadCostCacheWithMetrics(
        &self,
        newMetrics: HashMap<i64, TableReadCostMetrics>,
        latestVersionInStorage: u64,
    ) {
        *self.tableReadCostCache.write().unwrap() = TableReadCostCache {
            TableReadCostMetrics: newMetrics,
            Version: latestVersionInStorage,
        };
    }

    /// 按表 ID 查询缓存中的表读代价指标；未命中返回 `None`。
    pub fn GetTableReadCostMetrics(&self, tableID: i64) -> Option<TableReadCostMetrics> {
        self.tableReadCostCache
            .read()
            .unwrap()
            .TableReadCostMetrics
            .get(&tableID)
            .map(|metric| TableReadCostMetrics {
                TableScanTime: metric.TableScanTime,
                TableMemUsage: metric.TableMemUsage,
                ReadFrequency: metric.ReadFrequency,
                TableReadCost: metric.TableReadCost,
                ..TableReadCostMetrics::default()
            })
    }
}
