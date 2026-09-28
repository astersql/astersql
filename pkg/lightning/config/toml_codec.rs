// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// TOML load/encode helpers mirroring Go `LoadFromTOML` / BurntSushi decode.
//
// 将 TOML 配置映射到完整 `Config`：按段应用字段、跟踪已用键，并对
// Config/GlobalConfig 均未识别的键报错。另提供部分段的编码辅助。

use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::Ordering;

use crate::{
    ByteSize, CheckpointKeepStrategy, CompressionType, Config, ConfigError, Conflict,
    DuplicateResolutionAlgorithm, Duration, FileRouteRule, IgnoreColumns, MaxError, PostOpLevel,
};

/// 仅属于 GlobalConfig 的键前缀：在完整 Config 加载时视为“已使用”，不当未知项报错。
const GLOBAL_ONLY_PREFIXES: &[&str] = &[
    "lightning.status-addr",
    "lightning.server-mode",
    "lightning.pprof-port",
    "lightning.level",
    "lightning.file",
    "tidb.log-level",
];

/// 解析 TOML 并写入 `Config`；未知且非全局专属的键返回 Invalid。
pub fn load_config_from_toml(cfg: &mut Config, data: &[u8]) -> Result<(), ConfigError> {
    let text = std::str::from_utf8(data).map_err(|error| ConfigError::Parse(error.to_string()))?;
    let value: toml::Value = toml::from_str(text).map_err(|error| {
        // Preserve BurntSushi-style "toml: line N: ..." wording when possible.
        // 尽量保留 BurntSushi 风格的 "toml: line N: ..." 错误文案。
        ConfigError::Parse(format!("toml: {error}"))
    })?;
    let mut used = BTreeSet::new();
    apply_value(cfg, &value, "", &mut used)?;
    let all = collect_keys(&value, "");
    let mut both_unused = Vec::new();
    for key in all {
        if used.contains(&key) {
            continue;
        }
        if matches!(
            key.as_str(),
            "lightning.max-error.type" | "lightning.max-error.conflict"
        ) {
            continue;
        }
        // Go: unused in Config ∩ unused in GlobalConfig → error.
        // Go：Config 与 GlobalConfig 均未使用的键才报错。
        // Global-only keys are "used" by GlobalConfig, so they become warnings.
        // 全局专属键由 GlobalConfig“占用”，此处跳过。
        if is_global_config_key(&key) {
            continue;
        }
        // Skip pure structural parents already visited while walking.
        // 跳过遍历时已访问的纯结构父节点。
        if key
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-')
            && !key.contains('.')
        {
            continue;
        }
        both_unused.push(key);
    }
    if !both_unused.is_empty() {
        return Err(ConfigError::Invalid(format!(
            "config file contained unknown configuration options: {}",
            both_unused.join(", ")
        )));
    }
    Ok(())
}

/// 判断键是否为全局配置专属（不触发未知选项错误）。
fn is_global_config_key(key: &str) -> bool {
    GLOBAL_ONLY_PREFIXES.iter().any(|prefix| key == *prefix)
}

/// 递归收集 TOML 表/数组中所有点分路径键。
fn collect_keys(value: &toml::Value, prefix: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    match value {
        toml::Value::Table(table) => {
            for (key, child) in table {
                let path = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                out.insert(path.clone());
                out.extend(collect_keys(child, &path));
            }
        }
        toml::Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                let path = format!("{prefix}.{index}");
                out.insert(path.clone());
                out.extend(collect_keys(child, &path));
            }
        }
        _ => {}
    }
    out
}

/// 按顶层段名分发到各 `apply_*`；标记已处理路径。
fn apply_value(
    cfg: &mut Config,
    value: &toml::Value,
    prefix: &str,
    used: &mut BTreeSet<String>,
) -> Result<(), ConfigError> {
    let Some(table) = value.as_table() else {
        return Ok(());
    };
    for (key, child) in table {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}.{key}")
        };
        used.insert(path.clone());
        match path.as_str() {
            "lightning" => apply_lightning(&mut cfg.app, child, &path, used)?,
            "tidb" => apply_tidb(cfg, child, &path, used)?,
            "checkpoint" => apply_checkpoint(cfg, child, &path, used)?,
            "mydumper" => apply_mydumper(cfg, child, &path, used)?,
            "tikv-importer" => apply_importer(cfg, child, &path, used)?,
            "post-restore" => apply_post_restore(cfg, child, &path, used)?,
            "cron" => apply_cron(cfg, child, &path, used)?,
            "security" => apply_security(&mut cfg.security, child, &path, used)?,
            "conflict" => apply_conflict(&mut cfg.conflict, child, &path, used)?,
            "routes" => apply_routes(&mut cfg.routes, child, &path, used)?,
            _ => {
                // leave for unused-key analysis
                // 留给后续未知键分析。
            }
        }
    }
    Ok(())
}

