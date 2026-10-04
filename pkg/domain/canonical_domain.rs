// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// Canonical Domain 元数据服务与 InfoSchema 加载。
//
// 通过单一规范 KV 键持久化 DDL 目录（库表元数据），并由此构建 InfoSchema。
// InfoSchema：会话可见的库表结构快照；schema version 随每次成功变更递增。
// 本文件提供 `DdlMetadataService`（写路径）与 `KvInfoSchemaLoader`（读路径）。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, RwLock};

use astersql_infoschema::{self as infoschema, SchemaRef};
use astersql_kv as kv;
use astersql_meta_model::{
    DBInfo, DecodeDBInfo, DecodeTableInfo, EncodeDBInfo, EncodeTableInfo, PartitionDefinition,
    TableInfo,
};
use astersql_parser_ast as ast;

/// 规范 DDL 目录在 KV 中的持久化键。
const DDL_CATALOG_KEY: &[u8] = b"mDDL:canonical-catalog:v1";

#[derive(Clone, Default)]
/// 内存中的 DDL 目录：版本、ID 分配器、库表集合。
pub(crate) struct MetadataCatalog {
    version: i64,
    next_id: i64,
    pub(crate) databases: BTreeMap<String, DBInfo>,
    tables: BTreeMap<(String, String), TableInfo>,
}

#[derive(Clone, Default)]
/// 一次 DDL 元数据变更的结果摘要，供上层刷新缓存 / 统计。
pub struct DdlMetadataChange {
    pub old_tables: Vec<(String, TableInfo)>,
    pub new_tables: Vec<(String, TableInfo)>,
    pub removed_column_ids: std::collections::BTreeSet<i64>,
    pub removed_index_ids: std::collections::BTreeSet<i64>,
    pub schema_version: i64,
    pub changed: bool,
}

/// Serializes supported DDL metadata changes through the Domain's canonical KV
/// transaction. The lock only orders local writers; KV commit remains the
/// persistence and cross-session visibility boundary.
///
/// 将受支持的 DDL 元数据变更串行化写入 Domain 的规范 KV 事务。
/// 本地锁仅排序写者；跨会话可见性仍以 KV commit 为准。
#[derive(Default)]
pub struct DdlMetadataService {
    writer: Mutex<()>,
}

impl DdlMetadataService {
    /// Enumerate configured replicas from committed metadata for the physical
    /// status poller. The table ID is rechecked inside each update transaction.
    pub fn replica_tables(
        &self,
        store: &dyn kv::Storage,
    ) -> Result<Vec<(String, String, i64, u64, bool, Vec<i64>)>, kv::errors::SharedError> {
        let version = store.CurrentVersion("global")?;
        let catalog = read_catalog(store.GetSnapshot(version).as_ref())?;
        Ok(catalog
            .tables
            .into_iter()
            .filter_map(|((database, table), info)| {
                info.TiFlashReplica.and_then(|replica| {
                    (replica.Count > 0).then_some((
                        database,
                        table,
                        info.ID,
                        replica.Count,
                        replica.Available,
                        info.Partition
                            .as_ref()
                            .filter(|partition| !partition.Definitions.is_empty())
                            .map(|partition| {
                                partition.Definitions.iter().map(|part| part.ID).collect()
                            })
                            .unwrap_or_else(|| vec![info.ID]),
                    ))
                })
            })
            .collect())
    }
    /// Return every database persisted in the canonical KV catalog, including
    /// schemas that do not own tables yet.
    pub fn database_names(
        &self,
        store: &dyn kv::Storage,
    ) -> Result<Vec<String>, kv::errors::SharedError> {
        let version = store.CurrentVersion("global")?;
        let catalog = read_catalog(store.GetSnapshot(version).as_ref())?;
        Ok(catalog.databases.into_keys().collect())
    }

    /// 构造默认的 DDL 元数据服务。
    pub fn new() -> Self {
        Self::default()
    }

    /// 在写锁保护下开启 KV 事务，应用 catalog 变更并按需提交。
    fn mutate(
        &self,
        store: &dyn kv::Storage,
        operation: impl FnOnce(
            &mut MetadataCatalog,
        ) -> Result<DdlMetadataChange, kv::errors::SharedError>,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        self.mutate_with_kv(store, |catalog, _transaction| operation(catalog))
    }

    fn mutate_with_kv(
        &self,
        store: &dyn kv::Storage,
        operation: impl FnOnce(
            &mut MetadataCatalog,
            &mut dyn kv::Transaction,
        ) -> Result<DdlMetadataChange, kv::errors::SharedError>,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let _writer = self
            .writer
            .lock()
            .expect("DDL metadata writer lock poisoned");
        let mut transaction = store.Begin(&[])?;
        let start_ts = transaction.StartTS();
        // Go `meta.NewMutator` marks every metadata transaction as
        // AllowedOnAlmostFull so bootstrap/DDL can repair a nearly-full store.
        transaction.SetDiskFullOpt(kv::kvrpcpb::DiskFullOpt::AllowedOnAlmostFull);
        let mut catalog = read_catalog(transaction.as_ref())?;
        let previous = catalog.clone();
        let mut change = operation(&mut catalog, transaction.as_mut())?;
        // 无实际变更：回滚事务，仍返回当前 schema_version。
        if !change.changed {
            change.schema_version = catalog.version;
            transaction.Rollback()?;
            return Ok(change);
        }
        // Go records the DDL transaction start timestamp in every newly
        // published TableInfo.  The canonical Rust catalog previously left
        // UpdateTS at its parser default (zero), which broke metadata cloning
        // checks and made INFORMATION_SCHEMA create/update timestamps stale.
        for (database, table) in &mut change.new_tables {
            table.UpdateTS = start_ts;
            if let Some(catalog_table) = catalog
                .tables
                .get_mut(&(database.clone(), table.Name.L.clone()))
            {
                catalog_table.UpdateTS = start_ts;
            }
        }
        // 有变更：递增 schema 版本、写回 catalog 并提交事务。
        let schema_version_key = tidb_string_key(b"SchemaVersionKey");
        let existing_schema_version = match kv::GetValue(
            &kv::Context::default(),
            transaction.as_ref(),
            schema_version_key.clone(),
        ) {
            Ok(value) => std::str::from_utf8(&value)
                .map_err(|error| kv::errors::New(error.to_string()))?
                .parse::<i64>()
                .map_err(|error| kv::errors::New(error.to_string()))?,
            Err(error) if kv::IsErrNotFound(&error) => 0,
            Err(error) => return Err(error),
        };
        catalog.version = catalog
            .version
            .max(existing_schema_version)
            .saturating_add(1);
        change.schema_version = catalog.version;
        publish_tidb_schema_metadata(transaction.as_mut(), &previous, &catalog)?;
        // TiFlash reloads the complete schema when this flag is set. This is
        // required while the canonical DDL path does not emit action-specific
        // TiDB schema diffs, and keeps table ID mappings in sync after DDL.
        let diff = format!(
            "{{\"version\":{},\"type\":0,\"schema_id\":0,\"table_id\":0,\"old_table_id\":0,\"old_schema_id\":0,\"regenerate_schema_map\":true,\"affected_options\":null}}",
            catalog.version
        );
        transaction.Set(
            tidb_string_key(format!("Diff:{}", catalog.version).as_bytes()),
            diff.into_bytes(),
        )?;
        transaction.Set(schema_version_key, catalog.version.to_string().into_bytes())?;
        if catalog.next_id > previous.next_id {
            transaction.Set(
                tidb_string_key(b"NextGlobalID"),
                catalog.next_id.to_string().into_bytes(),
            )?;
        }
        transaction.Set(kv::Key(DDL_CATALOG_KEY.to_vec()), encode_catalog(&catalog)?)?;
        transaction.Commit(&kv::Context::default())?;
        Ok(change)
    }

    /// 创建数据库；数据库 ID 与表/分区 ID 共用 canonical 分配器。
    pub fn create_database(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        if_not_exists: bool,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        self.create_database_with_id(store, database, if_not_exists, None)
    }

    /// 创建数据库，并允许 next-gen bootstrap 指定 metadef 保留 ID。
    pub fn create_database_with_id(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        if_not_exists: bool,
        preferred_id: Option<i64>,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let database = database.to_ascii_lowercase();
        self.mutate(store, move |catalog| {
            if catalog.databases.contains_key(&database) {
                if if_not_exists {
                    return Ok(DdlMetadataChange::default());
                }
                return Err(kv::errors::New(format!(
                    "database {database} already exists"
                )));
            }
            let id = preferred_id.unwrap_or_else(|| allocate_id(catalog));
            if !astersql_meta_metadef::IsReservedID(id) {
                catalog.next_id = catalog.next_id.max(id);
            }
            catalog.databases.insert(
                database.clone(),
                DBInfo {
                    ID: id,
                    Name: ast::NewCIStr(&database),
                    State: astersql_meta_model::StatePublic,
                    ..DBInfo::default()
                },
            );
            Ok(DdlMetadataChange {
                changed: true,
                ..DdlMetadataChange::default()
            })
        })
    }

