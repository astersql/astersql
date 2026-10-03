// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 受限 SQL（Restricted SQL）统计持久化适配层。
//
// 将 `mysql.stats_*` 系统表语义映射到 KV 键值布局，使 statistics handle
// 在无完整 SQL 引擎时仍可读写 meta / histogram / bucket / FM Sketch /
// locked / history，并完成 schema 对账、批量 flush 与 GC。
// 「受限 SQL」指仅允许访问统计系统表的简化 SQL 子集。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use astersql_kv as kv;

use crate::lockstats::{RestrictedSQLExecutor, SqlRow, SqlValue, StatsError, StatsSession};
use crate::{ColumnStats, Handle, HandleBackend, IndexStats, TableStats, storage};

/// KV 键空间根前缀（统计 v1）。
const ROOT: &[u8] = b"__astersql/stats/v1/";
/// `stats_meta` 表前缀：表级行数/版本。
const META: &[u8] = b"meta/";
/// `stats_table_locked` 前缀：锁表累积 delta。
const LOCKED: &[u8] = b"locked/";
/// `stats_histograms` 前缀：列/索引是否已分析。
const HISTOGRAM: &[u8] = b"histogram/";
/// FM Sketch（基数估计草图）二进制载荷前缀。
const FM_SKETCH: &[u8] = b"fm-sketch/";
/// 直方图桶（bucket）载荷前缀。
const BUCKET: &[u8] = b"bucket/";
/// `stats_meta_history` 历史前缀。
const META_HISTORY: &[u8] = b"meta-history/";
/// `stats_history` 历史前缀。
const HISTORY: &[u8] = b"history/";
/// `mysql.tidb` 系统变量前缀（如 GC 时间戳）。
const SYSTEM: &[u8] = b"system/";

/// 统计 KV 存储抽象：提供版本、快照与事务。
pub trait StatsKvStorage: Send + Sync {
    /// 当前 KV 版本。
    fn current_version(&self) -> Result<kv::Version, kv::errors::SharedError>;
    /// 打开指定版本快照。
    fn snapshot(&self, version: kv::Version) -> Box<dyn kv::Snapshot>;
    /// 开启读写事务。
    fn begin(&self) -> Result<Box<dyn kv::Transaction>, kv::errors::SharedError>;
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// `mysql.stats_meta` 一行：物理表 ID、版本、行数与 modify_count。
pub struct StatsMetaRecord {
    /// 物理表 ID。
    pub table_id: i64,
    /// 统计版本（常与事务时间戳对齐）。
    pub version: u64,
    /// 估计行数（realtime count）。
    pub count: i64,
    /// 自上次分析以来的修改行数。
    pub modify_count: i64,
    /// 最近一次直方图相关版本。
    pub last_histogram_version: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// `mysql.stats_table_locked` 一行：锁表期间累积的 count/modify_count delta。
pub struct StatsLockedRecord {
    /// 被锁物理表 ID。
    pub table_id: i64,
    /// 锁定期累积行数 delta。
    pub count: i64,
    /// 锁定期累积修改数。
    pub modify_count: i64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// `mysql.stats_histograms` 标识：表 ID + 是否索引 + hist_id。
pub struct StatsHistogramRecord {
    /// 物理表 ID。
    pub table_id: i64,
    /// true 表示索引直方图，否则为列。
    pub is_index: bool,
    /// 列 ID 或索引 ID。
    pub histogram_id: i64,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// `mysql.stats_histograms` 持久化载荷；键中已有表、索引标记与 hist_id。
struct StatsHistogramPayload {
    analyzed: bool,
    stats_version: i64,
    version: u64,
    ndv: i64,
    null_count: i64,
    total_column_size: i64,
    correlation: f64,
}

/// 基于 KV 的统计 SqlStore / RestrictedSQL 实现，串行化写路径。
pub struct KvStatsStore<B: HandleBackend + Send> {
    /// 底层 KV。
    storage: Arc<dyn StatsKvStorage>,
    /// 内存统计 Handle（提交后同步）。
    handle: Arc<Mutex<Handle<B>>>,
    /// 串行化全部受限 SQL / 对账写路径。
    operation_lock: Mutex<()>,
    /// 测试：下次 DeleteStatsLock 失败。
    fail_next_lock_delete: AtomicBool,
    /// 测试：批量 flush 在第 N 次操作后失败（-1 表示关闭）。
    fail_stats_batch_after: AtomicI64,
}

impl<B: HandleBackend + Send> KvStatsStore<B> {
    /// 构造存储适配器；`handle` 用于提交后同步内存缓存。
    pub fn new(storage: Arc<dyn StatsKvStorage>, handle: Arc<Mutex<Handle<B>>>) -> Self {
        Self {
            storage,
            handle,
            operation_lock: Mutex::new(()),
            fail_next_lock_delete: AtomicBool::new(false),
            fail_stats_batch_after: AtomicI64::new(-1),
        }
    }

    /// 测试注入：下次删除锁表记录失败。
    pub fn fail_next_lock_delete_for_test(&self) {
        self.fail_next_lock_delete.store(true, Ordering::Release);
    }

    /// 测试注入：批量 flush 在第 N 次操作后失败。
    pub fn fail_stats_batch_after_for_test(&self, operations: usize) {
        self.fail_stats_batch_after
            .store(operations as i64, Ordering::Release);
    }

    /// 执行一条受限 SQL（SELECT 返回行，DML 返回空）。
    pub fn execute(&self, sql: &str, arguments: &[SqlValue]) -> Result<Vec<SqlRow>, StatsError> {
        astersql_statistics_handle_util::ExecRowsTimeout().map_err(StatsError)?;
        let _operation = self
            .operation_lock
            .lock()
            .expect("statistics SQL operation lock poisoned");
        self.with_executor(|executor| executor.ExecRestrictedSQL(sql, arguments))
    }

    /// Execute only mysql.tidb SQL in the caller's existing transaction. Do not
    /// commit or roll back it: starter bootstrap owns the SQL/version boundary.
    pub fn system_sql_in_transaction(
        &self,
        transaction: &mut Box<dyn kv::Transaction>,
        sql: &str,
        arguments: &[SqlValue],
    ) -> Result<Vec<Vec<String>>, StatsError> {
        let normalized = normalize(sql);
        let allowed = sql_table(&normalized) == Some("tidb")
            && (normalized.starts_with("select ")
                || normalized.starts_with("insert into mysql.tidb")
                || normalized.starts_with("insert ignore into mysql.tidb")
                || normalized.starts_with("insert high_priority into mysql.tidb")
                || normalized.starts_with("update mysql.tidb")
                || normalized.starts_with("delete from mysql.tidb"));
        if !allowed {
            return Err(StatsError("borrowed system SQL requires mysql.tidb".into()));
        }
        astersql_statistics_handle_util::ExecRowsTimeout().map_err(StatsError)?;
        let _operation = self
            .operation_lock
            .lock()
            .expect("statistics SQL operation lock poisoned");
        let start_ts = transaction.StartTS();
        let mut executor = KvRestrictedExecutor {
            transaction,
            handle: &self.handle,
            fail_next_lock_delete: &self.fail_next_lock_delete,
            pending_meta: BTreeMap::new(),
            pending_cache_meta: BTreeMap::new(),
            removed_tables: BTreeSet::new(),
            start_ts,
        };
        executor.ExecRestrictedSQL(sql, arguments).map(|rows| {
            rows.into_iter()
                .map(|row| {
                    row.0
                        .into_iter()
                        .map(|value| match value {
                            SqlValue::Int(value) => value.to_string(),
                            SqlValue::UInt(value) => value.to_string(),
                            SqlValue::Text(value) => value,
                        })
                        .collect()
                })
                .collect()
        })
    }

    /// 执行查询并将每个单元格格式化为字符串。
    pub fn query_strings(
        &self,
        sql: &str,
        arguments: &[SqlValue],
    ) -> Result<Vec<Vec<String>>, StatsError> {
        self.execute(sql, arguments).map(|rows| {
            rows.into_iter()
                .map(|row| {
                    row.0
                        .into_iter()
                        .map(|value| match value {
                            SqlValue::Int(value) => value.to_string(),
                            SqlValue::UInt(value) => value.to_string(),
                            SqlValue::Text(value) => value,
                        })
                        .collect()
                })
                .collect()
        })
    }

    /// 将 handle 缓存中的表统计写回 KV。
    fn persist_table_stats_result(&self, physical_id: i64) -> Result<(), StatsError> {
        // Clone the profile before taking operation_lock. All other paths
        // acquire operation_lock before the Handle lock during commit; keeping
        // that order here avoids a lock inversion under concurrent flushes.
        let stats = {
            let handle = self.handle.lock().expect("statistics handle lock poisoned");
            handle
                .stats_meta(physical_id)
                .cloned()
                .ok_or_else(|| StatsError(format!("unknown statistics table {physical_id}")))?
        };
        let _operation = self
            .operation_lock
            .lock()
            .expect("statistics SQL operation lock poisoned");
        self.with_executor(|executor| executor.replace_table_stats(&stats))
    }

    /// 读取当前所有锁表物理 ID。
    fn locked_ids_result(&self) -> Result<BTreeSet<i64>, StatsError> {
        let _operation = self
            .operation_lock
            .lock()
            .expect("statistics SQL operation lock poisoned");
        self.with_executor(|executor| {
            executor
                .LockedTableIds()
                .map(|table_ids| table_ids.into_iter().collect())
        })
    }

    /// 在已锁表记录上累加行数/修改数 delta。
    fn add_locked_delta_result(
        &self,
        physical_id: i64,
        row_delta: i64,
        modify_count: i64,
    ) -> Result<(), StatsError> {
        let _operation = self
            .operation_lock
            .lock()
            .expect("statistics SQL operation lock poisoned");
        self.with_executor(|executor| {
            executor.add_locked_delta(physical_id, row_delta, modify_count)
        })
    }

    /// 删除某物理表全部持久化记录并标记缓存移除。
    fn remove_table_result(&self, physical_id: i64) -> Result<(), StatsError> {
        let _operation = self
            .operation_lock
            .lock()
            .expect("statistics SQL operation lock poisoned");
        self.with_executor(|executor| executor.delete_table_records(physical_id, true))
    }

    /// 对外包装：持久化指定物理表 meta（错误转 String）。
    pub fn persist_meta(&self, physical_id: i64) -> Result<(), String> {
        self.persist_table_stats_result(physical_id)
            .map_err(|error| error.to_string())
    }

    /// 列出全部 `stats_meta` 行：(table_id, version, count, modify_count)。
    pub fn persisted_meta_rows(&self) -> Result<Vec<(i64, u64, i64, i64)>, String> {
        self.execute(
            "SELECT table_id, version, count, modify_count FROM mysql.stats_meta",
            &[],
        )
        .map(|rows| {
            rows.into_iter()
                .map(|row| {
                    (
                        row.0[0].int(),
                        row.0[1].int().max(0) as u64,
                        row.0[2].int(),
                        row.0[3].int(),
                    )
                })
                .collect()
        })
        .map_err(|error| error.to_string())
    }

    /// 列出全部锁表行：(table_id, count, modify_count)。
    pub fn locked_rows(&self) -> Result<Vec<(i64, i64, i64)>, String> {
        self.execute(
            "SELECT table_id, count, modify_count FROM mysql.stats_table_locked",
            &[],
        )
        .map(|rows| {
            rows.into_iter()
                .map(|row| (row.0[0].int(), row.0[1].int(), row.0[2].int()))
                .collect()
        })
        .map_err(|error| error.to_string())
    }