/// 将某子树全部路径记入 used（用于复杂结构如 routes，避免误报未知键）。
fn mark_subtree_used(value: &toml::Value, prefix: &str, used: &mut BTreeSet<String>) {
    used.insert(prefix.to_owned());
    if let Some(table) = value.as_table() {
        for (key, child) in table {
            mark_subtree_used(child, &format!("{prefix}.{key}"), used);
        }
    } else if let Some(items) = value.as_array() {
        for (index, child) in items.iter().enumerate() {
            mark_subtree_used(child, &format!("{prefix}.{index}"), used);
        }
    }
}

/// 应用 `[lightning]` 段：并发度、错误上限等；全局专属子键仅标记已用。
fn apply_lightning(
    app: &mut crate::Lightning,
    value: &toml::Value,
    prefix: &str,
    used: &mut BTreeSet<String>,
) -> Result<(), ConfigError> {
    let Some(table) = value.as_table() else {
        return Ok(());
    };
    for (key, child) in table {
        let path = format!("{prefix}.{key}");
        used.insert(path.clone());
        match key.as_str() {
            "table-concurrency" => app.table_concurrency = int_value(child, &path)?,
            "index-concurrency" => app.index_concurrency = int_value(child, &path)?,
            "region-concurrency" => app.region_concurrency = int_value(child, &path)?,
            "io-concurrency" => app.io_concurrency = int_value(child, &path)?,
            "check-requirements" => app.check_requirements = bool_value(child, &path)?,
            "meta-schema-name" => app.meta_schema_name = string_value(child)?,
            "task-info-schema-name" => app.task_info_schema_name = string_value(child)?,
            "max-error-records" => app.max_error_records = i64_value(child, &path)?,
            "max-error" => apply_max_error(&mut app.max_error, child, &path, used)?,
            "status-addr" | "server-mode" | "pprof-port" | "level" | "file" => {
                mark_subtree_used(child, &path, used);
            }
            "typo" => {
                // intentionally leave unused so unknown-key check fires
                // 故意保持未使用，以便触发未知键检查。
                used.remove(&path);
            }
            _ => {
                used.remove(&path);
            }
        }
    }
    Ok(())
}

/// 解析 `max-error`：支持整数遗留写法或按错误类型分项的表。
fn apply_max_error(
    max_error: &mut MaxError,
    value: &toml::Value,
    prefix: &str,
    used: &mut BTreeSet<String>,
) -> Result<(), ConfigError> {
    match value {
        toml::Value::Integer(v) => {
            max_error.set_legacy_value(*v);
            Ok(())
        }
        toml::Value::Table(table) => {
            let mut map = HashMap::new();
            for (key, child) in table {
                let path = format!("{prefix}.{key}");
                used.insert(path.clone());
                match key.as_str() {
                    "syntax" | "charset" | "type" | "conflict" => {
                        if let Some(v) = child.as_integer() {
                            map.insert(key.clone(), v);
                        } else {
                            return Err(ConfigError::Parse(format!(
                                "toml: line 1 (last key \"{path}\"): expected value but found \"{child}\" instead"
                            )));
                        }
                    }
                    _ => {
                        // unknown keys inside max-error are ignored by Go UnmarshalTOML
                        // Go UnmarshalTOML 忽略 max-error 内未知子键。
                    }
                }
            }
            max_error.set_table(&map);
            // charset stays MaxInt unless explicitly handled; set_table already resets.
            // charset 默认保持 MaxInt；set_table 已重置。
            if let Some(syntax) = map.get("syntax") {
                max_error
                    .syntax
                    .store(if *syntax >= 0 { 0 } else { 0 }, Ordering::Relaxed);
            }
            if let Some(charset) = map.get("charset").copied() {
                // Go keeps charset at MaxInt always in UnmarshalTOML map path.
                // Go 在 map 路径下始终将 charset 保持为 MaxInt。
                let _ = charset;
                max_error.charset.store(i64::MAX, Ordering::Relaxed);
            }
            if let Some(type_v) = map.get("type").copied() {
                max_error.r#type.store(type_v.max(0), Ordering::Relaxed);
            }
            if let Some(conflict) = map.get("conflict").copied() {
                max_error.conflict.store(conflict.max(0), Ordering::Relaxed);
            }
            Ok(())
        }
        toml::Value::String(text) => Err(ConfigError::Parse(format!(
            "toml: line 1 (last key \"max-error\"): invalid max-error '{text}', should be an integer or a map of string:int64"
        ))),
        other => Err(ConfigError::Parse(format!(
            "toml: line 1 (last key \"{prefix}\"): invalid max-error '{other}', should be an integer or a map of string:int64"
        ))),
    }
}

