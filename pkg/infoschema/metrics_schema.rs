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

// METRICS_SCHEMA 虚拟库：把 Prometheus 指标暴露成可 SQL 查询的表。
//
// PromQL（Prometheus Query Language）模板经标签/分位数/时间范围替换后，
// 由执行层拉取时序数据；本模块负责表定义、列生成与虚拟表包装。
// InfoSchema：库表等元数据的内存视图；此处注册的是无物理存储的虚拟表。

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals)]

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::infoschema::{CiString, ColumnInfo, DBInfo, Table, TableInfo};
use crate::metric_table_def::MetricTableMap;
use crate::tables::{ColumnType, columnInfo, infoschemaTable};

/// PromQL 模板中的分位数占位符。
pub const promQLQuantileKey: &str = "$QUANTILE";
/// PromQL 模板中的标签过滤条件占位符。
pub const promQLLabelConditionKey: &str = "$LABEL_CONDITIONS";
/// PromQL 模板中的查询时间窗口（秒）占位符。
pub const promQRangeDurationKey: &str = "$RANGE_DURATION";
/// METRICS_SCHEMA 库的固定负数 ID，与 autoid 约定一致，避免与用户表冲突。
pub const MetricSchemaDBID: i64 = -2000;

/// 单张指标表的静态定义：PromQL 模板、可筛选标签、默认分位数与说明。
#[derive(Clone, Copy, Debug)]
pub struct MetricTableDef {
    /// Prometheus 查询语言模板，含 `$QUANTILE` / `$LABEL_CONDITIONS` / `$RANGE_DURATION`。
    pub PromQL: &'static str,
    /// 允许作为 WHERE 条件的标签名列表（如 instance、sql_type）。
    pub Labels: &'static [&'static str],
    /// 默认分位数；为 0 表示该表不暴露 quantile 列。
    pub Quantile: f64,
    /// 表注释，写入 TableInfo.Comment。
    pub Comment: &'static str,
}

impl MetricTableDef {
    /// Go 结构体字面量未指定字段时的零值，供大型静态定义表拼接。
    pub const EMPTY: Self = Self {
        PromQL: "",
        Labels: &[],
        Quantile: 0.0,
        Comment: "",
    };

    /// 按 Go 顺序生成列：time、各 label、可选 quantile、value。
    pub fn genColumnInfos(&self) -> Vec<columnInfo> {
        let mut columns = vec![columnInfo {
            name: "time",
            column_type: ColumnType::Datetime,
            size: 19,
            decimal: None,
            unsigned: false,
            not_null: false,
            primary_key: false,
            binary: false,
            default_value: Some("CURRENT_TIMESTAMP"),
            comment: "",
        }];
        // 每个可筛选 label 对应一列 VARCHAR(512)。
        columns.extend(
            self.Labels
                .iter()
                .map(|label| columnInfo::varchar(label, 512)),
        );
        // 分位数大于零时才建 quantile 列，与 Go FormatFloat 最短十进制语义对齐。
        if self.Quantile > 0.0 {
            // `columnInfo` is a static catalog descriptor, while Go formats this
            // default from the definition's float. Metric definitions are static
            // too, so retain the formatted value for the catalog lifetime.
            let default_value: &'static str = Box::leak(self.Quantile.to_string().into_boxed_str());
            columns.push(columnInfo {
                name: "quantile",
                column_type: ColumnType::Double,
                size: 22,
                decimal: None,
                unsigned: false,
                not_null: false,
                primary_key: false,
                binary: false,
                default_value: Some(default_value),
                comment: "",
            });
        }
        columns.push(columnInfo {
            name: "value",
            column_type: ColumnType::Double,
            size: 22,
            decimal: None,
            unsigned: false,
            not_null: false,
            primary_key: false,
            binary: false,
            default_value: None,
            comment: "",
        });
        columns
    }

    /// 展开 PromQL 模板：依次替换分位数、label 条件与秒级时间窗口。
    /// 仅做字符串替换，不发起网络 IO；负数时长与分位数不在此额外校验。
    pub fn GenPromQL(
        &self,
        range_duration: i64,
        labels: &HashMap<String, HashSet<String>>,
        quantile: f64,
    ) -> String {
        self.PromQL
            .replace(promQLQuantileKey, &quantile.to_string())
            .replace(promQLLabelConditionKey, &self.genLabelCondition(labels))
            .replace(promQRangeDurationKey, &format!("{range_duration}s"))
    }

