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

// 集群巡检结果（inspection result）规则引擎。
//
// 对配置一致性、版本、节点负载、致命错误与阈值类指标执行检查，
// 产出带严重级别（severity）与偏离程度（degree）的巡检行，供
// `INFORMATION_SCHEMA` 巡检结果表展示。数据主要来自 cluster_* 与 metrics_schema。

#![allow(dead_code, non_camel_case_types, non_snake_case)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 巡检执行错误（SQL 失败、容量解析失败等）。
pub struct InspectionError(pub String);

impl fmt::Display for InspectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for InspectionError {}

/// 巡检操作的统一 Result 别名。
pub type InspectionResultValue<T = ()> = Result<T, InspectionError>;

#[derive(Clone, Debug, PartialEq)]
/// 简化版查询单元格：字符串 / 浮点 / 无符号整数 / NULL。
pub enum datum {
    String(String),
    Float(f64),
    Unsigned(u64),
    Null,
}

impl datum {
    /// 转为展示用字符串（NULL 为空串）。
    pub fn string(&self) -> String {
        match self {
            Self::String(value) => value.clone(),
            Self::Float(value) => value.to_string(),
            Self::Unsigned(value) => value.to_string(),
            Self::Null => String::new(),
        }
    }

    /// 尽量解析为 f64；失败或 NULL 为 0。
    pub fn float(&self) -> f64 {
        match self {
            Self::Float(value) => *value,
            Self::Unsigned(value) => *value as f64,
            Self::String(value) => value.parse().unwrap_or(0.0),
            Self::Null => 0.0,
        }
    }

