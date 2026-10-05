// Copyright 2024 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.

// 工作负载学习的分析 Handle：从语句统计与二进制执行计划中提取表读代价。
//
// 遍历近一周语句记录，解析 Explain 算子树中的扫描/内存信息，
// 按表 ID 累加后归一化为 `TableReadCost`，并批量写回 `WorkloadStore`。

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use crate::{CIStr, TableReadCostMetrics};

/// 批量写入存储时每批最大行数。
const batchInsertSize: usize = 1000;
/// PointGet / BatchPointGet 缺少真实内存数据时使用的默认内存用量。
const defaultPointGetMemUsage: i64 = 1;
/// Feedback 类别名常量。
pub const feedbackCategory: &str = "Feedback";
/// 表读代价指标类别名常量。
pub const tableReadCost: &str = "TableReadCost";

/// 一条语句统计记录：摘要、SQL、二进制计划与出现频率。
#[derive(Clone, Debug, PartialEq)]
pub struct StatementRecord {
    /// SQL digest（语句指纹）。
    pub digest: String,
    /// 原始 SQL 文本。
    pub sql: String,
    /// 二进制执行计划的 JSON 表示。
    pub binary_plan: String,
    /// 该语句在统计窗口内的出现次数。
    pub frequency: i64,
}

/// 工作负载持久化抽象：版本化指标、快照与语句加载、表 ID 解析。
pub trait WorkloadStore: Send + Sync {
    /// 返回当前最新指标版本号。
    fn latest_version(&self) -> Result<u64, String>;
    /// 加载指定版本的 (表 ID, JSON 指标) 行。
    fn load_metrics(&self, version: u64) -> Result<Vec<(i64, String)>, String>;
    /// 保存指定版本的指标行。
    fn save_metrics(&self, version: u64, rows: &[(i64, String)]) -> Result<(), String>;
    /// 查找最接近给定时间点的快照 ID。
    fn closest_snapshot_id(&self, at: SystemTime) -> Result<u64, String>;
    /// 加载两个快照之间的语句统计记录。
    fn load_statements(
        &self,
        start_snapshot: u64,
        end_snapshot: u64,
    ) -> Result<Vec<StatementRecord>, String>;
    /// 按库名与表名解析表 ID。
    fn table_id(&self, database: &str, table: &str) -> Result<i64, String>;
}

/// 元信息查询接口：将库表名映射为表 ID（InfoSchema 语义）。
pub trait InfoSchema {
    fn TableID(&self, database: &str, table: &str) -> Result<i64, String>;
}

impl<T: WorkloadStore + ?Sized> InfoSchema for T {
    fn TableID(&self, database: &str, table: &str) -> Result<i64, String> {
        self.table_id(database, table)
    }
}

/// 工作负载学习 Handle：持有 `WorkloadStore`，负责分析与落盘。
pub struct Handle {
    store: Arc<dyn WorkloadStore>,
}

/// 构造工作负载学习 Handle。
pub fn NewWorkloadLearningHandle(store: Arc<dyn WorkloadStore>) -> Handle {
    Handle { store }
}

impl Handle {
    /// 基于语句统计分析表读代价，落盘后返回按表 ID 的指标映射。
    pub fn HandleTableReadCost(&self) -> Result<HashMap<i64, TableReadCostMetrics>, String> {
        let (metrics, startTime, endTime) = self.analyzeBasedOnStatementStats()?;
        self.SaveTableReadCostMetrics(&metrics, startTime, endTime)?;
        Ok(metrics)
    }

    /// 取近 7 天语句，从二进制计划提取扫描/内存，按表累加并归一化代价。
    pub fn analyzeBasedOnStatementStats(
        &self,
    ) -> Result<(HashMap<i64, TableReadCostMetrics>, SystemTime, SystemTime), String> {
        let endTime = SystemTime::now();
        // 统计窗口：最近 7 天。
        let startTime = endTime - Duration::from_secs(7 * 24 * 3600);
        let startSnapshotID = findClosestSnapshotIDByTime(self.store.as_ref(), startTime)?;
        let endSnapshotID = findClosestSnapshotIDByTime(self.store.as_ref(), endTime)?;
        let mut tableIDToMetrics = HashMap::new();
        for record in self.store.load_statements(startSnapshotID, endSnapshotID)? {
            // 计划解析失败则跳过该语句。
            let current = match extractScanAndMemoryFromBinaryPlan(&record.binary_plan) {
                Ok(metrics) => metrics,
                Err(_) => continue,
            };
            AccumulateMetricsGroupByTableID(
                current,
                record.frequency,
                &mut tableIDToMetrics,
                self.store.as_ref(),
            );
        }
        let totalScanTime: u128 = tableIDToMetrics
            .values()
            .map(|metric| metric.TableScanTime.as_nanos())
            .sum();
        let totalMemUsage: i64 = tableIDToMetrics
            .values()
            .map(|metric| metric.TableMemUsage)
            .sum();
        // 归一化：扫描占比 + 内存占比作为 TableReadCost。
        for metric in tableIDToMetrics.values_mut() {
            let scan = if totalScanTime == 0 {
                0.0
            } else {
                metric.TableScanTime.as_nanos() as f64 / totalScanTime as f64
            };
            let memory = if totalMemUsage == 0 {
                0.0
            } else {
                metric.TableMemUsage as f64 / totalMemUsage as f64
            };
            metric.TableReadCost = scan + memory;
        }
        Ok((tableIDToMetrics, startTime, endTime))
    }