    /// 读取指定直方图的持久化桶，按 bucket_id 升序；损坏边界须以解码失败暴露（对齐 Go）。
    /// Reads the persisted buckets of one histogram in ascending bucket order.
    ///
    /// Async statistics loading reconstructs histograms from these rows, so the
    /// raw bound blobs are returned untouched: a corrupted `stats_buckets` row
    /// must surface as a decode failure to the caller, exactly like Go.
    pub fn histogram_buckets(
        &self,
        table_id: i64,
        is_index: bool,
        histogram_id: i64,
    ) -> Result<Vec<crate::Bucket>, String> {
        let _operation = self
            .operation_lock
            .lock()
            .expect("statistics SQL operation lock poisoned");
        self.with_executor(|executor| {
            let prefix = [
                table_id_prefix(BUCKET, table_id),
                vec![u8::from(is_index)],
                histogram_id.to_be_bytes().to_vec(),
            ]
            .concat();
            scan(executor.transaction.as_ref(), &prefix)?
                .into_iter()
                .map(|(_, value)| decode_bucket(&value))
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|error| error.to_string())
    }

    /// 返回锁表物理 ID 集合（错误转 String）。
    pub fn locked_ids(&self) -> Result<BTreeSet<i64>, String> {
        self.locked_ids_result().map_err(|error| error.to_string())
    }

    /// 对外包装：累加锁表 delta。
    pub fn add_locked_delta(
        &self,
        physical_id: i64,
        row_delta: i64,
        modify_count: i64,
    ) -> Result<(), String> {
        self.add_locked_delta_result(physical_id, row_delta, modify_count)
            .map_err(|error| error.to_string())
    }

    /// 对外包装：删除物理表全部持久化统计。
    pub fn remove_persisted_table(&self, physical_id: i64) -> Result<(), String> {
        self.remove_table_result(physical_id)
            .map_err(|error| error.to_string())
    }

    /// Schema 对账：按列/索引集合重建缓存并替换整表缓存。
    pub fn reconcile_tables(
        &self,
        tables: &BTreeMap<i64, (BTreeSet<i64>, BTreeSet<i64>)>,
    ) -> Result<(), String> {
        self.reconcile_tables_impl(tables, true)
    }

    /// 通过内部受限 SQL 事务执行轻量统计初始化。
    ///
    /// 与 Go `InitStatsLite` 一致：BEGIN 后只读取已有 meta/histogram，
    /// 缺失 meta 的表继续使用 pseudo 统计，最后提交只读事务。
    pub fn init_tables_lite(
        &self,
        tables: &BTreeMap<i64, (BTreeSet<i64>, BTreeSet<i64>)>,
        replace_cache: bool,
    ) -> Result<BTreeSet<i64>, String> {
        let _operation = self
            .operation_lock
            .lock()
            .expect("statistics SQL operation lock poisoned");
        let (persisted, profiles) = self
            .with_executor(|executor| {
                let mut persisted = BTreeSet::new();
                let mut profiles = Vec::with_capacity(tables.len());
                for (physical_id, (column_ids, index_ids)) in tables {
                    let Some(value) = get(
                        executor.transaction.as_ref(),
                        record_key(META, *physical_id),
                    )?
                    else {
                        continue;
                    };
                    let meta = decode_meta(*physical_id, &value)?;
                    persisted.insert(*physical_id);
                    let mut columns = BTreeMap::new();
                    let mut indexes = BTreeMap::new();
                    for (key, value) in scan(
                        executor.transaction.as_ref(),
                        &histogram_table_prefix(*physical_id),
                    )? {
                        let record = decode_histogram_key(&key)?;
                        let retained = if record.is_index {
                            index_ids.contains(&record.histogram_id)
                        } else {
                            column_ids.contains(&record.histogram_id)
                        };
                        if !retained {
                            continue;
                        }
                        let analyzed = value.first().is_some_and(|flag| *flag != 0);
                        if record.is_index {
                            indexes.insert(record.histogram_id, analyzed);
                        } else {
                            columns.insert(record.histogram_id, analyzed);
                        }
                    }
                    profiles.push(TableStats {
                        physical_id: *physical_id,
                        pseudo: false,
                        initialized: true,
                        version: meta.version,
                        modify_count: meta.modify_count,
                        realtime_count: meta.count,
                        last_analyze_version: meta.last_histogram_version,
                        last_stats_hist_version: meta.last_histogram_version,
                        indexes: indexes
                            .into_iter()
                            .map(|(id, analyzed)| {
                                (
                                    id,
                                    IndexStats {
                                        analyzed,
                                        ..IndexStats::default()
                                    },
                                )
                            })
                            .collect(),
                        columns: columns
                            .into_iter()
                            .map(|(id, analyzed)| {
                                (
                                    id,
                                    ColumnStats {
                                        analyzed_or_synthesized: analyzed,
                                        ..ColumnStats::default()
                                    },
                                )
                            })
                            .collect(),
                        ..TableStats::default()
                    });
                }
                Ok((persisted, profiles))
            })
            .map_err(|error| error.to_string())?;
        let mut handle = self.handle.lock().expect("statistics handle lock poisoned");
        if replace_cache {
            handle.reconcile_cache_profiles(profiles);
        } else {
            handle.merge_cache_profiles(profiles);
        }
        Ok(persisted)
    }

    /// 仅重载给定物理表统计（lite 初始化）；不驱逐无关表缓存。
    /// Reloads only the supplied physical tables from persisted statistics.
    ///
    /// This is the storage half of stats lite initialization: unlike schema
    /// reconciliation it must not evict cache entries for unrelated tables.
    pub fn reconcile_table_ids(
        &self,
        tables: &BTreeMap<i64, (BTreeSet<i64>, BTreeSet<i64>)>,
    ) -> Result<(), String> {
        self.reconcile_tables_impl(tables, false)
    }

    /// 对账实现：扫描直方图键、裁剪多余列/索引，再 merge 或 replace 缓存。
    fn reconcile_tables_impl(
        &self,
        tables: &BTreeMap<i64, (BTreeSet<i64>, BTreeSet<i64>)>,
        replace_cache: bool,
    ) -> Result<(), String> {
        let _operation = self
            .operation_lock
            .lock()
            .expect("statistics SQL operation lock poisoned");
        let mut transaction = self.storage.begin().map_err(|error| error.to_string())?;
        // DDL sessions in Go use AllowedOnAlmostFull. Schema reconciliation is
        // the corresponding internal DDL write path, not startup stats loading.
        transaction.SetDiskFullOpt(kv::kvrpcpb::DiskFullOpt::AllowedOnAlmostFull);
        let mut profiles = Vec::with_capacity(tables.len());
        // 逐表：读取已有 meta，扫描直方图并按 schema 裁剪，组装 TableStats。
        // DDL 本身不写 stats_meta；缺失行由后续 DDL-event handling 或
        // stats-delta flush 填充，与 Go 的统计生命周期保持一致。
        for (physical_id, (column_ids, index_ids)) in tables {
            let meta = match get(transaction.as_ref(), record_key(META, *physical_id))
                .map_err(|error| error.to_string())?
            {
                Some(value) => {
                    decode_meta(*physical_id, &value).map_err(|error| error.to_string())?
                }
                None => StatsMetaRecord {
                    table_id: *physical_id,
                    ..StatsMetaRecord::default()
                },
            };
            let mut columns = BTreeMap::new();
            let mut indexes = BTreeMap::new();
            for (key, value) in scan(transaction.as_ref(), &histogram_table_prefix(*physical_id))
                .map_err(|error| error.to_string())?
            {
                let record = decode_histogram_key(&key).map_err(|error| error.to_string())?;
                let retained = if record.is_index {
                    index_ids.contains(&record.histogram_id)
                } else {
                    column_ids.contains(&record.histogram_id)
                };
                let analyzed = value.first().is_some_and(|flag| *flag != 0);
                if retained {
                    if record.is_index {
                        indexes.insert(record.histogram_id, analyzed);
                    } else {
                        columns.insert(record.histogram_id, analyzed);
                    }
                } else {
                    delete(transaction.as_mut(), key).map_err(|error| error.to_string())?;
                }
            }
            // Keep column identities in the live profile even before ANALYZE.
            // A later stats-delta flush persists zero-valued histogram rows;
            // indexes remain absent until analyzed, matching Go's lifecycle.
            for column_id in column_ids {
                columns.entry(*column_id).or_insert(false);
            }
            profiles.push(TableStats {
                physical_id: *physical_id,
                pseudo: false,
                initialized: true,
                version: meta.version,
                modify_count: meta.modify_count,
                realtime_count: meta.count,
                last_analyze_version: meta.last_histogram_version,
                last_stats_hist_version: meta.last_histogram_version,
                indexes: indexes
                    .into_iter()
                    .map(|(id, analyzed)| {
                        (
                            id,
                            IndexStats {
                                analyzed,
                                ..IndexStats::default()
                            },
                        )
                    })
                    .collect(),
                columns: columns
                    .into_iter()
                    .map(|(id, analyzed)| {
                        (
                            id,
                            ColumnStats {
                                analyzed_or_synthesized: analyzed,
                                ..ColumnStats::default()
                            },
                        )
                    })
                    .collect(),
                ..TableStats::default()
            });
        }
        transaction
            .Commit(&kv::Context::default())
            .map_err(|error| error.to_string())?;
        let mut handle = self.handle.lock().expect("statistics handle lock poisoned");
        if replace_cache {
            handle.reconcile_cache_profiles(profiles);
        } else {
            handle.merge_cache_profiles(profiles);
        }
        Ok(())
    }

    /// 批量 flush：先写锁表 delta，再 replace 表统计；支持失败注入。
    pub fn apply_stats_flush_batch(
        &self,
        locked_deltas: &[(i64, (i64, i64))],
        persisted_stats: &[TableStats],
    ) -> Result<(), String> {
        let _operation = self
            .operation_lock
            .lock()
            .expect("statistics SQL operation lock poisoned");
        let fail_after = self.fail_stats_batch_after.swap(-1, Ordering::AcqRel);
        self.with_executor(|executor| {
            let mut operations = 0_i64;
            // 先应用锁表 delta，再持久化完整表统计；operations 计数用于失败注入。
            for (physical_id, (row_delta, modify_count)) in locked_deltas {
                executor.add_locked_delta(*physical_id, *row_delta, *modify_count)?;
                operations += 1;
                if operations == fail_after {
                    return Err(StatsError(
                        "injected statistics batch persistence failure".to_owned(),
                    ));
                }
            }
            for stats in persisted_stats {
                executor.replace_table_stats(stats)?;
                operations += 1;
                if operations == fail_after {
                    return Err(StatsError(
                        "injected statistics batch persistence failure".to_owned(),
                    ));
                }
            }
            Ok(())
        })
        .map_err(|error| error.to_string())
    }

    /// Persist schema histogram identities without creating/updating a
    /// stats_meta row. Static partition ANALYZE uses this for the logical
    /// table: Go exposes zero-valued histogram rows there while keeping global
    /// statistics absent from SHOW STATS_META.
    pub fn persist_unanalyzed_histograms(
        &self,
        table_id: i64,
        column_ids: &[i64],
        index_ids: &[i64],
    ) -> Result<(), String> {
        let _operation = self
            .operation_lock
            .lock()
            .expect("statistics SQL operation lock poisoned");
        self.with_executor(|executor| {
            for (is_index, ids) in [(false, column_ids), (true, index_ids)] {
                for histogram_id in ids {
                    let record = StatsHistogramRecord {
                        table_id,
                        is_index,
                        histogram_id: *histogram_id,
                    };
                    let key = histogram_key(&record);
                    if get(executor.transaction.as_ref(), key.clone())?.is_none() {
                        set(
                            executor.transaction.as_mut(),
                            key,
                            encode_histogram_payload(&StatsHistogramPayload::default()),
                        )?;
                    }
                }
            }
            Ok(())
        })
        .map_err(|error| error.to_string())
    }

    /// 开启事务并构造 `KvRestrictedExecutor`，成功则提交，失败回滚。
    fn with_executor<T>(
        &self,
        operation: impl FnOnce(&mut KvRestrictedExecutor<'_, B>) -> Result<T, StatsError>,
    ) -> Result<T, StatsError> {
        let mut transaction = self.storage.begin().map_err(stats_error)?;
        let start_ts = transaction.StartTS().max(current_stats_timestamp());
        let mut executor = KvRestrictedExecutor {
            transaction: &mut transaction,
            handle: &self.handle,
            fail_next_lock_delete: &self.fail_next_lock_delete,
            pending_meta: BTreeMap::new(),
            pending_cache_meta: BTreeMap::new(),
            removed_tables: BTreeSet::new(),
            start_ts,
        };
        // 成功提交；失败回滚事务，不触及 Handle 缓存。
        match operation(&mut executor) {
            Ok(value) => {
                executor.commit()?;
                Ok(value)
            }
            Err(error) => {
                let _ = executor.transaction.Rollback();
                Err(error)
            }
        }
    }
}

/// 将 `KvStatsStore` 暴露为 `StatsSession`：在同一把操作锁下跑回调。
impl<B: HandleBackend + Send> StatsSession for KvStatsStore<B> {
    /// 持有操作锁后把受限执行器交给回调。
    fn WithSession(
        &self,
        _wrap_transaction: bool,
        callback: &mut dyn FnMut(&mut dyn RestrictedSQLExecutor) -> Result<(), StatsError>,
    ) -> Result<(), StatsError> {
        let _operation = self
            .operation_lock
            .lock()
            .expect("statistics SQL operation lock poisoned");
        self.with_executor(|executor| callback(executor))
    }
}

/// `storage::SqlStore` 实现：给 GC / 读路径提供统一 SQL 与删除钩子。
impl<B: HandleBackend + Send> storage::SqlStore for KvStatsStore<B> {
    /// 当前存储版本与 GC 时间戳取较大值，作为统计时间戳。
    fn start_ts(&self) -> Result<u64, storage::Error> {
        self.storage
            .current_version()
            .map(|version| version.Ver.max(storage::gc::current_ts()))
            .map_err(|error| storage::Error(error.to_string()))
    }

    /// 无参数执行受限 SQL，并映射为 `storage::Row`。
    fn execute(&self, sql: &str) -> Result<Vec<storage::Row>, storage::Error> {
        KvStatsStore::execute(self, sql, &[])
            .map(|rows| {
                rows.into_iter()
                    .map(|row| {
                        storage::Row(
                            row.0
                                .into_iter()
                                .map(|value| match value {
                                    SqlValue::Int(value) => storage::Value::Int(value),
                                    SqlValue::UInt(value) => storage::Value::UInt(value),
                                    SqlValue::Text(value) => storage::Value::Text(value),
                                })
                                .collect(),
                        )
                    })
                    .collect()
            })
            .map_err(|error| storage::Error(error.to_string()))
    }

    /// 列出版本落在 [min, max) 的 meta 表 ID，供 GC 扫描。
    fn gc_meta_ids(
        &self,
        minimum_version: u64,
        maximum_version: u64,
    ) -> Result<Vec<i64>, storage::Error> {
        let _operation = self
            .operation_lock
            .lock()
            .expect("statistics SQL operation lock poisoned");
        self.with_executor(|executor| {
            Ok(executor
                .meta_records(None)?
                .into_iter()
                .filter(|record| {
                    record.version >= minimum_version && record.version < maximum_version
                })
                .map(|record| record.table_id)
                .collect())
        })
        .map_err(|error| storage::Error(error.to_string()))
    }

    /// 列出某表全部直方图身份 `(is_index, hist_id)`。
    fn gc_histogram_identities(&self, table_id: i64) -> Result<Vec<(bool, i64)>, storage::Error> {
        let _operation = self
            .operation_lock
            .lock()
            .expect("statistics SQL operation lock poisoned");
        self.with_executor(|executor| {
            let mut identities = scan(
                executor.transaction.as_ref(),
                &histogram_table_prefix(table_id),
            )?
            .into_iter()
            .map(|(key, _)| {
                decode_histogram_key(&key).map(|record| (record.is_index, record.histogram_id))
            })
            .collect::<Result<Vec<_>, _>>()?;
            identities.extend(
                scan(
                    executor.transaction.as_ref(),
                    &table_id_prefix(FM_SKETCH, table_id),
                )?
                .into_iter()
                .map(|(key, _)| {
                    decode_fm_sketch_key(&key)
                        .map(|(_, is_index, histogram_id)| (is_index, histogram_id))
                })
                .collect::<Result<Vec<_>, _>>()?,
            );
            identities.sort_unstable();
            identities.dedup();
            Ok(identities)
        })
        .map_err(|error| storage::Error(error.to_string()))
    }

    /// GC 删除表统计：更新 meta 版本；非 soft 时删直方图；始终删 bucket/FM/locked。
    fn gc_delete_table_stats(
        &self,
        table_id: i64,
        soft: bool,
        version: u64,
    ) -> Result<(), storage::Error> {
        let _operation = self
            .operation_lock
            .lock()
            .expect("statistics SQL operation lock poisoned");
        self.with_executor(|executor| {
            if let Some(mut meta) = executor.read_meta(table_id)? {
                meta.version = version;
                meta.last_histogram_version = version;
                executor.write_meta(meta)?;
            }
            if !soft {
                delete_prefix(
                    executor.transaction.as_mut(),
                    &histogram_table_prefix(table_id),
                )?;
            }
            // 对齐 Go：无论 soft 与否都删 bucket/TopN/FM。
            // Go `DeleteTableStatsFromKV` also drops the buckets, TopN and FM
            // sketch payloads regardless of the soft flag.
            delete_prefix(
                executor.transaction.as_mut(),
                &table_id_prefix(BUCKET, table_id),
            )?;
            delete_prefix(
                executor.transaction.as_mut(),
                &table_id_prefix(FM_SKETCH, table_id),
            )?;
            delete(executor.transaction.as_mut(), record_key(LOCKED, table_id))
        })
        .map_err(|error| storage::Error(error.to_string()))
    }

    /// GC：删除表全部记录（含 meta）。
    fn gc_delete_meta(&self, table_id: i64) -> Result<(), storage::Error> {
        let _operation = self
            .operation_lock
            .lock()
            .expect("statistics SQL operation lock poisoned");
        self.with_executor(|executor| executor.delete_table_records(table_id, true))
            .map_err(|error| storage::Error(error.to_string()))
    }

    /// GC：删除单个列/索引直方图及其 bucket / FM Sketch。
    fn gc_delete_histogram(
        &self,
        table_id: i64,
        histogram_id: i64,
        is_index: bool,
        version: u64,
    ) -> Result<(), storage::Error> {
        let _operation = self
            .operation_lock
            .lock()
            .expect("statistics SQL operation lock poisoned");
        self.with_executor(|executor| {
            if let Some(mut meta) = executor.read_meta(table_id)? {
                meta.version = version;
                meta.last_histogram_version = version;
                executor.write_meta(meta)?;
            }
            delete(
                executor.transaction.as_mut(),
                histogram_key(&StatsHistogramRecord {
                    table_id,
                    is_index,
                    histogram_id,
                }),
            )?;
            // 对齐 Go：同步删除该列/索引的 bucket 与 FM。
            // Go `deleteHistStatsFromKV` removes the buckets, TopN and FM
            // sketch rows of the dropped column or index too.
            delete_prefix(
                executor.transaction.as_mut(),
                &bucket_histogram_prefix(table_id, is_index, histogram_id),
            )?;
            delete(
                executor.transaction.as_mut(),
                fm_sketch_key(table_id, is_index, histogram_id),
            )
        })
        .map_err(|error| storage::Error(error.to_string()))
    }

    /// GC：删除某表 meta_history 与 history。
    fn gc_delete_history(&self, table_id: i64) -> Result<(), storage::Error> {
        let _operation = self
            .operation_lock
            .lock()
            .expect("statistics SQL operation lock poisoned");
        self.with_executor(|executor| {
            delete_prefix(
                executor.transaction.as_mut(),
                &table_id_prefix(META_HISTORY, table_id),
            )?;
            delete_prefix(
                executor.transaction.as_mut(),
                &table_id_prefix(HISTORY, table_id),
            )
        })
        .map_err(|error| storage::Error(error.to_string()))
    }

    /// GC：按保留秒数清理过期 history 记录。
    fn gc_clear_expired_history(&self, retention_seconds: u64) -> Result<(), storage::Error> {
        let cutoff = current_unix_seconds().saturating_sub(retention_seconds);
        let _operation = self
            .operation_lock
            .lock()
            .expect("statistics SQL operation lock poisoned");
        self.with_executor(|executor| {
            for table in [META_HISTORY, HISTORY] {
                for (key, _) in scan(executor.transaction.as_ref(), &prefix(table))? {
                    if decode_history_key(table, &key)?.3 <= cutoff {
                        delete(executor.transaction.as_mut(), key)?;
                    }
                }
            }
            Ok(())
        })
        .map_err(|error| storage::Error(error.to_string()))
    }

    /// 读取 `mysql.tidb` 中 GC 相关系统变量。
    fn gc_timestamp(&self, variable_name: &str) -> Result<Option<String>, storage::Error> {
        let _operation = self
            .operation_lock
            .lock()
            .expect("statistics SQL operation lock poisoned");
        self.with_executor(|executor| {
            get(executor.transaction.as_ref(), system_key(variable_name))?
                .map(|value| {
                    String::from_utf8(value)
                        .map_err(|error| StatsError(format!("invalid system value: {error}")))
                })
                .transpose()
        })
        .map_err(|error| storage::Error(error.to_string()))
    }

    /// 写入 GC 时间戳系统变量。
    fn set_gc_timestamp(&self, variable_name: &str, timestamp: u64) -> Result<(), storage::Error> {
        let _operation = self
            .operation_lock
            .lock()
            .expect("statistics SQL operation lock poisoned");
        self.with_executor(|executor| {
            set(
                executor.transaction.as_mut(),
                system_key(variable_name),
                timestamp.to_string().into_bytes(),
            )
        })
        .map_err(|error| storage::Error(error.to_string()))
    }
}

/// 单事务内的受限 SQL 执行器；提交时把 pending meta 刷进内存 Handle。
struct KvRestrictedExecutor<'a, B: HandleBackend + Send> {
    /// 当前写事务。
    transaction: &'a mut Box<dyn kv::Transaction>,
    handle: &'a Arc<Mutex<Handle<B>>>,
    fail_next_lock_delete: &'a AtomicBool,
    /// 本事务已写 meta（读路径可见）。
    pending_meta: BTreeMap<i64, StatsMetaRecord>,
    /// 提交后需刷入 Handle 缓存的 meta。
    pending_cache_meta: BTreeMap<i64, StatsMetaRecord>,
    /// 提交后需从 Handle 移除的表。
    removed_tables: BTreeSet<i64>,
    /// 事务开始时间戳。
    start_ts: u64,
}

impl<B: HandleBackend + Send> KvRestrictedExecutor<'_, B> {
    /// 提交 KV 事务，并把 pending_cache_meta / removed_tables 应用到 Handle。
    fn commit(&mut self) -> Result<(), StatsError> {
        self.transaction
            .Commit(&kv::Context::default())
            .map_err(stats_error)?;
        if !self.pending_cache_meta.is_empty() || !self.removed_tables.is_empty() {
            let mut handle = self.handle.lock().expect("statistics handle lock poisoned");
            for record in self.pending_cache_meta.values() {
                handle.apply_persisted_stats_meta(
                    record.table_id,
                    record.version,
                    record.count,
                    record.modify_count,
                    record.last_histogram_version,
                );
            }
            handle.remove_tables(&self.removed_tables.iter().copied().collect::<Vec<_>>());
        }
        Ok(())
    }