/// 应用 `[tidb]` 连接与扫描并发等字段。
fn apply_tidb(
    cfg: &mut Config,
    value: &toml::Value,
    prefix: &str,
    used: &mut BTreeSet<String>,
) -> Result<(), ConfigError> {
    let Some(table) = value.as_table() else {
        return Ok(());
    };
    for (key, child) in table {
        let path = format!("{prefix}.{key}");
        used.insert(path.clone());
        match key.as_str() {
            "host" => cfg.tidb.host = string_value(child)?,
            "port" => cfg.tidb.port = int_value(child, &path)?,
            "user" => cfg.tidb.user = string_value(child)?,
            "password" => cfg.tidb.password = string_value(child)?,
            "status-port" => cfg.tidb.status_port = int_value(child, &path)?,
            "pd-addr" => cfg.tidb.pd_addr = string_value(child)?,
            "sql-mode" => cfg.tidb.sql_mode_text = string_value(child)?,
            "tls" => cfg.tidb.tls = string_value(child)?,
            "distsql-scan-concurrency" => {
                cfg.tidb.distsql_scan_concurrency = int_value(child, &path)?
            }
            "max-allowed-packet" => cfg.tidb.max_allowed_packet = u64_value(child, &path)?,
            "build-stats-concurrency" => {
                cfg.tidb.build_stats_concurrency = int_value(child, &path)?
            }
            "index-serial-scan-concurrency" => {
                cfg.tidb.index_serial_scan_concurrency = int_value(child, &path)?
            }
            "checksum-table-concurrency" => {
                cfg.tidb.checksum_table_concurrency = int_value(child, &path)?
            }
            "session-vars" => {
                cfg.tidb.vars = string_map(child, &path)?;
                mark_subtree_used(child, &path, used);
            }
            "security" => {
                let security = cfg
                    .tidb
                    .security
                    .get_or_insert_with(crate::Security::default);
                apply_security(security, child, &path, used)?;
            }
            "log-level" => mark_subtree_used(child, &path, used),
            _ => {
                used.remove(&path);
            }
        }
    }
    Ok(())
}

/// 应用 `[checkpoint]`：驱动、DSN、成功后保留策略等。
fn apply_checkpoint(
    cfg: &mut Config,
    value: &toml::Value,
    prefix: &str,
    used: &mut BTreeSet<String>,
) -> Result<(), ConfigError> {
    let Some(table) = value.as_table() else {
        return Ok(());
    };
    for (key, child) in table {
        let path = format!("{prefix}.{key}");
        used.insert(path.clone());
        match key.as_str() {
            "enable" => cfg.checkpoint.enable = bool_value(child, &path)?,
            "driver" => cfg.checkpoint.driver = string_value(child)?,
            "dsn" => cfg.checkpoint.dsn = string_value(child)?,
            "schema" => cfg.checkpoint.schema = string_value(child)?,
            "keep-after-success" => {
                cfg.checkpoint
                    .keep_after_success
                    .from_toml_value(child)
                    .map_err(ConfigError::Parse)?;
            }
            _ => {
                used.remove(&path);
            }
        }
    }
    Ok(())
}