    /// 删除数据库及其全部表，作为一次 canonical 元数据事务提交。
    pub fn drop_database(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        if_exists: bool,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        self.drop_database_with_ttl(store, database, if_exists, |_| Ok(()))
    }

    /// Coordinate the external TTL controller before changing durable metadata.
    pub fn drop_database_with_ttl(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        if_exists: bool,
        before_drop: impl FnOnce(&[(String, TableInfo)]) -> Result<(), kv::errors::SharedError>,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let database = database.to_ascii_lowercase();
        self.mutate(store, move |catalog| {
            let existing = catalog
                .tables
                .iter()
                .filter(|((schema, _), _)| schema == &database)
                .map(|((schema, _), table)| (schema.clone(), table.clone()))
                .collect::<Vec<_>>();
            before_drop(&existing)?;
            let database_existed = catalog.databases.remove(&database).is_some();
            let table_keys = catalog
                .tables
                .keys()
                .filter(|(schema, _)| schema == &database)
                .cloned()
                .collect::<Vec<_>>();
            let mut old_tables = Vec::with_capacity(table_keys.len());
            for key in table_keys {
                if let Some(table) = catalog.tables.remove(&key) {
                    old_tables.push((database.clone(), table));
                }
            }
            if !database_existed && old_tables.is_empty() && !if_exists {
                return Err(kv::errors::New(format!(
                    "database {database} doesn't exist"
                )));
            }
            Ok(DdlMetadataChange {
                changed: database_existed || !old_tables.is_empty(),
                old_tables,
                ..DdlMetadataChange::default()
            })
        })
    }

    /// Apply TTL options and coordinate the controller in the current metadata
    /// transaction. A callback error leaves the durable catalog unchanged.
    pub fn alter_table_ttl(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        table: &str,
        apply: impl FnOnce(&mut TableInfo) -> Result<(), kv::errors::SharedError>,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let key = (database.to_ascii_lowercase(), table.to_ascii_lowercase());
        self.mutate(store, move |catalog| {
            let old = catalog
                .tables
                .get(&key)
                .cloned()
                .ok_or_else(|| kv::errors::New(format!("unknown table {}.{}", key.0, key.1)))?;
            let mut updated = old.clone();
            apply(&mut updated)?;
            catalog.tables.insert(key.clone(), updated.clone());
            Ok(DdlMetadataChange {
                changed: true,
                old_tables: vec![(key.0.clone(), old)],
                new_tables: vec![(key.0, updated)],
                ..Default::default()
            })
        })
    }

    /// 创建表；`if_not_exists` 为 true 且已存在时返回未变更摘要。
    pub fn create_table(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        mut table: TableInfo,
        if_not_exists: bool,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let database = database.to_ascii_lowercase();
        self.mutate(store, move |catalog| {
            let key = (database.clone(), table.Name.L.clone());
            if let Some(existing) = catalog.tables.get(&key) {
                if if_not_exists {
                    return Ok(DdlMetadataChange {
                        new_tables: vec![(database, existing.clone())],
                        ..DdlMetadataChange::default()
                    });
                }
                return Err(kv::errors::New(format!(
                    "table {}.{} already exists",
                    key.0, key.1
                )));
            }
            if !catalog.databases.contains_key(&database) {
                let id = allocate_id(catalog);
                catalog.databases.insert(
                    database.clone(),
                    DBInfo {
                        ID: id,
                        Name: ast::NewCIStr(&database),
                        State: astersql_meta_model::StatePublic,
                        ..DBInfo::default()
                    },
                );
            }
            let database_info = catalog
                .databases
                .get(&database)
                .expect("database inserted above");
            table.DBID = database_info.ID;
            assign_table_physical_ids(catalog, &mut table);
            catalog.tables.insert(key, table.clone());
            Ok(DdlMetadataChange {
                new_tables: vec![(database, table)],
                changed: true,
                ..DdlMetadataChange::default()
            })
        })
    }

    /// Install an MLog and its base-table link in one committed catalog change.
    pub fn create_materialized_view_log(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        base_name: &str,
        mut log: TableInfo,
        next_purge_unix_seconds: Option<i64>,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let database = database.to_ascii_lowercase();
        let base_name = base_name.to_ascii_lowercase();
        self.mutate_with_kv(store, move |catalog, transaction| {
            let purge_info = catalog.tables.get(&("mysql".to_owned(), "tidb_mlog_purge_info".to_owned()))
                .cloned().ok_or_else(|| kv::errors::New("create materialized view log: required system table mysql.tidb_mlog_purge_info does not exist"))?;
            if !purge_info.PKIsHandle {
                return Err(kv::errors::New("MLog purge info requires an integer primary key"));
            }
            let base_key = (database.clone(), base_name.clone());
            let old_base = catalog.tables.get(&base_key).cloned().ok_or_else(|| {
                kv::errors::New(format!("base table {database}.{base_name} does not exist"))
            })?;
            if old_base
                .MaterializedViewBase
                .as_ref()
                .is_some_and(|info| info.MLogID != 0)
            {
                return Err(kv::errors::New("materialized view log already exists"));
            }
            let log_key = (database.clone(), log.Name.L.clone());
            if catalog.tables.contains_key(&log_key) {
                return Err(kv::errors::New(format!(
                    "table {}.{} already exists",
                    log_key.0, log_key.1
                )));
            }
            log.DBID = old_base.DBID;
            assign_table_physical_ids(catalog, &mut log);
            if let Some(info) = log.MaterializedViewLog.as_mut() {
                info.BaseTableID = old_base.ID;
            }
            let mut base = old_base.clone();
            base.MaterializedViewBase
                .get_or_insert_with(Default::default)
                .MLogID = log.ID;
            let mut values = Vec::with_capacity(purge_info.Columns.len());
            let mut ids = Vec::with_capacity(purge_info.Columns.len());
            for column in &purge_info.Columns {
                let value = match column.Name.L.as_str() {
                    "mlog_id" => astersql_types::datum::NewIntDatum(log.ID),
                    "next_purge_unix_seconds" => next_purge_unix_seconds
                        .map(astersql_types::datum::NewIntDatum).unwrap_or_default(),
                    _ => astersql_types::datum::Datum::default(),
                };
                values.push(value);
                ids.push(column.ID);
            }
            let encoded = astersql_tablecodec::EncodeRow(
                Some(astersql_tablecodec::time::UTC),
                values, ids, Vec::new(), None, None,
                astersql_tablecodec::rowcodec::Encoder::new(true),
            ).map_err(|error| kv::errors::New(error.to_string()))?;
            let record_key = astersql_tablecodec::EncodeRowKeyWithHandle(
                purge_info.ID,
                Box::new(astersql_tablecodec::kv::IntHandle(log.ID)),
            );
            transaction.Set(kv::Key(record_key.0), encoded)?;
            catalog.tables.insert(base_key, base.clone());
            catalog.tables.insert(log_key, log.clone());
            Ok(DdlMetadataChange {
                old_tables: vec![(database.clone(), old_base)],
                new_tables: vec![(database.clone(), base), (database, log)],
                changed: true,
                ..DdlMetadataChange::default()
            })
        })
    }

    /// Set the virtual TiFlash replica metadata used by planner casetests.
    ///
    /// Go's `testkit.SetTiFlashReplica` mutates the cached `TableInfo` directly
    /// because the test only needs an available replica for planning. The
    /// canonical Rust catalog is immutable to readers, so publish the same
    /// metadata change through the normal catalog transaction instead.
    pub fn set_tiflash_replica(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        table: &str,
        count: u64,
        available: bool,
        location_labels: Vec<String>,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let database = database.to_ascii_lowercase();
        let table_name = table.to_ascii_lowercase();
        self.mutate(store, move |catalog| {
            let key = (database.clone(), table_name.clone());
            let existing = catalog
                .tables
                .get(&key)
                .cloned()
                .ok_or_else(|| kv::errors::New(format!("unknown table {}.{}", key.0, key.1)))?;
            let mut updated = existing.clone();
            updated.TiFlashReplica = Some(astersql_meta_model::TiFlashReplicaInfo {
                Count: count,
                Available: available,
                LocationLabels: location_labels.clone(),
                ..Default::default()
            });
            catalog.tables.insert(key, updated.clone());
            Ok(DdlMetadataChange {
                old_tables: vec![(database.clone(), existing)],
                new_tables: vec![(database, updated)],
                changed: true,
                ..DdlMetadataChange::default()
            })
        })
    }