    /// 整表替换：写 meta，重写 histogram / FM Sketch / bucket。
    fn replace_table_stats(&mut self, stats: &TableStats) -> Result<(), StatsError> {
        let meta = StatsMetaRecord {
            table_id: stats.physical_id,
            version: stats.version,
            count: stats.realtime_count,
            modify_count: stats.modify_count,
            last_histogram_version: stats.last_stats_hist_version,
        };
        self.write_meta(meta)?;
        delete_prefix(
            self.transaction.as_mut(),
            &histogram_table_prefix(stats.physical_id),
        )?;
        // 记录值保留已分析/合成判定，lite 初始化无需直方图载荷。
        // The record value keeps Go's `IsColumnAnalyzedOrSynthesized` verdict
        // so lite initialization can rebuild the existence map without the
        // histogram payload.
        for (histogram_id, column) in &stats.columns {
            let record = StatsHistogramRecord {
                table_id: stats.physical_id,
                is_index: false,
                histogram_id: *histogram_id,
            };
            set(
                self.transaction.as_mut(),
                histogram_key(&record),
                encode_histogram_payload(&StatsHistogramPayload {
                    analyzed: column.analyzed_or_synthesized,
                    stats_version: column.stats_version,
                    version: column.version,
                    ndv: column.ndv,
                    null_count: column.null_count,
                    total_column_size: column.total_column_size,
                    correlation: column.correlation,
                }),
            )?;
        }
        for (histogram_id, index) in &stats.indexes {
            let record = StatsHistogramRecord {
                table_id: stats.physical_id,
                is_index: true,
                histogram_id: *histogram_id,
            };
            set(
                self.transaction.as_mut(),
                histogram_key(&record),
                encode_histogram_payload(&StatsHistogramPayload {
                    analyzed: index.analyzed,
                    stats_version: index.stats_version,
                    version: index.version,
                    ndv: index.ndv,
                    null_count: index.null_count,
                    total_column_size: index.total_column_size,
                    correlation: index.correlation,
                }),
            )?;
        }
        delete_prefix(
            self.transaction.as_mut(),
            &table_id_prefix(FM_SKETCH, stats.physical_id),
        )?;
        for (histogram_id, column) in &stats.columns {
            if !column.fm_sketch.is_empty() {
                set(
                    self.transaction.as_mut(),
                    fm_sketch_key(stats.physical_id, false, *histogram_id),
                    column.fm_sketch.clone(),
                )?;
            }
        }
        for (histogram_id, index) in &stats.indexes {
            if !index.fm_sketch.is_empty() {
                set(
                    self.transaction.as_mut(),
                    fm_sketch_key(stats.physical_id, true, *histogram_id),
                    index.fm_sketch.clone(),
                )?;
            }
        }
        delete_prefix(
            self.transaction.as_mut(),
            &table_id_prefix(BUCKET, stats.physical_id),
        )?;
        for (histogram_id, column) in &stats.columns {
            for (bucket_id, bucket) in column.buckets.iter().enumerate() {
                set(
                    self.transaction.as_mut(),
                    bucket_key(stats.physical_id, false, *histogram_id, bucket_id as i64),
                    encode_bucket(bucket),
                )?;
            }
        }
        for (histogram_id, index) in &stats.indexes {
            for (bucket_id, bucket) in index.buckets.iter().enumerate() {
                set(
                    self.transaction.as_mut(),
                    bucket_key(stats.physical_id, true, *histogram_id, bucket_id as i64),
                    encode_bucket(bucket),
                )?;
            }
        }
        Ok(())
    }