/// 应用 `[mydumper]`：数据源目录、过滤、CSV、批大小与 Region 上限等。
fn apply_mydumper(
    cfg: &mut Config,
    value: &toml::Value,
    prefix: &str,
    used: &mut BTreeSet<String>,
) -> Result<(), ConfigError> {
    let Some(table) = value.as_table() else {
        return Ok(());
    };
    for (key, child) in table {
        let path = format!("{prefix}.{key}");
        used.insert(path.clone());
        match key.as_str() {
            "data-source-dir" => cfg.mydumper.source_dir = string_value(child)?,
            "source-id" => cfg.mydumper.source_id = string_value(child)?,
            "no-schema" => cfg.mydumper.no_schema = bool_value(child, &path)?,
            "strict-format" => cfg.mydumper.strict_format = bool_value(child, &path)?,
            "case-sensitive" => cfg.mydumper.case_sensitive = bool_value(child, &path)?,
            "character-set" => cfg.mydumper.character_set = string_value(child)?,
            "data-character-set" => cfg.mydumper.data_character_set = string_value(child)?,
            "data-invalid-char-replace" => {
                cfg.mydumper.data_invalid_char_replace = string_value(child)?
            }
            "batch-import-ratio" => cfg.mydumper.batch_import_ratio = float_value(child, &path)?,
            "filter" => {
                cfg.mydumper.filter = string_array(child, &path)?;
                mark_subtree_used(child, &path, used);
            }
            "csv" => apply_csv(&mut cfg.mydumper.csv, child, &path, used)?,
            "files" => apply_files(&mut cfg.mydumper.file_routers, child, &path, used)?,
            "ignore-data-columns" => {
                apply_ignore_columns(&mut cfg.mydumper.ignore_columns, child, &path, used)?
            }
            "read-block-size" => cfg.mydumper.read_block_size = ByteSize::from_toml_value(child)?,
            "batch-size" => cfg.mydumper.batch_size = ByteSize::from_toml_value(child)?,
            "max-region-size" => cfg.mydumper.max_region_size = ByteSize::from_toml_value(child)?,
            "default-file-rules" => cfg.mydumper.default_file_rules = bool_value(child, &path)?,
            _ => {
                used.remove(&path);
            }
        }
    }
    Ok(())
}

/// 应用 mydumper CSV 方言：分隔符、引号、空值表示与转义。
fn apply_csv(
    csv: &mut crate::CSVConfig,
    value: &toml::Value,
    prefix: &str,
    used: &mut BTreeSet<String>,
) -> Result<(), ConfigError> {
    let Some(table) = value.as_table() else {
        return Ok(());
    };
    for (key, child) in table {
        let path = format!("{prefix}.{key}");
        used.insert(path.clone());
        match key.as_str() {
            "separator" => csv.fields_terminated_by = string_value(child)?,
            "delimiter" => csv.fields_enclosed_by = string_value(child)?,
            "terminator" => csv.lines_terminated_by = string_value(child)?,
            "null" => {
                csv.field_null_defined_by = string_or_string_slice(child)?;
                mark_subtree_used(child, &path, used);
            }
            "header" => csv.header = bool_value(child, &path)?,
            "header-schema-match" => csv.header_schema_match = bool_value(child, &path)?,
            "trim-last-separator" => csv.trim_last_empty_field = bool_value(child, &path)?,
            "not-null" => csv.not_null = bool_value(child, &path)?,
            "backslash-escape" => csv.backslash_escape = bool_value(child, &path)?,
            "escaped-by" => csv.fields_escaped_by = string_value(child)?,
            _ => {
                used.remove(&path);
            }
        }
    }
    Ok(())
}

/// 应用文件路由规则数组（path/pattern/schema/table 等）。
fn apply_files(
    files: &mut Vec<FileRouteRule>,
    value: &toml::Value,
    prefix: &str,
    used: &mut BTreeSet<String>,
) -> Result<(), ConfigError> {
    let Some(items) = value.as_array() else {
        return Err(ConfigError::Parse(format!("{prefix} must be an array")));
    };
    files.clear();
    for (index, item) in items.iter().enumerate() {
        let path = format!("{prefix}.{index}");
        mark_subtree_used(item, &path, used);
        let mut rule = FileRouteRule::default();
        if let Some(table) = item.as_table() {
            for (key, child) in table {
                match key.as_str() {
                    "path" => rule.path = string_value(child)?,
                    "pattern" => rule.pattern = string_value(child)?,
                    "schema" => rule.schema = string_value(child)?,
                    "table" => rule.table = string_value(child)?,
                    "type" => rule.file_type = string_value(child)?,
                    "key" => rule.key = string_value(child)?,
                    "compression" => rule.compression = string_value(child)?,
                    _ => {}
                }
            }
        }
        files.push(rule);
    }
    Ok(())
}

