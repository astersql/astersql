// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// See the License for the specific language governing permissions and
// limitations under the License.
// 统计相关 AST：ANALYZE / DROP|LOAD|LOCK|UNLOCK|REFRESH STATS 与作用域去重。
//
// 对照 stats.go：分析选项、直方图操作、列选择，以及表/库/全局 StatsObject
// 在 REFRESH/FLUSH 中的 restore 与 dedup（库级覆盖表级、全局吸收全部）。

use crate::model::{AllColumns, CIStr, ColumnChoice, ColumnList, NewCIStr, PredicateColumns};
use std::collections::HashSet;

/// 反引号引用标识符。
fn quote_name(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}
/// 单引号字符串字面量。
fn quote_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// ANALYZE … WITH 选项种类。
pub type AnalyzeOptionType = i32;
/// 直方图桶数。
pub const AnalyzeOptNumBuckets: AnalyzeOptionType = 0;
/// TopN 项数。
pub const AnalyzeOptNumTopN: AnalyzeOptionType = 1;
/// CMSketch 深度。
pub const AnalyzeOptCMSketchDepth: AnalyzeOptionType = 2;
/// CMSketch 宽度。
pub const AnalyzeOptCMSketchWidth: AnalyzeOptionType = 3;
/// 采样行数。
pub const AnalyzeOptNumSamples: AnalyzeOptionType = 4;
/// 采样率。
pub const AnalyzeOptSampleRate: AnalyzeOptionType = 5;
/// NDV（Distinct 基数）估算比率。
pub const AnalyzeOptNDVRate: AnalyzeOptionType = 6;
/// 选项种类对应的 SQL 关键字文本。
pub fn analyze_option_string(option: AnalyzeOptionType) -> &'static str {
    match option {
        AnalyzeOptNumBuckets => "BUCKETS",
        AnalyzeOptNumTopN => "TOPN",
        AnalyzeOptCMSketchDepth => "CMSKETCH DEPTH",
        AnalyzeOptCMSketchWidth => "CMSKETCH WIDTH",
        AnalyzeOptNumSamples => "SAMPLES",
        AnalyzeOptSampleRate => "SAMPLERATE",
        AnalyzeOptNDVRate => "NDVRATE",
        _ => "",
    }
}

/// ANALYZE 上的直方图 UPDATE/DROP 操作。
pub type HistogramOperationType = i32;
/// 无直方图操作。
pub const HistogramOperationNop: HistogramOperationType = 0;
/// UPDATE HISTOGRAM。
pub const HistogramOperationUpdate: HistogramOperationType = 1;
/// DROP HISTOGRAM。
pub const HistogramOperationDrop: HistogramOperationType = 2;
/// 直方图操作对应的 SQL 片段。
pub fn histogram_operation_string(operation: HistogramOperationType) -> &'static str {
    match operation {
        HistogramOperationUpdate => "UPDATE HISTOGRAM",
        HistogramOperationDrop => "DROP HISTOGRAM",
        _ => "",
    }
}