    /// 在锁表记录上饱和累加 delta；表未锁则报错。
    fn add_locked_delta(
        &mut self,
        physical_id: i64,
        row_delta: i64,
        modify_count: i64,
    ) -> Result<(), StatsError> {
        let key = record_key(LOCKED, physical_id);
        let value = get(self.transaction.as_ref(), key.clone())?
            .ok_or_else(|| StatsError(format!("statistics table {physical_id} is not locked")))?;
        let mut record = decode_locked(physical_id, &value)?;
        record.count = record.count.saturating_add(row_delta);
        record.modify_count = record.modify_count.saturating_add(modify_count);
        set(self.transaction.as_mut(), key, encode_locked(&record))
    }

    /// 先读本事务 pending_meta，再回落 KV。
    fn read_meta(&self, table_id: i64) -> Result<Option<StatsMetaRecord>, StatsError> {
        if let Some(record) = self.pending_meta.get(&table_id) {
            return Ok(Some(record.clone()));
        }
        get(self.transaction.as_ref(), record_key(META, table_id))?
            .map(|value| decode_meta(table_id, &value))
            .transpose()
    }

    /// 写 meta 到 KV，并记入 pending_meta。
    fn write_meta(&mut self, record: StatsMetaRecord) -> Result<(), StatsError> {
        set(
            self.transaction.as_mut(),
            record_key(META, record.table_id),
            encode_meta(&record),
        )?;
        self.pending_meta.insert(record.table_id, record);
        Ok(())
    }

    /// 删除该表所有统计前缀；可选标记 Handle 缓存移除。
    fn delete_table_records(
        &mut self,
        table_id: i64,
        remove_handle: bool,
    ) -> Result<(), StatsError> {
        for table in [
            META,
            LOCKED,
            HISTOGRAM,
            FM_SKETCH,
            BUCKET,
            META_HISTORY,
            HISTORY,
        ] {
            if table == META || table == LOCKED {
                delete(self.transaction.as_mut(), record_key(table, table_id))?;
            } else {
                delete_prefix(self.transaction.as_mut(), &table_id_prefix(table, table_id))?;
            }
        }
        self.pending_meta.remove(&table_id);
        self.pending_cache_meta.remove(&table_id);
        if remove_handle {
            self.removed_tables.insert(table_id);
        }
        Ok(())
    }

    /// 解析 SELECT 目标系统表，过滤 WHERE 后投影列。
    fn query(&self, sql: &str) -> Result<Vec<SqlRow>, StatsError> {
        let lower = normalize(sql);
        let table = sql_table(&lower)
            .ok_or_else(|| StatsError(format!("unsupported restricted statistics SQL: {sql}")))?;
        // 按系统表名分发到对应 KV 扫描路径。
        let rows = match table {
            "stats_table_locked" => self.locked_rows(None)?,
            "stats_meta" => self.meta_rows(None)?,
            "stats_histograms" => self.histogram_rows(None)?,
            "stats_fm_sketch" => self.fm_sketch_rows(None)?,
            "stats_buckets" => self.bucket_rows(None)?,
            "stats_meta_history" => self.history_rows(META_HISTORY, None)?,
            "stats_history" => self.history_rows(HISTORY, None)?,
            "tidb" => self.system_rows()?,
            _ => {
                return Err(StatsError(format!(
                    "unsupported restricted statistics table mysql.{table}"
                )));
            }
        };
        project_rows(&lower, table, filter_rows(&lower, rows)?)
    }

    /// 扫描 `mysql.tidb` 系统变量行。
    fn system_rows(&self) -> Result<Vec<BTreeMap<&'static str, SqlValue>>, StatsError> {
        scan(self.transaction.as_ref(), &prefix(SYSTEM))?
            .into_iter()
            .map(|(key, value)| {
                let name = decode_system_name(&key)?;
                let value = String::from_utf8(value)
                    .map_err(|error| StatsError(format!("invalid system value: {error}")))?;
                Ok(BTreeMap::from([
                    ("variable_name", SqlValue::Text(name.clone())),
                    ("variable_value", SqlValue::Text(value)),
                    (
                        "comment",
                        SqlValue::Text(
                            get(self.transaction.as_ref(), system_comment_key(&name))?
                                .map(String::from_utf8)
                                .transpose()
                                .map_err(|e| StatsError(format!("invalid system comment: {e}")))?
                                .unwrap_or_default(),
                        ),
                    ),
                ]))
            })
            .collect()
    }

    /// 读锁表行为列映射（可选按 table_id 过滤）。
    fn locked_rows(
        &self,
        table_id: Option<i64>,
    ) -> Result<Vec<BTreeMap<&'static str, SqlValue>>, StatsError> {
        let rows = if let Some(table_id) = table_id {
            get(self.transaction.as_ref(), record_key(LOCKED, table_id))?
                .map(|value| decode_locked(table_id, &value))
                .transpose()?
                .into_iter()
                .collect()
        } else {
            scan(self.transaction.as_ref(), &prefix(LOCKED))?
                .into_iter()
                .map(|(key, value)| {
                    decode_record_id(LOCKED, &key).and_then(|id| decode_locked(id, &value))
                })
                .collect::<Result<Vec<_>, _>>()?
        };
        Ok(rows
            .into_iter()
            .map(|row| {
                BTreeMap::from([
                    ("table_id", SqlValue::Int(row.table_id)),
                    ("count", SqlValue::Int(row.count)),
                    ("modify_count", SqlValue::Int(row.modify_count)),
                    ("version", SqlValue::UInt(0)),
                ])
            })
            .collect())
    }

    /// 读 meta 行为列映射。
    fn meta_rows(
        &self,
        table_id: Option<i64>,
    ) -> Result<Vec<BTreeMap<&'static str, SqlValue>>, StatsError> {
        let rows = self.meta_records(table_id)?;
        Ok(rows
            .into_iter()
            .map(|row| {
                BTreeMap::from([
                    ("table_id", SqlValue::Int(row.table_id)),
                    ("version", SqlValue::UInt(row.version)),
                    ("count", SqlValue::Int(row.count)),
                    ("modify_count", SqlValue::Int(row.modify_count)),
                    (
                        "snapshot",
                        SqlValue::UInt(
                            (row.last_histogram_version != 0)
                                .then_some(row.version)
                                .unwrap_or_default(),
                        ),
                    ),
                    (
                        "last_stats_histograms_version",
                        SqlValue::UInt(row.last_histogram_version),
                    ),
                ])
            })
            .collect())
    }

    /// 读 `StatsMetaRecord` 列表。
    fn meta_records(&self, table_id: Option<i64>) -> Result<Vec<StatsMetaRecord>, StatsError> {
        if let Some(table_id) = table_id {
            Ok(self.read_meta(table_id)?.into_iter().collect())
        } else {
            scan(self.transaction.as_ref(), &prefix(META))?
                .into_iter()
                .map(|(key, value)| {
                    decode_record_id(META, &key).and_then(|id| decode_meta(id, &value))
                })
                .collect()
        }
    }

    /// 读直方图身份行为列映射。
    fn histogram_rows(
        &self,
        table_id: Option<i64>,
    ) -> Result<Vec<BTreeMap<&'static str, SqlValue>>, StatsError> {
        let prefix = table_id
            .map(histogram_table_prefix)
            .unwrap_or_else(|| prefix(HISTOGRAM));
        Ok(scan(self.transaction.as_ref(), &prefix)?
            .into_iter()
            .map(|(key, value)| {
                let row = decode_histogram_key(&key)?;
                let payload = decode_histogram_payload(&value)?;
                Ok(BTreeMap::from([
                    ("table_id", SqlValue::Int(row.table_id)),
                    ("is_index", SqlValue::Int(i64::from(row.is_index))),
                    ("hist_id", SqlValue::Int(row.histogram_id)),
                    ("distinct_count", SqlValue::Int(payload.ndv)),
                    ("null_count", SqlValue::Int(payload.null_count)),
                    ("tot_col_size", SqlValue::Int(payload.total_column_size)),
                    ("modify_count", SqlValue::Int(0)),
                    ("version", SqlValue::UInt(payload.version)),
                    ("cm_sketch", SqlValue::Text(String::new())),
                    ("stats_ver", SqlValue::Int(payload.stats_version)),
                    ("flag", SqlValue::Int(0)),
                    (
                        "correlation",
                        SqlValue::Text(payload.correlation.to_string()),
                    ),
                    ("last_analyze_pos", SqlValue::Text(String::new())),
                ]))
            })
            .collect::<Result<Vec<_>, StatsError>>()?)
    }