/// 应用忽略列规则：按库表或表过滤器跳过指定列。
fn apply_ignore_columns(
    items: &mut Vec<IgnoreColumns>,
    value: &toml::Value,
    prefix: &str,
    used: &mut BTreeSet<String>,
) -> Result<(), ConfigError> {
    let Some(arr) = value.as_array() else {
        return Ok(());
    };
    items.clear();
    for (index, item) in arr.iter().enumerate() {
        let path = format!("{prefix}.{index}");
        mark_subtree_used(item, &path, used);
        let mut ig = IgnoreColumns::default();
        if let Some(table) = item.as_table() {
            for (key, child) in table {
                match key.as_str() {
                    "db" => ig.db = string_value(child)?,
                    "table" => ig.table = string_value(child)?,
                    "columns" => ig.columns = string_array(child, &path)?,
                    "table-filter" => ig.table_filter = string_array(child, &path)?,
                    _ => {}
                }
            }
        }
        items.push(ig);
    }
    Ok(())
}

/// 应用 `[[routes]]` 表路由规则的 Go 公开字段。
fn apply_routes(
    routes: &mut crate::Routes,
    value: &toml::Value,
    prefix: &str,
    used: &mut BTreeSet<String>,
) -> Result<(), ConfigError> {
    let Some(items) = value.as_array() else {
        return Err(ConfigError::Parse(format!("{prefix} must be an array")));
    };
    routes.clear();
    for (index, item) in items.iter().enumerate() {
        let path = format!("{prefix}.{index}");
        let Some(table) = item.as_table() else {
            return Err(ConfigError::Parse(format!("{path} must be a table")));
        };
        let mut route = crate::TableRouteRule::default();
        used.insert(path.clone());
        for (key, child) in table {
            let field_path = format!("{path}.{key}");
            used.insert(field_path.clone());
            match key.as_str() {
                "schema-pattern" => route.schema_pattern = string_value(child)?,
                "table-pattern" => route.table_pattern = string_value(child)?,
                "target-schema" => route.target_schema = string_value(child)?,
                "target-table" => route.target_table = string_value(child)?,
                _ => {
                    used.remove(&field_path);
                }
            }
        }
        routes.push(route);
    }
    Ok(())
}

