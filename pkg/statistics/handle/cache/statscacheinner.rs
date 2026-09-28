// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.

use crate::{CacheError, StatisticsTable};
use cache_internal::StatsCacheInner;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Go StatsCache: select LFU or map storage, retaining the lifetime version watermark.
pub struct StatsCache {
    inner: Mutex<Box<dyn StatsCacheInner>>,
    maxTblStatsVer: AtomicU64,
}

pub fn NewStatsCache() -> Result<StatsCache, CacheError> {
    NewStatsCacheWithCapacity(
        config::get_global_config()
            .performance
            .enable_stats_cache_mem_quota,
        vardef::StatsCacheMemQuota.Load(),
    )
}

pub fn NewStatsCacheWithCapacity(quota: bool, capacity: i64) -> Result<StatsCache, CacheError> {
    let inner: Box<dyn StatsCacheInner> = if quota {
        Box::new(cache_lfu::NewLFU(capacity).map_err(|e| CacheError(e.to_string()))?)
    } else {
        Box::new(cache_map::NewMapCache())
    };
    Ok(StatsCache {
        inner: Mutex::new(inner),
        maxTblStatsVer: AtomicU64::new(0),
    })
}

impl StatsCache {
    pub fn Len(&self) -> usize {
        self.inner.lock().unwrap().Len()
    }
    pub fn Get(&self, id: i64) -> (Option<Arc<StatisticsTable>>, bool) {
        let table = self.inner.lock().unwrap().Get(id);
        count(if table.is_some() { "hit" } else { "miss" });
        let found = table.is_some();
        (table, found)
    }
    pub fn Put(&self, id: i64, table: Arc<StatisticsTable>) {
        let mut attempts = 1;
        loop {
            count("update");
            if self.inner.lock().unwrap().Put(id, table.clone()) {
                self.maxTblStatsVer
                    .fetch_max(table.Version, Ordering::AcqRel);
                return;
            }
            log::warn!("fail to put the stats cache: id={id}");
            if attempts % 10 == 0 {
                log::warn!("fail to put the stats cache: id={id}");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
            attempts += 1;
        }
    }
    pub fn Values(&self) -> Vec<Arc<StatisticsTable>> {
        self.inner.lock().unwrap().Values()
    }
    pub fn Cost(&self) -> i64 {
        self.inner.lock().unwrap().Cost()
    }
    pub fn SetCapacity(&self, capacity: i64) {
        self.inner.lock().unwrap().SetCapacity(capacity);
    }
    pub fn Close(&self) {
        self.inner.lock().unwrap().Close();
    }
    pub fn Version(&self) -> u64 {
        self.maxTblStatsVer.load(Ordering::Acquire)
    }
    pub fn CopyAndUpdate(&self, tables: &[Arc<StatisticsTable>], deleted: &[i64]) -> StatsCache {
        let mut inner = self.inner.lock().unwrap().Copy();
        let mut version = self.Version();
        for table in tables {
            inner.Put(table.PhysicalID, table.clone());
            version = version.max(table.Version);
        }
        for id in deleted {
            inner.Del(*id);
        }
        StatsCache {
            inner: Mutex::new(inner),
            maxTblStatsVer: AtomicU64::new(version),
        }
    }
    pub fn Update(&self, tables: &[Arc<StatisticsTable>], deleted: &[i64], skip: bool) {
        {
            let mut inner = self.inner.lock().unwrap();
            for table in tables {
                count("update");
                inner.Put(table.PhysicalID, table.clone());
            }
            for id in deleted {
                count("del");
                inner.Del(*id);
            }
        }
        if !skip {
            for table in tables {
                self.maxTblStatsVer
                    .fetch_max(table.Version, Ordering::AcqRel);
            }
        }
    }
    pub fn TriggerEvict(&self) {
        self.inner.lock().unwrap().TriggerEvict();
    }
    pub fn WaitForAsyncUpdates(&self) {
        self.inner.lock().unwrap().WaitForAsyncUpdates();
    }
}

// Reuse the package's bound Prometheus handles (initialized by the metrics subsystem).
#[allow(static_mut_refs)]
fn count(label: &str) {
    unsafe {
        let counter = match label {
            "hit" => &cache_metrics::cache_metrics::HitCounter,
            "miss" => &cache_metrics::cache_metrics::MissCounter,
            "del" => &cache_metrics::cache_metrics::DelCounter,
            _ => &cache_metrics::cache_metrics::UpdateCounter,
        };
        if let Some(counter) = counter {
            counter.inc();
        }
    }
}

#[allow(static_mut_refs)]
pub(crate) fn set_cost(cost: i64) {
    unsafe {
        if let Some(gauge) = &cache_metrics::cache_metrics::CostGauge {
            gauge.set(cost as f64);
        }
    }
}