    /// 读 FM Sketch 行；value 以十六进制文本暴露。
    fn fm_sketch_rows(
        &self,
        table_id: Option<i64>,
    ) -> Result<Vec<BTreeMap<&'static str, SqlValue>>, StatsError> {
        let prefix = table_id
            .map(|id| table_id_prefix(FM_SKETCH, id))
            .unwrap_or_else(|| prefix(FM_SKETCH));
        scan(self.transaction.as_ref(), &prefix)?
            .into_iter()
            .map(|(key, value)| {
                let (table_id, is_index, histogram_id) = decode_fm_sketch_key(&key)?;
                Ok(BTreeMap::from([
                    ("table_id", SqlValue::Int(table_id)),
                    ("is_index", SqlValue::Int(i64::from(is_index))),
                    ("hist_id", SqlValue::Int(histogram_id)),
                    // FM Sketch 为二进制，以 hex 暴露以免文本面损坏。
                    // FM sketches are binary payloads; expose them hex encoded
                    // like `mysql.stats_buckets` bounds so the textual SQL
                    // surface never mangles them.
                    ("value", SqlValue::Text(hex_encode(&value))),
                ]))
            })
            .collect()
    }

    /// 读 bucket 行；边界以 hex 文本暴露。
    fn bucket_rows(
        &self,
        table_id: Option<i64>,
    ) -> Result<Vec<BTreeMap<&'static str, SqlValue>>, StatsError> {
        let prefix = table_id
            .map(|id| table_id_prefix(BUCKET, id))
            .unwrap_or_else(|| prefix(BUCKET));
        scan(self.transaction.as_ref(), &prefix)?
            .into_iter()
            .map(|(key, value)| {
                let (table_id, is_index, histogram_id, bucket_id) = decode_bucket_key(&key)?;
                let bucket = decode_bucket(&value)?;
                Ok(BTreeMap::from([
                    ("table_id", SqlValue::Int(table_id)),
                    ("is_index", SqlValue::Int(i64::from(is_index))),
                    ("hist_id", SqlValue::Int(histogram_id)),
                    ("bucket_id", SqlValue::Int(bucket_id)),
                    ("count", SqlValue::Int(bucket.count)),
                    ("repeats", SqlValue::Int(bucket.repeats)),
                    ("ndv", SqlValue::Int(bucket.ndv)),
                    ("lower_bound", SqlValue::Text(hex_encode(&bucket.lower))),
                    ("upper_bound", SqlValue::Text(hex_encode(&bucket.upper))),
                ]))
            })
            .collect()
    }

    /// 就地改写 bucket 上下界（供测试注入损坏直方图）。
    /// Rewrites `lower_bound`/`upper_bound` blobs in place, mirroring the
    /// `UPDATE mysql.stats_buckets SET ...` statement Go tests use to corrupt
    /// persisted histograms. The assigned literal is stored verbatim so that
    /// readers observe exactly the bytes the statement wrote.
    fn update_bucket_bounds(
        &mut self,
        predicate: &str,
        lower: Option<&str>,
        upper: Option<&str>,
    ) -> Result<(), StatsError> {
        for (key, value) in scan(self.transaction.as_ref(), &prefix(BUCKET))? {
            let (table_id, is_index, histogram_id, bucket_id) = decode_bucket_key(&key)?;
            let mut bucket = decode_bucket(&value)?;
            let row = BTreeMap::from([
                ("table_id", SqlValue::Int(table_id)),
                ("is_index", SqlValue::Int(i64::from(is_index))),
                ("hist_id", SqlValue::Int(histogram_id)),
                ("bucket_id", SqlValue::Int(bucket_id)),
                ("count", SqlValue::Int(bucket.count)),
                ("repeats", SqlValue::Int(bucket.repeats)),
                ("ndv", SqlValue::Int(bucket.ndv)),
            ]);
            if !row_matches(predicate, &row)? {
                continue;
            }
            if let Some(lower) = lower {
                bucket.lower = lower.as_bytes().to_vec();
            }
            if let Some(upper) = upper {
                bucket.upper = upper.as_bytes().to_vec();
            }
            set(self.transaction.as_mut(), key, encode_bucket(&bucket))?;
        }
        Ok(())
    }

    /// 读 history / meta_history 行。
    fn history_rows(
        &self,
        table: &[u8],
        table_id: Option<i64>,
    ) -> Result<Vec<BTreeMap<&'static str, SqlValue>>, StatsError> {
        let prefix = table_id
            .map(|id| table_id_prefix(table, id))
            .unwrap_or_else(|| prefix(table));
        Ok(scan(self.transaction.as_ref(), &prefix)?
            .into_iter()
            .map(|(key, _)| {
                let fields = decode_history_key(table, &key)?;
                let mut row = BTreeMap::from([
                    ("table_id", SqlValue::Int(fields.0)),
                    ("version", SqlValue::UInt(fields.1)),
                    ("seq_no", SqlValue::UInt(fields.2)),
                    ("create_time", SqlValue::UInt(fields.3)),
                ]);
                if table == META_HISTORY {
                    row.insert("modify_count", SqlValue::Int(0));
                    row.insert("count", SqlValue::Int(0));
                    row.insert("source", SqlValue::Text(String::new()));
                } else {
                    row.insert("stats_data", SqlValue::Text(String::new()));
                }
                Ok(row)
            })
            .collect::<Result<Vec<_>, StatsError>>()?)
    }

    /// 按 WHERE/LIMIT 删除直方图身份行。
    fn delete_histogram_rows(&mut self, sql: &str) -> Result<(), StatsError> {
        let limit = sql_limit(sql).unwrap_or(usize::MAX);
        let mut deleted = 0;
        for (key, _) in scan(self.transaction.as_ref(), &prefix(HISTOGRAM))? {
            let record = decode_histogram_key(&key)?;
            let row = BTreeMap::from([
                ("table_id", SqlValue::Int(record.table_id)),
                ("is_index", SqlValue::Int(i64::from(record.is_index))),
                ("hist_id", SqlValue::Int(record.histogram_id)),
                ("column_id", SqlValue::Int(record.histogram_id)),
            ]);
            if row_matches(sql, &row)? && deleted < limit {
                delete(self.transaction.as_mut(), key)?;
                deleted += 1;
            }
        }
        Ok(())
    }

    /// 按 WHERE/LIMIT 删除 FM Sketch 行。
    fn delete_fm_sketch_rows(&mut self, sql: &str) -> Result<(), StatsError> {
        let limit = sql_limit(sql).unwrap_or(usize::MAX);
        let mut deleted = 0;
        for (key, _) in scan(self.transaction.as_ref(), &prefix(FM_SKETCH))? {
            let (table_id, is_index, histogram_id) = decode_fm_sketch_key(&key)?;
            let row = BTreeMap::from([
                ("table_id", SqlValue::Int(table_id)),
                ("is_index", SqlValue::Int(i64::from(is_index))),
                ("hist_id", SqlValue::Int(histogram_id)),
            ]);
            if row_matches(sql, &row)? && deleted < limit {
                delete(self.transaction.as_mut(), key)?;
                deleted += 1;
            }
        }
        Ok(())
    }

    /// 按 WHERE/LIMIT 删除 history 行。
    fn delete_history_rows(&mut self, table: &[u8], sql: &str) -> Result<(), StatsError> {
        let limit = sql_limit(sql).unwrap_or(usize::MAX);
        let mut deleted = 0;
        for (key, _) in scan(self.transaction.as_ref(), &prefix(table))? {
            let fields = decode_history_key(table, &key)?;
            let row = BTreeMap::from([
                ("table_id", SqlValue::Int(fields.0)),
                ("version", SqlValue::UInt(fields.1)),
                ("seq_no", SqlValue::UInt(fields.2)),
                ("create_time", SqlValue::UInt(fields.3)),
            ]);
            if row_matches(sql, &row)? && deleted < limit {
                delete(self.transaction.as_mut(), key)?;
                deleted += 1;
            }
        }
        Ok(())
    }

    /// 分派受限 DML：锁表、meta 更新、history 插入、bucket 改写等。
    fn mutate(&mut self, sql: &str, arguments: &[SqlValue]) -> Result<(), StatsError> {
        let lower = normalize(sql);
        // 下列分支覆盖 Go 受限 SQL 模板；未识别语句返回 unsupported。
        if lower.starts_with("insert into mysql.stats_table_locked") {
            let table_id = argument_i64(arguments, 0)
                .or_else(|| values_numbers(&lower).first().copied())
                .ok_or_else(|| StatsError("missing statistics lock table ID".to_owned()))?;
            let record = StatsLockedRecord {
                table_id,
                ..Default::default()
            };
            return set(
                self.transaction.as_mut(),
                record_key(LOCKED, table_id),
                encode_locked(&record),
            );
        }
        if lower.starts_with("update mysql.stats_meta") && lower.contains("count = if") {
            let version = argument_u64(arguments, 0).unwrap_or(self.start_ts);
            let count = argument_i64(arguments, 1).unwrap_or_default();
            let modify = argument_i64(arguments, 3).unwrap_or_default();
            let table_id = argument_i64(arguments, 4)
                .or_else(|| sql_i64_after(&lower, "table_id"))
                .ok_or_else(|| StatsError("missing statistics meta table ID".to_owned()))?;
            let Some(mut meta) = self.read_meta(table_id)? else {
                // SQL UPDATE over a missing stats_meta row affects zero rows.
                // LOCK STATS may legitimately precede statistics DDL-event
                // handling, so the lock record must still be committed.
                return Ok(());
            };
            meta.version = version;
            meta.count = meta.count.saturating_add(count).max(0);
            meta.modify_count = meta.modify_count.saturating_add(modify);
            self.write_meta(meta.clone())?;
            self.pending_cache_meta.insert(table_id, meta);
            return Ok(());
        }
        if lower.starts_with("update mysql.stats_meta") {
            let table_id = argument_i64(arguments, arguments.len().saturating_sub(1))
                .or_else(|| sql_i64_after(&lower, "table_id"))
                .ok_or_else(|| StatsError("missing statistics meta table ID".to_owned()))?;
            let Some(mut meta) = self.read_meta(table_id)? else {
                return Ok(());
            };
            if let Some(version) =
                argument_u64(arguments, 0).or_else(|| sql_u64_after(&lower, "version"))
            {
                meta.version = version;
                meta.last_histogram_version = meta.version;
            }
            if let Some(count) = sql_i64_after(&lower, "count") {
                meta.count = count.max(0);
            }
            if let Some(modify_count) = sql_i64_after(&lower, "modify_count") {
                meta.modify_count = modify_count;
            }
            self.write_meta(meta.clone())?;
            self.pending_cache_meta.insert(table_id, meta);
            return Ok(());
        }
        if lower.starts_with("delete from mysql.stats_table_locked") {
            if self.fail_next_lock_delete.swap(false, Ordering::AcqRel) {
                return Err(StatsError(
                    "injected statistics lock delete failure".to_owned(),
                ));
            }
            let table_id = argument_i64(arguments, 0)
                .or_else(|| sql_i64_after(&lower, "table_id"))
                .ok_or_else(|| StatsError("missing statistics lock table ID".to_owned()))?;
            return delete(self.transaction.as_mut(), record_key(LOCKED, table_id));
        }
        if lower == "delete from mysql.stats_meta"
            || lower.starts_with("delete from mysql.stats_meta;")
        {
            let table_ids = self
                .meta_rows(None)?
                .into_iter()
                .filter_map(|row| row.get("table_id").map(|value| value.int()))
                .collect::<BTreeSet<_>>();
            for table_id in table_ids {
                delete(self.transaction.as_mut(), record_key(META, table_id))?;
                self.pending_meta.remove(&table_id);
                self.pending_cache_meta.remove(&table_id);
                self.removed_tables.insert(table_id);
            }
            return Ok(());
        }
        if lower.starts_with("delete from mysql.stats_meta where") {
            let table_id = sql_i64_after(&lower, "table_id")
                .ok_or_else(|| StatsError("missing statistics meta table ID".to_owned()))?;
            delete(self.transaction.as_mut(), record_key(META, table_id))?;
            self.pending_meta.remove(&table_id);
            self.pending_cache_meta.remove(&table_id);
            self.removed_tables.insert(table_id);
            return Ok(());
        }
        if lower.starts_with("delete from mysql.stats_histograms") {
            return self.delete_histogram_rows(&lower);
        }
        if lower.starts_with("delete from mysql.stats_meta_history") {
            return self.delete_history_rows(META_HISTORY, &lower);
        }
        if lower.starts_with("delete from mysql.stats_history") {
            return self.delete_history_rows(HISTORY, &lower);
        }
        if lower.starts_with("insert into mysql.stats_meta_history")
            || lower.starts_with("insert into mysql.stats_history")
        {
            let table = if lower.starts_with("insert into mysql.stats_meta_history") {
                META_HISTORY
            } else {
                HISTORY
            };
            // 对齐 Go：history 与 meta_history 列序不同，按列名绑定。
            // Go writes `mysql.stats_history` as
            // `(table_id, stats_data, seq_no, version, create_time)` while
            // `mysql.stats_meta_history` uses `(table_id, modify_count, count,
            // version, source, create_time)`, so bind values by column name
            // instead of by tuple position.
            let names = insert_columns(&lower);
            for values in values_raw_tuples(&lower) {
                let field = |name: &str| -> Option<i64> {
                    let index = names.iter().position(|column| column == name)?;
                    values.get(index)?.trim().trim_matches('\'').parse().ok()
                };
                let numbers = values
                    .iter()
                    .filter_map(|value| value.trim().trim_matches('\'').parse::<i64>().ok())
                    .collect::<Vec<_>>();
                let table_id = field("table_id")
                    .or_else(|| numbers.first().copied())
                    .ok_or_else(|| StatsError("invalid statistics history insert".to_owned()))?;
                let version = field("version")
                    .or_else(|| numbers.get(1).copied())
                    .ok_or_else(|| StatsError("invalid statistics history insert".to_owned()))?;
                let sequence = if table == META_HISTORY {
                    0
                } else {
                    field("seq_no")
                        .or_else(|| numbers.get(2).copied())
                        .unwrap_or_default()
                };
                let create_time = field("create_time")
                    .or_else(|| {
                        numbers
                            .get(if table == META_HISTORY { 2 } else { 3 })
                            .copied()
                    })
                    .unwrap_or_default();
                let key = history_key(
                    table,
                    table_id,
                    version.max(0) as u64,
                    sequence.max(0) as u64,
                    create_time.max(0) as u64,
                );
                set(self.transaction.as_mut(), key, Vec::from([0]))?;
            }
            return Ok(());
        }
        // mysql.tidb has a unique VARIABLE_NAME. Plain INSERT must surface
        // duplicate keys; IGNORE preserves the existing row, whereas version
        // writes explicitly request ON DUPLICATE KEY UPDATE.
        if lower.starts_with("insert into mysql.tidb")
            || lower.starts_with("insert ignore into mysql.tidb")
            || lower.starts_with("insert high_priority into mysql.tidb")
        {
            let values = quoted_values(&lower);
            let name = values
                .first()
                .ok_or_else(|| StatsError("missing system variable name".into()))?;
            let value = values
                .get(1)
                .ok_or_else(|| StatsError("missing system variable value".into()))?;
            let key = system_key(name);
            let existing = get(self.transaction.as_ref(), key.clone())?.is_some();
            if existing {
                if lower.starts_with("insert ignore ") {
                    return Ok(());
                }
                if !lower.contains(" on duplicate key update ") {
                    return Err(StatsError(format!(
                        "[kv:1062]Duplicate entry '{name}' for key 'tidb.PRIMARY'"
                    )));
                }
            }
            set(self.transaction.as_mut(), key, value.as_bytes().to_vec())?;
            // Keep the bootstrap version comment while updating only its value.
            if !existing {
                if let Some(comment) = values.get(2) {
                    set(
                        self.transaction.as_mut(),
                        system_comment_key(name),
                        comment.as_bytes().to_vec(),
                    )?;
                }
            }
            return Ok(());
        }
        if lower.starts_with("delete from mysql.tidb") {
            let values = quoted_values(&lower);
            let name = values
                .first()
                .ok_or_else(|| StatsError("missing system variable name".to_owned()))?;
            delete(self.transaction.as_mut(), system_comment_key(name))?;
            return delete(self.transaction.as_mut(), system_key(name));
        }
        if lower.starts_with("update mysql.tidb") {
            let value = assignment_literals(&lower)
                .into_iter()
                .find_map(|(column, value)| (column == "variable_value").then_some(value))
                .ok_or_else(|| StatsError("missing system variable value assignment".to_owned()))?;
            let names = self
                .system_rows()?
                .into_iter()
                .filter_map(|row| match row_matches(&lower, &row) {
                    Ok(true) => match row.get("variable_name") {
                        Some(SqlValue::Text(name)) => Some(Ok(name.clone())),
                        _ => Some(Err(StatsError(
                            "invalid mysql.tidb variable name".to_owned(),
                        ))),
                    },
                    Ok(false) => None,
                    Err(error) => Some(Err(error)),
                })
                .collect::<Result<Vec<_>, _>>()?;
            for name in names {
                set(
                    self.transaction.as_mut(),
                    system_key(&name),
                    value.as_bytes().to_vec(),
                )?;
            }
            return Ok(());
        }
        if lower.starts_with("delete from mysql.stats_buckets")
            || lower.starts_with("delete from mysql.stats_top_n")
            || lower.starts_with("delete from mysql.column_stats_usage")
            || lower.starts_with("delete from mysql.analyze_options")
        {
            return Ok(());
        }
        if lower.starts_with("delete from mysql.stats_fm_sketch") {
            return self.delete_fm_sketch_rows(&lower);
        }
        if lower.starts_with("update mysql.stats_buckets") {
            let assignments = assignment_literals(&lower);
            return self.update_bucket_bounds(
                &lower,
                assignments
                    .iter()
                    .find(|(column, _)| column == "lower_bound")
                    .map(|(_, value)| value.as_str()),
                assignments
                    .iter()
                    .find(|(column, _)| column == "upper_bound")
                    .map(|(_, value)| value.as_str()),
            );
        }
        if lower.starts_with("update mysql.stats_histograms") {
            // Histogram membership is encoded by the key; stats_ver is kept
            // in the persisted profile and invalidated by Domain before this
            // restricted statement reaches the store.
            return Ok(());
        }
        Err(StatsError(format!(
            "unsupported restricted statistics SQL: {sql}"
        )))
    }
}

