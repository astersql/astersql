// Copyright 2026 AsterSQL.

//! Convert the canonical InfoSchema table model into TTL worker metadata.

use astersql_infoschema::infoschema::InfoSchema;
use astersql_meta_model::{StatePublic, TableInfo};
use astersql_parser_ast::TimeUnitType;
use astersql_parser_duration::ParseDuration;
use astersql_ttl_cache::table::{EvalExpireTime, TimeUnit};
use astersql_ttl_ttlworker::session::PhysicalTable;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use astersql_domain::{Domain, StorageHandle};
use astersql_ttl_cache::table::{
    Column as CacheColumn, IndexColumn as CacheIndexColumn, IndexInfo as CacheIndexInfo, KeyKind,
    KeyRange, PhysicalTable as CachePhysicalTable, RegionProvider, ScanRange,
    TableInfo as CacheTableInfo,
};
use astersql_ttl_ttlworker::job_version_checker::{
    JobVersionCheckResult, JobVersionChecker, ServerInfo as TtlServerInfo, VersionInfo,
};
use astersql_ttl_ttlworker::scan::ScanIndex;

pub struct TtlScanRanges {
    pub ranges: Vec<ScanRange>,
    pub index: Option<ScanIndex>,
}

struct StorageRegions(Arc<StorageHandle>);

