// Copyright 2026 AsterSQL.

//! Convert the canonical InfoSchema table model into TTL worker metadata.

use astersql_infoschema::infoschema::InfoSchema;
use astersql_meta_model::{StatePublic, TableInfo};
use astersql_parser_ast::TimeUnitType;
use astersql_parser_duration::ParseDuration;
use astersql_ttl_cache::table::{EvalExpireTime, TimeUnit};
use astersql_ttl_ttlworker::session::PhysicalTable;
use std::sync::Arc;

use astersql_domain::{Domain, StorageHandle};
use astersql_ttl_cache::table::{
    Column as CacheColumn, KeyKind, KeyRange, PhysicalTable as CachePhysicalTable, RegionProvider,
    ScanRange, TableInfo as CacheTableInfo,
};

struct StorageRegions(Arc<StorageHandle>);

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
) -> Result<Vec<ScanRange>, String> {
    let (_, model) = domain
        .stats_table(&table.schema, &table.table)
        .ok_or_else(|| format!("TTL table {}.{} disappeared", table.schema, table.table))?;
    // The current range decoder handles one integer or byte handle. A
    // composite handle or an unsupported collation must use Go's safe
    // full-range fallback until its exact boundary decoding is available.
    if table.key_columns.len() != 1 {
        return Ok(vec![astersql_ttl_cache::table::newFullRange()]);
    }
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
            return Ok(vec![astersql_ttl_cache::table::newFullRange()]);
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
                name: name.clone(),
                public: true,
                key_kind: kind,
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
    CachePhysicalTable {
        ID: table.physical_id,
        Schema: table.schema.clone(),
        TableInfo: CacheTableInfo {
            id: table.table_id,
            name: table.table.clone(),
            ..Default::default()
        },
        Partition: table.partition_name.clone().unwrap_or_default(),
        PartitionDef: None,
        KeyColumns: key_columns,
        TimeColumn: CacheColumn::default(),
    }
    .SplitScanRanges(
        Some(&StorageRegions(domain.storage_handle())),
        astersql_ttl_ttlworker::config::scan_split_count(
            store_count.is_some(),
            store_count.unwrap_or(0),
        ),
    )
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