    /// Persist an observed replica availability change without replacing its
    /// configured count or location labels.
    pub fn update_tiflash_replica_availability(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        table: &str,
        expected_table_id: i64,
        available: bool,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let database = database.to_ascii_lowercase();
        let table_name = table.to_ascii_lowercase();
        self.mutate(store, move |catalog| {
            let key = (database.clone(), table_name.clone());
            let existing = catalog
                .tables
                .get(&key)
                .cloned()
                .ok_or_else(|| kv::errors::New(format!("unknown table {}.{}", key.0, key.1)))?;
            if existing.ID != expected_table_id {
                return Err(kv::errors::New("TiFlash report table ID mismatch"));
            }
            let mut updated = existing.clone();
            let replica = updated.TiFlashReplica.as_mut().ok_or_else(|| {
                kv::errors::New(format!("table {}.{} has no TiFlash replica", key.0, key.1))
            })?;
            if replica.Count == 0 && available {
                return Err(kv::errors::New("TiFlash replica count is zero"));
            }
            if replica.Available == available {
                return Ok(DdlMetadataChange::default());
            }
            replica.Available = available;
            catalog.tables.insert(key, updated.clone());
            Ok(DdlMetadataChange {
                old_tables: vec![(database.clone(), existing)],
                new_tables: vec![(database, updated)],
                changed: true,
                ..DdlMetadataChange::default()
            })
        })
    }

    /// 批量删表；`if_exists` 为 false 时缺表报错。
    pub fn drop_tables(
        &self,
        store: &dyn kv::Storage,
        tables: Vec<(String, String)>,
        if_exists: bool,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        self.mutate(store, move |catalog| {
            let mut old_tables = Vec::new();
            for (database, table) in tables {
                let database = database.to_ascii_lowercase();
                let table = table.to_ascii_lowercase();
                if let Some(old) = catalog.tables.remove(&(database.clone(), table.clone())) {
                    old_tables.push((database, old));
                } else if !if_exists {
                    return Err(kv::errors::New(format!("unknown table {database}.{table}")));
                }
            }
            Ok(DdlMetadataChange {
                changed: !old_tables.is_empty(),
                old_tables,
                ..DdlMetadataChange::default()
            })
        })
    }

    /// Update hidden-row-ID sharding metadata while preserving table identity.
    pub fn set_shard_row_id_bits(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        table: &str,
        bits: u64,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let database = database.to_ascii_lowercase();
        let table_name = table.to_ascii_lowercase();
        self.mutate(store, move |catalog| {
            let key = (database.clone(), table_name.clone());
            let old =
                catalog.tables.get(&key).cloned().ok_or_else(|| {
                    kv::errors::New(format!("unknown table {database}.{table_name}"))
                })?;
            let mut updated = old.clone();
            updated.ShardRowIDBits = bits;
            updated.MaxShardRowIDBits = updated.MaxShardRowIDBits.max(bits);
            catalog.tables.insert(key, updated.clone());
            Ok(DdlMetadataChange {
                changed: true,
                old_tables: vec![(database.clone(), old)],
                new_tables: vec![(database, updated)],
                ..DdlMetadataChange::default()
            })
        })
    }

    /// 原子重命名单表或多表；物理表 ID 不变，跨库时仅更新 DBID。
    pub fn rename_tables(
        &self,
        store: &dyn kv::Storage,
        renames: Vec<(String, String, String, String)>,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let renames = renames
            .into_iter()
            .map(|(old_database, old_table, new_database, new_table)| {
                (
                    (
                        old_database.to_ascii_lowercase(),
                        old_table.to_ascii_lowercase(),
                    ),
                    (new_database.to_ascii_lowercase(), ast::NewCIStr(&new_table)),
                )
            })
            .collect::<Vec<_>>();
        self.mutate(store, move |catalog| {
            if renames.is_empty() {
                return Err(kv::errors::New("RENAME TABLE requires at least one table"));
            }
            let mut sources = std::collections::BTreeSet::new();
            let mut targets = std::collections::BTreeSet::new();
            for (source, (database, table)) in &renames {
                if !sources.insert(source.clone()) {
                    return Err(kv::errors::New(format!(
                        "table {}.{} is renamed more than once",
                        source.0, source.1
                    )));
                }
                let target = (database.clone(), table.L.clone());
                if !targets.insert(target.clone()) {
                    return Err(kv::errors::New(format!(
                        "duplicate RENAME TABLE target {}.{}",
                        target.0, target.1
                    )));
                }
                if !catalog.databases.contains_key(database) {
                    return Err(kv::errors::New(format!("unknown database {database}")));
                }
            }
            for target in &targets {
                if catalog.tables.contains_key(target) && !sources.contains(target) {
                    return Err(kv::errors::New(format!(
                        "table {}.{} already exists",
                        target.0, target.1
                    )));
                }
            }
            let reference_renames = renames
                .iter()
                .map(|(source, (database, table))| {
                    (source.clone(), (database.clone(), table.clone()))
                })
                .collect::<BTreeMap<_, _>>();
            let mut change = DdlMetadataChange::default();
            for ((old_database, old_table), (new_database, new_name)) in renames {
                let mut table = catalog
                    .tables
                    .remove(&(old_database.clone(), old_table.clone()))
                    .ok_or_else(|| {
                        kv::errors::New(format!("unknown table {old_database}.{old_table}"))
                    })?;
                let target = (new_database.clone(), new_name.L.clone());
                if catalog.tables.contains_key(&target) {
                    return Err(kv::errors::New(format!(
                        "table {}.{} already exists",
                        target.0, target.1
                    )));
                }
                change.old_tables.push((old_database, table.clone()));
                let old_database_id = table.DBID;
                let new_database_id = catalog
                    .databases
                    .get(&new_database)
                    .expect("destination database checked above")
                    .ID;
                if table.AutoIDSchemaID == 0 && new_database_id != old_database_id {
                    table.AutoIDSchemaID = old_database_id;
                }
                if table.AutoIDSchemaID == new_database_id {
                    table.AutoIDSchemaID = 0;
                }
                table.Name = new_name;
                table.DBID = new_database_id;
                for index in &mut table.Indices {
                    index.Table = table.Name.clone();
                }
                for constraint in &mut table.Constraints {
                    constraint.Table = table.Name.clone();
                }
                catalog.tables.insert(target, table.clone());
                change.new_tables.push((new_database, table));
            }
            // Go updates every child foreign key that refers to a renamed
            // parent in the same metadata transaction.
            for ((child_database, _), child) in &mut catalog.tables {
                for foreign_key in &mut child.ForeignKeys {
                    let mut reference = if foreign_key.RefSchema.L.is_empty() {
                        (child_database.clone(), foreign_key.RefTable.L.clone())
                    } else {
                        (
                            foreign_key.RefSchema.L.clone(),
                            foreign_key.RefTable.L.clone(),
                        )
                    };
                    let original = reference.clone();
                    for _ in 0..reference_renames.len() {
                        let Some((new_database, new_table)) = reference_renames.get(&reference)
                        else {
                            break;
                        };
                        reference = (new_database.clone(), new_table.L.clone());
                    }
                    if reference != original {
                        foreign_key.RefSchema = ast::NewCIStr(&reference.0);
                        foreign_key.RefTable = ast::NewCIStr(&reference.1);
                    }
                }
            }
            change.changed = true;
            Ok(change)
        })
    }

    /// 截断表：分配新物理 ID（含分区定义 ID），保留逻辑结构。
    pub fn truncate_table(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        table: &str,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let database = database.to_ascii_lowercase();
        let table = table.to_ascii_lowercase();
        self.mutate(store, move |catalog| {
            let key = (database.clone(), table.clone());
            let old = catalog
                .tables
                .get(&key)
                .cloned()
                .ok_or_else(|| kv::errors::New(format!("unknown table {}.{}", key.0, key.1)))?;
            let mut new = old.clone();
            new.ID = allocate_id(catalog);
            new.AutoIncID = 1;
            new.AutoIncIDExtra = 0;
            new.AutoRandID = 0;
            if let Some(partition) = new.Partition.as_mut() {
                for definition in &mut partition.Definitions {
                    definition.ID = allocate_id(catalog);
                }
            }
            catalog.tables.insert(key, new.clone());
            Ok(DdlMetadataChange {
                old_tables: vec![(database.clone(), old)],
                new_tables: vec![(database, new)],
                changed: true,
                ..DdlMetadataChange::default()
            })
        })
    }