    /// 将指标序列化为 JSON 并按批次写入新版本；返回新版本号。
    pub fn SaveTableReadCostMetrics(
        &self,
        metrics: &HashMap<i64, TableReadCostMetrics>,
        _startTime: SystemTime,
        _endTime: SystemTime,
    ) -> Result<u64, String> {
        let version = self.store.latest_version()?.saturating_add(1);
        let rows = metrics
            .iter()
            .map(|(tableID, metric)| {
                serde_json::to_string(metric)
                    .map(|value| (*tableID, value))
                    .map_err(|error| error.to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        for chunk in rows.chunks(batchInsertSize) {
            self.store.save_metrics(version, chunk)?;
        }
        Ok(version)
    }
}

/// 按时间查找最接近的快照 ID。
pub fn findClosestSnapshotIDByTime(
    store: &dyn WorkloadStore,
    time: SystemTime,
) -> Result<u64, String> {
    store.closest_snapshot_id(time)
}

/// 执行计划中的访问对象：库表名，以及动态分区等嵌套对象。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct AccessObject {
    pub database: String,
    pub table: String,
    #[serde(default)]
    pub dynamic: Vec<AccessObject>,
}

/// 二进制执行计划中的 Explain 算子节点。
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct ExplainOperator {
    /// 算子名，形如 `TableReader_1`（类型_编号）。
    pub name: String,
    #[serde(default)]
    pub memory_bytes: i64,
    #[serde(default)]
    pub root_basic_exec_info: String,
    #[serde(default)]
    pub root_group_exec_info: Vec<String>,
    #[serde(default)]
    pub cop_exec_info: String,
    #[serde(default)]
    pub task_type: String,
    #[serde(default)]
    pub access_objects: Vec<AccessObject>,
    #[serde(default)]
    pub children: Vec<ExplainOperator>,
}

/// 将二进制计划 JSON 反序列化为算子树，并提取扫描/内存指标。
pub fn extractScanAndMemoryFromBinaryPlan(
    binaryPlan: &str,
) -> Result<Vec<TableReadCostMetrics>, String> {
    let operator: ExplainOperator =
        serde_json::from_str(binaryPlan).map_err(|error| error.to_string())?;
    extractMetricsFromOperatorTree(&operator, Vec::new())
}

/// 按表 ID 累加当前语句的指标；扫描时间与内存按语句频率放大。
pub fn AccumulateMetricsGroupByTableID<I: InfoSchema + ?Sized>(
    currentRecordMetrics: Vec<TableReadCostMetrics>,
    frequency: i64,
    previousMetrics: &mut HashMap<i64, TableReadCostMetrics>,
    infoSchema: &I,
) {
    for mut metric in currentRecordMetrics {
        let Ok(tableID) = infoSchema.TableID(&metric.DbName.L, &metric.TableName.L) else {
            continue;
        };
        metric.TableScanTime = metric.TableScanTime.saturating_mul(frequency.max(0) as u32);
        metric.TableMemUsage = metric.TableMemUsage.saturating_mul(frequency);
        metric.ReadFrequency = frequency;
        previousMetrics
            .entry(tableID)
            .and_modify(|previous| {
                previous.TableScanTime =
                    previous.TableScanTime.saturating_add(metric.TableScanTime);
                previous.TableMemUsage =
                    previous.TableMemUsage.saturating_add(metric.TableMemUsage);
                previous.ReadFrequency =
                    previous.ReadFrequency.saturating_add(metric.ReadFrequency);
            })
            .or_insert(metric);
    }
}

/// 递归遍历算子树，按算子类型提取表读相关指标。
pub fn extractMetricsFromOperatorTree(
    op: &ExplainOperator,
    mut operatorMetrics: Vec<TableReadCostMetrics>,
) -> Result<Vec<TableReadCostMetrics>, String> {
    let operatorType = extractOperatorTypeFromName(&op.name)?;
    match operatorType.as_str() {
        "IndexLookUp" | "IndexReader" => {
            let (db, table) = extractTableNameFromIndexScan(op);
            if db.is_empty() || table.is_empty() {
                return Err("failed to get table name from index scan".into());
            }
            operatorMetrics.push(metric(
                db,
                table,
                extractScanTimeFromExecutionInfo(op)?,
                op.memory_bytes,
            ));
        }
        "PointGet" | "BatchPointGet" => {
            let access = op.access_objects.first().ok_or("access object is empty")?;
            let (db, table) = extractTableNameFromAccessObject(access);
            operatorMetrics.push(metric(
                db,
                table,
                extractScanTimeFromExecutionInfo(op)?,
                defaultPointGetMemUsage,
            ));
        }
        // 跳过 TiFlash（MPP）路径上的 TableReader。
        "TableReader" if !checkTiFlashOperator(op) => {
            let (db, table) = extractTableNameFromChildrenTableScan(op);
            if db.is_empty() || table.is_empty() {
                return Err("failed to get table name from table scan".into());
            }
            operatorMetrics.push(metric(
                db,
                table,
                extractScanTimeFromExecutionInfo(op)?,
                op.memory_bytes,
            ));
        }
        "IndexMerge" => {
            let mut partial = extractPartialMetricsFromChildrenIndexMerge(op)?;
            if partial.is_empty() {
                return Err("index merge has no index scan".into());
            }
            // 父算子内存均分到各部分索引扫描。
            let memory = op.memory_bytes / partial.len() as i64;
            for item in &mut partial {
                item.TableMemUsage = memory;
            }
            operatorMetrics.extend(partial);
        }
        _ => {}
    }
    for child in &op.children {
        operatorMetrics = extractMetricsFromOperatorTree(child, operatorMetrics)?;
    }
    Ok(operatorMetrics)
}

/// 构造仅含库表名、扫描时间与内存的指标条目。
fn metric(db: String, table: String, scan: Duration, memory: i64) -> TableReadCostMetrics {
    TableReadCostMetrics {
        DbName: CIStr::new(db),
        TableName: CIStr::new(table),
        TableScanTime: scan,
        TableMemUsage: memory,
        ..Default::default()
    }
}

/// 判断算子子树是否落在 TiFlash（MPP task）路径上。
pub fn checkTiFlashOperator(op: &ExplainOperator) -> bool {
    op.children
        .iter()
        .any(|child| child.task_type.eq_ignore_ascii_case("mpp") || checkTiFlashOperator(child))
}

/// 从访问对象或其 dynamic 子项中取出库表名。
pub fn extractTableNameFromAccessObject(accessObject: &AccessObject) -> (String, String) {
    if !accessObject.database.is_empty() || !accessObject.table.is_empty() {
        return (accessObject.database.clone(), accessObject.table.clone());
    }
    accessObject
        .dynamic
        .iter()
        .find(|item| !item.table.is_empty())
        .map(|item| (item.database.clone(), item.table.clone()))
        .unwrap_or_default()
}

/// 在子树中查找 TableFullScan / TableRangeScan 的访问对象。
pub fn extractTableNameFromChildrenTableScan(op: &ExplainOperator) -> (String, String) {
    for child in &op.children {
        if matches!(
            extractOperatorTypeFromName(&child.name).as_deref(),
            Ok("TableFullScan" | "TableRangeScan")
        ) {
            return child
                .access_objects
                .first()
                .map(extractTableNameFromAccessObject)
                .unwrap_or_default();
        }
        let result = extractTableNameFromChildrenTableScan(child);
        if !result.0.is_empty() && !result.1.is_empty() {
            return result;
        }
    }
    Default::default()
}

/// 在子树中查找 IndexRangeScan / IndexFullScan 的访问对象。
pub fn extractTableNameFromIndexScan(op: &ExplainOperator) -> (String, String) {
    for child in &op.children {
        if matches!(
            extractOperatorTypeFromName(&child.name).as_deref(),
            Ok("IndexRangeScan" | "IndexFullScan")
        ) {
            return child
                .access_objects
                .first()
                .map(extractTableNameFromAccessObject)
                .unwrap_or_default();
        }
        let result = extractTableNameFromIndexScan(child);
        if !result.0.is_empty() && !result.1.is_empty() {
            return result;
        }
    }
    Default::default()
}

/// 从 IndexMerge 子树提取各部分索引扫描的指标。
pub fn extractPartialMetricsFromChildrenIndexMerge(
    op: &ExplainOperator,
) -> Result<Vec<TableReadCostMetrics>, String> {
    if op.children.is_empty() {
        return Ok(Vec::new());
    }
    // 子节点全是索引扫描时，直接在本层提取。
    if op.children.iter().all(|child| {
        matches!(
            extractOperatorTypeFromName(&child.name).as_deref(),
            Ok("IndexRangeScan" | "IndexFullScan")
        )
    }) {
        return op
            .children
            .iter()
            .map(|child| {
                let (db, table) = child
                    .access_objects
                    .first()
                    .map(extractTableNameFromAccessObject)
                    .unwrap_or_default();
                if db.is_empty() || table.is_empty() {
                    return Err("index scan missing access object".into());
                }
                Ok(metric(
                    db,
                    table,
                    extractScanTimeFromExecutionInfo(child)?,
                    0,
                ))
            })
            .collect();
    }
    for child in &op.children {
        let result = extractPartialMetricsFromChildrenIndexMerge(child)?;
        if !result.is_empty() {
            return Ok(result);
        }
    }
    Ok(Vec::new())
}

/// 从 `Type_ID` 形式的算子名中解析算子类型部分。
pub fn extractOperatorTypeFromName(name: &str) -> Result<String, String> {
    let (operator, id) = name
        .rsplit_once('_')
        .ok_or_else(|| format!("failed to extract operator type from operator name: {name}"))?;
    if operator.is_empty() || id.is_empty() || operator.contains('_') {
        return Err(format!(
            "failed to extract operator type from operator name: {name}"
        ));
    }
    Ok(operator.into())
}

/// 从算子的各类执行信息字符串中提取扫描时间；全空则返回零时长。
pub fn extractScanTimeFromExecutionInfo(op: &ExplainOperator) -> Result<Duration, String> {
    let mut scan_time = Duration::ZERO;
    let mut last_error = None;
    for input in [
        (!op.root_basic_exec_info.is_empty()).then_some(op.root_basic_exec_info.as_str()),
        op.root_group_exec_info.first().map(String::as_str),
        (!op.cop_exec_info.is_empty()).then_some(op.cop_exec_info.as_str()),
    ] {
        if scan_time.is_zero()
            && let Some(input) = input
        {
            match extractScanTimeFromString(input) {
                Ok(duration) => scan_time = duration,
                Err(error) => last_error = Some(error),
            }
        }
    }
    if !scan_time.is_zero() {
        Ok(scan_time)
    } else {
        last_error.map_or(Ok(Duration::ZERO), Err)
    }
}

/// 从形如 `time:274.5µs, loops:1` 的执行信息中解析 `time:` 字段。
pub fn extractScanTimeFromString(input: &str) -> Result<Duration, String> {
    let value = input
        .split_once("time:")
        .ok_or("'time:' not found in input string")?
        .1
        .split_once(',')
        .ok_or("',' not found after 'time:'")?
        .0
        .trim();
    parseDuration(value)
}

/// 按 Go `time.ParseDuration` 的正时长语法解析复合单位片段。
fn parseDuration(value: &str) -> Result<Duration, String> {
    let value = value.trim();
    if value == "0" {
        return Ok(Duration::ZERO);
    }
    if value.is_empty() {
        return Err("failed to parse duration: empty value".into());
    }
    let mut rest = value;
    let mut total_nanos = 0.0_f64;
    while !rest.is_empty() {
        let number_end = rest
            .char_indices()
            .take_while(|(_, ch)| ch.is_ascii_digit() || *ch == '.')
            .map(|(index, ch)| index + ch.len_utf8())
            .last()
            .ok_or_else(|| format!("failed to parse duration: {value}"))?;
        let number = rest[..number_end]
            .parse::<f64>()
            .map_err(|_| format!("failed to parse duration: {value}"))?;
        rest = &rest[number_end..];
        let (unit_len, factor) = [
            ("ns", 1.0),
            ("us", 1_000.0),
            ("µs", 1_000.0),
            ("ms", 1_000_000.0),
            ("s", 1_000_000_000.0),
            ("m", 60_000_000_000.0),
            ("h", 3_600_000_000_000.0),
        ]
        .into_iter()
        .find_map(|(unit, factor)| rest.starts_with(unit).then_some((unit.len(), factor)))
        .ok_or_else(|| format!("failed to parse duration: {value}"))?;
        total_nanos += number * factor;
        rest = &rest[unit_len..];
        if !total_nanos.is_finite() || total_nanos > u64::MAX as f64 {
            return Err(format!("failed to parse duration: {value}"));
        }
    }
    Ok(Duration::from_nanos(total_nanos.round() as u64))
}

/// AST/计划遍历用的节点：表名节点或其他。
#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    TableName { schema: String, table: String },
    Other,
}

/// 从节点流中收集出现过的数据库名（小写）。
#[derive(Default)]
pub struct DBNameExtractor {
    pub DBs: HashSet<String>,
}
impl DBNameExtractor {
    /// 进入节点：若为表名则记录 schema。
    pub fn Enter(&mut self, node: &Node) -> bool {
        if let Node::TableName { schema, .. } = node {
            self.DBs.insert(schema.to_ascii_lowercase());
        }
        false
    }
    /// 离开节点：始终继续遍历。
    pub fn Leave(&self, _node: &Node) -> bool {
        true
    }
}