/// 应用 `[tikv-importer]`：后端、本地排序目录、Region 切分与冲突策略等。
/// Region 是 TiKV 的数据分片单位。
fn apply_importer(
    cfg: &mut Config,
    value: &toml::Value,
    prefix: &str,
    used: &mut BTreeSet<String>,
) -> Result<(), ConfigError> {
    let Some(table) = value.as_table() else {
        return Ok(());
    };
    for (key, child) in table {
        let path = format!("{prefix}.{key}");
        used.insert(path.clone());
        match key.as_str() {
            "addr" => cfg.tikv_importer.addr = string_value(child)?,
            "backend" => cfg.tikv_importer.backend = string_value(child)?,
            "max-kv-pairs" => cfg.tikv_importer.max_kv_pairs = int_value(child, &path)?,
            "send-kv-pairs" => cfg.tikv_importer.send_kv_pairs = int_value(child, &path)?,
            "send-kv-size" => cfg.tikv_importer.send_kv_size = ByteSize::from_toml_value(child)?,
            "region-split-size" => {
                cfg.tikv_importer.region_split_size = ByteSize::from_toml_value(child)?
            }
            "region-split-keys" => cfg.tikv_importer.region_split_keys = int_value(child, &path)?,
            "sorted-kv-dir" => cfg.tikv_importer.sorted_kv_dir = string_value(child)?,
            "parallel-import" => cfg.tikv_importer.parallel_import = bool_value(child, &path)?,
            "incremental-import" => {
                cfg.tikv_importer.incremental_import = bool_value(child, &path)?
            }
            "add-index-by-sql" => cfg.tikv_importer.add_index_by_sql = bool_value(child, &path)?,
            "pause-pd-scheduler-scope" => {
                cfg.tikv_importer.pause_pd_scheduler_scope = string_value(child)?
            }
            "disk-quota" => cfg.tikv_importer.disk_quota = ByteSize::from_toml_value(child)?,
            "block-size" => cfg.tikv_importer.block_size = ByteSize::from_toml_value(child)?,
            "range-concurrency" => cfg.tikv_importer.range_concurrency = int_value(child, &path)?,
            "keyspace-name" => cfg.tikv_importer.keyspace_name = string_value(child)?,
            "engine-mem-cache-size" => {
                cfg.tikv_importer.engine_mem_cache_size = ByteSize::from_toml_value(child)?
            }
            "local-writer-mem-cache-size" => {
                cfg.tikv_importer.local_writer_mem_cache_size = ByteSize::from_toml_value(child)?
            }
            "store-write-bwlimit" => {
                cfg.tikv_importer.store_write_bw_limit = ByteSize::from_toml_value(child)?
            }
            "logical-import-batch-size" => {
                cfg.tikv_importer.logical_import_batch_size = ByteSize::from_toml_value(child)?
            }
            "logical-import-batch-rows" => {
                cfg.tikv_importer.logical_import_batch_rows = int_value(child, &path)?
            }
            "logical-import-prep-stmt" => {
                cfg.tikv_importer.logical_import_prep_stmt = bool_value(child, &path)?
            }
            "on-duplicate" => cfg
                .tikv_importer
                .on_duplicate
                .from_string_value(&string_value(child)?)?,
            "duplicate-resolution" => cfg
                .tikv_importer
                .duplicate_resolution
                .from_string_value(&string_value(child)?)?,
            "compress-kv-pairs" => cfg
                .tikv_importer
                .compress_kv_pairs
                .from_string_value(&string_value(child)?)?,
            "region-split-batch-size" => {
                cfg.tikv_importer.region_split_batch_size = int_value(child, &path)?
            }
            "region-split-concurrency" => {
                cfg.tikv_importer.region_split_concurrency = int_value(child, &path)?
            }
            "region-check-backoff-limit" => {
                cfg.tikv_importer.region_check_backoff_limit = int_value(child, &path)?
            }
            _ => {
                used.remove(&path);
            }
        }
    }
    Ok(())
}

/// 应用 `[post-restore]`：checksum/analyze 级别与压缩等后处理开关。
fn apply_post_restore(
    cfg: &mut Config,
    value: &toml::Value,
    prefix: &str,
    used: &mut BTreeSet<String>,
) -> Result<(), ConfigError> {
    let Some(table) = value.as_table() else {
        return Ok(());
    };
    for (key, child) in table {
        let path = format!("{prefix}.{key}");
        used.insert(path.clone());
        match key.as_str() {
            "checksum" => cfg.post_restore.checksum.from_toml_value(child)?,
            "analyze" => cfg.post_restore.analyze.from_toml_value(child)?,
            "level-1-compact" => cfg.post_restore.level1_compact = bool_value(child, &path)?,
            "post-process-at-last" => {
                cfg.post_restore.post_process_at_last = bool_value(child, &path)?
            }
            "compact" => cfg.post_restore.compact = bool_value(child, &path)?,
            "checksum-via-sql" => cfg.post_restore.checksum_via_sql = bool_value(child, &path)?,
            _ => {
                used.remove(&path);
            }
        }
    }
    Ok(())
}

/// 应用 `[cron]` 周期任务间隔：切换模式、进度日志、磁盘配额检查。
fn apply_cron(
    cfg: &mut Config,
    value: &toml::Value,
    prefix: &str,
    used: &mut BTreeSet<String>,
) -> Result<(), ConfigError> {
    let Some(table) = value.as_table() else {
        return Ok(());
    };
    for (key, child) in table {
        let path = format!("{prefix}.{key}");
        used.insert(path.clone());
        match key.as_str() {
            "switch-mode" => {
                cfg.cron.switch_mode = Duration::from_toml_value(child)?;
            }
            "log-progress" => {
                cfg.cron.log_progress = Duration::from_toml_value(child)?;
            }
            "check-disk-quota" => {
                cfg.cron.check_disk_quota = Duration::from_toml_value(child)?;
            }
            _ => {
                used.remove(&path);
            }
        }
    }
    Ok(())
}