fn ttl_index_scan_version_check() -> JobVersionCheckResult {
    static CHECKER: OnceLock<Mutex<JobVersionChecker>> = OnceLock::new();
    let convert = |info: astersql_domain_infosync::ServerInfo| TtlServerInfo {
        version: VersionInfo {
            version: info.Version,
            git_hash: info.GitHash,
        },
        // The Rust infosync model contains only registered, concrete TiDB
        // servers. Synthetic assumed entries are not returned by this API.
        assumed: false,
    };
    let local = astersql_domain_infosync::GetServerInfo()
        .map(convert)
        .map(Some)
        .map_err(|error| error.to_string());
    let all = astersql_domain_infosync::GetAllServerInfo()
        .map(|servers| {
            servers
                .into_iter()
                .map(|(id, info)| (id, Some(convert(info))))
                .collect()
        })
        .map_err(|error| error.to_string());
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    CHECKER
        .get_or_init(|| Mutex::new(JobVersionChecker::default()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .check(now, local, all)
}

impl RegionProvider for StorageRegions {
    fn locate_key_range(&self, start: &[u8], end: &[u8]) -> Result<Vec<KeyRange>, String> {
        self.0
            .with_storage(|store| store.TTLRegionRanges(start, end))
            .map(|ranges| {
                ranges
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(start, end)| KeyRange { start, end })
                    .collect()
            })
            .map_err(|error| error.to_string())
    }
}

pub fn split_ttl_scan_ranges(
    domain: &Arc<Domain>,
    table: &PhysicalTable,
    expire_time: i64,
) -> Result<TtlScanRanges, String> {
    let (_, model) = domain
        .stats_table(&table.schema, &table.table)
        .ok_or_else(|| format!("TTL table {}.{} disappeared", table.schema, table.table))?;
    // These limitations apply only to the legacy primary-key splitter. An
    // eligible TTL index is evaluated first and can safely serve tables with a
    // composite or otherwise unsupported primary key.
    let mut primary_split_supported = table.key_columns.len() == 1;
    if table.key_columns[0] != "_tidb_rowid" {
        let first = model
            .Columns
            .iter()
            .find(|column| column.Name.L.eq_ignore_ascii_case(&table.key_columns[0]))
            .ok_or_else(|| "TTL key column disappeared".to_owned())?;
        let supported_string = first.GetType() == astersql_parser_mysql::r#type::TypeBit
            || matches!(
                first.GetType(),
                astersql_parser_mysql::r#type::TypeString
                    | astersql_parser_mysql::r#type::TypeVarString
                    | astersql_parser_mysql::r#type::TypeVarchar
            ) && (astersql_parser_mysql::r#type::HasBinaryFlag(first.GetFlag())
                || matches!(first.GetCharset(), "ascii" | "latin1")
                || matches!(
                    first.GetCollate(),
                    "utf8_bin" | "utf8mb4_bin" | "utf8mb4_0900_bin"
                ));
        if !astersql_parser_mysql::util::IsIntegerType(first.GetType()) && !supported_string {
            primary_split_supported = false;
        }
    }
    let key_columns = table
        .key_columns
        .iter()
        .map(|name| {
            let kind = if name == "_tidb_rowid" {
                KeyKind::SignedInt
            } else {
                let column = model
                    .Columns
                    .iter()
                    .find(|column| column.Name.L.eq_ignore_ascii_case(name))
                    .ok_or_else(|| format!("TTL key column {name} disappeared"))?;
                if astersql_parser_mysql::util::IsIntegerType(column.GetType()) {
                    if astersql_parser_mysql::r#type::HasUnsignedFlag(column.GetFlag()) {
                        KeyKind::UnsignedInt
                    } else {
                        KeyKind::SignedInt
                    }
                } else {
                    KeyKind::Bytes
                }
            };
            Ok(CacheColumn {
                id: if name == "_tidb_rowid" {
                    -1
                } else {
                    model
                        .Columns
                        .iter()
                        .find(|column| column.Name.L.eq_ignore_ascii_case(name))
                        .map(|column| column.ID)
                        .unwrap_or(-1)
                },
                name: name.clone(),
                public: true,
                key_kind: kind,
                nullable: false,
                hidden: false,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let store_count = match domain
        .storage_handle()
        .with_storage(|store| store.TTLStoreCount())
    {
        Ok(count) => count,
        Err(error) => {
            super::BgLogger().log(
                super::LogLevel::Warn,
                "read TiKV store count for TTL scan splitting failed; use default",
                [super::LogField::String("error".into(), error.to_string())],
            );
            None
        }
    };
    let cache_columns = model
        .Columns
        .iter()
        .map(|column| CacheColumn {
            id: column.ID,
            name: column.Name.O.clone(),
            public: column.State == StatePublic,
            key_kind: if matches!(
                column.GetType(),
                astersql_parser_mysql::r#type::TypeFloat
                    | astersql_parser_mysql::r#type::TypeDouble
            ) {
                KeyKind::Float
            } else if column.GetType() == astersql_parser_mysql::r#type::TypeSet {
                KeyKind::Set
            } else if astersql_parser_mysql::util::IsIntegerType(column.GetType()) {
                if astersql_parser_mysql::r#type::HasUnsignedFlag(column.GetFlag()) {
                    KeyKind::UnsignedInt
                } else {
                    KeyKind::SignedInt
                }
            } else {
                KeyKind::Bytes
            },
            nullable: !astersql_parser_mysql::r#type::HasNotNullFlag(column.GetFlag()),
            hidden: column.Hidden,
        })
        .collect::<Vec<_>>();
    let cache_indexes = model
        .Indices
        .iter()
        .map(|index| CacheIndexInfo {
            id: index.ID,
            name: index.Name.O.clone(),
            columns: index
                .Columns
                .iter()
                .filter_map(|column| {
                    usize::try_from(column.Offset)
                        .ok()
                        .map(|offset| CacheIndexColumn {
                            column_offset: offset,
                            prefix_length: (column.Length != -1)
                                .then_some(column.Length.max(0) as usize),
                        })
                })
                .collect(),
            unique: index.Unique,
            primary: index.Primary,
            public: index.State == StatePublic,
            invisible: index.Invisible,
            global: index.Global,
            multi_valued: index.MVIndex,
            columnar: index.VectorInfo.is_some()
                || index.InvertedInfo.is_some()
                || index.FullTextInfo.is_some(),
            conditional: !index.ConditionExprString.is_empty(),
        })
        .collect::<Vec<_>>();
    let time_column = cache_columns
        .iter()
        .find(|column| column.name.eq_ignore_ascii_case(&table.ttl_column))
        .cloned()
        .ok_or_else(|| "TTL time column disappeared".to_owned())?;
    let cache_table = CachePhysicalTable {
        ID: table.physical_id,
        Schema: table.schema.clone(),
        TableInfo: CacheTableInfo {
            id: table.table_id,
            name: table.table.clone(),
            columns: cache_columns,
            indexes: cache_indexes.clone(),
            ..Default::default()
        },
        Partition: table.partition_name.clone().unwrap_or_default(),
        PartitionDef: None,
        KeyColumns: key_columns,
        TimeColumn: time_column,
        Indices: cache_indexes,
    };
    let split_count = astersql_ttl_ttlworker::config::scan_split_count(
        store_count.is_some(),
        store_count.unwrap_or(0),
    );
    let regions = StorageRegions(domain.storage_handle());
    if astersql_sessionctx_vardef::TTLEnableIndexScan.Load()
        && let Some(index) = cache_table.FindTTLIndex()
    {
        match ttl_index_scan_version_check() {
            JobVersionCheckResult::FallbackToPrimaryKey => {}
            JobVersionCheckResult::BlockJob => {
                return Err(
                    "cannot create TTL job while TiDB server build versions are inconsistent"
                        .into(),
                );
            }
            JobVersionCheckResult::AllowIndexScan => {
                let ranges = cache_table.SplitIndexScanRanges(
                    Some(&regions),
                    &index,
                    expire_time,
                    split_count,
                )?;
                let columns = index
                    .columns
                    .iter()
                    .filter_map(|index_column| {
                        cache_table
                            .TableInfo
                            .columns
                            .get(index_column.column_offset)
                            .map(|column| column.name.clone())
                    })
                    .collect();
                return Ok(TtlScanRanges {
                    ranges,
                    index: Some(ScanIndex {
                        id: index.id,
                        name: index.name,
                        columns,
                        unique: index.unique,
                    }),
                });
            }
        }
    }
    if !primary_split_supported {
        return Ok(TtlScanRanges {
            ranges: vec![astersql_ttl_cache::table::newFullRange()],
            index: None,
        });
    }
    Ok(TtlScanRanges {
        ranges: cache_table.SplitScanRanges(Some(&regions), split_count)?,
        index: None,
    })
}

pub struct TtlSchedule {
    pub table: PhysicalTable,
    pub job_interval_seconds: u64,
    pub job_interval_expression: String,
}

pub fn collect_ttl_schedules(
    info_schema: &dyn InfoSchema,
    now_seconds: u64,
) -> Result<Vec<TtlSchedule>, String> {
    let mut result = Vec::new();
    for schema in info_schema.AllSchemas() {
        let tables = info_schema
            .SchemaTableInfos(&schema.name)
            .map_err(|error| error.to_string())?;
        for table in tables {
            let Some(model) = table.model_meta.as_ref() else {
                continue;
            };
            let Some(ttl) = model.TTLInfo.as_ref().filter(|ttl| ttl.Enable) else {
                continue;
            };
            let interval = if ttl.JobInterval.is_empty() {
                astersql_meta_model::DefaultTTLJobInterval
            } else {
                &ttl.JobInterval
            };
            let job_interval_seconds = ParseDuration(interval)?.as_secs();
            for table in physical_ttl_tables(&schema.name.original, model, now_seconds)? {
                result.push(TtlSchedule {
                    table,
                    job_interval_seconds,
                    job_interval_expression: interval.to_owned(),
                });
            }
        }
    }
    Ok(result)
}

/// Read a single InfoSchema snapshot so a TTL scheduling pass never mixes
/// table definitions from different schema versions.
pub fn collect_physical_ttl_tables(
    info_schema: &dyn InfoSchema,
    now_seconds: u64,
) -> Result<Vec<PhysicalTable>, String> {
    collect_ttl_schedules(info_schema, now_seconds).map(|schedules| {
        schedules
            .into_iter()
            .map(|schedule| schedule.table)
            .collect()
    })
}

pub fn physical_ttl_tables(
    schema: &str,
    table: &TableInfo,
    now_seconds: u64,
) -> Result<Vec<PhysicalTable>, String> {
    let Some(ttl) = table.TTLInfo.as_ref().filter(|ttl| ttl.Enable) else {
        return Ok(Vec::new());
    };
    if table.State != StatePublic {
        return Ok(Vec::new());
    }
    let unit = match ttl.IntervalTimeUnit {
        value if value == TimeUnitType::Microsecond as i32 => TimeUnit::Microsecond,
        value if value == TimeUnitType::Second as i32 => TimeUnit::Second,
        value if value == TimeUnitType::Minute as i32 => TimeUnit::Minute,
        value if value == TimeUnitType::HourMinute as i32 => TimeUnit::HourMinute,
        value if value == TimeUnitType::Hour as i32 => TimeUnit::Hour,
        value if value == TimeUnitType::Day as i32 => TimeUnit::Day,
        value if value == TimeUnitType::Week as i32 => TimeUnit::Week,
        value if value == TimeUnitType::Month as i32 => TimeUnit::Month,
        value if value == TimeUnitType::Quarter as i32 => TimeUnit::Quarter,
        value if value == TimeUnitType::Year as i32 => TimeUnit::Year,
        _ => {
            return Err(format!(
                "unsupported TTL interval time unit {}",
                ttl.IntervalTimeUnit
            ));
        }
    };
    let now = i64::try_from(now_seconds).map_err(|_| "TTL time is outside i64 range")?;
    let expire = EvalExpireTime(now, &ttl.IntervalExprStr, unit)?;
    let expire_after_seconds = now.saturating_sub(expire).max(0) as u64;
    let key_columns = if table.PKIsHandle {
        vec![
            table
                .GetPkColInfo()
                .ok_or_else(|| {
                    format!(
                        "TTL table {schema}.{} has no primary key column",
                        table.Name.O
                    )
                })?
                .Name
                .O
                .clone(),
        ]
    } else if table.IsCommonHandle {
        table
            .Indices
            .iter()
            .find(|index| index.Primary && index.State == StatePublic)
            .ok_or_else(|| {
                format!(
                    "TTL table {schema}.{} has no public primary index",
                    table.Name.O
                )
            })?
            .Columns
            .iter()
            .map(|column| column.Name.O.clone())
            .collect()
    } else {
        vec!["_tidb_rowid".into()]
    };
    let partitions = table
        .Partition
        .as_ref()
        .filter(|partition| partition.Enable)
        .map(|partition| {
            partition
                .Definitions
                .iter()
                .map(|definition| (definition.ID, Some(definition.Name.O.clone())))
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![(table.ID, None)]);
    Ok(partitions
        .into_iter()
        .map(|(physical_id, partition_name)| PhysicalTable {
            partition_name,
            table_id: table.ID,
            physical_id,
            schema: schema.into(),
            table: table.Name.O.clone(),
            key_columns: key_columns.clone(),
            ttl_column: ttl.ColumnName.O.clone(),
            ttl_enabled: true,
            definition_version: table.UpdateTS,
            expire_after_seconds,
        })
        .collect())
}