    /// 替换分区：删除指定名称分区并追加新定义（重新分配 ID）。
    pub fn replace_partitions(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        table: &str,
        removed_names: &std::collections::BTreeSet<String>,
        mut added: Vec<PartitionDefinition>,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let database = database.to_ascii_lowercase();
        let table = table.to_ascii_lowercase();
        let removed_names = removed_names.clone();
        self.mutate(store, move |catalog| {
            let key = (database.clone(), table.clone());
            let old = catalog
                .tables
                .get(&key)
                .cloned()
                .ok_or_else(|| kv::errors::New(format!("unknown table {}.{}", key.0, key.1)))?;
            let mut new = old.clone();
            let partition = new.Partition.as_mut().ok_or_else(|| {
                kv::errors::New(format!("table {}.{} is not partitioned", key.0, key.1))
            })?;
            let offset = partition
                .Definitions
                .iter()
                .position(|definition| removed_names.contains(&definition.Name.L))
                .unwrap_or(partition.Definitions.len());
            partition
                .Definitions
                .retain(|definition| !removed_names.contains(&definition.Name.L));
            for definition in &mut added {
                definition.ID = allocate_id(catalog);
            }
            partition.Definitions.splice(offset..offset, added);
            astersql_ddl::storage_class::normalize_checked_partitions(&mut new)
                .map_err(kv::errors::New)?;
            astersql_ddl::storage_class::check_final_definitions(&new).map_err(kv::errors::New)?;
            astersql_ddl::storage_class::rebuild_partitions(&mut new).map_err(kv::errors::New)?;
            // Go copies only the checked new definitions into the job. Existing
            // physical partitions retain their metadata, including current tiers.
            if let Some(original) = &old.Partition {
                for definition in &mut new.Partition.as_mut().unwrap().Definitions {
                    if let Some(existing) =
                        original.Definitions.iter().find(|d| d.ID == definition.ID)
                    {
                        *definition = existing.clone();
                    }
                }
            }
            let partition = new.Partition.as_mut().unwrap();
            partition.Num = partition.Definitions.len() as u64;
            catalog.tables.insert(key, new.clone());
            Ok(DdlMetadataChange {
                old_tables: vec![(database.clone(), old)],
                new_tables: vec![(database, new)],
                changed: true,
                ..DdlMetadataChange::default()
            })
        })
    }

    /// Replace or remove a table's complete partition definition while preserving table identity.
    pub fn set_table_partitioning(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        table: &str,
        mut partition: Option<astersql_meta_model::PartitionInfo>,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let database = database.to_ascii_lowercase();
        let table = table.to_ascii_lowercase();
        self.mutate(store, move |catalog| {
            let key = (database.clone(), table.clone());
            let old = catalog
                .tables
                .get(&key)
                .cloned()
                .ok_or_else(|| kv::errors::New(format!("unknown table {}.{}", key.0, key.1)))?;
            if let Some(partition) = partition.as_mut() {
                for definition in &mut partition.Definitions {
                    definition.ID = allocate_id(catalog);
                }
                partition.Num = partition.Definitions.len() as u64;
            }
            let mut new = old.clone();
            new.Partition = partition;
            astersql_ddl::storage_class::normalize_checked_partitions(&mut new)
                .map_err(kv::errors::New)?;
            astersql_ddl::storage_class::rebuild_partitions(&mut new).map_err(kv::errors::New)?;
            catalog.tables.insert(key, new.clone());
            Ok(DdlMetadataChange {
                old_tables: vec![(database.clone(), old)],
                new_tables: vec![(database, new)],
                changed: true,
                ..DdlMetadataChange::default()
            })
        })
    }

    /// Update only the table-level placement reference.
    pub fn set_table_placement(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        table: &str,
        placement: Option<astersql_meta_model::PolicyRefInfo>,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let database = database.to_ascii_lowercase();
        let table = table.to_ascii_lowercase();
        self.mutate(store, move |catalog| {
            let key = (database.clone(), table.clone());
            let old = catalog
                .tables
                .get(&key)
                .cloned()
                .ok_or_else(|| kv::errors::New(format!("unknown table {}.{}", key.0, key.1)))?;
            let mut new = old.clone();
            new.PlacementPolicyRef = placement;
            catalog.tables.insert(key, new.clone());
            Ok(DdlMetadataChange {
                old_tables: vec![(database.clone(), old)],
                new_tables: vec![(database, new)],
                changed: true,
                ..DdlMetadataChange::default()
            })
        })
    }

    /// Persist a table-mode transition through the canonical DDL metadata path.
    pub fn set_table_mode(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        table: &str,
        mode: astersql_meta_model::TableMode,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let database = database.to_ascii_lowercase();
        let table = table.to_ascii_lowercase();
        self.mutate(store, move |catalog| {
            let key = (database.clone(), table.clone());
            let old = catalog
                .tables
                .get(&key)
                .cloned()
                .ok_or_else(|| kv::errors::New(format!("unknown table {}.{}", key.0, key.1)))?;
            if !old.Mode.CanTransitionTo(mode) {
                return Err(kv::errors::New(format!(
                    "invalid table mode transition {} -> {} for {}.{}",
                    old.Mode.String(),
                    mode.String(),
                    key.0,
                    key.1,
                )));
            }
            if old.Mode == mode {
                return Ok(DdlMetadataChange::default());
            }
            let mut new = old.clone();
            new.Mode = mode;
            catalog.tables.insert(key, new.clone());
            Ok(DdlMetadataChange {
                old_tables: vec![(database.clone(), old)],
                new_tables: vec![(database, new)],
                changed: true,
                ..DdlMetadataChange::default()
            })
        })
    }

    /// 删除列 / 索引，并修正剩余列 Offset 与索引列引用。
    pub fn drop_table_items(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        table: &str,
        columns: &std::collections::BTreeSet<String>,
        indexes: &std::collections::BTreeSet<String>,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let database = database.to_ascii_lowercase();
        let table = table.to_ascii_lowercase();
        let columns = columns.clone();
        let indexes = indexes.clone();
        self.mutate(store, move |catalog| {
            let key = (database.clone(), table.clone());
            let old = catalog
                .tables
                .get(&key)
                .cloned()
                .ok_or_else(|| kv::errors::New(format!("unknown table {}.{}", key.0, key.1)))?;
            let mut new = old.clone();
            let removed_column_ids = new
                .Columns
                .iter()
                .filter(|column| columns.contains(&column.Name.L))
                .map(|column| column.ID)
                .collect::<std::collections::BTreeSet<_>>();
            let removed_index_ids = new
                .Indices
                .iter()
                .filter(|index| {
                    indexes.contains(&index.Name.L)
                        || index
                            .Columns
                            .iter()
                            .any(|column| columns.contains(&column.Name.L))
                })
                .map(|index| index.ID)
                .collect::<std::collections::BTreeSet<_>>();
            if removed_column_ids.len() != columns.len() {
                return Err(kv::errors::New("unknown column in ALTER TABLE DROP COLUMN"));
            }
            if !indexes
                .iter()
                .all(|name| new.Indices.iter().any(|index| index.Name.L == *name))
            {
                return Err(kv::errors::New("unknown index in ALTER TABLE DROP INDEX"));
            }
            new.Columns
                .retain(|column| !removed_column_ids.contains(&column.ID));
            for (offset, column) in new.Columns.iter_mut().enumerate() {
                column.Offset = offset as isize;
            }
            let removed_primary_columns = new
                .Indices
                .iter()
                .filter(|index| index.Primary && removed_index_ids.contains(&index.ID))
                .flat_map(|index| index.Columns.iter().map(|column| column.Name.L.clone()))
                .collect::<std::collections::BTreeSet<_>>();
            for column in &mut new.Columns {
                if removed_primary_columns.contains(&column.Name.L) {
                    // Dropping a nonclustered primary key retains NOT NULL.
                    column.SetFlag(column.GetFlag() & !astersql_meta_model::mysql::PriKeyFlag);
                }
            }
            new.Indices
                .retain(|index| !removed_index_ids.contains(&index.ID));
            for index in &mut new.Indices {
                for column in &mut index.Columns {
                    column.Offset = new
                        .Columns
                        .iter()
                        .position(|candidate| candidate.Name.L == column.Name.L)
                        .ok_or_else(|| {
                            kv::errors::New(format!(
                                "index {} references dropped column {}",
                                index.Name.O, column.Name.O
                            ))
                        })? as isize;
                }
            }
            catalog.tables.insert(key, new.clone());
            Ok(DdlMetadataChange {
                old_tables: vec![(database.clone(), old)],
                new_tables: vec![(database, new)],
                removed_column_ids,
                removed_index_ids,
                changed: true,
                ..DdlMetadataChange::default()
            })
        })
    }