#[derive(Clone, Debug, PartialEq)]
/// 单条 ANALYZE WITH 选项。None 表示 DEFAULT，清除持久化值。
pub struct AnalyzeOpt {
    pub option_type: AnalyzeOptionType,
    pub value: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
/// ANALYZE TABLE 语句：分区/索引/列选择与增量等标志。
pub struct AnalyzeTableStmt {
    pub table_names: Vec<String>,
    pub partition_names: Vec<CIStr>,
    pub index_names: Vec<CIStr>,
    pub analyze_opts: Vec<AnalyzeOpt>,
    pub index_flag: bool,
    pub incremental: bool,
    pub no_write_to_bin_log: bool,
    pub histogram_operation: HistogramOperationType,
    pub column_names: Vec<CIStr>,
    pub column_choice: ColumnChoice,
}
impl AnalyzeTableStmt {
    /// 还原 ANALYZE [NO_WRITE_TO_BINLOG] [INCREMENTAL] TABLE … 文本。
    pub fn restore(&self) -> Result<String, String> {
        let mut out = String::from("ANALYZE ");
        // 可选 NO_WRITE_TO_BINLOG / INCREMENTAL，再拼表名与分区。
        if self.no_write_to_bin_log {
            out.push_str("NO_WRITE_TO_BINLOG ");
        }
        out.push_str(if self.incremental {
            "INCREMENTAL TABLE "
        } else {
            "TABLE "
        });
        out.push_str(
            &self
                .table_names
                .iter()
                .map(|v| quote_name(v))
                .collect::<Vec<_>>()
                .join(","),
        );
        if !self.partition_names.is_empty() {
            out.push_str(" PARTITION ");
            out.push_str(
                &self
                    .partition_names
                    .iter()
                    .map(|v| quote_name(&v.O))
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        if self.histogram_operation != HistogramOperationNop {
            out.push(' ');
            out.push_str(histogram_operation_string(self.histogram_operation));
            out.push(' ');
            if !self.column_names.is_empty() {
                out.push_str("ON ");
                out.push_str(
                    &self
                        .column_names
                        .iter()
                        .map(|v| quote_name(&v.O))
                        .collect::<Vec<_>>()
                        .join(","),
                );
            }
        }
        // 列选择：ALL / PREDICATE / 显式 COLUMNS 列表。
        match self.column_choice {
            AllColumns => out.push_str(" ALL COLUMNS"),
            PredicateColumns => out.push_str(" PREDICATE COLUMNS"),
            ColumnList => {
                out.push_str(" COLUMNS ");
                out.push_str(
                    &self
                        .column_names
                        .iter()
                        .map(|v| quote_name(&v.O))
                        .collect::<Vec<_>>()
                        .join(","),
                );
            }
            _ => {}
        }
        if self.index_flag {
            out.push_str(" INDEX");
        }
        for (index, item) in self.index_names.iter().enumerate() {
            out.push_str(if index == 0 { " " } else { "," });
            out.push_str(&quote_name(&item.O));
        }
        if !self.analyze_opts.is_empty() {
            out.push_str(" WITH");
            for (index, option) in self.analyze_opts.iter().enumerate() {
                if index != 0 {
                    out.push(',');
                }
                out.push_str(&format!(
                    " {} {}",
                    option.value.as_deref().unwrap_or("DEFAULT"),
                    analyze_option_string(option.option_type)
                ));
            }
        }
        Ok(out)
    }
}

impl crate::AnalyzeTableStmt {
    /// Restore the parser's canonical ANALYZE AST, including DEFAULT resets.
    pub fn restore(&self) -> Result<String, String> {
        use crate::{AnalyzeOptionType, ColumnChoice as AstColumnChoice, HistogramOperationType};
        let mut out = String::from("ANALYZE ");
        if self.NoWriteToBinLog {
            out.push_str("NO_WRITE_TO_BINLOG ");
        }
        out.push_str(if self.Incremental {
            "INCREMENTAL TABLE "
        } else {
            "TABLE "
        });
        out.push_str(
            &self
                .TableNames
                .iter()
                .map(|table| {
                    if table.Schema.O.is_empty() {
                        quote_name(&table.Name.O)
                    } else {
                        format!(
                            "{}.{}",
                            quote_name(&table.Schema.O),
                            quote_name(&table.Name.O)
                        )
                    }
                })
                .collect::<Vec<_>>()
                .join(","),
        );
        if !self.PartitionNames.is_empty() {
            out.push_str(" PARTITION ");
            out.push_str(
                &self
                    .PartitionNames
                    .iter()
                    .map(|name| quote_name(&name.O))
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        match self.HistogramOperation {
            HistogramOperationType::Nop => {}
            HistogramOperationType::Update => out.push_str(" UPDATE HISTOGRAM "),
            HistogramOperationType::Drop => out.push_str(" DROP HISTOGRAM "),
        }
        if self.HistogramOperation != HistogramOperationType::Nop && !self.ColumnNames.is_empty() {
            out.push_str("ON ");
            out.push_str(
                &self
                    .ColumnNames
                    .iter()
                    .map(|name| quote_name(&name.O))
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        match self.ColumnChoice {
            AstColumnChoice::All => out.push_str(" ALL COLUMNS"),
            AstColumnChoice::Predicate => out.push_str(" PREDICATE COLUMNS"),
            AstColumnChoice::List => {
                out.push_str(" COLUMNS ");
                out.push_str(
                    &self
                        .ColumnNames
                        .iter()
                        .map(|name| quote_name(&name.O))
                        .collect::<Vec<_>>()
                        .join(","),
                );
            }
            AstColumnChoice::Default => {}
        }
        if self.IndexFlag {
            out.push_str(" INDEX");
        }
        for (index, name) in self.IndexNames.iter().enumerate() {
            out.push_str(if index == 0 { " " } else { "," });
            out.push_str(&quote_name(&name.O));
        }
        if !self.AnalyzeOpts.is_empty() {
            out.push_str(" WITH");
            for (index, option) in self.AnalyzeOpts.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push(' ');
                if let Some(value) = &option.Value {
                    out.push_str(&crate::sql_restore::restore_expr(value)?);
                } else {
                    out.push_str("DEFAULT");
                }
                out.push(' ');
                out.push_str(match option.Type {
                    AnalyzeOptionType::NumBuckets => "BUCKETS",
                    AnalyzeOptionType::NumTopN => "TOPN",
                    AnalyzeOptionType::CMSketchDepth => "CMSKETCH DEPTH",
                    AnalyzeOptionType::CMSketchWidth => "CMSKETCH WIDTH",
                    AnalyzeOptionType::NumSamples => "SAMPLES",
                    AnalyzeOptionType::SampleRate => "SAMPLERATE",
                    AnalyzeOptionType::NDVRate => "NDVRATE",
                });
            }
        }
        Ok(out)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
/// DROP STATS：表列表，可选 GLOBAL 或 PARTITION。
pub struct DropStatsStmt {
    pub tables: Vec<String>,
    pub partition_names: Vec<CIStr>,
    pub is_global_stats: bool,
}
impl DropStatsStmt {
    /// 还原 DROP STATS … [GLOBAL|PARTITION …]。
    pub fn restore(&self) -> String {
        let mut out = format!(
            "DROP STATS {}",
            self.tables
                .iter()
                .map(|v| quote_name(v))
                .collect::<Vec<_>>()
                .join(", ")
        );
        if self.is_global_stats {
            out.push_str(" GLOBAL");
            return out;
        }
        if !self.partition_names.is_empty() {
            out.push_str(" PARTITION ");
            out.push_str(
                &self
                    .partition_names
                    .iter()
                    .map(|v| quote_name(&v.O))
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        out
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// LOAD STATS：从路径加载统计文件。
pub struct LoadStatsStmt {
    pub path: String,
}
impl LoadStatsStmt {
    /// 还原 LOAD STATS 'path'。
    pub fn restore(&self) -> String {
        format!("LOAD STATS {}", quote_string(&self.path))
    }
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// LOCK STATS：锁定表统计，防止自动更新。
pub struct LockStatsStmt {
    pub tables: Vec<String>,
}
impl LockStatsStmt {
    /// 还原 LOCK STATS 表列表。
    pub fn restore(&self) -> String {
        format!(
            "LOCK STATS {}",
            self.tables
                .iter()
                .map(|v| quote_name(v))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}
#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// UNLOCK STATS：解锁表统计。
pub struct UnlockStatsStmt {
    pub tables: Vec<String>,
}
impl UnlockStatsStmt {
    /// 还原 UNLOCK STATS 表列表。
    pub fn restore(&self) -> String {
        format!(
            "UNLOCK STATS {}",
            self.tables
                .iter()
                .map(|v| quote_name(v))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

/// REFRESH STATS 模式：LITE 或 FULL。
pub type RefreshStatsMode = i32;
/// 轻量刷新。
pub const RefreshStatsModeLite: RefreshStatsMode = 0;
/// 全量刷新。
pub const RefreshStatsModeFull: RefreshStatsMode = 1;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// REFRESH STATS：对象列表、可选模式与 CLUSTER。
pub struct RefreshStatsStmt {
    pub refresh_objects: Vec<StatsObject>,
    pub refresh_mode: Option<RefreshStatsMode>,
    pub is_cluster_wide: bool,
}
impl RefreshStatsStmt {
    /// 仅带对象列表构造，模式默认 None、非 CLUSTER。
    pub fn new(refresh_objects: Vec<StatsObject>) -> Self {
        Self {
            refresh_objects,
            refresh_mode: None,
            is_cluster_wide: false,
        }
    }
    /// 还原 REFRESH STATS objects [LITE|FULL] [CLUSTER]。
    pub fn restore(&self) -> Result<String, String> {
        let objects = self
            .refresh_objects
            .iter()
            .map(StatsObject::restore)
            .collect::<Result<Vec<_>, _>>()?
            .join(", ");
        let mode = match self.refresh_mode {
            None => "",
            Some(RefreshStatsModeLite) => " LITE",
            Some(RefreshStatsModeFull) => " FULL",
            Some(v) => return Err(format!("invalid refresh stats mode: {v}")),
        };
        Ok(format!(
            "REFRESH STATS {objects}{mode}{}",
            if self.is_cluster_wide { " CLUSTER" } else { "" }
        ))
    }
    /// 就地按作用域规则去重 refresh_objects。
    pub fn dedup(&mut self) {
        self.refresh_objects = dedup_stats_objects(std::mem::take(&mut self.refresh_objects));
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// FLUSH 统计相关对象列表（含 dedup 入口）。
pub struct FlushStmt {
    pub flush_objects: Vec<StatsObject>,
}
impl FlushStmt {
    /// 就地去重 flush_objects。
    pub fn dedup_flush_objects(&mut self) {
        self.flush_objects = dedup_stats_objects(std::mem::take(&mut self.flush_objects));
    }
}

#[derive(Clone, Hash, Eq, PartialEq)]
/// 去重用的表键：库名与表名的小写形式。
struct TableKey {
    db_name: String,
    table_name: String,
}

/// 按作用域去重：全局吸收全部；库级吸收同库表；表级按 (db,table) 去重。
pub fn dedup_stats_objects(objects: Vec<StatsObject>) -> Vec<StatsObject> {
    if objects.is_empty() {
        return objects;
    }
    // 顺序扫描：遇全局立即返回；库级剔除已收集的同库表；表级跳过已被库覆盖者。
    let mut db_seen = HashSet::new();
    let mut table_seen = HashSet::new();
    let mut result = Vec::with_capacity(objects.len());
    for object in objects {
        match object.stats_object_scope {
            StatsObjectScopeGlobal => return vec![object],
            StatsObjectScopeDatabase => {
                let db = object.db_name.L.clone();
                if !db_seen.insert(db.clone()) {
                    continue;
                }
                result.retain(|existing: &StatsObject| {
                    let remove = existing.stats_object_scope == StatsObjectScopeTable
                        && !existing.db_name.L.is_empty()
                        && existing.db_name.L == db;
                    if remove {
                        table_seen.remove(&TableKey {
                            db_name: existing.db_name.L.clone(),
                            table_name: existing.table_name.L.clone(),
                        });
                    }
                    !remove
                });
                result.push(object);
            }
            StatsObjectScopeTable => {
                let db = object.db_name.L.clone();
                if !db.is_empty() && db_seen.contains(&db) {
                    continue;
                }
                if table_seen.insert(TableKey {
                    db_name: db,
                    table_name: object.table_name.L.clone(),
                }) {
                    result.push(object);
                }
            }
            _ => {}
        }
    }
    result
}

/// 统计对象作用域：表 / 库 / 全局。
pub type StatsObjectScopeType = i32;
/// 单表作用域。
pub const StatsObjectScopeTable: StatsObjectScopeType = 1;
/// 整库作用域（db.*）。
pub const StatsObjectScopeDatabase: StatsObjectScopeType = 2;
/// 全局作用域（*.*）。
pub const StatsObjectScopeGlobal: StatsObjectScopeType = 3;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 统计作用对象：作用域 + 库/表名（CIStr）。
pub struct StatsObject {
    pub stats_object_scope: StatsObjectScopeType,
    pub db_name: CIStr,
    pub table_name: CIStr,
}
impl StatsObject {
    /// 表作用域对象。
    pub fn table(db: impl Into<String>, table: impl Into<String>) -> Self {
        let db = db.into();
        let table = table.into();
        Self {
            stats_object_scope: StatsObjectScopeTable,
            db_name: NewCIStr(&db),
            table_name: NewCIStr(&table),
        }
    }
    /// 库作用域对象（restore 为 `db`.*）。
    pub fn database(db: impl Into<String>) -> Self {
        let db = db.into();
        Self {
            stats_object_scope: StatsObjectScopeDatabase,
            db_name: NewCIStr(&db),
            table_name: CIStr::default(),
        }
    }
    /// 全局作用域（restore 为 *.*）。
    pub fn global() -> Self {
        Self {
            stats_object_scope: StatsObjectScopeGlobal,
            ..Self::default()
        }
    }
    /// 按作用域还原为 `db`.`t` / `db`.* / *.*。
    pub fn restore(&self) -> Result<String, String> {
        match self.stats_object_scope {
            StatsObjectScopeTable => Ok(if self.db_name.O.is_empty() {
                quote_name(&self.table_name.O)
            } else {
                format!(
                    "{}.{}",
                    quote_name(&self.db_name.O),
                    quote_name(&self.table_name.O)
                )
            }),
            StatsObjectScopeDatabase => Ok(format!("{}.*", quote_name(&self.db_name.O))),
            StatsObjectScopeGlobal => Ok("*.*".into()),
            value => Err(format!("invalid stats object scope: {value}")),
        }
    }
}
