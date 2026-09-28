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

// 火焰图（flamegraph）树构建与行输出。
//
// 将 pprof `Profile` 的采样栈折叠为 DAG，再按累计值排序展开为文本树行
//（`Datum` 六列：标识、占比、父占比、根子序号、深度、文件行）。
// 对应 Go `pkg/util/profile` 中火焰图相关逻辑；供 `Collector` 展示
// performance_schema 性能剖析结果。

use std::collections::HashMap;

use pprof::protos::{Function, Location, Profile, Sample};
use types::datum::{Datum, NewIntDatum, NewStringDatum};

/// 火焰图展开后的结果行集合（每行六个 `Datum`）。
pub(crate) type DatumRows = Vec<Vec<Datum>>;

/// 对 pprof Profile 的 location / function / string_table 建立 id 索引，加速查名。
pub(crate) struct ProfileIndex<'profile> {
    locations: HashMap<u64, &'profile Location>,
    functions: HashMap<u64, &'profile Function>,
    strings: &'profile [String],
}

impl<'profile> ProfileIndex<'profile> {
    /// 从 Profile 构建 id→对象 映射与字符串表引用。
    pub(crate) fn new(profile: &'profile Profile) -> Self {
        Self {
            locations: profile
                .location
                .iter()
                .map(|location| (location.id, location))
                .collect(),
            functions: profile
                .function
                .iter()
                .map(|function| (function.id, function))
                .collect(),
            strings: &profile.string_table,
        }
    }

    /// 按 location id 取 Location；调用方保证 id 合法。
    fn location(&self, id: u64) -> &'profile Location {
        self.locations[&id]
    }

    /// 解析 location 对应的函数名与 `文件:行号`；无行信息时返回 `<unknown>`。
    fn location_name(&self, id: u64) -> (String, String) {
        let location = self.location(id);
        let Some(line) = location.line.first() else {
            return ("<unknown>".to_string(), "<unknown>".to_string());
        };
        let function = self.functions[&line.function_id];
        let name = self.strings[function.name as usize].clone();
        let filename = &self.strings[function.filename as usize];
        (name, format!("{filename}:{}", line.line))
    }
}

/// 火焰图 DAG 节点：子节点按 location id 索引，累计采样值与函数名。
pub(crate) struct FlamegraphNode {
    children: HashMap<u64, Box<FlamegraphNode>>,
    name: String,
    cumulative_value: i64,
}

/// 创建空的火焰图根/子节点。
pub(crate) fn new_flamegraph_node() -> Box<FlamegraphNode> {
    Box::new(FlamegraphNode {
        cumulative_value: 0,
        children: HashMap::new(),
        name: String::new(),
    })
}

impl FlamegraphNode {
    // Add the value from a sample into the flamegraph DAG. Like Go, callers
    // invoke this only on the root node and use the sample's last value.
    /// 将一条采样累加进 DAG：取最后一个 value，再沿 location 栈自叶向根递归。
    pub(crate) fn add(&mut self, sample: &Sample, index: &ProfileIndex<'_>) {
        let value = *sample
            .value
            .last()
            .expect("validated pprof samples always contain a value");
        if value == 0 {
            return;
        }
        self.add_locations(&sample.location_id, value, index);
    }

    /// pprof location_id 末项是叶子；先累加本节点，再向下层（调用方）递归。
    fn add_locations(&mut self, locations: &[u64], value: i64, index: &ProfileIndex<'_>) {
        self.cumulative_value += value;
        let Some((&location_id, parent_locations)) = locations.split_last() else {
            return;
        };

        // 首次见到该 location 时填充函数名，再继续向父栈帧累加。
        let child = self.children.entry(location_id).or_insert_with(|| {
            let mut child = new_flamegraph_node();
            let location = index.location(location_id);
            if let Some(line) = location.line.first()
                && let Some(function) = index.functions.get(&line.function_id)
            {
                child.name = index.strings[function.name as usize].clone();
            }
            child
        });
        child.add_locations(parent_locations, value, index);
    }

    /// 子节点按累计值降序、同值按 location_id 升序，保证输出稳定。
    fn sorted_children(&self) -> Vec<FlamegraphNodeWithLocation<'_>> {
        let mut children: Vec<_> = self
            .children
            .iter()
            .map(|(location_id, node)| FlamegraphNodeWithLocation {
                node,
                location_id: *location_id,
            })
            .collect();
        children.sort_by(|left, right| {
            right
                .node
                .cumulative_value
                .cmp(&left.node.cumulative_value)
                .then_with(|| left.location_id.cmp(&right.location_id))
        });
        children
    }
}

/// 遍历时携带 location_id，便于查函数名与文件行。
struct FlamegraphNodeWithLocation<'node> {
    node: &'node FlamegraphNode,
    location_id: u64,
}