/// `RestrictedSQLExecutor`：SELECT 走 query，其余走 mutate。
impl<B: HandleBackend + Send> RestrictedSQLExecutor for KvRestrictedExecutor<'_, B> {
    /// 执行受限 SQL；SELECT 先把 `%?` 占位符替换为字面量。
    fn ExecRestrictedSQL(
        &mut self,
        sql: &str,
        arguments: &[SqlValue],
    ) -> Result<Vec<SqlRow>, StatsError> {
        if normalize(sql).starts_with("select ") {
            let mut rendered = sql.to_owned();
            for argument in arguments {
                rendered = rendered.replacen("%?", &render_argument(argument), 1);
            }
            self.query(&rendered)
        } else {
            self.mutate(sql, arguments)?;
            Ok(Vec::new())
        }
    }

    /// 本事务起始时间戳（与统计版本对齐）。
    fn StartTS(&self) -> u64 {
        self.start_ts
    }

    /// 扫描锁表前缀，返回全部 table_id。
    fn LockedTableIds(&mut self) -> Result<Vec<i64>, StatsError> {
        scan(self.transaction.as_ref(), &prefix(LOCKED))?
            .into_iter()
            .map(|(key, _)| decode_record_id(LOCKED, &key))
            .collect()
    }

    /// 若不存在则插入空的锁表记录。
    fn InsertStatsLock(&mut self, table_id: i64) -> Result<(), StatsError> {
        let key = record_key(LOCKED, table_id);
        if get(self.transaction.as_ref(), key.clone())?.is_none() {
            let record = StatsLockedRecord {
                table_id,
                ..Default::default()
            };
            set(self.transaction.as_mut(), key, encode_locked(&record))?;
        }
        Ok(())
    }

    /// 将 meta.version 提升为当前 start_ts。
    fn UpdateStatsMetaVersion(&mut self, table_id: i64) -> Result<(), StatsError> {
        let Some(mut meta) = self.read_meta(table_id)? else {
            // Restricted UPDATE follows SQL semantics: a missing row is a
            // successful no-op, which lets LOCK STATS precede DDL-event stats.
            return Ok(());
        };
        meta.version = self.start_ts;
        self.write_meta(meta)
    }

    /// 读取锁表上的 (count, modify_count)；无记录返回 (0,0)。
    fn LockedStatsDelta(&mut self, table_id: i64) -> Result<(i64, i64), StatsError> {
        let Some(value) = get(self.transaction.as_ref(), record_key(LOCKED, table_id))? else {
            return Ok((0, 0));
        };
        let record = decode_locked(table_id, &value)?;
        Ok((record.count, record.modify_count))
    }

    /// 把 delta 合并进 stats_meta，并标记缓存待更新。
    fn ApplyStatsDelta(
        &mut self,
        table_id: i64,
        count: i64,
        modify_count: i64,
    ) -> Result<(), StatsError> {
        let Some(mut meta) = self.read_meta(table_id)? else {
            return Ok(());
        };
        meta.version = self.start_ts;
        meta.count = meta.count.saturating_add(count).max(0);
        meta.modify_count = meta.modify_count.saturating_add(modify_count);
        self.write_meta(meta.clone())?;
        self.pending_cache_meta.insert(table_id, meta);
        Ok(())
    }

    /// 删除锁表记录（支持失败注入）。
    fn DeleteStatsLock(&mut self, table_id: i64) -> Result<(), StatsError> {
        if self.fail_next_lock_delete.swap(false, Ordering::AcqRel) {
            return Err(StatsError(
                "injected statistics lock delete failure".to_owned(),
            ));
        }
        delete(self.transaction.as_mut(), record_key(LOCKED, table_id))
    }
}

/// 将任意错误转为 `StatsError`。
fn stats_error(error: impl ToString) -> StatsError {
    StatsError(error.to_string())
}

/// 拼出某逻辑表在 ROOT 下的前缀字节。
fn prefix(table: &[u8]) -> Vec<u8> {
    [ROOT, table].concat()
}

/// 单记录键：`prefix(table) || table_id(be)`。
fn record_key(table: &[u8], table_id: i64) -> kv::Key {
    kv::Key([prefix(table), table_id.to_be_bytes().to_vec()].concat())
}

/// 系统变量键：`system/ || name`。
fn system_key(name: &str) -> kv::Key {
    kv::Key([prefix(SYSTEM), name.as_bytes().to_vec()].concat())
}

fn system_comment_key(name: &str) -> kv::Key {
    kv::Key([prefix(b"system-comment/"), name.as_bytes().to_vec()].concat())
}

/// 从系统变量键解码变量名。
fn decode_system_name(key: &kv::Key) -> Result<String, StatsError> {
    let prefix = prefix(SYSTEM);
    let name = key
        .as_ref()
        .strip_prefix(prefix.as_slice())
        .ok_or_else(|| StatsError("invalid system record key".to_owned()))?;
    String::from_utf8(name.to_vec())
        .map_err(|error| StatsError(format!("invalid system variable name: {error}")))
}

/// 某表下多行记录的公共前缀。
fn table_id_prefix(table: &[u8], table_id: i64) -> Vec<u8> {
    record_key(table, table_id).0
}

/// 某物理表直方图前缀。
fn histogram_table_prefix(table_id: i64) -> Vec<u8> {
    table_id_prefix(HISTOGRAM, table_id)
}

/// 直方图身份键：table / is_index / hist_id。
fn histogram_key(record: &StatsHistogramRecord) -> kv::Key {
    kv::Key(
        [
            histogram_table_prefix(record.table_id),
            vec![u8::from(record.is_index)],
            record.histogram_id.to_be_bytes().to_vec(),
        ]
        .concat(),
    )
}

/// FM Sketch 键。
fn fm_sketch_key(table_id: i64, is_index: bool, histogram_id: i64) -> kv::Key {
    kv::Key(
        [
            table_id_prefix(FM_SKETCH, table_id),
            vec![u8::from(is_index)],
            histogram_id.to_be_bytes().to_vec(),
        ]
        .concat(),
    )
}

/// 某直方图全部 bucket 的前缀。
fn bucket_histogram_prefix(table_id: i64, is_index: bool, histogram_id: i64) -> Vec<u8> {
    [
        table_id_prefix(BUCKET, table_id),
        vec![u8::from(is_index)],
        histogram_id.to_be_bytes().to_vec(),
    ]
    .concat()
}

/// 单个 bucket 键。
fn bucket_key(table_id: i64, is_index: bool, histogram_id: i64, bucket_id: i64) -> kv::Key {
    kv::Key(
        [
            table_id_prefix(BUCKET, table_id),
            vec![u8::from(is_index)],
            histogram_id.to_be_bytes().to_vec(),
            bucket_id.to_be_bytes().to_vec(),
        ]
        .concat(),
    )
}

/// 编码 bucket：count/repeats/ndv + 变长 lower/upper。
fn encode_bucket(bucket: &crate::Bucket) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(40 + bucket.lower.len() + bucket.upper.len());
    encoded.extend_from_slice(&bucket.count.to_be_bytes());
    encoded.extend_from_slice(&bucket.repeats.to_be_bytes());
    encoded.extend_from_slice(&bucket.ndv.to_be_bytes());
    encoded.extend_from_slice(&(bucket.lower.len() as u64).to_be_bytes());
    encoded.extend_from_slice(&bucket.lower);
    encoded.extend_from_slice(&(bucket.upper.len() as u64).to_be_bytes());
    encoded.extend_from_slice(&bucket.upper);
    encoded
}

/// 解码 bucket；长度不匹配则报 invalid。
fn decode_bucket(value: &[u8]) -> Result<crate::Bucket, StatsError> {
    let invalid = || StatsError("invalid statistics bucket record".to_owned());
    let read_i64 = |at: usize| -> Result<i64, StatsError> {
        value
            .get(at..at + 8)
            .and_then(|bytes| bytes.try_into().ok())
            .map(i64::from_be_bytes)
            .ok_or_else(invalid)
    };
    let count = read_i64(0)?;
    let repeats = read_i64(8)?;
    let ndv = read_i64(16)?;
    let lower_len = read_i64(24)?.max(0) as usize;
    let upper_length_offset = 32 + lower_len;
    let upper_len = read_i64(upper_length_offset)?.max(0) as usize;
    let upper_start = upper_length_offset + 8;
    if value.len() != upper_start + upper_len {
        return Err(invalid());
    }
    Ok(crate::Bucket {
        count,
        repeats,
        ndv,
        lower: value[32..upper_length_offset].to_vec(),
        upper: value[upper_start..].to_vec(),
    })
}

/// 字节转大写十六进制文本。
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

/// history 键：table_id / version / seq_no / create_time。
fn history_key(
    table: &[u8],
    table_id: i64,
    version: u64,
    sequence: u64,
    create_time: u64,
) -> kv::Key {
    kv::Key(
        [
            table_id_prefix(table, table_id),
            version.to_be_bytes().to_vec(),
            sequence.to_be_bytes().to_vec(),
            create_time.to_be_bytes().to_vec(),
        ]
        .concat(),
    )
}

/// 编码 meta：四字段各 8 字节大端。
fn encode_meta(record: &StatsMetaRecord) -> Vec<u8> {
    [
        record.version.to_be_bytes().as_slice(),
        record.count.to_be_bytes().as_slice(),
        record.modify_count.to_be_bytes().as_slice(),
        record.last_histogram_version.to_be_bytes().as_slice(),
    ]
    .concat()
}

/// 编码完整直方图元数据；首字节保留旧格式的 analyzed 标记，便于 lite 加载。
fn encode_histogram_payload(payload: &StatsHistogramPayload) -> Vec<u8> {
    [
        vec![u8::from(payload.analyzed)],
        payload.stats_version.to_be_bytes().to_vec(),
        payload.version.to_be_bytes().to_vec(),
        payload.ndv.to_be_bytes().to_vec(),
        payload.null_count.to_be_bytes().to_vec(),
        payload.total_column_size.to_be_bytes().to_vec(),
        payload.correlation.to_bits().to_be_bytes().to_vec(),
    ]
    .concat()
}

/// 解码直方图元数据；兼容历史单字节 analyzed 记录。
fn decode_histogram_payload(value: &[u8]) -> Result<StatsHistogramPayload, StatsError> {
    let analyzed = value.first().is_some_and(|flag| *flag != 0);
    if value.len() <= 1 {
        return Ok(StatsHistogramPayload {
            analyzed,
            ..StatsHistogramPayload::default()
        });
    }
    if value.len() != 49 {
        return Err(StatsError(
            "invalid statistics histogram payload".to_owned(),
        ));
    }
    Ok(StatsHistogramPayload {
        analyzed,
        stats_version: i64::from_be_bytes(value[1..9].try_into().expect("histogram stats version")),
        version: u64::from_be_bytes(value[9..17].try_into().expect("histogram version")),
        ndv: i64::from_be_bytes(value[17..25].try_into().expect("histogram NDV")),
        null_count: i64::from_be_bytes(value[25..33].try_into().expect("histogram null count")),
        total_column_size: i64::from_be_bytes(
            value[33..41]
                .try_into()
                .expect("histogram total column size"),
        ),
        correlation: f64::from_bits(u64::from_be_bytes(
            value[41..49].try_into().expect("histogram correlation"),
        )),
    })
}

