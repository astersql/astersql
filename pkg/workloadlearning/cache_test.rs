// Copyright 2025 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// 表读代价缓存（`WLCacheWorker`）的单元测试。
//
// 使用内存版 `MemoryStore` 模拟持久化存储，验证缓存刷新与按表 ID 查询。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::*;

/// 内存中的 `WorkloadStore` 替身，供缓存与 handle 相关测试复用。
#[derive(Default)]
pub(crate) struct MemoryStore {
    /// 当前最新指标版本号。
    pub latest: Mutex<u64>,
    /// 版本 → (表 ID, JSON 指标) 列表。
    pub metrics: Mutex<HashMap<u64, Vec<(i64, String)>>>,
    /// (库名, 表名) → 表 ID。
    pub tables: Mutex<HashMap<(String, String), i64>>,
    /// 语句统计记录。
    pub statements: Mutex<Vec<StatementRecord>>,
}

impl WorkloadStore for MemoryStore {
    fn latest_version(&self) -> Result<u64, String> {
        Ok(*self.latest.lock().unwrap())
    }
    fn load_metrics(&self, version: u64) -> Result<Vec<(i64, String)>, String> {
        Ok(self
            .metrics
            .lock()
            .unwrap()
            .get(&version)
            .cloned()
            .unwrap_or_default())
    }
    fn save_metrics(&self, version: u64, rows: &[(i64, String)]) -> Result<(), String> {
        self.metrics
            .lock()
            .unwrap()
            .entry(version)
            .or_default()
            .extend_from_slice(rows);
        // 写入后推进 latest 版本。
        *self.latest.lock().unwrap() = version;
        Ok(())
    }
    fn closest_snapshot_id(&self, _: SystemTime) -> Result<u64, String> {
        Ok(1)
    }
    fn load_statements(&self, _: u64, _: u64) -> Result<Vec<StatementRecord>, String> {
        Ok(self.statements.lock().unwrap().clone())
    }
    fn table_id(&self, database: &str, table: &str) -> Result<i64, String> {
        self.tables
            .lock()
            .unwrap()
            .get(&(database.into(), table.into()))
            .copied()
            .ok_or("table not found".into())
    }
}

/// 先经 Handle 落盘指标，再验证缓存首次刷新成功、再次刷新因版本未变而返回 false。
#[test]
fn TestUpdateTableCostCache() {
    let store = Arc::new(MemoryStore::default());
    let store_trait: Arc<dyn WorkloadStore> = store.clone();
    let handle = NewWorkloadLearningHandle(store_trait.clone());
    let expected = TableReadCostMetrics {
        DbName: CIStr::new("test"),
        TableName: CIStr::new("test"),
        TableScanTime: std::time::Duration::from_nanos(10),
        TableMemUsage: 10,
        ReadFrequency: 10,
        TableReadCost: 10.0,
    };
    handle
        .SaveTableReadCostMetrics(
            &HashMap::from([(42, expected.clone())]),
            SystemTime::now(),
            SystemTime::now(),
        )
        .unwrap();
    let worker = NewWLCacheWorker(store_trait);
    assert!(worker.UpdateTableReadCostCache().unwrap());
    let metric = worker.GetTableReadCostMetrics(42).unwrap();
    assert_eq!(metric.TableScanTime, expected.TableScanTime);
    assert_eq!(metric.TableMemUsage, expected.TableMemUsage);
    assert_eq!(metric.ReadFrequency, expected.ReadFrequency);
    assert_eq!(metric.TableReadCost, expected.TableReadCost);
    // 版本未前进，第二次更新应返回 false。
    assert!(!worker.UpdateTableReadCostCache().unwrap());
}

/// 空存储时按表 ID 查询应得到 `None`。
#[test]
fn TestGetTableReadCacheMetricsWithNoData() {
    let store: Arc<dyn WorkloadStore> = Arc::new(MemoryStore::default());
    assert_eq!(NewWLCacheWorker(store).GetTableReadCostMetrics(1), None);
}

/// Go 版本只返回成本字段的深拷贝，不暴露缓存中用于落盘的库名和表名。
#[test]
fn get_table_read_cost_metrics_matches_go_projection() {
    let store: Arc<dyn WorkloadStore> = Arc::new(MemoryStore::default());
    let worker = NewWLCacheWorker(store);
    worker.updateTableReadCostCacheWithMetrics(
        HashMap::from([(
            42,
            TableReadCostMetrics {
                DbName: CIStr::new("test"),
                TableName: CIStr::new("t"),
                TableScanTime: std::time::Duration::from_nanos(10),
                TableMemUsage: 20,
                ReadFrequency: 30,
                TableReadCost: 1.5,
            },
        )]),
        1,
    );

    let metric = worker.GetTableReadCostMetrics(42).unwrap();
    assert_eq!(metric.DbName, CIStr::default());
    assert_eq!(metric.TableName, CIStr::default());
    assert_eq!(metric.TableScanTime, std::time::Duration::from_nanos(10));
    assert_eq!(metric.TableMemUsage, 20);
    assert_eq!(metric.ReadFrequency, 30);
    assert_eq!(metric.TableReadCost, 1.5);
}