    /// 按定义中 Labels 顺序生成条件；未提供值的 label 跳过。
    /// 单值用 `=`，多值用 `=~`，多个 label 以逗号分隔。
    fn genLabelCondition(&self, labels: &HashMap<String, HashSet<String>>) -> String {
        self.Labels
            .iter()
            .filter_map(|label| {
                let values = labels.get(*label)?;
                if values.is_empty() {
                    return None;
                }
                // 多值走正则匹配，单值走精确匹配。
                let operator = if values.len() == 1 { "=" } else { "=~" };
                Some(format!(
                    "{label}{operator}\"{}\"",
                    GenLabelConditionValues(values)
                ))
            })
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// 检查已转小写的表名是否存在于指标定义映射中。
pub fn IsMetricTable(lower_table_name: &str) -> bool {
    MetricTableMap.contains_key(lower_table_name)
}

/// 返回指标表定义；不存在时保留 Go 错误文案。
pub fn GetMetricTableDef(lower_table_name: &str) -> Result<&'static MetricTableDef, String> {
    MetricTableMap
        .get(lower_table_name)
        .ok_or_else(|| format!("can not find metric table: {lower_table_name}"))
}

/// 将标签值集合排序后用 `|` 连接，消除 HashSet 迭代顺序不确定性。
pub fn GenLabelConditionValues(values: &HashSet<String>) -> String {
    let mut values: Vec<&str> = values.iter().map(String::as_str).collect();
    values.sort_unstable();
    values.join("|")
}

/// 从 MetricTableMap 构造 METRICS_SCHEMA 的 DBInfo；表按名排序后分配稳定 ID。
pub fn metric_schema_db() -> DBInfo {
    let mut definitions: Vec<(&str, &MetricTableDef)> = MetricTableMap
        .iter()
        .map(|(name, definition)| (*name, definition))
        .collect();
    // 按表名排序再赋 ID，保证不同实例得到一致 schema。
    definitions.sort_by_key(|(name, _)| *name);
    let tables = definitions
        .into_iter()
        .enumerate()
        .map(|(index, (name, definition))| {
            Arc::new(TableInfo {
                id: MetricSchemaDBID + index as i64 + 1,
                db_id: MetricSchemaDBID,
                name: CiString::new(name),
                columns: definition
                    .genColumnInfos()
                    .iter()
                    .enumerate()
                    .map(|(index, column)| ColumnInfo {
                        id: index as i64 + 1,
                        name: CiString::new(column.name),
                        auto_increment: false,
                    })
                    .collect(),
                ..TableInfo::default()
            })
        })
        .collect();
    DBInfo {
        id: MetricSchemaDBID,
        name: CiString::new("METRICS_SCHEMA"),
        tables,
        table_name_2_id: Default::default(),
    }
}

/// 指标虚拟表包装：数据不来自物理存储，委托 infoschemaTable。
#[derive(Clone, Debug)]
pub struct metricSchemaTable {
    table: infoschemaTable,
}

impl metricSchemaTable {
    /// 返回底层 TableInfo 元数据。
    pub fn Meta(&self) -> &TableInfo {
        self.table.Meta()
    }
    /// 遍历虚拟表行；回调返回 false 时停止。
    pub fn IterRecords(&self, visit: impl FnMut(&[crate::cluster::Datum]) -> bool) {
        self.table.IterRecords(visit);
    }
}

/// 由 TableInfo 组装指标虚拟表；列在查询时再物化。
pub fn tableFromMetaForMetricsTable(meta: TableInfo) -> metricSchemaTable {
    metricSchemaTable {
        table: infoschemaTable::new(meta, Vec::new()),
    }
}

/// 导出 METRICS_SCHEMA 下全部表包装，供 InfoSchema 注册。
pub fn metricTables() -> Vec<Table> {
    metric_schema_db().tables.into_iter().map(Table).collect()
}
