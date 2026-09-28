// Copyright 2026 AsterSQL.

//! TiDB JSON 表统计 fixture 加载。

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use astersql_domain::Domain;
use astersql_statistics_handle::{Bucket, ColumnStats, IndexStats, TableStats};
use base64::Engine as _;
use serde_json::Value;

use crate::{TestError, TestResult};

fn field<'a>(value: &'a Value, name: &str) -> TestResult<&'a Value> {
    value
        .get(name)
        .ok_or_else(|| TestError::new(format!("statistics field {name:?} is missing")))
}

fn string_field<'a>(value: &'a Value, name: &str) -> TestResult<&'a str> {
    field(value, name)?
        .as_str()
        .ok_or_else(|| TestError::new(format!("statistics field {name:?} is not a string")))
}

fn i64_field(value: &Value, name: &str) -> TestResult<i64> {
    field(value, name)?
        .as_i64()
        .ok_or_else(|| TestError::new(format!("statistics field {name:?} is not an integer")))
}

fn u64_field(value: &Value, name: &str) -> TestResult<u64> {
    field(value, name)?.as_u64().ok_or_else(|| {
        TestError::new(format!(
            "statistics field {name:?} is not an unsigned integer"
        ))
    })
}

fn f64_field(value: &Value, name: &str) -> TestResult<f64> {
    field(value, name)?
        .as_f64()
        .ok_or_else(|| TestError::new(format!("statistics field {name:?} is not a number")))
}

fn decode_bound(value: &Value, name: &str) -> TestResult<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(string_field(value, name)?)
        .map_err(|error| TestError::new(format!("decode statistics {name}: {error}")))
}

fn histogram(value: &Value) -> TestResult<(i64, Vec<Bucket>)> {
    let Some(histogram) = value
        .get("histogram")
        .filter(|histogram| !histogram.is_null())
    else {
        return Ok((0, Vec::new()));
    };
    let buckets = histogram
        .get("buckets")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|bucket| {
            Ok(Bucket {
                count: i64_field(bucket, "count")?,
                repeats: i64_field(bucket, "repeats")?,
                lower: decode_bound(bucket, "lower_bound")?,
                upper: decode_bound(bucket, "upper_bound")?,
                ndv: i64_field(bucket, "ndv")?,
            })
        })
        .collect::<TestResult<Vec<_>>>()?;
    Ok((i64_field(histogram, "ndv")?, buckets))
}

fn top_n(value: &Value) -> TestResult<Vec<(Vec<u8>, u64)>> {
    value
        .get("cm_sketch")
        .filter(|sketch| !sketch.is_null())
        .and_then(|sketch| sketch.get("top_n"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .map(|item| {
            Ok((
                base64::engine::general_purpose::STANDARD
                    .decode(string_field(item, "data")?)
                    .map_err(|error| TestError::new(format!("decode statistics TopN: {error}")))?,
                u64_field(item, "count")?,
            ))
        })
        .collect()
}

/// 加载 Go `testkit.LoadTableStats` 使用的 TiDB JSON 表统计到 `Domain`。
pub fn LoadTableStats(path: impl AsRef<Path>, domain: &Domain) -> TestResult {
    let path = path.as_ref();
    let bytes = fs::read(path)
        .map_err(|error| TestError::new(format!("read {}: {error}", path.display())))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| TestError::new(format!("decode {}: {error}", path.display())))?;
    let database_name = string_field(&value, "database_name")?;
    let table_name = string_field(&value, "table_name")?;
    let info = domain
        .table_by_name(database_name, table_name)
        .map_err(|error| {
            TestError::new(format!("stats table {database_name}.{table_name}: {error}"))
        })?;
    let count = i64_field(&value, "count")?;
    let version = u64_field(&value, "version")?;
    let mut stats = TableStats {
        physical_id: info.ID,
        pseudo: false,
        initialized: true,
        version,
        modify_count: i64_field(&value, "modify_count")?,
        realtime_count: count,
        analyze_count: count,
        last_analyze_version: version,
        last_stats_hist_version: version,
        stats_version: 0,
        indexes: HashMap::new(),
        columns: HashMap::new(),
        pre_scalar_ready: false,
    };

    let columns = field(&value, "columns")?
        .as_object()
        .ok_or_else(|| TestError::new("statistics field \"columns\" is not an object"))?;
    for (name, column) in columns {
        let column_info = info
            .Columns
            .iter()
            .find(|candidate| candidate.Name.L == *name)
            .ok_or_else(|| {
                TestError::new(format!(
                    "fixture column {name:?} is missing from {database_name}.{table_name}"
                ))
            })?;
        let (ndv, buckets) = histogram(column)?;
        let stats_version = column
            .get("stats_ver")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        let null_count = i64_field(column, "null_count")?;
        let total_column_size = i64_field(column, "tot_col_size")?;
        stats.stats_version = stats.stats_version.max(stats_version);
        stats.columns.insert(
            column_info.ID,
            ColumnStats {
                analyzed_or_synthesized: stats_version != 0 || ndv > 0 || null_count > 0,
                stats_version,
                ndv,
                null_count,
                total_column_size,
                version: u64_field(column, "last_update_version")?,
                loaded_or_evicted: true,
                field_type: 0,
                correlation: f64_field(column, "correlation")?,
                average_size: if count > 0 {
                    total_column_size as f64 / count as f64
                } else {
                    0.0
                },
                top_n: top_n(column)?,
                buckets,
                fm_sketch: Vec::new(),
            },
        );
    }

    let indexes = field(&value, "indices")?
        .as_object()
        .ok_or_else(|| TestError::new("statistics field \"indices\" is not an object"))?;
    for (name, index) in indexes {
        let index_info = info
            .Indices
            .iter()
            .find(|candidate| candidate.Name.L == *name)
            .ok_or_else(|| {
                TestError::new(format!(
                    "fixture index {name:?} is missing from {database_name}.{table_name}"
                ))
            })?;
        let (ndv, buckets) = histogram(index)?;
        let stats_version = index
            .get("stats_ver")
            .and_then(Value::as_i64)
            .unwrap_or_default();
        stats.stats_version = stats.stats_version.max(stats_version);
        stats.indexes.insert(
            index_info.ID,
            IndexStats {
                analyzed: stats_version != 0,
                stats_version,
                version: u64_field(index, "last_update_version")?,
                ndv,
                null_count: i64_field(index, "null_count")?,
                total_column_size: i64_field(index, "tot_col_size")?,
                correlation: f64_field(index, "correlation")?,
                cms_loaded: false,
                top_n: top_n(index)?,
                buckets,
                fully_loaded: true,
                fm_sketch: Vec::new(),
            },
        );
    }

    domain
        .stats_handle()
        .lock()
        .map_err(|_| TestError::new("statistics handle lock poisoned"))?
        .cache_mut()
        .put(stats);
    Ok(())
}
