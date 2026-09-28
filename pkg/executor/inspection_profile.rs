// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// 指标剖析（metric profile）构建器：将 metrics_schema 时间序列聚合成 DOT 图。
//
// 按固定的 TiDB 查询链路 / GC 链路树形结构，查询各阶段耗时或次数，
// 输出 Graphviz digraph，用于可视化 SQL 执行与存储层各环节占比。
// Profile 这里指基于监控指标的火焰图式剖析，而非 CPU pprof。

#![allow(dead_code, non_camel_case_types, non_snake_case)]

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// 指标树节点的共享可变引用（父子树用 Rc 共享）。
type NodeRef = Rc<RefCell<metricNode>>;

#[derive(Clone, Debug, Eq, PartialEq)]
/// 剖析构建过程中的错误（如未知 value_type、SQL 失败）。
pub struct ProfileError(pub String);

impl ProfileError {
    /// 由错误消息构造 ProfileError。
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ProfileError {}

/// 剖析操作的统一 Result 别名。
pub type ProfileResult<T = ()> = Result<T, ProfileError>;

#[derive(Clone, Debug, PartialEq)]
/// 单次 metrics SQL 查询返回的一行：数值 + 标签值列表。
pub struct MetricQueryRow {
    pub value: f64,
    pub labels: Vec<String>,
}

/// 剖析所需的数据源：执行 metrics SQL、取表注释、格式化时间。
pub trait ProfileDataSource: Send + Sync {
    fn execute_metric_sql(&self, sql: &str) -> ProfileResult<Vec<MetricQueryRow>>;
    fn metric_comment(&self, metric_table: &str) -> ProfileResult<Option<String>>;
    fn format_metric_time(&self, time: SystemTime) -> ProfileResult<String>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(i32)]
/// 节点展示的聚合类型：总和 / 平均 / 次数。
pub enum metricValueType {
    Sum = 1,
    Avg = 2,
    Count = 3,
}

impl metricValueType {
    /// 转为小写类型名字符串。
    pub fn String(self) -> String {
        match self {
            Self::Avg => "avg",
            Self::Count => "count",
            Self::Sum => "sum",
        }
        .to_owned()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
/// 某指标（或某标签组合）的聚合值：sum/count 与分位平均耗时。
pub struct metricValue {
    pub sum: f64,
    pub count: i64,
    pub avg_p99: f64,
    pub avg_p90: f64,
    pub avg_p80: f64,
    pub comment: String,
}

impl metricValue {
    /// 按当前 value_type 取出展示用数值；NaN 归一为 0。
    pub fn getValue(&self, value_type: metricValueType) -> f64 {
        let value = match value_type {
            metricValueType::Count => return self.count as f64,
            metricValueType::Sum => self.sum,
            metricValueType::Avg => {
                if self.count == 0 {
                    return 0.0;
                }
                self.sum / self.count as f64
            }
        };
        if value.is_nan() { 0.0 } else { value }
    }

    /// 生成节点 tooltip：基础注释 + total/avg/分位耗时摘要。
    pub fn getComment(&self) -> String {
        if self.count == 0 {
            return String::new();
        }
        let mut buffer = String::with_capacity(64);
        buffer.push_str(&self.comment);
        buffer.push_str("\n\n");
        buffer.push_str("total_time: ");
        buffer.push_str(&format_go_duration(self.sum));
        buffer.push('\n');
        buffer.push_str("total_count: ");
        buffer.push_str(&self.count.to_string());
        buffer.push('\n');
        buffer.push_str("avg_time: ");
        buffer.push_str(&format_go_duration(self.sum / self.count as f64));
        buffer.push('\n');
        buffer.push_str("avgP99: ");
        buffer.push_str(&format_go_duration(self.avg_p99));
        buffer.push('\n');
        buffer.push_str("avgP90: ");
        buffer.push_str(&format_go_duration(self.avg_p90));
        buffer.push('\n');
        buffer.push_str("avgP80: ");
        buffer.push_str(&format_go_duration(self.avg_p80));
        buffer
    }
}

#[derive(Clone)]
/// 剖析树中的一个指标节点，可挂标签维与子节点。
pub struct metricNode {
    pub table: String,
    pub name: String,
    pub labels: Vec<String>,
    pub condition: String,
    pub label_value: BTreeMap<String, metricValue>,
    pub value: metricValue,
    pub unit: i64,
    pub children: Vec<NodeRef>,
    pub is_part_of_parent: bool,
    pub initialized: bool,
}

impl metricNode {
    /// 以 metrics 表名为核心创建空节点。
    fn new(table: &str) -> NodeRef {
        Rc::new(RefCell::new(Self {
            table: table.to_owned(),
            name: String::new(),
            labels: Vec::new(),
            condition: String::new(),
            label_value: BTreeMap::new(),
            value: metricValue::default(),
            unit: 0,
            children: Vec::new(),
            is_part_of_parent: false,
            initialized: false,
        }))
    }

    /// 节点显示名：优先自定义 name，否则用 table；可附加标签后缀。
    pub fn getName(&self, label: &str) -> String {
        let mut name = if self.name.is_empty() {
            self.table.clone()
        } else {
            self.name.clone()
        };
        if !label.is_empty() {
            name.push('.');
            name.push_str(label);
        }
        name
    }

    /// 懒初始化后返回节点聚合值。
    pub fn getValue(&mut self, builder: &profileBuilder) -> ProfileResult<metricValue> {
        if !self.initialized {
            self.initialized = true;
            self.initializeMetricValue(builder)?;
        }
        Ok(self.value.clone())
    }

    /// 按标签键取/建子聚合槽。
    pub fn getLabelValue(&mut self, label: &str) -> &mut metricValue {
        self.label_value.entry(label.to_owned()).or_default()
    }

    /// 执行 SQL 并按标签拼接键返回 (label, value)；按 unit 缩放。
    pub fn queryRowsByLabel(
        &self,
        builder: &profileBuilder,
        query: &str,
    ) -> ProfileResult<Vec<(String, f64)>> {
        let rows = builder.data_source.execute_metric_sql(query)?;
        let mut values = Vec::with_capacity(rows.len());
        for row in rows {
            let mut value = row.value;
            if self.unit != 0 {
                value /= self.unit as f64;
            }
            let label = row.labels.join(",");
            if label.is_empty() && !self.labels.is_empty() {
                continue;
            }
            values.push((label, value));
        }
        Ok(values)
    }

    /// 从 metrics_schema 拉取 count/sum/分位，填充 value 与 label_value。
    pub fn initializeMetricValue(&mut self, builder: &profileBuilder) -> ProfileResult {
        self.label_value.clear();
        self.value = metricValue::default();
        // 时间窗口 + 正值过滤；节点可追加额外条件。
        let mut condition = format!(
            "where time >= '{}' and time <= '{}' and value is not null and value>0",
            builder.data_source.format_metric_time(builder.start)?,
            builder.data_source.format_metric_time(builder.end)?
        );
        if !self.condition.is_empty() {
            condition.push_str(" and ");
            condition.push_str(&self.condition);
        }

        // 总次数：*_total_count
        let count_query = metric_query(
            "sum",
            &format!("{}_total_count", self.table),
            &condition,
            &self.labels,
        );
        let mut total_count = 0.0;
        for (label, value) in self.queryRowsByLabel(builder, &count_query)? {
            total_count += value;
            self.getLabelValue(&label).count = value as i64;
        }
        if total_count as i64 == 0 {
            return Ok(());
        }
        self.value.count = total_count as i64;

        // 总耗时：*_total_time（可能按 unit 换算为秒）
        let sum_query = metric_query(
            "sum",
            &format!("{}_total_time", self.table),
            &condition,
            &self.labels,
        );
        let mut total_sum = 0.0;
        for (label, mut value) in self.queryRowsByLabel(builder, &sum_query)? {
            if self.unit != 0 {
                value /= self.unit as f64;
            }
            total_sum += value;
            self.getLabelValue(&label).sum = value;
        }
        self.value.sum = total_sum;

        // 分位耗时：*_duration 且带 quantile 过滤
        for quantile in [0.99_f64, 0.90, 0.80] {
            let quantile_condition = format!("{condition} and quantile={quantile}");
            let query = metric_query(
                "avg",
                &format!("{}_duration", self.table),
                &quantile_condition,
                &self.labels,
            );
            let mut total_value = 0.0;
            let mut count = 0_u64;
            for (label, mut value) in self.queryRowsByLabel(builder, &query)? {
                if self.unit != 0 {
                    value /= self.unit as f64;
                }
                total_value += value;
                count += 1;
                set_quantile_value(self.getLabelValue(&label), quantile, value);
            }
            let average = total_value / count as f64;
            set_quantile_value(&mut self.value, quantile, average);
        }

        if let Some(comment) = builder
            .data_source
            .metric_comment(&format!("{}_total_time", self.table))?
        {
            self.value.comment = comment.clone();
            let label_names = self.labels.join(",");
            for (label, value) in &mut self.label_value {
                value.comment = format!("{comment}, the label of [{label_names}] is [{label}]");
            }
        }
        Ok(())
    }
}

/// DOT 剖析图构建器：持有时间窗、聚合类型与输出缓冲。
pub struct profileBuilder {
    pub id_map: BTreeMap<String, u64>,
    pub id_allocator: u64,
    pub total_value: f64,
    pub unique_map: BTreeSet<String>,
    pub buffer: String,
    pub start: SystemTime,
    pub end: SystemTime,
    pub value_type: metricValueType,
    pub data_source: Arc<dyn ProfileDataSource>,
}

/// 构造 builder；value_type 接受 sum/avg/count（空串视为 sum）。
pub fn NewProfileBuilder(
    data_source: Arc<dyn ProfileDataSource>,
    start: SystemTime,
    end: SystemTime,
    value_type: &str,
) -> ProfileResult<profileBuilder> {
    let value_type = match value_type.to_ascii_lowercase().as_str() {
        "sum" | "" => metricValueType::Sum,
        "avg" => metricValueType::Avg,
        "count" => metricValueType::Count,
        other => {
            return Err(ProfileError::new(format!(
                "unknown metric profile type: {other}, expect value should be one of 'sum', 'avg' or 'count'"
            )));
        }
    };
    Ok(profileBuilder {
        id_map: BTreeMap::new(),
        id_allocator: 1,
        total_value: 0.0,
        unique_map: BTreeSet::new(),
        buffer: String::with_capacity(1024),
        start,
        end,
        value_type,
        data_source,
    })
}

impl profileBuilder {
    /// 生成查询树与 GC 树并遍历写入 DOT 内容。
    pub fn Collect(&mut self) -> ProfileResult {
        let query_tree = self.genTiDBQueryTree();
        self.init(Some(&query_tree), "tidb_query")?;
        self.traversal(Some(&query_tree))?;
        let gc_tree = self.genTiDBGCTree();
        self.traversal(Some(&gc_tree))
    }

    /// 闭合 digraph 并返回字节内容。
    pub fn Build(&mut self) -> Vec<u8> {
        self.buffer.push('}');
        self.buffer.as_bytes().to_vec()
    }

    /// 为节点名分配稳定数字 ID（DOT 中用 N{id}）。
    pub fn getNameID(&mut self, name: &str) -> u64 {
        if let Some(id) = self.id_map.get(name) {
            return *id;
        }
        let id = self.id_allocator;
        self.id_allocator += 1;
        self.id_map.insert(name.to_owned(), id);
        id
    }

    /// 写入 digraph 头与类型/时间子图，并记录 total_value 作占比分母。
    pub fn init(&mut self, total: Option<&NodeRef>, name: &str) -> ProfileResult {
        let Some(total) = total else {
            return Ok(());
        };
        let value_name = match self.value_type {
            metricValueType::Avg => "avg_time",
            metricValueType::Count => "total_count",
            metricValueType::Sum => "total_time",
        };
        self.buffer.push_str("digraph \"tidb_profile\" {\n");
        self.buffer
            .push_str("node [style=filled fillcolor=\"#f8f8f8\"]\n");
        self.buffer.push_str(&format!(
            "subgraph {0}_{1} {{ \"{0}_{1}\" [shape=box fontsize=16 label=\"Type: {0}_{1}\\lTime: {2}\\lDuration: {3}\\l\"] }}\n",
            name,
            value_name,
            self.data_source.format_metric_time(self.start)?,
            format_system_time_difference(self.end, self.start)
        ));
        let value = self.GetTotalValue(total)?;
        self.total_value = if value != 0.0 { value } else { 1.0 };
        Ok(())
    }

    /// Sum/Count 取根节点值；Avg 取树中最大值以免占比失真。
    pub fn GetTotalValue(&self, root: &NodeRef) -> ProfileResult<f64> {
        match self.value_type {
            metricValueType::Sum | metricValueType::Count => {
                let value = root.borrow_mut().getValue(self)?;
                Ok(value.getValue(self.value_type))
            }
            metricValueType::Avg => self.GetMaxNodeValue(root),
        }
    }

    /// 递归求树（含标签维）上的最大指标值。
    pub fn GetMaxNodeValue(&self, root: &NodeRef) -> ProfileResult<f64> {
        let value = root.borrow_mut().getValue(self)?;
        let mut maximum = value.getValue(self.value_type);
        {
            let node = root.borrow();
            for value in node.label_value.values() {
                maximum = maximum.max(value.getValue(self.value_type));
            }
        }
        let children = root.borrow().children.clone();
        for child in children {
            maximum = maximum.max(self.GetMaxNodeValue(&child)?);
            let node = root.borrow();
            for value in node.label_value.values() {
                maximum = maximum.max(value.getValue(self.value_type));
            }
        }
        Ok(maximum)
    }

    /// DFS 遍历：去重、忽略过小占比、画边与节点后递归子节点。
    pub fn traversal(&mut self, node: Option<&NodeRef>) -> ProfileResult {
        let Some(node) = node else {
            return Ok(());
        };
        let node_name = node.borrow().getName("");
        // 同名节点只输出一次，避免环/共享子树重复展开。
        if !self.unique_map.insert(node_name) {
            return Ok(());
        }
        let node_value = node.borrow_mut().getValue(self)?;
        if self.ignoreFraction(&node_value, self.total_value) {
            return Ok(());
        }
        let children = node.borrow().children.clone();
        let mut total_children_value = 0.0;
        for child in &children {
            let child_value = child.borrow_mut().getValue(self)?;
            self.addNodeEdge(node, child, &child_value);
            // is_part_of_parent 表示子耗时已含于父，不计入 self_cost 扣减。
            if !child.borrow().is_part_of_parent {
                total_children_value += child_value.getValue(self.value_type);
            }
        }
        let node_total = node_value.getValue(self.value_type);
        self.addNode(node, node_total - total_children_value, node_total)?;
        for child in &children {
            self.traversal(Some(child))?;
        }
        Ok(())
    }

    /// 父→子边；父有标签维时从各标签节点连到子。
    pub fn addNodeEdge(&mut self, parent: &NodeRef, child: &NodeRef, value: &metricValue) {
        if self.ignoreFraction(value, self.total_value) {
            return;
        }
        let style = if child.borrow().is_part_of_parent {
            "dotted"
        } else {
            ""
        };
        let child_value = value.getValue(self.value_type);
        if parent.borrow().labels.is_empty() {
            let label = if child.borrow().is_part_of_parent {
                String::new()
            } else {
                self.formatValueByTp(child_value)
            };
            self.addEdge(
                &parent.borrow().getName(""),
                &child.borrow().getName(""),
                &label,
                style,
                child_value,
            );
        } else {
            let label_values = parent.borrow().label_value.clone();
            for (label, label_value) in label_values {
                if self.ignoreFraction(&label_value, self.total_value) {
                    continue;
                }
                self.addEdge(
                    &parent.borrow().getName(&label),
                    &child.borrow().getName(""),
                    "",
                    style,
                    child_value,
                );
            }
        }
    }

    /// 输出节点定义；有标签时额外画标签子节点并把 self 权重减半。
    pub fn addNode(
        &mut self,
        node: &NodeRef,
        mut self_cost: f64,
        node_total: f64,
    ) -> ProfileResult {
        let name = node.borrow().getName("");
        let mut weight = self_cost;
        if !node.borrow().labels.is_empty() {
            let label_values = node.borrow().label_value.clone();
            for (label, value) in label_values {
                if self.ignoreFraction(&value, self.total_value) {
                    continue;
                }
                let numeric_value = value.getValue(self.value_type);
                let value_string = self.formatValueByTp(numeric_value);
                self.addEdge(
                    &node.borrow().getName(""),
                    &node.borrow().getName(&label),
                    &format!(" {value_string}"),
                    "",
                    numeric_value,
                );
                let label_value = format!(
                    "{}\n {} ({:.2}%)",
                    node.borrow().getName(&label),
                    value_string,
                    numeric_value * 100.0 / self.total_value
                );
                self.addNodeDef(
                    &node.borrow().getName(&label),
                    &label_value,
                    &value.getComment(),
                    numeric_value,
                    numeric_value,
                );
            }
            weight = self_cost / 2.0;
            self_cost = 0.0;
        }
        let label = format!(
            "{name}\n {} ({:.2}%)\nof {} ({:.2}%)",
            self.formatValueByTp(self_cost),
            self_cost * 100.0 / self.total_value,
            self.formatValueByTp(node_total),
            node_total * 100.0 / self.total_value
        );
        self.addNodeDef(
            &node.borrow().getName(""),
            &label,
            &node.borrow().value.getComment(),
            weight,
            self_cost,
        );
        Ok(())
    }

    /// 写入单个 DOT 节点：字号与颜色随权重相对 total_value 变化。
    pub fn addNodeDef(
        &mut self,
        name: &str,
        label_value: &str,
        comment: &str,
        font_weight: f64,
        color_weight: f64,
    ) {
        let mut font_size =
            5 + (18.0 * (font_weight.abs() / self.total_value).sqrt()).ceil() as i32;
        font_size = font_size.min(64);
        let id = self.getNameID(name);
        let foreground = self.dotColor(color_weight / self.total_value, false);
        let background = self.dotColor(color_weight / self.total_value, true);
        self.buffer.push_str(&format!(
            "N{id} [label=\"{label_value}\" tooltip=\"{comment}\" fontsize={font_size} shape=box color=\"{foreground}\" fillcolor=\"{background}\"]\n"
        ));
    }

    /// 写入 DOT 边，weight/color 反映相对占比。
    pub fn addEdge(&mut self, from: &str, to: &str, label: &str, style: &str, value: f64) {
        let weight = 1 + (value * 100.0 / self.total_value).min(100.0) as i32;
        let color = self.dotColor(value / self.total_value, false);
        let from_id = self.getNameID(from);
        let to_id = self.getNameID(to);
        self.buffer.push_str(&format!("N{from_id} -> N{to_id} ["));
        if !label.is_empty() {
            self.buffer.push_str(&format!(" label=\"{label}\" "));
        }
        if !style.is_empty() {
            self.buffer.push_str(&format!(" style=\"{style}\" "));
        }
        self.buffer
            .push_str(&format!(" weight={weight} color=\"{color}\"]\n"));
    }

    /// 占比低于 0.01% 的节点/边忽略，降低图噪声。
    pub fn ignoreFraction(&self, value: &metricValue, total: f64) -> bool {
        value.getValue(self.value_type) * 100.0 / total < 0.01
    }

    /// 按 value_type 格式化展示：次数为整数，耗时为 s/ms。
    pub fn formatValueByTp(&self, value: f64) -> String {
        match self.value_type {
            metricValueType::Count => (value as i64).to_string(),
            metricValueType::Sum | metricValueType::Avg => {
                if value.is_nan() {
                    return String::new();
                }
                if value.abs() > 1.0 {
                    format!("{value:.2}s")
                } else if (value * 1_000.0).abs() > 1.0 {
                    format!("{:.2} ms", value * 1_000.0)
                } else if (value * 1_000_000.0).abs() > 1.0 {
                    // Keep the Go implementation's historical millisecond suffix here.
                    format!("{:.2} ms", value * 1_000_000.0)
                } else {
                    format_go_duration(value)
                }
            }
        }
    }

    /// 将 [-1,1] 分数映射为红-绿渐变十六进制颜色（对齐 Go pprof 配色思路）。
    pub fn dotColor(&self, mut score: f64, is_background: bool) -> String {
        const SHIFT: f64 = 0.7;
        let (mut saturation, value) = if is_background {
            (0.1, 0.93)
        } else {
            (1.0, 0.7)
        };
        score = score.clamp(-1.0, 1.0);
        if score.abs() < 0.2 {
            saturation *= score.abs() / 0.2;
        }
        if score > 0.0 {
            score = score.powf(1.0 - SHIFT);
        } else if score < 0.0 {
            score = -(-score).powf(1.0 - SHIFT);
        }
        let (red, green) = if score < 0.0 {
            (value * (1.0 + saturation * score), value)
        } else {
            (value, value * (1.0 - saturation * score))
        };
        let blue = value * (1.0 - saturation);
        format!(
            "#{:02x}{:02x}{:02x}",
            (red * 255.0) as u8,
            (green * 255.0) as u8,
            (blue * 255.0) as u8
        )
    }

    /// 构造 GC 相关指标子树（含 kv_request）。
    pub fn genTiDBGCTree(&self) -> NodeRef {
        let request = metricNode::new("tidb_kv_request");
        request.borrow_mut().is_part_of_parent = true;
        let gc = metricNode::new("tidb_gc");
        {
            let mut gc = gc.borrow_mut();
            gc.is_part_of_parent = true;
            gc.labels = vec!["stage".to_owned()];
            gc.children = vec![request];
        }
        gc
    }

    /// 构造完整查询链路树：parse/compile/execute → cop/txn/ddl → TiKV gRPC 等。
    pub fn genTiDBQueryTree(&self) -> NodeRef {
        let kv_request = metricNode::new("tidb_kv_request");
        {
            let mut node = kv_request.borrow_mut();
            node.is_part_of_parent = true;
            node.labels = vec!["type".to_owned()];
            node.children = vec![
                node_with_unit("tidb_batch_client_wait", 1_000_000_000),
                metricNode::new("tidb_batch_client_wait_conn"),
                metricNode::new("tidb_batch_client_unavailable"),
                node_with_condition(
                    "pd_client_cmd",
                    "type not in ('tso','wait','tso_async_wait')",
                ),
                tikv_grpc_tree(),
            ];
        }

        let ddl = metricNode::new("tidb_ddl");
        ddl.borrow_mut().labels = vec!["type".to_owned()];
        let ddl_worker = metricNode::new("tidb_ddl_worker");
        {
            let mut worker = ddl_worker.borrow_mut();
            worker.labels = vec!["type".to_owned()];
            worker.children = [
                "tidb_ddl_batch_add_index",
                "tidb_load_schema",
                "tidb_ddl_update_self_version",
                "tidb_owner_handle_syncer",
                "tidb_meta_operation",
            ]
            .into_iter()
            .map(metricNode::new)
            .collect();
        }
        ddl.borrow_mut().children = vec![ddl_worker];

        let execute = metricNode::new("tidb_execute");
        let cop = metricNode::new("tidb_cop");
        cop.borrow_mut().is_part_of_parent = true;
        let cop_backoff = labeled_partial_node("tidb_kv_backoff", "type");
        cop.borrow_mut().children = vec![cop_backoff, Rc::clone(&kv_request)];
        let txn_command = metricNode::new("tidb_txn_cmd");
        txn_command.borrow_mut().labels = vec!["type".to_owned()];
        let txn_backoff = labeled_partial_node("tidb_kv_backoff", "type");
        txn_command.borrow_mut().children = vec![txn_backoff, kv_request];
        execute.borrow_mut().children = vec![
            metricNode::new("pd_start_tso_wait"),
            metricNode::new("tidb_auto_id_request"),
            cop,
            txn_command,
            ddl,
        ];

        let query = metricNode::new("tidb_query");
        query.borrow_mut().labels = vec!["sql_type".to_owned()];
        query.borrow_mut().children = vec![
            node_with_unit("tidb_get_token", 1_000_000),
            metricNode::new("tidb_parse"),
            metricNode::new("tidb_compile"),
            execute,
        ];
        query
    }
}

/// 拼 metrics_schema 聚合 SQL；有标签则 GROUP BY。
fn metric_query(aggregation: &str, table: &str, condition: &str, labels: &[String]) -> String {
    if labels.is_empty() {
        format!("select {aggregation}(value), '' from `metrics_schema`.`{table}` {condition}")
    } else {
        let labels = labels.join("`,`");
        format!(
            "select {aggregation}(value), `{labels}` from `metrics_schema`.`{table}` {condition} group by `{labels}` having sum(value) > 0"
        )
    }
}

/// 将分位结果写入 avg_p99/p90/p80 字段。
fn set_quantile_value(value: &mut metricValue, quantile: f64, metric: f64) {
    if quantile == 0.99 {
        value.avg_p99 = metric;
    } else if quantile == 0.90 {
        value.avg_p90 = metric;
    } else if quantile == 0.80 {
        value.avg_p80 = metric;
    }
}

/// 创建带单位换算因子的节点（如纳秒→秒）。
fn node_with_unit(table: &str, unit: i64) -> NodeRef {
    let node = metricNode::new(table);
    node.borrow_mut().unit = unit;
    node
}

/// 创建带额外 WHERE 片段的节点。
fn node_with_condition(table: &str, condition: &str) -> NodeRef {
    let node = metricNode::new(table);
    node.borrow_mut().condition = condition.to_owned();
    node
}

/// 创建带标签且标记为父耗时组成部分的节点。
fn labeled_partial_node(table: &str, label: &str) -> NodeRef {
    let node = metricNode::new(table);
    {
        let mut node = node.borrow_mut();
        node.labels = vec![label.to_owned()];
        node.is_part_of_parent = true;
    }
    node
}

/// TiKV gRPC 侧子树：coprocessor、scheduler、storage/raft、GC tasks。
fn tikv_grpc_tree() -> NodeRef {
    let grpc = metricNode::new("tikv_grpc_message");
    let cop_request = metricNode::new("tikv_cop_request");
    let cop_wait = node_with_condition("tikv_cop_wait", "type != 'all'");
    cop_wait.borrow_mut().labels = vec!["type".to_owned()];
    cop_request.borrow_mut().children = vec![cop_wait, metricNode::new("tikv_cop_handle")];

    let scheduler = metricNode::new("tikv_scheduler_command");
    let storage = metricNode::new("tikv_storage_async_request");
    let snapshot = node_with_condition("tikv_storage_async_request", "type='snapshot'");
    snapshot.borrow_mut().name = "tikv_storage_async_request.snapshot".to_owned();
    let write = node_with_condition("tikv_storage_async_request", "type='write'");
    write.borrow_mut().name = "tikv_storage_async_request.write".to_owned();
    let raft_process = metricNode::new("tikv_raftstore_process");
    raft_process.borrow_mut().children = vec![metricNode::new("tikv_raftstore_append_log")];
    write.borrow_mut().children = vec![
        metricNode::new("tikv_raftstore_propose_wait"),
        raft_process,
        metricNode::new("tikv_raftstore_commit_log"),
        metricNode::new("tikv_raftstore_apply_wait"),
        metricNode::new("tikv_raftstore_apply_log"),
    ];
    storage.borrow_mut().children = vec![snapshot, write];
    scheduler.borrow_mut().children = vec![
        metricNode::new("tikv_scheduler_latch_wait"),
        metricNode::new("tikv_scheduler_processing_read"),
        storage,
    ];
    let gc_tasks = metricNode::new("tikv_gc_tasks");
    gc_tasks.borrow_mut().labels = vec!["task".to_owned()];
    grpc.borrow_mut().children = vec![cop_request, scheduler, gc_tasks];
    grpc
}

/// Unix 秒转为 `YYYY-MM-DD HH:MM:SS`（本地 civil 日期算法）。
fn format_unix_datetime(seconds: u64) -> String {
    let days = seconds / 86_400;
    let seconds_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        seconds_of_day / 3_600,
        seconds_of_day % 3_600 / 60,
        seconds_of_day % 60
    )
}

/// 从 Unix epoch 起的天数换算公历年/月/日（Howard Hinnant 算法）。
fn civil_from_days(days_since_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

/// Duration 格式化为类 Go duration 字符串。
fn format_duration(duration: Duration) -> String {
    format_go_duration_nanos(
        i128::from(duration.as_secs()) * 1_000_000_000 + i128::from(duration.subsec_nanos()),
    )
}

/// 将秒数格式化为 Go 风格 duration（ns/µs/ms/s/m/h）。
fn format_go_duration(seconds: f64) -> String {
    if !seconds.is_finite() {
        return seconds.to_string();
    }
    format_go_duration_nanos((seconds * 1_000_000_000.0).trunc() as i128)
}

/// 计算两个时间点的有符号差值，保持 Go `Time.Sub` 的负时长行为。
fn format_system_time_difference(end: SystemTime, start: SystemTime) -> String {
    match end.duration_since(start) {
        Ok(duration) => format_duration(duration),
        Err(error) => {
            let duration = error.duration();
            format_go_duration_nanos(
                -(i128::from(duration.as_secs()) * 1_000_000_000
                    + i128::from(duration.subsec_nanos())),
            )
        }
    }
}

fn format_go_duration_nanos(nanos: i128) -> String {
    if nanos == 0 {
        return "0s".to_owned();
    }
    let negative = nanos < 0;
    let nanos = nanos.unsigned_abs();
    let prefix = if negative { "-" } else { "" };
    if nanos < 1_000 {
        return format!("{prefix}{nanos}ns");
    }
    if nanos < 1_000_000 {
        return format_decimal_with_suffix(prefix, nanos as f64 / 1_000.0, "µs", 3);
    }
    if nanos < 1_000_000_000 {
        return format_decimal_with_suffix(prefix, nanos as f64 / 1_000_000.0, "ms", 6);
    }
    let total_seconds = nanos as f64 / 1_000_000_000.0;
    if total_seconds < 60.0 {
        return format_decimal_with_suffix(prefix, total_seconds, "s", 9);
    }
    let hours = nanos / 3_600_000_000_000;
    let minutes = nanos % 3_600_000_000_000 / 60_000_000_000;
    let remaining_nanos = nanos % 60_000_000_000;
    let remaining =
        format_decimal_with_suffix("", remaining_nanos as f64 / 1_000_000_000.0, "s", 9);
    if hours > 0 {
        format!("{prefix}{hours}h{minutes}m{remaining}")
    } else {
        format!("{prefix}{minutes}m{remaining}")
    }
}

/// 定点小数去尾零后拼接单位后缀。
fn format_decimal_with_suffix(prefix: &str, value: f64, suffix: &str, precision: usize) -> String {
    let mut number = format!("{value:.precision$}");
    while number.ends_with('0') {
        number.pop();
    }
    if number.ends_with('.') {
        number.pop();
    }
    format!("{prefix}{number}{suffix}")
}