/// 应用 TLS 证书路径与日志脱敏开关。
fn apply_security(
    security: &mut crate::Security,
    value: &toml::Value,
    prefix: &str,
    used: &mut BTreeSet<String>,
) -> Result<(), ConfigError> {
    let Some(table) = value.as_table() else {
        return Ok(());
    };
    for (key, child) in table {
        let path = format!("{prefix}.{key}");
        used.insert(path.clone());
        match key.as_str() {
            "ca-path" => security.ca_path = string_value(child)?,
            "cert-path" => security.cert_path = string_value(child)?,
            "key-path" => security.key_path = string_value(child)?,
            "redact-info-log" => security.redact_info_log = bool_value(child, &path)?,
            _ => {
                used.remove(&path);
            }
        }
    }
    Ok(())
}

/// 应用 `[conflict]`：冲突策略、预检与阈值。
fn apply_conflict(
    conflict: &mut Conflict,
    value: &toml::Value,
    prefix: &str,
    used: &mut BTreeSet<String>,
) -> Result<(), ConfigError> {
    let Some(table) = value.as_table() else {
        return Ok(());
    };
    for (key, child) in table {
        let path = format!("{prefix}.{key}");
        used.insert(path.clone());
        match key.as_str() {
            "strategy" => conflict.strategy.from_string_value(&string_value(child)?)?,
            "precheck-conflict-before-import" => {
                conflict.precheck_conflict_before_import = bool_value(child, &path)?
            }
            "threshold" => conflict.threshold = i64_value(child, &path)?,
            "max-record-rows" => conflict.max_record_rows = i64_value(child, &path)?,
            _ => {
                used.remove(&path);
            }
        }
    }
    Ok(())
}

/// 接受单个字符串或字符串数组，统一为 `Vec<String>`（CSV null 等字段）。
fn string_or_string_slice(value: &toml::Value) -> Result<Vec<String>, ConfigError> {
    match value {
        toml::Value::String(text) => Ok(vec![text.clone()]),
        toml::Value::Array(items) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    toml::Value::String(text) => out.push(text.clone()),
                    _ => {
                        return Err(ConfigError::Parse("invalid string slice".into()));
                    }
                }
            }
            Ok(out)
        }
        _ => Err(ConfigError::Parse("invalid string slice".into())),
    }
}

/// 将 TOML 标量转为字符串（含整数/布尔/浮点的字符串化）。
fn string_value(value: &toml::Value) -> Result<String, ConfigError> {
    match value {
        toml::Value::String(text) => Ok(text.clone()),
        toml::Value::Integer(v) => Ok(v.to_string()),
        toml::Value::Boolean(v) => Ok(v.to_string()),
        toml::Value::Float(v) => Ok(v.to_string()),
        other => Err(ConfigError::Parse(format!(
            "expected string, got {}",
            other.type_str()
        ))),
    }
}

/// 解析字符串数组；非数组时报带路径的错误。
fn string_array(value: &toml::Value, path: &str) -> Result<Vec<String>, ConfigError> {
    let Some(items) = value.as_array() else {
        return Err(ConfigError::Parse(format!("{path} must be an array")));
    };
    items.iter().map(string_value).collect()
}

/// 解析 TOML 字符串表；对应 Go `map[string]string` 的严格值类型。
fn string_map(value: &toml::Value, path: &str) -> Result<HashMap<String, String>, ConfigError> {
    let Some(table) = value.as_table() else {
        return Err(ConfigError::Parse(format!("{path} must be a table")));
    };
    table
        .iter()
        .map(|(key, value)| match value {
            toml::Value::String(text) => Ok((key.clone(), text.clone())),
            _ => Err(ConfigError::Parse(format!("{path}.{key} must be a string"))),
        })
        .collect()
}

/// 解析布尔值。
fn bool_value(value: &toml::Value, path: &str) -> Result<bool, ConfigError> {
    value
        .as_bool()
        .ok_or_else(|| ConfigError::Parse(format!("{path} must be a boolean")))
}

/// 解析可落入 i32 的整数。
fn int_value(value: &toml::Value, path: &str) -> Result<i32, ConfigError> {
    value
        .as_integer()
        .and_then(|v| i32::try_from(v).ok())
        .ok_or_else(|| ConfigError::Parse(format!("{path} must be an integer")))
}