/// 解码 meta；固定 32 字节。
fn decode_meta(table_id: i64, value: &[u8]) -> Result<StatsMetaRecord, StatsError> {
    if value.len() != 32 {
        return Err(StatsError("invalid statistics meta record".to_owned()));
    }
    Ok(StatsMetaRecord {
        table_id,
        version: u64::from_be_bytes(value[0..8].try_into().expect("meta version")),
        count: i64::from_be_bytes(value[8..16].try_into().expect("meta count")),
        modify_count: i64::from_be_bytes(value[16..24].try_into().expect("meta modify count")),
        last_histogram_version: u64::from_be_bytes(
            value[24..32].try_into().expect("meta histogram version"),
        ),
    })
}

/// 编码锁表记录：count + modify_count。
fn encode_locked(record: &StatsLockedRecord) -> Vec<u8> {
    [
        record.count.to_be_bytes().as_slice(),
        record.modify_count.to_be_bytes().as_slice(),
    ]
    .concat()
}

/// 解码锁表记录。
fn decode_locked(table_id: i64, value: &[u8]) -> Result<StatsLockedRecord, StatsError> {
    if value.len() != 16 {
        return Err(StatsError("invalid statistics lock record".to_owned()));
    }
    Ok(StatsLockedRecord {
        table_id,
        count: i64::from_be_bytes(value[0..8].try_into().expect("lock count")),
        modify_count: i64::from_be_bytes(value[8..16].try_into().expect("lock modify count")),
    })
}

/// 从单记录键末尾解析 table_id。
fn decode_record_id(table: &[u8], key: &kv::Key) -> Result<i64, StatsError> {
    let prefix = prefix(table);
    let bytes = key
        .as_ref()
        .strip_prefix(prefix.as_slice())
        .ok_or_else(|| StatsError("invalid statistics record key".to_owned()))?;
    let id = bytes
        .get(..8)
        .ok_or_else(|| StatsError("invalid statistics record ID".to_owned()))?;
    Ok(i64::from_be_bytes(id.try_into().expect("record ID")))
}

/// 解码直方图身份键。
fn decode_histogram_key(key: &kv::Key) -> Result<StatsHistogramRecord, StatsError> {
    let table_id = decode_record_id(HISTOGRAM, key)?;
    let offset = prefix(HISTOGRAM).len() + 8;
    let bytes = key
        .as_ref()
        .get(offset..)
        .ok_or_else(|| StatsError("invalid statistics histogram key".to_owned()))?;
    if bytes.len() != 9 {
        return Err(StatsError("invalid statistics histogram key".to_owned()));
    }
    Ok(StatsHistogramRecord {
        table_id,
        is_index: bytes[0] != 0,
        histogram_id: i64::from_be_bytes(bytes[1..9].try_into().expect("histogram ID")),
    })
}

/// 解码 FM Sketch 键。
fn decode_fm_sketch_key(key: &kv::Key) -> Result<(i64, bool, i64), StatsError> {
    let table_id = decode_record_id(FM_SKETCH, key)?;
    let offset = prefix(FM_SKETCH).len() + 8;
    let bytes = key
        .as_ref()
        .get(offset..)
        .ok_or_else(|| StatsError("invalid FM sketch key".to_owned()))?;
    if bytes.len() != 9 {
        return Err(StatsError("invalid FM sketch key".to_owned()));
    }
    Ok((
        table_id,
        bytes[0] != 0,
        i64::from_be_bytes(bytes[1..9].try_into().expect("FM sketch histogram ID")),
    ))
}

/// 解码 bucket 键。
fn decode_bucket_key(key: &kv::Key) -> Result<(i64, bool, i64, i64), StatsError> {
    let table_id = decode_record_id(BUCKET, key)?;
    let offset = prefix(BUCKET).len() + 8;
    let bytes = key
        .as_ref()
        .get(offset..)
        .ok_or_else(|| StatsError("invalid statistics bucket key".to_owned()))?;
    if bytes.len() != 17 {
        return Err(StatsError("invalid statistics bucket key".to_owned()));
    }
    Ok((
        table_id,
        bytes[0] != 0,
        i64::from_be_bytes(bytes[1..9].try_into().expect("bucket histogram ID")),
        i64::from_be_bytes(bytes[9..17].try_into().expect("bucket ID")),
    ))
}

/// 解码 history 键。
fn decode_history_key(table: &[u8], key: &kv::Key) -> Result<(i64, u64, u64, u64), StatsError> {
    let table_id = decode_record_id(table, key)?;
    let offset = prefix(table).len() + 8;
    let bytes = key
        .as_ref()
        .get(offset..)
        .ok_or_else(|| StatsError("invalid statistics history key".to_owned()))?;
    if bytes.len() != 24 {
        return Err(StatsError("invalid statistics history key".to_owned()));
    }
    Ok((
        table_id,
        u64::from_be_bytes(bytes[0..8].try_into().expect("history version")),
        u64::from_be_bytes(bytes[8..16].try_into().expect("history sequence")),
        u64::from_be_bytes(bytes[16..24].try_into().expect("history create time")),
    ))
}

/// KV Get；NotFound 映射为 None。
fn get(retriever: &dyn kv::Retriever, key: kv::Key) -> Result<Option<Vec<u8>>, StatsError> {
    match retriever.Get(&kv::Context::default(), key, &[]) {
        Ok(value) => Ok(Some(value.Value)),
        Err(error) if kv::IsErrNotFound(&error) => Ok(None),
        Err(error) => Err(stats_error(error)),
    }
}

/// 前缀扫描：Iter 从 prefix 到 PrefixNext。
fn scan(
    retriever: &dyn kv::Retriever,
    prefix: &[u8],
) -> Result<Vec<(kv::Key, Vec<u8>)>, StatsError> {
    let start = kv::Key(prefix.to_vec());
    let mut iterator = retriever
        .Iter(start.clone(), Some(start.PrefixNext()))
        .map_err(stats_error)?;
    let mut rows = Vec::new();
    while iterator.Valid() {
        rows.push((iterator.Key(), iterator.Value()));
        iterator.Next().map_err(stats_error)?;
    }
    iterator.Close();
    Ok(rows)
}

/// 事务内 Set。
fn set(
    transaction: &mut dyn kv::Transaction,
    key: kv::Key,
    value: Vec<u8>,
) -> Result<(), StatsError> {
    transaction.Set(key, value).map_err(stats_error)
}

/// 事务内 Delete。
fn delete(transaction: &mut dyn kv::Transaction, key: kv::Key) -> Result<(), StatsError> {
    transaction.Delete(key).map_err(stats_error)
}

/// 扫描并删除某前缀下全部键。
fn delete_prefix(transaction: &mut dyn kv::Transaction, prefix: &[u8]) -> Result<(), StatsError> {
    let keys = scan(transaction, prefix)?
        .into_iter()
        .map(|(key, _)| key)
        .collect::<Vec<_>>();
    for key in keys {
        delete(transaction, key)?;
    }
    Ok(())
}

/// 将绑定参数渲染为 SQL 字面量。
fn render_argument(argument: &SqlValue) -> String {
    match argument {
        SqlValue::Int(value) => value.to_string(),
        SqlValue::UInt(value) => value.to_string(),
        SqlValue::Text(value) => format!("'{}'", value.replace('\'', "''")),
    }
}

/// 规范化 SQL：压缩空白、非引号区转小写。
fn normalize(sql: &str) -> String {
    // 引号外压缩空白并小写；引号内原样，支持 '' 转义。
    let mut normalized = String::with_capacity(sql.len());
    let mut quoted = false;
    let mut pending_space = false;
    let mut characters = sql.chars().peekable();
    while let Some(character) = characters.next() {
        if !quoted && character == '/' && characters.peek() == Some(&'*') {
            characters.next();
            let mut previous = '\0';
            for comment_character in characters.by_ref() {
                if previous == '*' && comment_character == '/' {
                    break;
                }
                previous = comment_character;
            }
            pending_space = !normalized.is_empty();
            continue;
        }
        if character == '\'' {
            if pending_space {
                if !normalized.is_empty() {
                    normalized.push(' ');
                }
                pending_space = false;
            }
            normalized.push(character);
            if quoted && characters.peek() == Some(&'\'') {
                normalized.push(characters.next().expect("peeked escaped quote"));
            } else {
                quoted = !quoted;
            }
            continue;
        }
        if !quoted && character.is_whitespace() {
            pending_space = true;
            continue;
        }
        if pending_space {
            if !normalized.is_empty() {
                normalized.push(' ');
            }
            pending_space = false;
        }
        if quoted {
            normalized.push(character);
        } else {
            normalized.extend(character.to_lowercase());
        }
    }
    normalized
}

/// 从规范化 UPDATE 的 SET 子句提取列字面量赋值对。
/// Extracts `column = 'literal'` pairs from the `SET` clause of a normalized
/// UPDATE statement. Only single-quoted literals are recognized because the
/// statistics system tables the restricted executor rewrites store blobs.
fn assignment_literals(sql: &str) -> Vec<(String, String)> {
    let Some(clause) = sql.split(" set ").nth(1) else {
        return Vec::new();
    };
    let clause = clause.split(" where ").next().unwrap_or(clause);
    let mut assignments = Vec::new();
    for assignment in clause.split(", ") {
        let Some((column, value)) = assignment.split_once('=') else {
            continue;
        };
        let value = value.trim();
        let Some(literal) = value
            .strip_prefix('\'')
            .and_then(|value| value.strip_suffix('\''))
        else {
            continue;
        };
        assignments.push((column.trim().to_owned(), literal.replace("''", "'")));
    }
    assignments
}

/// 从 SQL 中提取 `mysql.<table>` 表名。
fn sql_table(sql: &str) -> Option<&str> {
    let mysql = sql.find("mysql.")? + "mysql.".len();
    Some(
        &sql[mysql
            ..mysql
                + sql[mysql..]
                    .find(|character: char| {
                        !(character.is_ascii_alphanumeric() || character == '_')
                    })
                    .unwrap_or(sql.len() - mysql)],
    )
}

/// 取字段赋值后的 i64。
fn sql_i64_after(sql: &str, field: &str) -> Option<i64> {
    let offset = sql.rfind(field)? + field.len();
    let tail = sql[offset..].trim_start();
    let tail = tail.strip_prefix('=')?.trim_start();
    tail.split(|character: char| !character.is_ascii_digit() && character != '-')
        .next()?
        .parse()
        .ok()
}

/// 取字段赋值后的 u64。
fn sql_u64_after(sql: &str, field: &str) -> Option<u64> {
    sql_i64_after(sql, field).map(|value| value.max(0) as u64)
}

/// 取第 index 个绑定参数为 i64。
fn argument_i64(arguments: &[SqlValue], index: usize) -> Option<i64> {
    arguments.get(index).map(SqlValue::int)
}

/// 取第 index 个绑定参数为 u64。
fn argument_u64(arguments: &[SqlValue], index: usize) -> Option<u64> {
    argument_i64(arguments, index).map(|value| value.max(0) as u64)
}

/// 从 VALUES 子句抽出全部整数字面量。
fn values_numbers(sql: &str) -> Vec<i64> {
    let Some(values) = sql.split("values").nth(1) else {
        return Vec::new();
    };
    values
        .split(|character: char| !(character.is_ascii_digit() || character == '-'))
        .filter(|value| !value.is_empty())
        .filter_map(|value| value.parse().ok())
        .collect()
}

/// 提取 INSERT 列名列表，便于按名绑定 VALUES。
/// Extracts the parenthesized column list of an `INSERT INTO mysql.<table>
/// (...) VALUES ...` statement so values can be bound by name.
fn insert_columns(sql: &str) -> Vec<String> {
    let Some(head) = sql.split(" values").next() else {
        return Vec::new();
    };
    let Some(open) = head.find('(') else {
        return Vec::new();
    };
    let Some(close) = head.rfind(')') else {
        return Vec::new();
    };
    if close <= open {
        return Vec::new();
    }
    head[open + 1..close]
        .split(',')
        .map(|column| column.trim().trim_matches('`').to_owned())
        .collect()
}

