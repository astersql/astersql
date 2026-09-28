// Copyright 2026 AsterSQL.

// `initstats` 测试子包入口。
//
// 覆盖统计信息初始化（InitStats / InitStatsLite：启动或按需把持久化统计载入缓存）
// 相关用例，聚合包级入口与初始化逻辑测试。

use std::collections::BTreeMap;

/// 初始化阶段写入缓存的单列/索引统计摘要。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FieldStats {
    pub(crate) loaded: bool,
    pub(crate) top_n: usize,
    pub(crate) buckets: usize,
}

/// 一张物理表在缓存中的统计状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TableStats {
    pub(crate) physical_id: i64,
    pub(crate) row_count: i64,
    pub(crate) initialized: bool,
    pub(crate) stats_version: i64,
    pub(crate) columns: BTreeMap<i64, FieldStats>,
    pub(crate) indexes: BTreeMap<i64, FieldStats>,
}

impl TableStats {
    pub(crate) fn all_evicted(&self) -> bool {
        self.columns
            .values()
            .chain(self.indexes.values())
            .all(|field| !field.loaded && field.top_n == 0 && field.buckets == 0)
    }

    pub(crate) fn all_full_load(&self) -> bool {
        self.columns
            .values()
            .chain(self.indexes.values())
            .all(|field| field.loaded && field.top_n > 0 && field.buckets > 0)
    }
}

#[derive(Clone, Debug)]
struct TableDefinition {
    schema: String,
    live: bool,
    physical_ids: Vec<i64>,
    column_count: usize,
    index_count: usize,
}

/// Persisted statistics used by the initialization model.
#[derive(Clone, Debug)]
struct PersistedStats {
    row_count: i64,
    columns: BTreeMap<i64, FieldStats>,
    indexes: BTreeMap<i64, FieldStats>,
}

/// Executable model of the Go InitStats/InitStatsLite contract.
///
/// The crate's integration dependencies are intentionally disabled in its
/// Cargo manifest, so the tests use this deterministic store model instead of
/// pretending that a SQL session is a real integration test.  The model keeps
/// the production-visible state transitions: table/partition filtering,
/// selective initialization, skipped initialization, cache clearing, deferred
/// histogram loading, and the full-load memory policy.
#[derive(Debug, Default)]
pub(crate) struct StatsInitializer {
    tables: BTreeMap<i64, TableDefinition>,
    persisted: BTreeMap<i64, PersistedStats>,
    cache: BTreeMap<i64, TableStats>,
    /// Mirrors the process-wide IsFullCacheFunc test seam in Go.  Installing
    /// either override makes initialization defer payload loading; queries
    /// then load it explicitly, as the two concurrency tests assert.
    full_cache_override: Option<bool>,
    skip_init_stats: bool,
    init_stats_done: bool,
}

impl StatsInitializer {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Registers a table.  A partitioned table receives two physical IDs,
    /// matching the range-partition fixture in the Go tests.
    pub(crate) fn register_table(
        &mut self,
        table_id: i64,
        schema: &str,
        partitioned: bool,
        column_count: usize,
        index_count: usize,
    ) {
        let physical_ids = if partitioned {
            vec![table_id, table_id * 100 + 1, table_id * 100 + 2]
        } else {
            vec![table_id]
        };
        self.tables.insert(
            table_id,
            TableDefinition {
                schema: schema.to_owned(),
                live: true,
                physical_ids,
                column_count,
                index_count,
            },
        );
    }

    pub(crate) fn physical_ids(&self, table_id: i64) -> &[i64] {
        &self
            .tables
            .get(&table_id)
            .unwrap_or_else(|| panic!("unknown table {table_id}"))
            .physical_ids
    }

    /// Persists analyzed payload for every physical table in the fixture.
    pub(crate) fn analyze<I>(&mut self, table_ids: I, row_count: i64)
    where
        I: IntoIterator<Item = i64>,
    {
        for table_id in table_ids {
            let table = self
                .tables
                .get(&table_id)
                .unwrap_or_else(|| panic!("unknown table {table_id}"))
                .clone();
            for physical_id in table.physical_ids {
                self.persisted.insert(
                    physical_id,
                    PersistedStats {
                        row_count,
                        columns: fields(table.column_count, true, 2, 10),
                        indexes: fields(table.index_count, true, 2, 10),
                    },
                );
            }
        }
    }

    pub(crate) fn drop_table(&mut self, table_id: i64) {
        let table = self
            .tables
            .get_mut(&table_id)
            .unwrap_or_else(|| panic!("unknown table {table_id}"));
        table.live = false;
        for physical_id in &table.physical_ids {
            self.cache.remove(physical_id);
        }
    }

    pub(crate) fn clear(&mut self) {
        self.cache.clear();
    }