/// 解析 i64 整数。
fn i64_value(value: &toml::Value, path: &str) -> Result<i64, ConfigError> {
    value
        .as_integer()
        .ok_or_else(|| ConfigError::Parse(format!("{path} must be an integer")))
}

/// 解析可落入 u64 的非负整数。
fn u64_value(value: &toml::Value, path: &str) -> Result<u64, ConfigError> {
    value
        .as_integer()
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| ConfigError::Parse(format!("{path} must be a non-negative integer")))
}

/// 解析浮点；整数会提升为 f64。
fn float_value(value: &toml::Value, path: &str) -> Result<f64, ConfigError> {
    match value {
        toml::Value::Float(v) => Ok(*v),
        toml::Value::Integer(v) => Ok(*v as f64),
        _ => Err(ConfigError::Parse(format!("{path} must be a float"))),
    }
}

impl PostOpLevel {
    /// 从 TOML 值解析后处理级别：off/optional/required（及 true/false 别名）。
    pub fn from_toml_value(&mut self, value: &toml::Value) -> Result<(), ConfigError> {
        let text = match value {
            toml::Value::String(text) => text.clone(),
            toml::Value::Boolean(true) => "true".into(),
            toml::Value::Boolean(false) => "false".into(),
            toml::Value::Integer(v) => v.to_string(),
            other => other.to_string(),
        };
        *self = match text.to_ascii_lowercase().as_str() {
            "off" | "false" => Self::Off,
            "optional" => Self::Optional,
            "required" | "true" => Self::Required,
            _ => {
                return Err(ConfigError::Parse(format!(
                    "toml: line 3 (last key \"post-restore\"): invalid op level '{text}', please choose valid option between ['off', 'optional', 'required']"
                )));
            }
        };
        Ok(())
    }
}

impl CheckpointKeepStrategy {
    /// 从布尔或字符串解析检查点保留策略（true→Rename，false→Remove）。
    pub fn from_toml_value(&mut self, value: &toml::Value) -> Result<(), String> {
        match value {
            toml::Value::Boolean(true) => *self = Self::Rename,
            toml::Value::Boolean(false) => *self = Self::Remove,
            toml::Value::String(text) => self
                .from_string_value(text)
                .map_err(|error| error.to_string())?,
            other => {
                return Err(format!("invalid checkpoint keep strategy '{other}'"));
            }
        }
        Ok(())
    }

    /// 序列化为配置文本：remove/rename/origin。
    pub fn marshal_text(self) -> &'static str {
        match self {
            Self::Remove => "remove",
            Self::Rename => "rename",
            Self::Origin => "origin",
        }
    }
}

impl Duration {
    /// 从 TOML 字符串解析 Go 风格 Duration。
    pub fn from_toml_value(value: &toml::Value) -> Result<Self, ConfigError> {
        let text = string_value(value)?;
        let mut duration = Self::default();
        duration.unmarshal_text(text.as_bytes())?;
        Ok(duration)
    }
}

/// 将 post-restore 段编码为 TOML 文本片段。
pub fn encode_post_restore(post: &crate::PostRestore) -> Result<String, ConfigError> {
    Ok(format!(
        "checksum = \"{}\"\nanalyze = \"{}\"\nlevel-1-compact = {}\npost-process-at-last = {}\ncompact = {}\nchecksum-via-sql = {}\n",
        post.checksum.as_str(),
        post.analyze.as_str(),
        post.level1_compact,
        post.post_process_at_last,
        post.compact,
        post.checksum_via_sql,
    ))
}

/// 将 cron 段编码为 TOML 文本片段。
pub fn encode_cron(cron: &crate::Cron) -> Result<String, ConfigError> {
    Ok(format!(
        "switch-mode = \"{}\"\nlog-progress = \"{}\"\ncheck-disk-quota = \"{}\"\n",
        cron.switch_mode.go_string(),
        cron.log_progress.go_string(),
        cron.check_disk_quota.go_string(),
    ))
}

#[allow(dead_code)]
/// 重复键解决算法编码占位（迁移基线尚未实现）。
pub fn encode_duplicate(_: DuplicateResolutionAlgorithm) {}

#[allow(dead_code)]
/// 压缩类型编码占位（迁移基线尚未实现）。
pub fn encode_compression(_: CompressionType) {}