/// 将火焰图 DAG 展开为带缩进的 Datum 行；`total`/`root_child` 用于占比与分组序号。
pub(crate) struct FlamegraphCollector<'profile> {
    index: ProfileIndex<'profile>,
    rows: DatumRows,
    total: i64,
    root_child: i64,
}

/// 基于 Profile 构造收集器（索引已建好，行缓冲为空）。
pub(crate) fn new_flamegraph_collector(profile: &Profile) -> FlamegraphCollector<'_> {
    FlamegraphCollector {
        index: ProfileIndex::new(profile),
        rows: Vec::new(),
        total: 0,
        root_child: 0,
    }
}

/// 组装火焰图一行的六个 Datum 列。
fn make_row(
    identifier: String,
    profile_percent: String,
    parent_percent: String,
    root_child: i64,
    depth: i64,
    file_line: String,
) -> Vec<Datum> {
    vec![
        NewStringDatum(identifier),
        NewStringDatum(profile_percent),
        NewStringDatum(parent_percent),
        NewIntDatum(root_child),
        NewIntDatum(depth),
        NewStringDatum(file_line),
    ]
}

impl FlamegraphCollector<'_> {
    /// 递归输出子树：树形标识、相对总量/父节点占比，并下钻排序后的子节点。
    fn collect_child(
        &mut self,
        node: FlamegraphNodeWithLocation<'_>,
        depth: i64,
        indent: String,
        parent_cumulative_value: i64,
        is_last_child: bool,
    ) {
        let (function_name, file_line) = self.index.location_name(node.location_id);
        self.rows.push(make_row(
            texttree::PrettyIdentifier(&function_name, &indent, is_last_child),
            percentage(node.node.cumulative_value, self.total),
            percentage(node.node.cumulative_value, parent_cumulative_value),
            self.root_child,
            depth,
            file_line,
        ));

        if node.node.children.is_empty() {
            return;
        }

        let child_indent = texttree::Indent4Child(&indent, is_last_child);
        let children = node.node.sorted_children();
        let child_count = children.len();
        for (index, child) in children.into_iter().enumerate() {
            self.collect_child(
                child,
                depth + 1,
                child_indent.clone(),
                node.node.cumulative_value,
                index + 1 == child_count,
            );
        }
    }

    /// 先写 root 行，再按排序后的根子节点展开整棵树。
    pub(crate) fn collect(mut self, root: &FlamegraphNode) -> DatumRows {
        self.rows.push(make_row(
            "root".to_string(),
            "100%".to_string(),
            "100%".to_string(),
            0,
            0,
            "root".to_string(),
        ));
        if root.children.is_empty() {
            return self.rows;
        }

        self.total = root.cumulative_value;
        let child_indent = texttree::Indent4Child("", false);
        let children = root.sorted_children();
        let child_count = children.len();
        for (index, child) in children.into_iter().enumerate() {
            // root_child 从 1 起，标记属于哪棵根下子树。
            self.root_child = (index + 1) as i64;
            self.collect_child(
                child,
                1,
                child_indent.clone(),
                root.cumulative_value,
                index + 1 == child_count,
            );
        }
        self.rows
    }
}

/// 计算 `value/total` 百分比字符串；近 100% 显示为 `100%`，小于 1% 用两位有效数字。
pub(crate) fn percentage(value: i64, total: i64) -> String {
    let ratio = if total == 0 {
        0.0
    } else {
        ((value as f64) / (total as f64)).abs() * 100.0
    };
    if (99.95..=100.05).contains(&ratio) {
        "100%".to_string()
    } else if ratio >= 1.0 {
        format!("{ratio:.2}%")
    } else {
        format!("{}%", format_two_significant_digits(ratio))
    }
}

// Go's %.2g uses two significant digits and switches to an exponent below
// 1e-4. Rust has no equivalent formatter, so keep that formatting rule here.
/// 模拟 Go `%.2g`：两位有效数字，指数超出 [-4, 2) 时用科学计数法。
fn format_two_significant_digits(value: f64) -> String {
    if value == 0.0 {
        return "0".to_string();
    }
    let exponent = value.abs().log10().floor() as i32;
    let scale = 10_f64.powi(1 - exponent);
    let rounded = (value * scale).round() / scale;
    let exponent = rounded.abs().log10().floor() as i32;
    if !(-4..2).contains(&exponent) {
        let mantissa = rounded / 10_f64.powi(exponent);
        let mut mantissa = format!("{mantissa:.1}");
        if mantissa.ends_with(".0") {
            mantissa.truncate(mantissa.len() - 2);
        }
        return format!("{mantissa}e{exponent:+03}");
    }

    // 小数位数由数量级决定，再去掉尾随 0。
    let decimal_places = (1 - exponent).max(0) as usize;
    let mut formatted = format!("{rounded:.decimal_places$}");
    if formatted.contains('.') {
        while formatted.ends_with('0') {
            formatted.pop();
        }
        if formatted.ends_with('.') {
            formatted.pop();
        }
    }
    formatted
}