    /// Replace a table's public foreign-key metadata atomically.
    pub fn replace_foreign_keys(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        table: &str,
        foreign_keys: Vec<astersql_meta_model::FKInfo>,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let database = database.to_ascii_lowercase();
        let table = table.to_ascii_lowercase();
        self.mutate(store, move |catalog| {
            let key = (database.clone(), table.clone());
            let old = catalog
                .tables
                .get(&key)
                .cloned()
                .ok_or_else(|| kv::errors::New(format!("unknown table {}.{}", key.0, key.1)))?;
            let mut new = old.clone();
            new.ForeignKeys = foreign_keys;
            catalog.tables.insert(key, new.clone());
            Ok(DdlMetadataChange {
                old_tables: vec![(database.clone(), old)],
                new_tables: vec![(database, new)],
                changed: true,
                ..DdlMetadataChange::default()
            })
        })
    }

    /// Go `onAddColumn`: appends public columns and allocates their IDs from
    /// `TableInfo.MaxColumnID`.
    pub fn add_columns(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        table: &str,
        columns: Vec<astersql_meta_model::ColumnInfo>,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let database = database.to_ascii_lowercase();
        let table = table.to_ascii_lowercase();
        self.mutate(store, move |catalog| {
            let key = (database.clone(), table.clone());
            let old = catalog
                .tables
                .get(&key)
                .cloned()
                .ok_or_else(|| kv::errors::New(format!("unknown table {}.{}", key.0, key.1)))?;
            let mut new = old.clone();
            new.MaxColumnID = new.MaxColumnID.max(new.Columns.len() as i64);
            for mut column in columns.clone() {
                if column.Name.L.is_empty() {
                    return Err(kv::errors::New(
                        "ALTER TABLE ADD COLUMN requires a column name",
                    ));
                }
                if new
                    .Columns
                    .iter()
                    .any(|existing| existing.Name.L == column.Name.L)
                {
                    return Err(kv::errors::New(format!(
                        "duplicate column {}",
                        column.Name.O
                    )));
                }
                new.MaxColumnID += 1;
                column.ID = new.MaxColumnID;
                column.Offset = new.Columns.len() as isize;
                new.Columns.push(column);
            }
            catalog.tables.insert(key, new.clone());
            Ok(DdlMetadataChange {
                old_tables: vec![(database.clone(), old)],
                new_tables: vec![(database, new)],
                changed: true,
                ..DdlMetadataChange::default()
            })
        })
    }

    /// 向表追加索引：校验列存在、分配索引 ID，并写入 catalog。
    pub fn add_index(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        table: &str,
        mut index: astersql_meta_model::IndexInfo,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let database = database.to_ascii_lowercase();
        let table = table.to_ascii_lowercase();
        self.mutate(store, move |catalog| {
            let key = (database.clone(), table.clone());
            let old = catalog
                .tables
                .get(&key)
                .cloned()
                .ok_or_else(|| kv::errors::New(format!("unknown table {}.{}", key.0, key.1)))?;
            let mut new = old.clone();
            if index.Primary {
                if new.PKIsHandle
                    || new.IsCommonHandle
                    || new.Indices.iter().any(|index| index.Primary)
                {
                    return Err(kv::errors::New("[ddl:1068]Multiple primary key defined"));
                }
                index.Name = ast::NewCIStr("PRIMARY");
                index.Unique = true;
            }
            if index.Name.L.is_empty() {
                return Err(kv::errors::New(
                    "ALTER TABLE ADD INDEX requires an index name",
                ));
            }
            if new
                .Indices
                .iter()
                .any(|existing| existing.Name.L == index.Name.L)
            {
                return Err(kv::errors::New(format!("duplicate index {}", index.Name.O)));
            }
            if index.Columns.is_empty() {
                return Err(kv::errors::New("ALTER TABLE ADD INDEX requires columns"));
            }
            for column in &mut index.Columns {
                let offset = new
                    .Columns
                    .iter()
                    .position(|candidate| candidate.Name.L == column.Name.L)
                    .ok_or_else(|| {
                        kv::errors::New(format!(
                            "index {} references unknown column {}",
                            index.Name.O, column.Name.O
                        ))
                    })?;
                column.Offset = offset as isize;
                if index.Primary {
                    let field = &mut new.Columns[offset];
                    field.SetFlag(
                        field.GetFlag()
                            | astersql_meta_model::mysql::PriKeyFlag
                            | astersql_meta_model::mysql::NotNullFlag,
                    );
                }
            }
            // Index IDs are table-local in TiDB metadata. Global catalog IDs
            // are reserved for schemas/tables/partitions.
            new.MaxIndexID = new
                .MaxIndexID
                .checked_add(1)
                .ok_or_else(|| kv::errors::New("table index ID exhausted"))?;
            index.ID = new.MaxIndexID;
            index.Table = new.Name.clone();
            new.Indices.push(index);
            catalog.tables.insert(key, new.clone());
            Ok(DdlMetadataChange {
                old_tables: vec![(database.clone(), old)],
                new_tables: vec![(database, new)],
                changed: true,
                ..DdlMetadataChange::default()
            })
        })
    }

    /// Rename one public index while preserving its stable ID and predicate.
    pub fn rename_index(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        table: &str,
        from: &str,
        to: &str,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let database = database.to_ascii_lowercase();
        let table = table.to_ascii_lowercase();
        let from = from.to_ascii_lowercase();
        let to = ast::NewCIStr(to);
        self.mutate(store, move |catalog| {
            let key = (database.clone(), table.clone());
            let old = catalog
                .tables
                .get(&key)
                .cloned()
                .ok_or_else(|| kv::errors::New(format!("unknown table {}.{}", key.0, key.1)))?;
            let mut new = old.clone();
            if from == to.L && new.Indices.iter().any(|index| index.Name.L == from) {
                return Ok(DdlMetadataChange {
                    new_tables: vec![(database, new)],
                    ..DdlMetadataChange::default()
                });
            }
            if new.Indices.iter().any(|index| index.Name.L == to.L) {
                return Err(kv::errors::New(format!("duplicate index {}", to.O)));
            }
            let index = new
                .Indices
                .iter_mut()
                .find(|index| index.Name.L == from)
                .ok_or_else(|| kv::errors::New(format!("unknown index {from}")))?;
            index.Name = to;
            catalog.tables.insert(key, new.clone());
            Ok(DdlMetadataChange {
                old_tables: vec![(database.clone(), old)],
                new_tables: vec![(database, new)],
                changed: true,
                ..DdlMetadataChange::default()
            })
        })
    }

    /// Toggle the public visibility flag of one secondary index.
    pub fn set_index_visibility(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        table: &str,
        index_name: &str,
        invisible: bool,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let database = database.to_ascii_lowercase();
        let table = table.to_ascii_lowercase();
        let index_name = index_name.to_ascii_lowercase();
        self.mutate(store, move |catalog| {
            let key = (database.clone(), table.clone());
            let old = catalog
                .tables
                .get(&key)
                .cloned()
                .ok_or_else(|| kv::errors::New(format!("unknown table {}.{}", key.0, key.1)))?;
            let mut new = old.clone();
            let index = new
                .Indices
                .iter_mut()
                .find(|index| index.Name.L == index_name)
                .ok_or_else(|| kv::errors::New(format!("unknown index {index_name}")))?;
            if index.Primary {
                return Err(kv::errors::New("A primary key index cannot be invisible"));
            }
            if index.Invisible == invisible {
                return Ok(DdlMetadataChange {
                    new_tables: vec![(database, new)],
                    ..DdlMetadataChange::default()
                });
            }
            // A suffix alone does not identify a modify-column temporary index.
            // Keep its visibility in sync only while it references changing columns.
            for index in &mut new.Indices {
                if index.Name.L == index_name
                    || (index.IsChanging()
                        && index.Columns.iter().any(|column| {
                            column.UseChangingType
                                || new.Columns[column.Offset as usize]
                                    .ChangeStateInfo
                                    .is_some()
                        })
                        && index_visibility_equal_fold(&index.GetChangingOriginName(), &index_name))
                {
                    index.Invisible = invisible;
                }
            }
            catalog.tables.insert(key, new.clone());
            Ok(DdlMetadataChange {
                old_tables: vec![(database.clone(), old)],
                new_tables: vec![(database, new)],
                changed: true,
                ..DdlMetadataChange::default()
            })
        })
    }