    pub(crate) fn set_skip_init_stats(&mut self, skip: bool) {
        self.skip_init_stats = skip;
    }

    pub(crate) fn set_is_full_cache_override(&mut self, is_full: bool) {
        self.full_cache_override = Some(is_full);
    }

    pub(crate) fn restore_is_full_cache_override(&mut self) {
        self.full_cache_override = None;
    }

    pub(crate) fn init_stats_lite(&mut self, table_ids: &[i64]) -> Result<(), &'static str> {
        if self.skip_init_stats {
            self.init_stats_done = true;
            return Ok(());
        }
        for physical_id in self.selected_physical_ids(table_ids) {
            if let Some(persisted) = self.persisted.get(&physical_id) {
                self.cache
                    .insert(physical_id, self.snapshot(physical_id, persisted, false));
            }
        }
        self.init_stats_done = true;
        Ok(())
    }

    pub(crate) fn init_stats(&mut self, table_ids: &[i64]) -> Result<(), &'static str> {
        if self.skip_init_stats {
            self.init_stats_done = true;
            return Ok(());
        }
        let load_payload = self.full_cache_override.is_none();
        for physical_id in self.selected_physical_ids(table_ids) {
            if let Some(persisted) = self.persisted.get(&physical_id) {
                self.cache.insert(
                    physical_id,
                    self.snapshot(physical_id, persisted, load_payload),
                );
            }
        }
        self.init_stats_done = true;
        Ok(())
    }

    /// Simulates the first predicate-driven histogram load after InitStats.
    pub(crate) fn load_needed_histograms(&mut self, table_id: i64) {
        let physical_ids = self
            .tables
            .get(&table_id)
            .unwrap_or_else(|| panic!("unknown table {table_id}"))
            .physical_ids
            .clone();
        for physical_id in physical_ids {
            if let Some(stats) = self.cache.get_mut(&physical_id) {
                for field in stats.columns.values_mut().chain(stats.indexes.values_mut()) {
                    field.loaded = true;
                    field.top_n = 2;
                    field.buckets = 10;
                }
            }
        }
    }

    pub(crate) fn stats(&self, physical_id: i64) -> Option<&TableStats> {
        self.cache.get(&physical_id)
    }

    pub(crate) fn cache_len(&self) -> usize {
        self.cache.len()
    }

    pub(crate) fn init_stats_done(&self) -> bool {
        self.init_stats_done
    }

    pub(crate) fn has_persisted_stats(&self, physical_id: i64) -> bool {
        self.persisted.contains_key(&physical_id)
    }

    pub(crate) fn max_physical_table_id(&self) -> i64 {
        self.cache
            .keys()
            .filter(|physical_id| {
                self.tables.values().any(|table| {
                    table.live
                        && !table.schema.starts_with("mysql")
                        && table.physical_ids.contains(physical_id)
                })
            })
            .copied()
            .max()
            .unwrap_or_default()
    }

    fn selected_physical_ids(&self, table_ids: &[i64]) -> Vec<i64> {
        if table_ids.is_empty() {
            return self
                .tables
                .values()
                .filter(|table| table.live)
                .flat_map(|table| table.physical_ids.iter().copied())
                .collect();
        }
        table_ids
            .iter()
            .flat_map(|table_id| {
                self.tables
                    .get(table_id)
                    .into_iter()
                    .filter(|table| table.live)
                    .flat_map(|table| table.physical_ids.iter().copied())
            })
            .collect()
    }

    fn snapshot(
        &self,
        physical_id: i64,
        persisted: &PersistedStats,
        load_payload: bool,
    ) -> TableStats {
        let mut snapshot = TableStats {
            physical_id,
            row_count: persisted.row_count,
            initialized: true,
            stats_version: 2,
            columns: persisted.columns.clone(),
            indexes: persisted.indexes.clone(),
        };
        if !load_payload {
            for field in snapshot
                .columns
                .values_mut()
                .chain(snapshot.indexes.values_mut())
            {
                field.loaded = false;
                field.top_n = 0;
                field.buckets = 0;
            }
        }
        snapshot
    }
}

fn fields(count: usize, loaded: bool, top_n: usize, buckets: usize) -> BTreeMap<i64, FieldStats> {
    (0..count)
        .map(|index| {
            (
                index as i64 + 1,
                FieldStats {
                    loaded,
                    top_n: if loaded { top_n } else { 0 },
                    buckets: if loaded { buckets } else { 0 },
                },
            )
        })
        .collect()
}

#[cfg(test)]
/// 统计初始化：仅加载仍存活的表、可跳过整次初始化。
mod init_stats_test;
#[cfg(test)]
/// 包级测试生命周期骨架（setup → run → leak check）。
mod main_test;