/// 拆分 VALUES 为原始文本元组。
/// Splits the `VALUES` clause into raw (still textual) tuples so callers can
/// keep non-numeric columns such as blob payloads in their original position.
fn values_raw_tuples(sql: &str) -> Vec<Vec<String>> {
    let Some(values) = sql.split(" values").nth(1) else {
        return Vec::new();
    };
    let values = values.split(" on duplicate").next().unwrap_or(values);
    let mut tuples = Vec::new();
    let mut start = None;
    let mut depth = 0;
    for (index, character) in values.char_indices() {
        match character {
            '(' => {
                if depth == 0 {
                    start = Some(index + 1);
                }
                depth += 1;
            }
            ')' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    let tuple = &values[start.unwrap_or_default()..index];
                    tuples.push(
                        tuple
                            .split(',')
                            .map(|value| value.trim().to_owned())
                            .collect(),
                    );
                }
            }
            _ => {}
        }
    }
    tuples
}

/// 拆分 VALUES 为整型元组。
fn values_tuples(sql: &str) -> Vec<Vec<i64>> {
    let Some(values) = sql.split("values").nth(1) else {
        return Vec::new();
    };
    let values = values.split(" on duplicate").next().unwrap_or(values);
    let mut tuples = Vec::new();
    let mut start = None;
    let mut depth = 0;
    for (index, character) in values.char_indices() {
        match character {
            '(' => {
                if depth == 0 {
                    start = Some(index + 1);
                }
                depth += 1;
            }
            ')' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    let tuple = &values[start.unwrap_or_default()..index];
                    tuples.push(
                        tuple
                            .split(',')
                            .filter_map(|value| value.trim().trim_matches('\'').parse().ok())
                            .collect(),
                    );
                }
            }
            _ => {}
        }
    }
    tuples
}

/// 按单引号切分取出全部引用字符串。
fn quoted_values(sql: &str) -> Vec<String> {
    sql.split('\'')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

/// 解析 LIMIT N。
fn sql_limit(sql: &str) -> Option<usize> {
    sql.split(" limit ")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()
}

/// 按 WHERE 过滤行集合。
fn filter_rows(
    sql: &str,
    rows: Vec<BTreeMap<&'static str, SqlValue>>,
) -> Result<Vec<BTreeMap<&'static str, SqlValue>>, StatsError> {
    rows.into_iter()
        .filter_map(|row| match row_matches(sql, &row) {
            Ok(true) => Some(Ok(row)),
            Ok(false) => None,
            Err(error) => Some(Err(error)),
        })
        .collect()
}

/// 判断行是否满足 WHERE 中全部 AND 条件。
fn row_matches(sql: &str, row: &BTreeMap<&'static str, SqlValue>) -> Result<bool, StatsError> {
    let Some(where_clause) = sql.split(" where ").nth(1) else {
        return Ok(true);
    };
    let where_clause = where_clause
        .split(" order by ")
        .next()
        .unwrap_or(where_clause)
        .split(" limit ")
        .next()
        .unwrap_or(where_clause);
    for condition in where_clause.split(" and ") {
        if !condition_matches(condition.trim(), row)? {
            return Ok(false);
        }
    }
    Ok(true)
}

/// 解析单个比较条件（含保留期谓词）。
fn condition_matches(
    condition: &str,
    row: &BTreeMap<&'static str, SqlValue>,
) -> Result<bool, StatsError> {
    for operator in ["<=", ">=", "!=", "=", "<", ">"] {
        let Some((field, expected)) = condition.split_once(operator) else {
            continue;
        };
        let field = field
            .trim()
            .rsplit('.')
            .next()
            .unwrap_or(field)
            .trim_matches('`');
        let actual = row
            .get(field)
            .ok_or_else(|| StatsError(format!("unknown statistics predicate column {field}")))?;
        if expected.trim_start().starts_with("now() - interval ") {
            let seconds = expected
                .trim()
                .strip_prefix("now() - interval ")
                .and_then(|value| value.split_whitespace().next())
                .and_then(|value| value.parse::<u64>().ok())
                .ok_or_else(|| StatsError("invalid statistics retention predicate".to_owned()))?;
            let cutoff = current_unix_seconds().saturating_sub(seconds);
            return Ok(compare_i128(actual.int() as i128, cutoff as i128, operator));
        }
        return match actual {
            SqlValue::Text(actual) => {
                let expected = expected.trim().trim_matches('\'').replace("''", "'");
                Ok(compare_text(actual, &expected, operator))
            }
            SqlValue::Int(actual) => {
                let expected =
                    expected
                        .trim()
                        .trim_matches('\'')
                        .parse::<i128>()
                        .map_err(|error| {
                            StatsError(format!("invalid statistics predicate value: {error}"))
                        })?;
                Ok(compare_i128(*actual as i128, expected, operator))
            }
            SqlValue::UInt(actual) => {
                let expected =
                    expected
                        .trim()
                        .trim_matches('\'')
                        .parse::<i128>()
                        .map_err(|error| {
                            StatsError(format!("invalid statistics predicate value: {error}"))
                        })?;
                Ok(compare_i128(*actual as i128, expected, operator))
            }
        };
    }
    Err(StatsError(format!(
        "unsupported statistics predicate {condition}"
    )))
}

/// 数值比较运算符。
fn compare_i128(actual: i128, expected: i128, operator: &str) -> bool {
    match operator {
        "<=" => actual <= expected,
        ">=" => actual >= expected,
        "!=" => actual != expected,
        "=" => actual == expected,
        "<" => actual < expected,
        ">" => actual > expected,
        _ => false,
    }
}

/// 文本比较运算符。
fn compare_text(actual: &str, expected: &str, operator: &str) -> bool {
    match operator {
        "<=" => actual <= expected,
        ">=" => actual >= expected,
        "!=" => actual != expected,
        "=" => actual == expected,
        "<" => actual < expected,
        ">" => actual > expected,
        _ => false,
    }
}

/// 当前 Unix 秒（用于 history 过期）。
fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// 统计子系统当前时间戳。
fn current_stats_timestamp() -> u64 {
    storage::gc::current_ts()
}

/// 按顶层逗号拆分 SELECT 投影，函数参数中的逗号不得误切列。
fn split_select_projections(select: &str) -> Result<Vec<&str>, StatsError> {
    let mut projections = Vec::new();
    let mut depth = 0_usize;
    let mut start = 0_usize;
    for (offset, character) in select.char_indices() {
        match character {
            '(' => depth += 1,
            ')' => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| StatsError("invalid restricted SELECT projection".to_owned()))?;
            }
            ',' if depth == 0 => {
                projections.push(select[start..offset].trim());
                start = offset + 1;
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(StatsError(
            "invalid restricted SELECT projection".to_owned(),
        ));
    }
    projections.push(select[start..].trim());
    Ok(projections)
}

/// 对数值执行 MySQL `TRUNCATE(value, digits)` 的朝零截断。
fn truncate_sql_value(value: SqlValue, digits: u32) -> Result<SqlValue, StatsError> {
    let number = match value {
        SqlValue::Int(value) => value as f64,
        SqlValue::UInt(value) => value as f64,
        SqlValue::Text(value) => value
            .parse::<f64>()
            .map_err(|error| StatsError(format!("invalid statistics TRUNCATE value: {error}")))?,
    };
    let factor = 10_f64.powi(i32::try_from(digits).unwrap_or(i32::MAX));
    let truncated = (number * factor).trunc() / factor;
    Ok(SqlValue::Text(if truncated == 0.0 {
        "0".to_owned()
    } else {
        truncated.to_string()
    }))
}

/// SELECT 投影：支持 count(*)、列别名、hex()、truncate()、ORDER BY。
fn project_rows(
    sql: &str,
    table: &str,
    mut rows: Vec<BTreeMap<&'static str, SqlValue>>,
) -> Result<Vec<SqlRow>, StatsError> {
    // count(*) 短路；否则解析投影列与 ORDER BY 后输出 SqlRow。
    if sql.contains("count(*)") {
        return Ok(vec![SqlRow(vec![SqlValue::Int(rows.len() as i64)])]);
    }
    let select = sql
        .strip_prefix("select ")
        .and_then(|sql| sql.split(" from ").next())
        .ok_or_else(|| StatsError("invalid restricted SELECT".to_owned()))?;
    let select = select.strip_prefix("high_priority ").unwrap_or(select);
    let mut columns = split_select_projections(select)?
        .into_iter()
        .map(str::trim)
        .map(|projection| {
            let (source, alias) = projection
                .rsplit_once(" as ")
                .map_or((projection, None), |(source, alias)| {
                    (source.trim(), Some(alias.trim()))
                });
            let source = source
                .rsplit('.')
                .next()
                .unwrap_or(source)
                .trim_matches('`');
            let (source, truncate_digits) = if let Some(arguments) = source
                .strip_prefix("truncate(")
                .and_then(|value| value.strip_suffix(')'))
            {
                let arguments = split_select_projections(arguments)?;
                if arguments.len() != 2 {
                    return Err(StatsError(
                        "invalid statistics TRUNCATE projection".to_owned(),
                    ));
                }
                let digits = arguments[1].trim().parse::<u32>().map_err(|error| {
                    StatsError(format!("invalid statistics TRUNCATE precision: {error}"))
                })?;
                (arguments[0].trim(), Some(digits))
            } else {
                (
                    source
                        .strip_prefix("hex(")
                        .and_then(|value| value.strip_suffix(')'))
                        .unwrap_or(source)
                        .trim(),
                    None,
                )
            };
            let alias = alias.map(|alias| alias.trim_matches('`'));
            Ok((source, alias, truncate_digits))
        })
        .collect::<Result<Vec<_>, StatsError>>()?;
    if columns.len() == 1 && columns[0].0 == "*" {
        let wildcard_columns: &[&str] = match table {
            "stats_table_locked" => &["table_id", "modify_count", "count", "version"],
            "stats_meta" => &[
                "version",
                "table_id",
                "modify_count",
                "count",
                "snapshot",
                "last_stats_histograms_version",
            ],
            "stats_histograms" => &[
                "table_id",
                "is_index",
                "hist_id",
                "distinct_count",
                "null_count",
                "tot_col_size",
                "modify_count",
                "version",
                "cm_sketch",
                "stats_ver",
                "flag",
                "correlation",
                "last_analyze_pos",
            ],
            "stats_fm_sketch" => &["table_id", "is_index", "hist_id", "value"],
            "stats_buckets" => &[
                "table_id",
                "is_index",
                "hist_id",
                "bucket_id",
                "count",
                "repeats",
                "upper_bound",
                "lower_bound",
                "ndv",
            ],
            "stats_meta_history" => &[
                "table_id",
                "modify_count",
                "count",
                "version",
                "source",
                "create_time",
            ],
            "stats_history" => &["table_id", "stats_data", "seq_no", "version", "create_time"],
            "tidb" => &["variable_name", "variable_value", "comment"],
            _ => &[],
        };
        if wildcard_columns.is_empty() && !rows.is_empty() {
            return Err(StatsError(format!(
                "unsupported wildcard projection for mysql.{table}"
            )));
        }
        columns = wildcard_columns
            .iter()
            .map(|column| (*column, None, None))
            .collect();
    }
    let order = sql.split(" order by ").nth(1).map(|clause| {
        let mut parts = clause.split_whitespace();
        let requested = parts
            .next()
            .unwrap_or("table_id")
            .trim_matches('`')
            .rsplit('.')
            .next()
            .unwrap_or("table_id");
        let field = columns
            .iter()
            .find(|(_, alias, _)| alias.is_some_and(|alias| alias == requested))
            .map_or(requested, |(source, _, _)| *source)
            .to_owned();
        let descending = parts
            .next()
            .is_some_and(|direction| direction.eq_ignore_ascii_case("desc"));
        (field, descending)
    });
    let (order_field, descending) = order
        .as_ref()
        .map_or(("table_id", false), |(field, desc)| (field.as_str(), *desc));
    rows.sort_by(|left, right| {
        let ordering = match (left.get(order_field), right.get(order_field)) {
            (Some(SqlValue::Text(left)), Some(SqlValue::Text(right))) => left.cmp(right),
            (Some(left), Some(right)) => left.int().cmp(&right.int()),
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Less,
            (Some(_), None) => std::cmp::Ordering::Greater,
        };
        if descending {
            ordering.reverse()
        } else {
            ordering
        }
    });
    let limit = sql_limit(sql).unwrap_or(usize::MAX);
    rows.into_iter()
        .take(limit)
        .map(|row| {
            columns
                .iter()
                .map(|(source, _, truncate_digits)| {
                    let value = row
                        .get(source)
                        .cloned()
                        .ok_or_else(|| StatsError(format!("unknown statistics column {source}")))?;
                    truncate_digits.map_or(Ok(value.clone()), |digits| {
                        truncate_sql_value(value, digits)
                    })
                })
                .collect::<Result<Vec<_>, _>>()
                .map(SqlRow)
        })
        .collect()
}