    /// Change a public column definition/name and repair every index-column
    /// reference atomically.
    pub fn change_column(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        table: &str,
        old_name: &str,
        mut column: astersql_meta_model::ColumnInfo,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let database = database.to_ascii_lowercase();
        let table = table.to_ascii_lowercase();
        let old_name = old_name.to_ascii_lowercase();
        self.mutate(store, move |catalog| {
            let key = (database.clone(), table.clone());
            let old = catalog
                .tables
                .get(&key)
                .cloned()
                .ok_or_else(|| kv::errors::New(format!("unknown table {}.{}", key.0, key.1)))?;
            let mut new = old.clone();
            if column.Name.L != old_name
                && new
                    .Columns
                    .iter()
                    .any(|candidate| candidate.Name.L == column.Name.L)
            {
                return Err(kv::errors::New(format!(
                    "duplicate column {}",
                    column.Name.O
                )));
            }
            let offset = new
                .Columns
                .iter()
                .position(|candidate| candidate.Name.L == old_name)
                .ok_or_else(|| kv::errors::New(format!("unknown column {old_name}")))?;
            let previous = &new.Columns[offset];
            column.ID = previous.ID;
            column.Offset = previous.Offset;
            column.SetFlag(column.GetFlag() | previous.GetFlag());
            let new_name = column.Name.clone();
            new.Columns[offset] = column;
            for index in &mut new.Indices {
                for indexed in &mut index.Columns {
                    if indexed.Name.L == old_name {
                        indexed.Name = new_name.clone();
                        indexed.Offset = offset as isize;
                    }
                }
            }
            catalog.tables.insert(key, new.clone());
            Ok(DdlMetadataChange {
                old_tables: vec![(database.clone(), old)],
                new_tables: vec![(database, new)],
                changed: true,
                ..DdlMetadataChange::default()
            })
        })
    }

    /// Replace one public column while preserving its stable ID and offset.
    pub fn modify_column(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        table: &str,
        mut column: astersql_meta_model::ColumnInfo,
        auto_random: Option<(u64, u64)>,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let database = database.to_ascii_lowercase();
        let table = table.to_ascii_lowercase();
        self.mutate(store, move |catalog| {
            let key = (database.clone(), table.clone());
            let old = catalog
                .tables
                .get(&key)
                .cloned()
                .ok_or_else(|| kv::errors::New(format!("unknown table {}.{}", key.0, key.1)))?;
            let mut new = old.clone();
            let offset = new
                .Columns
                .iter()
                .position(|candidate| candidate.Name.L == column.Name.L)
                .ok_or_else(|| kv::errors::New(format!("unknown column {}", column.Name.O)))?;
            let structural_flags = new.Columns[offset].GetFlag()
                & (astersql_meta_model::mysql::PriKeyFlag
                    | astersql_meta_model::mysql::NotNullFlag);
            column.SetFlag(column.GetFlag() | structural_flags);
            column.ID = new.Columns[offset].ID;
            column.Offset = new.Columns[offset].Offset;
            new.Columns[offset] = column;
            if let Some((shard_bits, range_bits)) = auto_random {
                new.AutoRandomBits = shard_bits;
                new.AutoRandomRangeBits = range_bits;
            }
            catalog.tables.insert(key, new.clone());
            Ok(DdlMetadataChange {
                old_tables: vec![(database.clone(), old)],
                new_tables: vec![(database, new)],
                changed: true,
                ..DdlMetadataChange::default()
            })
        })
    }

    /// 交换分区与普通表的物理 ID（EXCHANGE PARTITION）。
    pub fn exchange_partition(
        &self,
        store: &dyn kv::Storage,
        database: &str,
        table: &str,
        partition_name: &str,
        exchange_database: &str,
        exchange_table: &str,
    ) -> Result<DdlMetadataChange, kv::errors::SharedError> {
        let database = database.to_ascii_lowercase();
        let table = table.to_ascii_lowercase();
        let exchange_database = exchange_database.to_ascii_lowercase();
        let exchange_table = exchange_table.to_ascii_lowercase();
        let partition_name = partition_name.to_ascii_lowercase();
        self.mutate(store, move |catalog| {
            let partitioned_key = (database.clone(), table.clone());
            let exchange_key = (exchange_database.clone(), exchange_table.clone());
            if partitioned_key == exchange_key {
                return Err(kv::errors::New(
                    "cannot exchange a partition with its own table",
                ));
            }
            let old_partitioned = catalog
                .tables
                .get(&partitioned_key)
                .cloned()
                .ok_or_else(|| kv::errors::New(format!("unknown table {database}.{table}")))?;
            let old_exchange = catalog.tables.get(&exchange_key).cloned().ok_or_else(|| {
                kv::errors::New(format!(
                    "unknown table {exchange_database}.{exchange_table}"
                ))
            })?;
            if old_exchange.GetPartitionInfo().is_some() {
                return Err(kv::errors::New("exchange table must not be partitioned"));
            }
            let mut new_partitioned = old_partitioned.clone();
            let mut new_exchange = old_exchange.clone();
            let definition = new_partitioned
                .Partition
                .as_mut()
                .and_then(|partition| {
                    partition
                        .Definitions
                        .iter_mut()
                        .find(|definition| definition.Name.L == partition_name)
                })
                .ok_or_else(|| kv::errors::New(format!("unknown partition {partition_name}")))?;
            // 交换分区与普通表的物理 ID。
            let old_partition_id = definition.ID;
            definition.ID = old_exchange.ID;
            new_exchange.ID = old_partition_id;
            catalog
                .tables
                .insert(partitioned_key, new_partitioned.clone());
            catalog.tables.insert(exchange_key, new_exchange.clone());
            Ok(DdlMetadataChange {
                old_tables: vec![
                    (database.clone(), old_partitioned),
                    (exchange_database.clone(), old_exchange),
                ],
                new_tables: vec![
                    (database, new_partitioned),
                    (exchange_database, new_exchange),
                ],
                changed: true,
                ..DdlMetadataChange::default()
            })
        })
    }
}

pub(super) fn tidb_string_key(key: &[u8]) -> kv::Key {
    let encoded = astersql_util_codec::EncodeBytes(b"m".to_vec(), key);
    kv::Key(astersql_util_codec::EncodeUint(encoded, b's' as u64))
}

pub(super) fn tidb_hash_key(key: &[u8], field: &[u8]) -> kv::Key {
    let encoded = astersql_util_codec::EncodeBytes(b"m".to_vec(), key);
    let encoded = astersql_util_codec::EncodeUint(encoded, b'h' as u64);
    kv::Key(astersql_util_codec::EncodeBytes(encoded, field))
}

fn tidb_hash_prefix(key: &[u8]) -> kv::Key {
    let encoded = astersql_util_codec::EncodeBytes(b"m".to_vec(), key);
    kv::Key(astersql_util_codec::EncodeUint(encoded, b'h' as u64))
}

fn read_tidb_string(
    retriever: &dyn kv::Retriever,
    key: &[u8],
) -> Result<Option<Vec<u8>>, kv::errors::SharedError> {
    match retriever.Get(&kv::Context::default(), tidb_string_key(key), &[]) {
        Ok(value) => Ok(Some(value.Value)),
        Err(error) if kv::IsErrNotFound(&error) => Ok(None),
        Err(error) => Err(error),
    }
}

fn scan_tidb_hash(
    retriever: &dyn kv::Retriever,
    key: &[u8],
) -> Result<Vec<(Vec<u8>, Vec<u8>)>, kv::errors::SharedError> {
    let prefix = tidb_hash_prefix(key);
    let mut iterator = retriever.Iter(prefix.clone(), Some(prefix.PrefixNext()))?;
    let mut values = Vec::new();
    let result = (|| {
        while iterator.Valid() {
            let encoded_key = iterator.Key();
            let (_, field) =
                astersql_util_codec::DecodeBytes(&encoded_key.0[prefix.0.len()..], None)
                    .map_err(|error| kv::errors::New(error.to_string()))?;
            values.push((field, iterator.Value()));
            iterator.Next()?;
        }
        Ok::<(), kv::errors::SharedError>(())
    })();
    iterator.Close();
    result?;
    Ok(values)
}

fn publish_tidb_schema_metadata(
    transaction: &mut dyn kv::Transaction,
    previous: &MetadataCatalog,
    catalog: &MetadataCatalog,
) -> Result<(), kv::errors::SharedError> {
    for (name, database) in &previous.databases {
        if !catalog.databases.contains_key(name) {
            transaction.Delete(tidb_hash_key(
                b"DBs",
                format!("DB:{}", database.ID).as_bytes(),
            ))?;
        }
    }
    for (name, database) in &catalog.databases {
        let encoded = EncodeDBInfo(database).map_err(kv::errors::New)?;
        let changed = previous
            .databases
            .get(name)
            .map(|old| EncodeDBInfo(old).map(|old| old != encoded))
            .transpose()
            .map_err(kv::errors::New)?
            .unwrap_or(true);
        if changed {
            transaction.Set(
                tidb_hash_key(b"DBs", format!("DB:{}", database.ID).as_bytes()),
                encoded,
            )?;
        }
    }
    for (key, table) in &previous.tables {
        if !catalog.tables.contains_key(key) {
            let db_id = previous
                .databases
                .get(&key.0)
                .map(|db| db.ID)
                .unwrap_or(table.DBID);
            transaction.Delete(tidb_hash_key(
                format!("DB:{db_id}").as_bytes(),
                format!("Table:{}", table.ID).as_bytes(),
            ))?;
        }
    }
    for (key, table) in &catalog.tables {
        let db_id = catalog
            .databases
            .get(&key.0)
            .map(|db| db.ID)
            .unwrap_or(table.DBID);
        let encoded = EncodeTableInfo(table).map_err(kv::errors::New)?;
        let changed = previous
            .tables
            .get(key)
            .map(|old| EncodeTableInfo(old).map(|old| old != encoded))
            .transpose()
            .map_err(kv::errors::New)?
            .unwrap_or(true);
        if changed {
            transaction.Set(
                tidb_hash_key(
                    format!("DB:{db_id}").as_bytes(),
                    format!("Table:{}", table.ID).as_bytes(),
                ),
                encoded,
            )?;
        }
    }
    Ok(())
}