    /// 尽量解析为 u64；失败或 NULL 为 0。
    pub fn unsigned(&self) -> u64 {
        match self {
            Self::Unsigned(value) => *value,
            Self::Float(value) => *value as u64,
            Self::String(value) => value.parse().unwrap_or(0),
            Self::Null => 0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 一行查询结果，按列下标取值。
pub struct queryRow(pub Vec<datum>);

impl queryRow {
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn string(&self, index: usize) -> String {
        self.0.get(index).map(datum::string).unwrap_or_default()
    }

    pub fn float(&self, index: usize) -> f64 {
        self.0.get(index).map(datum::float).unwrap_or_default()
    }

    pub fn unsigned(&self, index: usize) -> u64 {
        self.0.get(index).map(datum::unsigned).unwrap_or_default()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 巡检时间窗口（闭区间），用于拼 metrics SQL 的 time 条件。
pub struct queryTimeRange {
    pub from: String,
    pub to: String,
}

impl queryTimeRange {
    /// 生成 `where time >= ... and time <= ...` 片段。
    pub fn condition(&self) -> String {
        format!("where time >= '{}' and time <= '{}'", self.from, self.to)
    }
}

/// 巡检数据源：执行 SQL、查 metric 标签、警告与表缓存生命周期。
pub trait inspectionDataSource: Send + Sync {
    fn execute_sql(&self, sql: &str, parameters: &[datum]) -> InspectionResultValue<Vec<queryRow>>;
    fn metric_labels(&self, table: &str) -> InspectionResultValue<Option<Vec<String>>>;
    fn append_warning(&self, warning: String);
    fn begin_table_cache(&self) -> InspectionResultValue;
    fn merge_mock_table_cache(&self) -> InspectionResultValue;
    fn end_table_cache(&self) -> InspectionResultValue;
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 单条巡检发现：类型/实例/项/实际与期望值/严重级别/详情/偏离度。
pub struct inspectionResult {
    pub tp: String,
    pub instance: String,
    pub statusAddress: String,
    pub item: String,
    pub actual: String,
    pub expected: String,
    pub severity: String,
    pub detail: String,
    pub degree: f64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 巡检规则显示名包装。
pub struct inspectionName(pub String);

impl inspectionName {
    pub fn name(&self) -> String {
        self.0.clone()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 规则/检查项过滤器：空集合表示全部启用；附带时间范围。
pub struct inspectionFilter {
    pub set: BTreeSet<String>,
    pub timeRange: queryTimeRange,
}

impl inspectionFilter {
    /// 空 set 或包含 name 则启用。
    pub fn enable(&self, name: &str) -> bool {
        self.set.is_empty() || self.set.contains(name)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 配置一致性与合理性检查规则。
pub struct configInspection {
    pub inspectionName: inspectionName,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 同类型组件 git_hash 是否一致的检查。
pub struct versionInspection {
    pub inspectionName: inspectionName,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 节点 CPU/内存/磁盘负载检查。
pub struct nodeLoadInspection {
    pub inspectionName: inspectionName,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 致命错误计数与宕机/重启类检查。
pub struct criticalErrorInspection {
    pub inspectionName: inspectionName,
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 各类延迟/CPU/平衡性阈值检查。
pub struct thresholdCheckInspection {
    pub inspectionName: inspectionName,
}

/// 顶层巡检规则枚举，分发到具体 inspect 实现。
pub enum inspectionRule {
    Config(configInspection),
    Version(versionInspection),
    NodeLoad(nodeLoadInspection),
    CriticalError(criticalErrorInspection),
    ThresholdCheck(thresholdCheckInspection),
}

impl inspectionRule {
    /// 规则注册名（如 config、version）。
    fn rule_name(&self) -> String {
        match self {
            Self::Config(rule) => rule.inspectionName.name(),
            Self::Version(rule) => rule.inspectionName.name(),
            Self::NodeLoad(rule) => rule.inspectionName.name(),
            Self::CriticalError(rule) => rule.inspectionName.name(),
            Self::ThresholdCheck(rule) => rule.inspectionName.name(),
        }
    }

    /// 执行对应规则的 inspect。
    fn run(
        &self,
        source: &dyn inspectionDataSource,
        filter: &inspectionFilter,
    ) -> Vec<inspectionResult> {
        match self {
            Self::Config(rule) => rule.inspect(source, filter),
            Self::Version(rule) => rule.inspect(source, filter),
            Self::NodeLoad(rule) => rule.inspect(source, filter),
            Self::CriticalError(rule) => rule.inspect(source, filter),
            Self::ThresholdCheck(rule) => rule.inspect(source, filter),
        }
    }
}

/// 巡检结果检索器：跑完全部启用规则并物化为 datum 行。
pub struct inspectionResultRetriever {
    pub retrieved: bool,
    pub skipInspection: bool,
    pub rules: BTreeSet<String>,
    pub items: BTreeSet<String>,
    pub timeRange: queryTimeRange,
    pub instanceToStatusAddress: BTreeMap<String, String>,
    pub statusToInstanceAddress: BTreeMap<String, String>,
    pub source: Arc<dyn inspectionDataSource>,
}

impl inspectionResultRetriever {
    /// 打开表缓存 → 跑规则 → 补全实例/status 地址 → 关闭缓存。
    pub fn retrieve(&mut self) -> InspectionResultValue<Vec<Vec<datum>>> {
        // 已取过或跳过则直接返回空。
        if self.retrieved || self.skipInspection {
            return Ok(Vec::new());
        }
        self.retrieved = true;
        self.source.begin_table_cache()?;

        let result = (|| {
            self.source.merge_mock_table_cache()?;
            // 首次从 cluster_info 填充 instance ↔ status_address 映射。
            if self.instanceToStatusAddress.is_empty() {
                match self.source.execute_sql(
                    "select instance,status_address from information_schema.cluster_info;",
                    &[],
                ) {
                    Ok(rows) => {
                        for row in rows.into_iter().filter(|row| row.len() >= 2) {
                            let instance = row.string(0);
                            let status = row.string(1);
                            self.instanceToStatusAddress
                                .insert(instance.clone(), status.clone());
                            self.statusToInstanceAddress.insert(status, instance);
                        }
                    }
                    Err(error) => self
                        .source
                        .append_warning(format!("get cluster info failed: {error}")),
                }
            }

            let rule_filter = inspectionFilter {
                set: self.rules.clone(),
                timeRange: queryTimeRange::default(),
            };
            let item_filter = inspectionFilter {
                set: self.items.clone(),
                timeRange: self.timeRange.clone(),
            };
            // 固定注册的五类巡检规则。
            let rules = [
                inspectionRule::Config(configInspection {
                    inspectionName: inspectionName("config".into()),
                }),
                inspectionRule::Version(versionInspection {
                    inspectionName: inspectionName("version".into()),
                }),
                inspectionRule::NodeLoad(nodeLoadInspection {
                    inspectionName: inspectionName("node-load".into()),
                }),
                inspectionRule::CriticalError(criticalErrorInspection {
                    inspectionName: inspectionName("critical-error".into()),
                }),
                inspectionRule::ThresholdCheck(thresholdCheckInspection {
                    inspectionName: inspectionName("threshold-check".into()),
                }),
            ];
            let mut final_rows = Vec::new();
            for rule in rules {
                let name = rule.rule_name();
                if !rule_filter.enable(&name) {
                    continue;
                }
                let mut results = rule.run(self.source.as_ref(), &item_filter);
                // 按偏离度降序，再按 item/actual/tp/instance 稳定排序。
                results.sort_by(|left, right| {
                    right
                        .degree
                        .total_cmp(&left.degree)
                        .then_with(|| left.item.cmp(&right.item))
                        .then_with(|| left.actual.cmp(&right.actual))
                        .then_with(|| left.tp.cmp(&right.tp))
                        .then_with(|| left.instance.cmp(&right.instance))
                });
                for mut result in results {
                    // 用双向映射补全缺失的 instance 或 statusAddress。
                    if result.instance.is_empty() {
                        result.instance = self
                            .statusToInstanceAddress
                            .get(&result.statusAddress)
                            .cloned()
                            .unwrap_or_default();
                    }
                    if result.statusAddress.is_empty() {
                        result.statusAddress = self
                            .instanceToStatusAddress
                            .get(&result.instance)
                            .cloned()
                            .unwrap_or_default();
                    }
                    final_rows.push(vec![
                        datum::String(name.clone()),
                        datum::String(result.item),
                        datum::String(result.tp),
                        datum::String(result.instance),
                        datum::String(result.statusAddress),
                        datum::String(result.actual),
                        datum::String(result.expected),
                        datum::String(result.severity),
                        datum::String(result.detail),
                    ]);
                }
            }
            Ok(final_rows)
        })();

        // 无论成败都结束表缓存；优先返回业务错误。
        let cleanup = self.source.end_table_cache();
        match (result, cleanup) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(rows), Ok(())) => Ok(rows),
        }
    }
}

/// 执行 SQL；失败时记 warning 并返回空行，不中断整次巡检。
fn query_or_warn(
    source: &dyn inspectionDataSource,
    sql: &str,
    parameters: &[datum],
    prefix: &str,
) -> Vec<queryRow> {
    match source.execute_sql(sql, parameters) {
        Ok(rows) => rows,
        Err(error) => {
            source.append_warning(format!("{prefix}: {error}"));
            Vec::new()
        }
    }
}

impl configInspection {
    /// 配置差异 + 不合理配置两项检查。
    pub fn inspect(
        &self,
        source: &dyn inspectionDataSource,
        filter: &inspectionFilter,
    ) -> Vec<inspectionResult> {
        let mut results = self.inspectDiffConfig(source, filter);
        results.extend(self.inspectCheckConfig(source, filter));
        results
    }

    /// 同类型组件间配置值不一致（忽略端口/路径等本机相关键）。
    pub fn inspectDiffConfig(
        &self,
        source: &dyn inspectionDataSource,
        filter: &inspectionFilter,
    ) -> Vec<inspectionResult> {
        // 实例本地字段（端口、路径、advertise 等）不参与一致性比较。
        const IGNORED_KEYS: &[&str] = &[
            "port",
            "status.status-port",
            "host",
            "path",
            "advertise-address",
            "log.file.filename",
            "log.slow-query-file",
            "tmp-storage-path",
            "advertise-client-urls",
            "advertise-peer-urls",
            "client-urls",
            "data-dir",
            "log-file",
            "metric.job",
            "name",
            "peer-urls",
            "initial-cluster",
            "initial-cluster-state",
            "join",
            "server.addr",
            "server.advertise-addr",
            "server.advertise-status-addr",
            "server.status-addr",
            "raftstore.raftdb-path",
            "storage.data-dir",
            "storage.block-cache.capacity",
            "proxy.advertise-addr",
        ];
        let placeholders = std::iter::repeat_n("?", IGNORED_KEYS.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "select type, `key`, count(distinct value) as c from information_schema.cluster_config where `key` not in ({placeholders}) group by type, `key` having c > 1"
        );
        let parameters = IGNORED_KEYS
            .iter()
            .map(|value| datum::String((*value).into()))
            .collect::<Vec<_>>();
        let rows = query_or_warn(
            source,
            &sql,
            &parameters,
            "check configuration consistency failed",
        );
        let mut results = Vec::new();
        for row in rows {
            let tp = row.string(0);
            let item = row.string(1);
            if !filter.enable(&item) {
                continue;
            }
            // 按配置值分组实例，生成可读差异说明。
            let detail_sql = "select value, instance from information_schema.cluster_config where type=? and `key`=?;";
            let detail = match source.execute_sql(
                detail_sql,
                &[datum::String(tp.clone()), datum::String(item.clone())],
            ) {
                Ok(detail_rows) => {
                    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
                    for detail_row in detail_rows {
                        groups
                            .entry(detail_row.string(0))
                            .or_default()
                            .push(detail_row.string(1));
                    }
                    let mut descriptions = groups
                        .into_iter()
                        .map(|(value, mut instances)| {
                            instances.sort();
                            format!("{} config value is {value}", instances.join(","))
                        })
                        .collect::<Vec<_>>();
                    descriptions.sort();
                    descriptions.join("\n")
                }
                Err(error) => {
                    source
                        .append_warning(format!("check configuration consistency failed: {error}"));
                    format!(
                        "the cluster has different config value of {item}, execute the sql to see more detail: select * from information_schema.cluster_config where type='{tp}' and `key`='{item}'"
                    )
                }
            };
            results.push(inspectionResult {
                tp,
                item,
                actual: "inconsistent".into(),
                expected: "consistent".into(),
                severity: "warning".into(),
                detail,
                ..Default::default()
            });
        }
        results
    }

    /// 已知不合理配置项（慢日志阈值、sync-log、透明大页等）。
    pub fn inspectCheckConfig(
        &self,
        source: &dyn inspectionDataSource,
        filter: &inspectionFilter,
    ) -> Vec<inspectionResult> {
        let cases = [
            (
                "cluster_config",
                "log.slow-threshold",
                "> 0",
                "type = 'tidb' and `key` = 'log.slow-threshold' and value = '0'",
                "slow-threshold = 0 will record every query to slow log, it may affect performance",
            ),
            (
                "cluster_config",
                "raftstore.sync-log",
                "true",
                "type = 'tikv' and `key` = 'raftstore.sync-log' and value = 'false'",
                "sync-log should be true to avoid recover region when the machine breaks down",
            ),
            (
                "cluster_systeminfo",
                "transparent_hugepage_enabled",
                "always madvise [never]",
                "system_name = 'kernel' and name = 'transparent_hugepage_enabled' and value not like '%[never]%'",
                "Transparent HugePages can cause memory allocation delays during runtime, TiDB recommends that you disable Transparent HugePages on all TiDB servers",
            ),
        ];
        let mut results = Vec::new();
        for (table, item, expected, condition, detail) in cases {
            if !filter.enable(item) {
                continue;
            }
            let sql = format!(
                "select type,instance,value from information_schema.{table} where {condition}"
            );
            for row in query_or_warn(source, &sql, &[], "check configuration in reason failed") {
                results.push(inspectionResult {
                    tp: row.string(0),
                    instance: row.string(1),
                    item: item.into(),
                    actual: row.string(2),
                    expected: expected.into(),
                    severity: "warning".into(),
                    detail: detail.into(),
                    ..Default::default()
                });
            }
        }
        results.extend(self.checkTiKVBlockCacheSizeConfig(source, filter));
        results
    }

    /// 同机多 TiKV 的 block-cache 总和是否超过节点内存的 45%。
    pub fn checkTiKVBlockCacheSizeConfig(
        &self,
        source: &dyn inspectionDataSource,
        filter: &inspectionFilter,
    ) -> Vec<inspectionResult> {
        let item = "storage.block-cache.capacity";
        if !filter.enable(item) {
            return Vec::new();
        }
        let config_rows = query_or_warn(
            source,
            "select instance,value from information_schema.cluster_config where type='tikv' and `key` = 'storage.block-cache.capacity'",
            &[],
            "check configuration in reason failed",
        );
        // 按 IP（去端口）聚合同机多实例。
        let extract_ip = |address: String| {
            address
                .split_once(':')
                .map_or(address.clone(), |(ip, _)| ip.to_owned())
        };
        let mut block_sizes = BTreeMap::<String, u64>::new();
        let mut counts = BTreeMap::<String, usize>::new();
        for row in config_rows {
            let ip = extract_ip(row.string(0));
            let size = match self.convertReadableSizeToByteSize(&row.string(1)) {
                Ok(size) => size,
                Err(error) => {
                    source.append_warning(format!(
                        "check TiKV block-cache configuration in reason failed: {error}"
                    ));
                    return Vec::new();
                }
            };
            *block_sizes.entry(ip.clone()).or_default() += size;
            *counts.entry(ip).or_default() += 1;
        }
        let memory_rows = query_or_warn(
            source,
            "select instance, value from metrics_schema.node_total_memory where time=now()",
            &[],
            "check configuration in reason failed",
        );
        let mut memory_sizes = BTreeMap::<String, f64>::new();
        for row in memory_rows {
            *memory_sizes.entry(extract_ip(row.string(0))).or_default() += row.float(1);
        }
        block_sizes
            .into_iter()
            .filter_map(|(ip, block_size)| {
                let memory_size = *memory_sizes.get(&ip)?;
                (block_size as f64 > memory_size * 0.45).then(|| inspectionResult {
                    tp: "tikv".into(),
                    instance: ip.clone(),
                    item: item.into(),
                    actual: block_size.to_string(),
                    expected: format!("< {:.0}", memory_size * 0.45),
                    severity: "warning".into(),
                    detail: format!(
                        "There are {} TiKV server in {} node, the total 'storage.block-cache.capacity' of TiKV is more than (0.45 * total node memory)",
                        counts.get(&ip).copied().unwrap_or_default(), ip
                    ),
                    ..Default::default()
                })
            })
            .collect()
    }

    /// 将 KiB/MiB/... 或纯数字容量字符串转为字节数。
    pub fn convertReadableSizeToByteSize(&self, size: &str) -> InspectionResultValue<u64> {
        let (digits, rate) = [
            ("KiB", 1_u64 << 10),
            ("MiB", 1_u64 << 20),
            ("GiB", 1_u64 << 30),
            ("TiB", 1_u64 << 40),
            ("PiB", 1_u64 << 50),
        ]
        .into_iter()
        .find_map(|(suffix, rate)| size.strip_suffix(suffix).map(|digits| (digits, rate)))
        .unwrap_or_else(|| (size.strip_suffix('B').unwrap_or(size), 1));
        digits
            .parse::<i64>()
            .map(|value| (value as u64).wrapping_mul(rate))
            .map_err(|error| InspectionError(error.to_string()))
    }
}

impl versionInspection {
    /// 检查同 type 是否存在多个不同 git_hash。
    pub fn inspect(
        &self,
        source: &dyn inspectionDataSource,
        filter: &inspectionFilter,
    ) -> Vec<inspectionResult> {
        let rows = query_or_warn(
            source,
            "select type, count(distinct git_hash) as c from information_schema.cluster_info group by type having c > 1;",
            &[],
            "check version consistency failed",
        );
        if !filter.enable("git_hash") {
            return Vec::new();
        }
        rows.into_iter()
            .map(|row| {
                let tp = row.string(0);
                inspectionResult {
                    tp: tp.clone(),
                    item: "git_hash".into(),
                    actual: "inconsistent".into(),
                    expected: "consistent".into(),
                    severity: "critical".into(),
                    detail: format!(
                        "the cluster has {} different {} versions, execute the sql to see more detail: select * from information_schema.cluster_info where type='{}'",
                        row.unsigned(1), tp, tp
                    ),
                    ..Default::default()
                }
            })
            .collect()
    }
}

#[derive(Clone, Debug)]
/// 节点虚拟内存使用率 ≥ 70% 告警。
pub struct inspectVirtualMemUsage;

impl inspectVirtualMemUsage {
    pub fn genSQL(&self, timeRange: &queryTimeRange) -> String {
        format!(
            "select instance, max(value) as max_usage from metrics_schema.node_memory_usage {} group by instance having max_usage >= 70",
            timeRange.condition()
        )
    }

    pub fn genResult(&self, _sql: &str, row: &queryRow) -> inspectionResult {
        inspectionResult {
            tp: "node".into(),
            instance: row.string(0),
            item: self.getItem(),
            actual: format!("{:.1}%", row.float(1)),
            expected: "< 70%".into(),
            severity: "warning".into(),
            detail: "the memory-usage is too high".into(),
            ..Default::default()
        }
    }

    pub fn getItem(&self) -> String {
        "virtual-memory-usage".into()
    }
}

#[derive(Clone, Debug)]
/// 节点出现 swap 使用则告警。
pub struct inspectSwapMemoryUsed;

impl inspectSwapMemoryUsed {
    pub fn genSQL(&self, timeRange: &queryTimeRange) -> String {
        format!(
            "select instance, max(value) as max_used from metrics_schema.node_memory_swap_used {} group by instance having max_used > 0",
            timeRange.condition()
        )
    }

    pub fn genResult(&self, _sql: &str, row: &queryRow) -> inspectionResult {
        inspectionResult {
            tp: "node".into(),
            instance: row.string(0),
            item: self.getItem(),
            actual: format!("{:.1}", row.float(1)),
            expected: "0".into(),
            severity: "warning".into(),
            ..Default::default()
        }
    }

    pub fn getItem(&self) -> String {
        "swap-memory-used".into()
    }
}

#[derive(Clone, Debug)]
/// 挂载点磁盘使用率 ≥ 70% 告警。
pub struct inspectDiskUsage;

impl inspectDiskUsage {
    pub fn genSQL(&self, timeRange: &queryTimeRange) -> String {
        format!(
            "select instance, device, max(value) as max_usage from metrics_schema.node_disk_usage {} and device like '/%' group by instance, device having max_usage >= 70",
            timeRange.condition()
        )
    }

    pub fn genResult(&self, _sql: &str, row: &queryRow) -> inspectionResult {
        inspectionResult {
            tp: "node".into(),
            instance: row.string(0),
            item: self.getItem(),
            actual: format!("{:.1}%", row.float(2)),
            expected: "< 70%".into(),
            severity: "warning".into(),
            detail: format!("the disk-usage of {} is too high", row.string(1)),
            ..Default::default()
        }
    }

    pub fn getItem(&self) -> String {
        "disk-usage".into()
    }
}

#[derive(Clone, Debug)]
/// CPU load1/5/15 相对逻辑核数超 70% 告警。
pub struct inspectCPULoad {
    pub item: String,
    pub tbl: String,
}

impl inspectCPULoad {
    pub fn genSQL(&self, timeRange: &queryTimeRange) -> String {
        format!(
            "select t1.instance, t1.max_load , 0.7*t2.cpu_count from (select instance,max(value) as max_load from metrics_schema.{} {} group by instance) as t1 join (select instance,max(value) as cpu_count from metrics_schema.node_virtual_cpus {} group by instance) as t2 on t1.instance=t2.instance where t1.max_load>(0.7*t2.cpu_count);",
            self.tbl,
            timeRange.condition(),
            timeRange.condition()
        )
    }

    pub fn genResult(&self, _sql: &str, row: &queryRow) -> inspectionResult {
        inspectionResult {
            tp: "node".into(),
            instance: row.string(0),
            item: format!("cpu-{}", self.item),
            actual: format!("{:.1}", row.float(1)),
            expected: format!("< {:.1}", row.float(2)),
            severity: "warning".into(),
            detail: format!(
                "{} should less than (cpu_logical_cores * 0.7)",
                self.getItem()
            ),
            ..Default::default()
        }
    }

    pub fn getItem(&self) -> String {
        format!("cpu-{}", self.item)
    }
}

#[derive(Clone, Debug)]
/// 可统一 genSQL/genResult 的检查项适配器。
pub enum ruleChecker {
    VirtualMemory(inspectVirtualMemUsage),
    SwapMemory(inspectSwapMemoryUsed),
    DiskUsage(inspectDiskUsage),
    CpuLoad(inspectCPULoad),
    CompareStore(compareStoreStatus),
    RegionHealth(checkRegionHealth),
    RegionCount(checkStoreRegionTooMuch),
}

impl ruleChecker {
    /// 生成该检查项的 metrics SQL。
    fn sql(&self, time_range: &queryTimeRange) -> String {
        match self {
            Self::VirtualMemory(rule) => rule.genSQL(time_range),
            Self::SwapMemory(rule) => rule.genSQL(time_range),
            Self::DiskUsage(rule) => rule.genSQL(time_range),
            Self::CpuLoad(rule) => rule.genSQL(time_range),
            Self::CompareStore(rule) => rule.genSQL(time_range),
            Self::RegionHealth(rule) => rule.genSQL(time_range),
            Self::RegionCount(rule) => rule.genSQL(time_range),
        }
    }

    /// 将结果行转为 inspectionResult。
    fn result(&self, sql: &str, row: &queryRow) -> inspectionResult {
        match self {
            Self::VirtualMemory(rule) => rule.genResult(sql, row),
            Self::SwapMemory(rule) => rule.genResult(sql, row),
            Self::DiskUsage(rule) => rule.genResult(sql, row),
            Self::CpuLoad(rule) => rule.genResult(sql, row),
            Self::CompareStore(rule) => rule.genResult(sql, row),
            Self::RegionHealth(rule) => rule.genResult(sql, row),
            Self::RegionCount(rule) => rule.genResult(sql, row),
        }
    }

    /// 检查项名称（用于 filter.enable）。
    fn item(&self) -> String {
        match self {
            Self::VirtualMemory(rule) => rule.getItem(),
            Self::SwapMemory(rule) => rule.getItem(),
            Self::DiskUsage(rule) => rule.getItem(),
            Self::CpuLoad(rule) => rule.getItem(),
            Self::CompareStore(rule) => rule.getItem(),
            Self::RegionHealth(rule) => rule.getItem(),
            Self::RegionCount(rule) => rule.getItem(),
        }
    }
}

impl nodeLoadInspection {
    /// 运行 CPU load / 内存 / 磁盘 一组 ruleChecker。
    pub fn inspect(
        &self,
        source: &dyn inspectionDataSource,
        filter: &inspectionFilter,
    ) -> Vec<inspectionResult> {
        let rules = vec![
            ruleChecker::CpuLoad(inspectCPULoad {
                item: "load1".into(),
                tbl: "node_load1".into(),
            }),
            ruleChecker::CpuLoad(inspectCPULoad {
                item: "load5".into(),
                tbl: "node_load5".into(),
            }),
            ruleChecker::CpuLoad(inspectCPULoad {
                item: "load15".into(),
                tbl: "node_load15".into(),
            }),
            ruleChecker::VirtualMemory(inspectVirtualMemUsage),
            ruleChecker::SwapMemory(inspectSwapMemoryUsed),
            ruleChecker::DiskUsage(inspectDiskUsage),
        ];
        checkRules(source, filter, &rules)
    }
}

impl criticalErrorInspection {
    /// 致命错误指标 + 宕机/重启检测。
    pub fn inspect(
        &self,
        source: &dyn inspectionDataSource,
        filter: &inspectionFilter,
    ) -> Vec<inspectionResult> {
        let mut results = self.inspectError(source, filter);
        results.extend(self.inspectForServerDown(source, filter));
        results
    }

    /// 扫描 critical/panic/busy/stall 等 metrics 累计值。
    pub fn inspectError(
        &self,
        source: &dyn inspectionDataSource,
        filter: &inspectionFilter,
    ) -> Vec<inspectionResult> {
        let rules = [
            ("tikv", "critical-error", "tikv_critical_error_total_count"),
            ("tidb", "panic-count", "tidb_panic_count_total_count"),
            ("tidb", "binlog-error", "tidb_binlog_error_total_count"),
            (
                "tikv",
                "scheduler-is-busy",
                "tikv_scheduler_is_busy_total_count",
            ),
            (
                "tikv",
                "coprocessor-is-busy",
                "tikv_coprocessor_is_busy_total_count",
            ),
            ("tikv", "channel-is-full", "tikv_channel_full_total_count"),
            ("tikv", "tikv_engine_write_stall", "tikv_engine_write_stall"),
        ];
        let mut results = Vec::new();
        for (tp, item, table) in rules {
            if !filter.enable(item) {
                continue;
            }
            // 动态取表标签列，按标签 GROUP BY 汇总。
            let labels = match source.metric_labels(table) {
                Ok(Some(labels)) if !labels.is_empty() => labels,
                Ok(_) => {
                    source.append_warning(format!("metrics table: {table} not found"));
                    continue;
                }
                Err(error) => {
                    source.append_warning(format!("metrics table: {table} not found: {error}"));
                    continue;
                }
            };
            let quoted_labels = labels
                .iter()
                .map(|label| format!("`{label}`"))
                .collect::<Vec<_>>()
                .join(",");
            let sql = format!(
                "select {quoted_labels},sum(value) as total from `METRICS_SCHEMA`.`{table}` {} group by {quoted_labels} having total>=1.0",
                filter.timeRange.condition()
            );
            let rows = match source.execute_sql(&sql, &[]) {
                Ok(rows) => rows,
                Err(error) => {
                    source.append_warning(format!("execute '{sql}' failed: {error}"));
                    continue;
                }
            };
            for row in rows {
                let total_index = labels.len();
                let total = row.float(total_index);
                let actual = if labels.len() > 1 {
                    let values = (1..labels.len())
                        .map(|index| row.string(index))
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{total:.2}({values})")
                } else {
                    format!("{total:.2}")
                };
                results.push(inspectionResult {
                    tp: tp.into(),
                    statusAddress: row.string(0),
                    item: item.into(),
                    actual,
                    expected: "0".into(),
                    severity: "critical".into(),
                    detail: format!("the total number of errors about '{item}' is too many"),
                    degree: total,
                    ..Default::default()
                });
            }
        }
        results
    }

    /// 通过 up 指标波动与 Welcome 日志检测断连/重启。
    pub fn inspectForServerDown(
        &self,
        source: &dyn inspectionDataSource,
        filter: &inspectionFilter,
    ) -> Vec<inspectionResult> {
        let item = "server-down";
        if !filter.enable(item) {
            return Vec::new();
        }
        let condition = filter.timeRange.condition();
        // up 序列 max-min>0 且出现 value=0 视为曾断开 Prometheus。
        let sql = format!(
            "select t1.job,t1.instance, t2.min_time from (select instance,job from metrics_schema.up {condition} group by instance,job having max(value)-min(value)>0) as t1 join (select instance,min(time) as min_time from metrics_schema.up {condition} and value=0 group by instance,job) as t2 on t1.instance=t2.instance order by job"
        );
        let rows = query_or_warn(source, &sql, &[], &format!("execute '{sql}' failed"));
        let mut results = Vec::new();
        for row in rows.into_iter().filter(|row| row.len() >= 3) {
            results.push(inspectionResult {
                tp: row.string(0),
                statusAddress: row.string(1),
                item: item.into(),
                severity: "critical".into(),
                detail: format!(
                    "{} {} disconnect with prometheus around time '{}'",
                    row.string(0),
                    row.string(1),
                    row.string(2)
                ),
                degree: 10_000.0 + results.len() as f64,
                ..Default::default()
            });
        }
        let log_sql = format!(
            "select type,instance,time from information_schema.cluster_log {condition} and level = 'info' and message like '%Welcome to'"
        );
        for row in query_or_warn(
            source,
            &log_sql,
            &[],
            &format!("execute '{log_sql}' failed"),
        )
        .into_iter()
        .filter(|row| row.len() >= 3)
        {
            results.push(inspectionResult {
                tp: row.string(0),
                instance: row.string(1),
                item: item.into(),
                severity: "critical".into(),
                detail: format!(
                    "{} {} restarted at time '{}'",
                    row.string(0),
                    row.string(1),
                    row.string(2)
                ),
                degree: 10_000.0 + results.len() as f64,
                ..Default::default()
            });
        }
        results
    }
}

#[derive(Clone, Debug)]
/// 比较两 store 某状态指标相对差是否超过阈值（负载不均）。
pub struct compareStoreStatus {
    pub item: String,
    pub tp: String,
    pub threshold: f64,
}

impl compareStoreStatus {
    pub fn genSQL(&self, timeRange: &queryTimeRange) -> String {
        let condition = format!(
            "where t1.time>='{}' and t1.time<='{}' and t2.time>='{}' and t2.time<='{}'",
            timeRange.from, timeRange.to, timeRange.from, timeRange.to
        );
        format!(
            "SELECT t1.address, max(t1.value), t2.address, min(t2.value), max((t1.value-t2.value)/t1.value) AS ratio FROM metrics_schema.pd_scheduler_store_status t1 JOIN metrics_schema.pd_scheduler_store_status t2 {condition} AND t1.type='{}' AND t1.time = t2.time AND t1.type=t2.type AND t1.address != t2.address AND (t1.value-t2.value)/t1.value>{} AND t1.value > 0 GROUP BY t1.address,t2.address ORDER BY ratio desc",
            self.tp, self.threshold
        )
    }

    pub fn genResult(&self, _sql: &str, row: &queryRow) -> inspectionResult {
        let addr1 = row.string(0);
        let value1 = row.float(1);
        let addr2 = row.string(2);
        let value2 = row.float(3);
        let ratio = row.float(4);
        inspectionResult {
            tp: "tikv".into(),
            instance: addr2.clone(),
            item: self.item.clone(),
            actual: format!("{:.2}%", ratio * 100.0),
            expected: format!("< {:.2}%", self.threshold * 100.0),
            severity: "warning".into(),
            detail: format!(
                "{addr1} max {} is {value1:.2}, much more than {addr2} min {} {value2:.2}",
                self.tp, self.tp
            ),
            degree: ratio,
            ..Default::default()
        }
    }

    pub fn getItem(&self) -> String {
        self.item.clone()
    }
}

#[derive(Clone, Debug)]
/// extra/learner/pending peer 数量过多（调度过频或过慢）。
/// Region 是 TiKV 数据分片单位；peer 为其 Raft 副本。
pub struct checkRegionHealth;

impl checkRegionHealth {
    pub fn genSQL(&self, timeRange: &queryTimeRange) -> String {
        format!(
            "select instance, sum(value) as sum_value from metrics_schema.pd_region_health {} and type in ('extra-peer-region-count','learner-peer-region-count','pending-peer-region-count') having sum_value>100",
            timeRange.condition()
        )
    }

    pub fn genResult(&self, _sql: &str, row: &queryRow) -> inspectionResult {
        let count = row.float(1);
        inspectionResult {
            tp: "pd".into(),
            instance: row.string(0),
            item: self.getItem(),
            actual: format!("{count:.2}"),
            expected: "< 100".into(),
            severity: "warning".into(),
            detail: format!(
                "the count of extra-perr and learner-peer and pending-peer are {count}, it means the scheduling is too frequent or too slow"
            ),
            degree: (count - 100.0).abs() / count.max(100.0),
            ..Default::default()
        }
    }

    pub fn getItem(&self) -> String {
        "region-health".into()
    }
}

#[derive(Clone, Debug)]
/// 单 store Region 数超过 20000 告警。
pub struct checkStoreRegionTooMuch;

impl checkStoreRegionTooMuch {
    pub fn genSQL(&self, timeRange: &queryTimeRange) -> String {
        format!(
            "select address, max(value) from metrics_schema.pd_scheduler_store_status {} and type='region_count' and value > 20000 group by address",
            timeRange.condition()
        )
    }

    pub fn genResult(&self, _sql: &str, row: &queryRow) -> inspectionResult {
        let count = row.float(1);
        inspectionResult {
            tp: "tikv".into(),
            instance: row.string(0),
            item: self.getItem(),
            actual: format!("{count:.2}"),
            expected: "<= 20000".into(),
            severity: "warning".into(),
            detail: format!("{} tikv has too many regions", row.string(0)),
            degree: (count - 20_000.0).abs() / count.max(20_000.0),
            ..Default::default()
        }
    }

    pub fn getItem(&self) -> String {
        "region-count".into()
    }
}

impl thresholdCheckInspection {
    /// 串联阈值检查 1/2/3 与 leader-drop。
    pub fn inspect(
        &self,
        source: &dyn inspectionDataSource,
        filter: &inspectionFilter,
    ) -> Vec<inspectionResult> {
        let mut results = self.inspectThreshold1(source, filter);
        results.extend(self.inspectThreshold2(source, filter));
        results.extend(self.inspectThreshold3(source, filter));
        results.extend(self.inspectForLeaderDrop(source, filter));
        results
    }

    /// TiKV 线程 CPU 相对并发配置是否过高。
    pub fn inspectThreshold1(
        &self,
        source: &dyn inspectionDataSource,
        filter: &inspectionFilter,
    ) -> Vec<inspectionResult> {
        let rules = [
            (
                "coprocessor-normal-cpu",
                "cop_normal%",
                "readpool.coprocessor.normal-concurrency",
                0.9,
            ),
            (
                "coprocessor-high-cpu",
                "cop_high%",
                "readpool.coprocessor.high-concurrency",
                0.9,
            ),
            (
                "coprocessor-low-cpu",
                "cop_low%",
                "readpool.coprocessor.low-concurrency",
                0.9,
            ),
            ("grpc-cpu", "grpc%", "server.grpc-concurrency", 0.9),
            (
                "raftstore-cpu",
                "raftstore_%",
                "raftstore.store-pool-size",
                0.8,
            ),
            ("apply-cpu", "apply_%", "raftstore.apply-pool-size", 0.8),
            (
                "storage-readpool-normal-cpu",
                "store_read_norm%",
                "readpool.storage.normal-concurrency",
                0.9,
            ),
            (
                "storage-readpool-high-cpu",
                "store_read_high%",
                "readpool.storage.high-concurrency",
                0.9,
            ),
            (
                "storage-readpool-low-cpu",
                "store_read_low%",
                "readpool.storage.low-concurrency",
                0.9,
            ),
            (
                "scheduler-worker-cpu",
                "sched_%",
                "storage.scheduler-worker-pool-size",
                0.85,
            ),
            ("split-check-cpu", "split_check", "", 0.9),
        ];
        let condition = filter.timeRange.condition();
        let mut results = Vec::new();
        for (item, component, config_key, threshold) in rules {
            if !filter.enable(item) {
                continue;
            }
            // 无配置键时用固定阈值；否则阈值 = 配置并发 × 系数。
            let sql = if config_key.is_empty() {
                format!(
                    "select t1.instance, t1.cpu, {threshold} from (select instance, max(value) as cpu from metrics_schema.tikv_thread_cpu {condition} and name like '{component}' group by instance) as t1 where t1.cpu > {threshold};"
                )
            } else {
                format!(
                    "select t1.status_address, t1.cpu, (t2.value * {threshold}) as threshold, t2.value from (select status_address, max(sum_value) as cpu from (select instance as status_address, sum(value) as sum_value from metrics_schema.tikv_thread_cpu {condition} and name like '{component}' group by instance, time) as tmp group by tmp.status_address) as t1 join (select instance, value from information_schema.cluster_config where type='tikv' and `key` = '{config_key}') as t2 join (select instance,status_address from information_schema.cluster_info where type='tikv') as t3 on t1.status_address=t3.status_address and t2.instance=t3.instance where t1.cpu > (t2.value * {threshold})"
                )
            };
            let rows = match source.execute_sql(&sql, &[]) {
                Ok(rows) => rows,
                Err(error) => {
                    source.append_warning(format!("execute '{sql}' failed: {error}"));
                    continue;
                }
            };
            for row in rows {
                let actual_value = row.float(1);
                let threshold_value = row.float(2);
                let expected = if config_key.is_empty() {
                    format!("< {threshold_value:.2}")
                } else {
                    format!(
                        "< {threshold_value:.2}, config: {config_key}={}",
                        row.string(3)
                    )
                };
                results.push(inspectionResult {
                    tp: "tikv".into(),
                    statusAddress: row.string(0),
                    item: item.into(),
                    actual: format!("{actual_value:.2}"),
                    expected,
                    severity: "warning".into(),
                    detail: format!(
                        "the '{item}' max cpu-usage of {} tikv is too high",
                        row.string(0)
                    ),
                    degree: (actual_value - threshold_value).abs()
                        / actual_value.max(threshold_value),
                    ..Default::default()
                });
            }
        }
        results
    }

    /// 延迟分位、pending 命令数、block cache 命中率等阈值。
    pub fn inspectThreshold2(
        &self,
        source: &dyn inspectionDataSource,
        filter: &inspectionFilter,
    ) -> Vec<inspectionResult> {
        // 单条阈值规则：表名、附加条件、阈值、单位换算因子、min/max 方向。
        struct thresholdRule {
            tp: &'static str,
            item: &'static str,
            table: &'static str,
            condition: &'static str,
            threshold: f64,
            factor: f64,
            is_min: bool,
            detail: &'static str,
        }
        let rules = [
            thresholdRule {
                tp: "tidb",
                item: "tso-duration",
                table: "pd_tso_wait_duration",
                condition: "quantile=0.999",
                threshold: 0.05,
                factor: 1.0,
                is_min: false,
                detail: "",
            },
            thresholdRule {
                tp: "tidb",
                item: "get-token-duration",
                table: "tidb_get_token_duration",
                condition: "quantile=0.999",
                threshold: 0.001,
                factor: 1_000_000.0,
                is_min: false,
                detail: "",
            },
            thresholdRule {
                tp: "tidb",
                item: "load-schema-duration",
                table: "tidb_load_schema_duration",
                condition: "quantile=0.99",
                threshold: 1.0,
                factor: 1.0,
                is_min: false,
                detail: "",
            },
            thresholdRule {
                tp: "tikv",
                item: "scheduler-cmd-duration",
                table: "tikv_scheduler_command_duration",
                condition: "quantile=0.99",
                threshold: 0.1,
                factor: 1.0,
                is_min: false,
                detail: "",
            },
            thresholdRule {
                tp: "tikv",
                item: "handle-snapshot-duration",
                table: "tikv_handle_snapshot_duration",
                condition: "",
                threshold: 30.0,
                factor: 1.0,
                is_min: false,
                detail: "",
            },
            thresholdRule {
                tp: "tikv",
                item: "storage-write-duration",
                table: "tikv_storage_async_request_duration",
                condition: "type='write'",
                threshold: 0.1,
                factor: 1.0,
                is_min: false,
                detail: "",
            },
            thresholdRule {
                tp: "tikv",
                item: "storage-snapshot-duration",
                table: "tikv_storage_async_request_duration",
                condition: "type='snapshot'",
                threshold: 0.05,
                factor: 1.0,
                is_min: false,
                detail: "",
            },
            thresholdRule {
                tp: "tikv",
                item: "rocksdb-write-duration",
                table: "tikv_engine_write_duration",
                condition: "type='write_max'",
                threshold: 0.1,
                factor: 1_000_000.0,
                is_min: false,
                detail: "",
            },
            thresholdRule {
                tp: "tikv",
                item: "rocksdb-get-duration",
                table: "tikv_engine_max_get_duration",
                condition: "type='get_max'",
                threshold: 0.05,
                factor: 1_000_000.0,
                is_min: false,
                detail: "",
            },
            thresholdRule {
                tp: "tikv",
                item: "rocksdb-seek-duration",
                table: "tikv_engine_max_seek_duration",
                condition: "type='seek_max'",
                threshold: 0.05,
                factor: 1_000_000.0,
                is_min: false,
                detail: "",
            },
            thresholdRule {
                tp: "tikv",
                item: "scheduler-pending-cmd-count",
                table: "tikv_scheduler_pending_commands",
                condition: "",
                threshold: 1000.0,
                factor: 1.0,
                is_min: false,
                detail: " %s tikv scheduler has too many pending commands",
            },
            thresholdRule {
                tp: "tikv",
                item: "index-block-cache-hit",
                table: "tikv_block_index_cache_hit",
                condition: "value > 0",
                threshold: 0.95,
                factor: 1.0,
                is_min: true,
                detail: "",
            },
            thresholdRule {
                tp: "tikv",
                item: "filter-block-cache-hit",
                table: "tikv_block_filter_cache_hit",
                condition: "value > 0",
                threshold: 0.95,
                factor: 1.0,
                is_min: true,
                detail: "",
            },
            thresholdRule {
                tp: "tikv",
                item: "data-block-cache-hit",
                table: "tikv_block_data_cache_hit",
                condition: "value > 0",
                threshold: 0.80,
                factor: 1.0,
                is_min: true,
                detail: "",
            },
        ];
        let mut results = Vec::new();
        for rule in rules {
            if !filter.enable(rule.item) {
                continue;
            }
            let mut condition = filter.timeRange.condition();
            if !rule.condition.is_empty() {
                condition.push_str(" and ");
                condition.push_str(rule.condition);
            }
            // is_min：命中率类要求不低于阈值；否则检查最大值是否超标。
            let (aggregate, alias, comparator) = if rule.is_min {
                ("min", "min_value", "<")
            } else {
                ("max", "max_value", ">")
            };
            let sql = format!(
                "select instance, {aggregate}(value)/{:.0} as {alias} from metrics_schema.{} {condition} group by instance having {alias} {comparator} {};",
                rule.factor, rule.table, rule.threshold
            );
            let rows = match source.execute_sql(&sql, &[]) {
                Ok(rows) => rows,
                Err(error) => {
                    source.append_warning(format!("execute '{sql}' failed: {error}"));
                    continue;
                }
            };
            for row in rows {
                let value = row.float(1);
                let expected = if rule.is_min {
                    format!("> {:.3}", rule.threshold)
                } else {
                    format!("< {:.3}", rule.threshold)
                };
                let detail = if !rule.detail.is_empty() {
                    rule.detail.replace("%s", &row.string(0))
                } else if rule.item.ends_with("duration") {
                    format!(
                        "max duration of {} {} {} is too slow",
                        row.string(0),
                        rule.tp,
                        rule.item
                    )
                } else if rule.item.ends_with("hit") {
                    format!(
                        "min {} rate of {} {} is too low",
                        rule.item,
                        row.string(0),
                        rule.tp
                    )
                } else {
                    String::new()
                };
                results.push(inspectionResult {
                    tp: rule.tp.into(),
                    statusAddress: row.string(0),
                    item: rule.item.into(),
                    actual: format!("{value:.3}"),
                    expected,
                    severity: "warning".into(),
                    detail,
                    degree: (value - rule.threshold).abs() / value.max(rule.threshold),
                    ..Default::default()
                });
            }
        }
        results
    }

    /// store 间 leader/region/available 均衡与 Region 健康度。
    pub fn inspectThreshold3(
        &self,
        source: &dyn inspectionDataSource,
        filter: &inspectionFilter,
    ) -> Vec<inspectionResult> {
        let rules = vec![
            ruleChecker::CompareStore(compareStoreStatus {
                item: "leader-score-balance".into(),
                tp: "leader_score".into(),
                threshold: 0.05,
            }),
            ruleChecker::CompareStore(compareStoreStatus {
                item: "region-score-balance".into(),
                tp: "region_score".into(),
                threshold: 0.05,
            }),
            ruleChecker::CompareStore(compareStoreStatus {
                item: "store-available-balance".into(),
                tp: "store_available".into(),
                threshold: 0.2,
            }),
            ruleChecker::RegionHealth(checkRegionHealth),
            ruleChecker::RegionCount(checkStoreRegionTooMuch),
        ];
        checkRules(source, filter, &rules)
    }

    /// 时间窗口内 leader 数骤降超过阈值则告警。
    pub fn inspectForLeaderDrop(
        &self,
        source: &dyn inspectionDataSource,
        filter: &inspectionFilter,
    ) -> Vec<inspectionResult> {
        let condition = filter.timeRange.condition();
        let threshold = 50.0_f64;
        let sql = format!(
            "select address,min(value) as mi,max(value) as mx from metrics_schema.pd_scheduler_store_status {condition} and type='leader_count' group by address having mx-mi>{threshold}"
        );
        let rows = match source.execute_sql(&sql, &[]) {
            Ok(rows) => rows,
            Err(error) => {
                source.append_warning(format!("execute '{sql}' failed: {error}"));
                return Vec::new();
            }
        };
        let mut results = Vec::new();
        for row in rows {
            let address = row.string(0);
            let detail_sql = format!(
                "select time, value from metrics_schema.pd_scheduler_store_status {condition} and type='leader_count' and address = '{address}' order by time"
            );
            let samples = match source.execute_sql(&detail_sql, &[]) {
                Ok(rows) => rows,
                Err(error) => {
                    source.append_warning(format!("execute '{detail_sql}' failed: {error}"));
                    continue;
                }
            };
            let Some(first) = samples.first() else {
                continue;
            };
            // 按时间扫描，发现相邻采样骤降即记一条。
            let mut last_value = first.float(1);
            for sample in samples.iter().skip(1) {
                let value = sample.float(1);
                let drop = last_value - value;
                if drop > threshold {
                    results.push(inspectionResult {
                        tp: "tikv".into(),
                        instance: address.clone(),
                        item: "leader-drop".into(),
                        actual: format!("{drop:.0}"),
                        expected: format!("<= {threshold:.0}"),
                        severity: if value == 0.0 { "critical" } else { "warning" }.into(),
                        detail: format!(
                            "{address} tikv has too many leader-drop around time {}, leader count from {last_value:.0} drop to {value:.0}",
                            sample.string(0)
                        ),
                        degree: drop,
                        ..Default::default()
                    });
                    break;
                }
                last_value = value;
            }
        }
        results
    }
}

/// 对一组 ruleChecker：过滤 → 执行 SQL → 映射为 inspectionResult。
pub fn checkRules(
    source: &dyn inspectionDataSource,
    filter: &inspectionFilter,
    rules: &[ruleChecker],
) -> Vec<inspectionResult> {
    let mut results = Vec::new();
    for rule in rules {
        if !filter.enable(&rule.item()) {
            continue;
        }
        let sql = rule.sql(&filter.timeRange);
        match source.execute_sql(&sql, &[]) {
            Ok(rows) => results.extend(rows.iter().map(|row| rule.result(&sql, row))),
            Err(error) => source.append_warning(format!("execute '{sql}' failed: {error}")),
        }
    }
    results
}