/// 从目录分配下一个全局物理 ID。
fn allocate_id(catalog: &mut MetadataCatalog) -> i64 {
    catalog.next_id = catalog.next_id.saturating_add(1).max(1);
    catalog.next_id
}

/// 为表及分区定义补齐 / 校准物理 ID，并回写用户 ID 的 next_id 上界。
/// metadef 的固定系统 ID 来自独立保留区间，不得把普通对象分配器推进到该区间。
pub(crate) fn assign_table_physical_ids(catalog: &mut MetadataCatalog, table: &mut TableInfo) {
    if table.ID <= 0 {
        table.ID = allocate_id(catalog);
    } else if !astersql_meta_metadef::IsReservedID(table.ID) {
        catalog.next_id = catalog.next_id.max(table.ID);
    }
    if let Some(partition) = table.Partition.as_mut() {
        if partition.Definitions.is_empty() && partition.Num > 0 {
            partition.Definitions = (0..partition.Num)
                .map(|number| PartitionDefinition {
                    Name: ast::NewCIStr(&format!("p{number}")),
                    ..PartitionDefinition::default()
                })
                .collect();
        }
        for definition in &mut partition.Definitions {
            if definition.ID <= 0 || definition.ID == table.ID {
                definition.ID = allocate_id(catalog);
            } else if !astersql_meta_metadef::IsReservedID(definition.ID) {
                catalog.next_id = catalog.next_id.max(definition.ID);
            }
        }
        partition.Num = partition.Definitions.len() as u64;
    }
}

/// Go strings.EqualFold uses simple Unicode folding, including final sigma,
/// long s and Kelvin sign, but excluding Turkish dotted/dotless I mappings.
fn index_visibility_equal_fold(left: &str, right: &str) -> bool {
    fn fold(character: char) -> char {
        if matches!(character, 'İ' | 'ı') {
            return character;
        }
        let single = |mut mapping: std::char::ToLowercase, fallback| match (
            mapping.next(),
            mapping.next(),
        ) {
            (Some(value), None) => value,
            _ => fallback,
        };
        let lower = single(character.to_lowercase(), character);
        let mut uppercase = lower.to_uppercase();
        let upper = match (uppercase.next(), uppercase.next()) {
            (Some(value), None) => value,
            _ => lower,
        };
        single(upper.to_lowercase(), upper)
    }
    left.chars().map(fold).eq(right.chars().map(fold))
}

/// Loads the current typed InfoSchema from the same KV metadata written by
/// `DdlMetadataService`.
///
/// 从 `DdlMetadataService` 写入的同一份 KV 元数据加载类型化 InfoSchema。
pub struct KvInfoSchemaLoader {
    v2_data: Option<Arc<infoschema::infoschema_v2::Data>>,
}

impl Default for KvInfoSchemaLoader {
    fn default() -> Self {
        Self::new()
    }
}

impl KvInfoSchemaLoader {
    /// 构造默认 Loader。
    pub fn new() -> Self {
        Self { v2_data: None }
    }

    /// Construct a loader whose snapshots share production InfoSchema-v2
    /// history and table cache.
    pub fn new_v2(cache_capacity: u64) -> Self {
        let data = infoschema::infoschema_v2::NewData();
        data.SetCacheCapacity(cache_capacity);
        Self {
            v2_data: Some(data),
        }
    }

    /// 在指定 KV 版本快照上读取 catalog 并构建 InfoSchema。
    fn load_at(
        &self,
        store: &dyn kv::Storage,
        version: kv::Version,
    ) -> Result<LoadedInfoSchema, kv::errors::SharedError> {
        let catalog = read_catalog(store.GetSnapshot(version).as_ref())?;
        let schema = self.v2_data.as_ref().map_or_else(
            || build_info_schema(&catalog),
            |data| build_info_schema_v2(&catalog, Arc::clone(data), version.Ver),
        );
        Ok(LoadedInfoSchema::new(schema, version.Ver))
    }
}

impl InfoSchemaLoader for KvInfoSchemaLoader {
    fn load_info_schema(
        &self,
        store: &dyn kv::Storage,
        _keyspace: &str,
    ) -> Result<LoadedInfoSchema, kv::errors::SharedError> {
        self.load_at(store, store.CurrentVersion("global")?)
    }

    fn load_snapshot_info_schema(
        &self,
        store: &dyn kv::Storage,
        _keyspace: &str,
        timestamp: u64,
    ) -> Result<LoadedInfoSchema, kv::errors::SharedError> {
        self.load_at(store, kv::NewVersion(timestamp))
    }

    fn keyspace_exists(
        &self,
        _store: &dyn kv::Storage,
        keyspace: &str,
    ) -> Result<bool, kv::errors::SharedError> {
        Ok(keyspace == "SYSTEM")
    }
}

/// 从 KV 读取并解码 DDL 目录；键不存在时返回空目录。
fn read_catalog(retriever: &dyn kv::Retriever) -> Result<MetadataCatalog, kv::errors::SharedError> {
    let private_catalog = match retriever.Get(
        &kv::Context::default(),
        kv::Key(DDL_CATALOG_KEY.to_vec()),
        &[],
    ) {
        Ok(value) => Some(decode_catalog(&value.Value)?),
        Err(error) if kv::IsErrNotFound(&error) => None,
        Err(error) => return Err(error),
    };
    let schema_version = read_tidb_string(retriever, b"SchemaVersionKey")?
        .map(|value| {
            std::str::from_utf8(&value)
                .map_err(|error| kv::errors::New(error.to_string()))?
                .parse::<i64>()
                .map_err(|error| kv::errors::New(error.to_string()))
        })
        .transpose()?
        .unwrap_or(0);
    let next_id = read_tidb_string(retriever, b"NextGlobalID")?
        .map(|value| {
            std::str::from_utf8(&value)
                .map_err(|error| kv::errors::New(error.to_string()))?
                .parse::<i64>()
                .map_err(|error| kv::errors::New(error.to_string()))
        })
        .transpose()?
        .unwrap_or(0);
    // Go metadata is authoritative even when a bootstrap repair deleted a
    // table without publishing a schema version. The private catalog cannot
    // override such edits at the same version.
    let database_entries = scan_tidb_hash(retriever, b"DBs")?;
    if database_entries.is_empty()
        && private_catalog
            .as_ref()
            .is_some_and(|catalog| catalog.version >= schema_version)
    {
        let mut catalog = private_catalog.expect("catalog checked above");
        catalog.next_id = catalog.next_id.max(next_id);
        return Ok(catalog);
    }
    let mut catalog = MetadataCatalog {
        version: schema_version,
        next_id,
        ..MetadataCatalog::default()
    };
    for (field, value) in database_entries {
        if !field.starts_with(b"DB:") {
            continue;
        }
        let database = DecodeDBInfo(&value).map_err(kv::errors::New)?;
        let name = database.Name.L.clone();
        for (table_field, table_value) in
            scan_tidb_hash(retriever, format!("DB:{}", database.ID).as_bytes())?
        {
            if !table_field.starts_with(b"Table:") {
                continue;
            }
            let mut table = DecodeTableInfo(&table_value).map_err(kv::errors::New)?;
            table.DBID = database.ID;
            catalog
                .tables
                .insert((name.clone(), table.Name.L.clone()), table);
        }
        catalog.databases.insert(name, database);
    }
    Ok(catalog)
}

/// 将 MetadataCatalog 转为 infoschema 运行时结构。
pub(crate) fn build_info_schema(catalog: &MetadataCatalog) -> SchemaRef {
    let mut schema = infoschema::infoschema::infoSchema::new(catalog.version);
    let mut databases = BTreeMap::<String, Vec<TableInfo>>::new();
    // An empty database is still schema metadata.  Keeping only databases
    // reached through tables made SHOW DATABASES hide a newly created schema,
    // so clients such as mysql-tester could not discover and clean it up.
    for database in catalog.databases.keys() {
        databases.entry(database.clone()).or_default();
    }
    for ((database, _), table) in &catalog.tables {
        databases
            .entry(database.clone())
            .or_default()
            .push(table.clone());
    }
    for (database, tables) in databases {
        let database_info = catalog.databases.get(&database);
        let db_id = database_info
            .map(|database| database.ID)
            .unwrap_or_else(|| tables.first().map_or(1, |table| table.DBID.max(1)));
        schema.add_schema(
            infoschema::DBInfo {
                id: db_id,
                name: infoschema::CiString::new(database),
                ..infoschema::DBInfo::default()
            },
            tables
                .into_iter()
                .map(infoschema::Table::from_model)
                .collect(),
        );
    }
    Arc::new(schema)
}

/// Build a V2 snapshot from the canonical KV catalog while retaining all
/// previously loaded versions in one shared `Data`.
fn build_info_schema_v2(
    catalog: &MetadataCatalog,
    data: Arc<infoschema::infoschema_v2::Data>,
    start_ts: u64,
) -> SchemaRef {
    let current = build_info_schema(catalog);
    data.resetBeforeFullLoad(catalog.version);
    for database in current.AllSchemas() {
        let tables = database.tables.clone();
        data.addDB(catalog.version, (*database).clone());
        for table in tables {
            data.add(
                &database,
                infoschema::infoschema::Table(table),
                catalog.version,
            );
        }
    }
    Arc::new(infoschema::infoschema_v2::NewInfoSchemaV2(
        data,
        catalog.version,
        start_ts,
    ))
}

/// 小端长度前缀二进制编码器，用于序列化 catalog。
struct Encoder(Vec<u8>);

impl Encoder {
    fn u32(&mut self, value: u32) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn i64(&mut self, value: i64) {
        self.0.extend_from_slice(&value.to_le_bytes());
    }
    fn string(&mut self, value: &str) {
        self.bytes(value.as_bytes());
    }
    fn bytes(&mut self, value: &[u8]) {
        self.u32(value.len() as u32);
        self.0.extend_from_slice(value);
    }
}

/// 对应 Encoder 的解码器。
struct Decoder<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Decoder<'a> {
    fn take(&mut self, length: usize) -> Result<&'a [u8], kv::errors::SharedError> {
        let end = self
            .offset
            .checked_add(length)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| kv::errors::New("truncated canonical DDL metadata"))?;
        let value = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(value)
    }
    fn u32(&mut self) -> Result<u32, kv::errors::SharedError> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("four bytes"),
        ))
    }
    fn i64(&mut self) -> Result<i64, kv::errors::SharedError> {
        Ok(i64::from_le_bytes(
            self.take(8)?.try_into().expect("eight bytes"),
        ))
    }
    fn string(&mut self) -> Result<String, kv::errors::SharedError> {
        String::from_utf8(self.bytes()?.to_vec())
            .map_err(|error| kv::errors::New(error.to_string()))
    }
    fn bytes(&mut self) -> Result<&'a [u8], kv::errors::SharedError> {
        let length = self.u32()? as usize;
        self.take(length)
    }
    fn is_exhausted(&self) -> bool {
        self.offset == self.bytes.len()
    }
}

/// 将 catalog 编码为带 `ASTERDDL2` 头的字节串。
fn encode_catalog(catalog: &MetadataCatalog) -> Result<Vec<u8>, kv::errors::SharedError> {
    let mut encoder = Encoder(b"ASTERDDL2".to_vec());
    encoder.i64(catalog.version);
    encoder.i64(catalog.next_id);
    encoder.u32(catalog.databases.len() as u32);
    for (name, database) in &catalog.databases {
        encoder.string(name);
        encoder.bytes(&EncodeDBInfo(database).map_err(kv::errors::New)?);
    }
    encoder.u32(catalog.tables.len() as u32);
    for ((database, _), table) in &catalog.tables {
        encoder.string(database);
        encoder.bytes(&EncodeTableInfo(table).map_err(kv::errors::New)?);
    }
    Ok(encoder.0)
}

/// 解码 catalog；校验魔数与无尾随字节。
fn decode_catalog(bytes: &[u8]) -> Result<MetadataCatalog, kv::errors::SharedError> {
    let mut decoder = Decoder { bytes, offset: 0 };
    if decoder.take(9)? != b"ASTERDDL2" {
        return Err(kv::errors::New("invalid canonical DDL metadata header"));
    }
    let version = decoder.i64()?;
    let next_id = decoder.i64()?;
    let database_count = decoder.u32()? as usize;
    let mut databases = BTreeMap::new();
    for _ in 0..database_count {
        let name = decoder.string()?.to_ascii_lowercase();
        let database = DecodeDBInfo(decoder.bytes()?).map_err(kv::errors::New)?;
        databases.insert(name, database);
    }
    let count = decoder.u32()? as usize;
    let mut tables = BTreeMap::new();
    for _ in 0..count {
        let database = decoder.string()?.to_ascii_lowercase();
        let mut table = DecodeTableInfo(decoder.bytes()?).map_err(kv::errors::New)?;
        table.DBID = databases
            .get(&database)
            .map(|info: &DBInfo| info.ID)
            .ok_or_else(|| kv::errors::New(format!("table has unknown database {database}")))?;
        tables.insert((database, table.Name.L.clone()), table);
    }
    if !decoder.is_exhausted() {
        return Err(kv::errors::New("trailing bytes in canonical DDL metadata"));
    }
    Ok(MetadataCatalog {
        version,
        next_id,
        databases,
        tables,
    })
}

/// The schema and commit timestamp loaded from the canonical KV store.
///
/// 从规范 KV 加载得到的 InfoSchema 及其提交时间戳。
pub struct LoadedInfoSchema {
    pub schema: SchemaRef,
    pub timestamp: u64,
}

impl LoadedInfoSchema {
    /// 组合 schema 与时间戳。
    pub fn new(schema: SchemaRef, timestamp: u64) -> Self {
        Self { schema, timestamp }
    }
}

/// Builds InfoSchema objects from metadata stored behind the canonical KV ABI.
///
/// Keeping metadata decoding separate mirrors Go Domain's use of `meta` and
/// `infoschema.Builder`; it does not introduce a second storage abstraction.
///
/// 从规范 KV ABI 背后的元数据构建 InfoSchema；解码与 Domain 存储抽象分离，
/// 对齐 Go 侧 `meta` + `infoschema.Builder` 的分工。
pub trait InfoSchemaLoader: Send + Sync {
    fn load_info_schema(
        &self,
        store: &dyn kv::Storage,
        keyspace: &str,
    ) -> Result<LoadedInfoSchema, kv::errors::SharedError>;

    fn load_snapshot_info_schema(
        &self,
        store: &dyn kv::Storage,
        keyspace: &str,
        timestamp: u64,
    ) -> Result<LoadedInfoSchema, kv::errors::SharedError>;

    fn keyspace_exists(
        &self,
        store: &dyn kv::Storage,
        keyspace: &str,
    ) -> Result<bool, kv::errors::SharedError>;
}

/// Shared ownership for the one canonical KV Storage owned by a Domain.
///
/// `Storage::Close` requires mutable access, while all other operations use a
/// shared reference. The lock preserves that lifecycle boundary without
/// copying the Storage method surface into Domain.
///
/// Domain 持有的唯一规范 KV Storage 的共享句柄。
/// `Close` 需要可变借用，其余操作用共享引用；锁维护该生命周期边界。
pub struct StorageHandle {
    inner: RwLock<Box<dyn kv::Storage + Send + Sync>>,
}

impl StorageHandle {
    /// 包装具体 Storage 实现。
    pub fn new<S>(store: S) -> Self
    where
        S: kv::Storage + Send + Sync + 'static,
    {
        Self {
            inner: RwLock::new(Box::new(store)),
        }
    }

    /// 在读锁下访问内部 Storage。
    pub fn with_storage<R>(&self, operation: impl FnOnce(&dyn kv::Storage) -> R) -> R {
        let store = self.inner.read().expect("domain storage lock poisoned");
        operation(store.as_ref())
    }

    /// 在写锁下关闭 Storage。
    pub fn close(&self) -> Result<(), kv::errors::SharedError> {
        self.inner
            .write()
            .expect("domain storage lock poisoned")
            .Close()
    }
}

impl astersql_statistics_handle::StatsKvStorage for StorageHandle {
    fn current_version(&self) -> Result<kv::Version, kv::errors::SharedError> {
        self.with_storage(|store| store.CurrentVersion("global"))
    }

    fn snapshot(&self, version: kv::Version) -> Box<dyn kv::Snapshot> {
        self.with_storage(|store| store.GetSnapshot(version))
    }

    fn begin(&self) -> Result<Box<dyn kv::Transaction>, kv::errors::SharedError> {
        self.with_storage(|store| store.Begin(&[]))
    }
}
