// Copyright 2026 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// Keyspace（键空间）可观测性配置模块。
//
// Keyspace 是多租户场景下的键空间隔离单元：同一集群内不同租户的数据
// 通过键前缀划分到不同 keyspace。本模块负责把 keyspace 的 metadata
// （元数据键值对）映射到三类可观测性输出：
// - Prometheus 监控指标的 label（标签）；
// - 慢查询日志（slow log）中的附加字段；
// - 语句日志（statement log）中的附加字段。
//
// 主要内容：
// - [`KeyspaceObservability`]：配置文件中声明的映射规则集合，含校验逻辑；
// - [`KeyspaceObservabilityValues`]：根据实际 metadata 解析出的缓存值；
// - `Config::ResolveKeyspaceObservability` 等方法：解析与读取入口。

use super::Config;
use std::collections::{HashMap, HashSet};

// KeyspaceObservability maps metadata entries to observability outputs.
// KeyspaceObservability 对应 Go struct，字段顺序与 toml/json 标签对应关系保持一致。
/// keyspace 可观测性配置：把 metadata 条目映射为可观测性输出的规则集合。
///
/// 该结构直接由配置文件（toml/json）反序列化得到，随后通过 [`Self::Valid`]
/// 做静态校验，再由 `Config::ResolveKeyspaceObservability` 结合实际
/// metadata 解析出最终值。
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct KeyspaceObservability {
    // Go field: Fields []KeyspaceObservabilityField `toml:"fields" json:"fields,omitempty"`
    /// 映射规则列表，每条规则描述一个 metadata 条目到输出的映射。
    #[serde(rename = "fields")]
    pub Fields: Vec<KeyspaceObservabilityField>,
}

// KeyspaceObservabilityField describes one metadata entry mapping.
// KeyspaceObservabilityField 对应一条 metadata 到观测输出的配置映射。
/// 单条映射规则：描述某个 metadata 条目应输出到哪些可观测性目标。
///
/// 三个输出字段（metric-label / slow-log-field / stmt-log-field）至少
/// 需要设置一个，允许同一 metadata 同时输出到多个目标。
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct KeyspaceObservabilityField {
    /// metadata 中的来源键名，不能为空。
    #[serde(rename = "source")]
    pub Source: String,
    /// 输出为 Prometheus 指标 label 时使用的 label 名，必须以
    /// `keyspace_meta_` 前缀开头；为空表示不输出到指标。
    #[serde(rename = "metric-label")]
    pub MetricLabel: String,
    /// 输出到慢查询日志时使用的字段名，必须以 `Keyspace_meta_` 前缀
    /// 开头；为空表示不输出到慢日志。
    #[serde(rename = "slow-log-field")]
    pub SlowLogField: String,
    /// 输出到语句日志时使用的字段名；为空表示不输出到语句日志。
    #[serde(rename = "stmt-log-field")]
    pub StmtLogField: String,
    /// 是否必填：为 true 时，若 metadata 中缺少该来源键则解析报错；
    /// 为 false 时缺失则静默跳过。
    #[serde(rename = "required")]
    pub Required: bool,
}

// KeyspaceObservabilityValues stores resolved metadata values.
// KeyspaceObservabilityValues 对应 Go 中解析后的缓存值；map/slice 语义用 HashMap/Vec 表达。
/// 解析结果缓存：根据映射规则与实际 metadata 计算出的最终输出值。
///
/// 由 `Config::ResolveKeyspaceObservability` 生成并缓存在 `Config` 上，
/// 供指标上报、慢日志与语句日志写入时直接读取。
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct KeyspaceObservabilityValues {
    /// 指标 label 名 -> metadata 值。
    pub MetricLabels: HashMap<String, String>,
    /// 慢日志字段列表，按字段名稳定排序，保证日志输出顺序确定。
    pub SlowLogFields: Vec<KeyspaceObservabilityLogField>,
    /// 语句日志字段名 -> metadata 值。
    pub StmtLogFields: HashMap<String, String>,
}

// KeyspaceObservabilityLogField stores a resolved log field value.
/// 一条已解析的日志字段：字段名与对应的 metadata 值。
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct KeyspaceObservabilityLogField {
    /// 日志字段名。
    pub Name: String,
    /// 从 metadata 中解析出的字段值。
    pub Value: String,
}

// Go const block：保留配置校验需要的输出名前缀。
/// Prometheus 指标 label 必须使用的前缀（大小写不敏感比较）。
pub const keyspaceObservabilityMetricLabelPrefix: &str = "keyspace_meta_";
/// 慢日志字段必须使用的前缀（大小写敏感比较）。
pub const keyspaceObservabilitySlowLogFieldPrefix: &str = "Keyspace_meta_";

impl KeyspaceObservability {
    // Valid validates metadata observability mappings.
    // Valid 对应 Go 值接收者方法：逐项检查 source、输出字段、Prometheus label 和重复项。
    /// 校验全部映射规则的合法性。
    ///
    /// 检查内容：source 非空、至少设置一个输出、输出名满足前缀与
    /// Prometheus label 语法要求、同类输出名不重复（忽略大小写）。
    /// 出错时返回带规则下标的错误信息，便于定位配置项。
    pub fn Valid(&self) -> Result<(), String> {
        // 三个去重集合分别跟踪各类输出名（统一转小写后比较）。
        let mut metricLabels: HashSet<String> = HashSet::with_capacity(self.Fields.len());
        let mut slowLogFields: HashSet<String> = HashSet::with_capacity(self.Fields.len());
        let mut stmtLogFields: HashSet<String> = HashSet::with_capacity(self.Fields.len());

        for (i, field) in self.Fields.iter().enumerate() {
            // 来源键名不能为空，否则无从取值。
            if field.Source.is_empty() {
                return Err(format!(
                    "[keyspace-observability.fields.{}] source cannot be empty",
                    i
                ));
            }
            // 三个输出全为空说明该条规则没有任何效果，视为配置错误。
            if field.MetricLabel.is_empty()
                && field.SlowLogField.is_empty()
                && field.StmtLogField.is_empty()
            {
                return Err(format!(
                    "[keyspace-observability.fields.{}] at least one output must be set",
                    i
                ));
            }

            // 指标 label 校验：语法合法、带规定前缀、不与其他规则重复。
            if !field.MetricLabel.is_empty() {
                if !validPrometheusLabelName(&field.MetricLabel) {
                    return Err(format!(
                        "[keyspace-observability.fields.{}] invalid metric-label {:?}",
                        i, field.MetricLabel
                    ));
                }
                let key = field.MetricLabel.to_lowercase();
                if !key.starts_with(keyspaceObservabilityMetricLabelPrefix) {
                    return Err(format!(
                        "[keyspace-observability.fields.{}] metric-label {:?} must start with {:?}",
                        i, field.MetricLabel, keyspaceObservabilityMetricLabelPrefix
                    ));
                }
                if !metricLabels.insert(key) {
                    return Err(format!(
                        "[keyspace-observability.fields.{}] duplicated metric-label {:?}",
                        i, field.MetricLabel
                    ));
                }
            }

            // 慢日志字段校验：语法合法、带规定前缀、不与其他规则重复。
            if !field.SlowLogField.is_empty() {
                if !validKeyspaceObservabilityLogFieldName(&field.SlowLogField) {
                    return Err(format!(
                        "[keyspace-observability.fields.{}] invalid slow-log-field {:?}",
                        i, field.SlowLogField
                    ));
                }
                if !field
                    .SlowLogField
                    .starts_with(keyspaceObservabilitySlowLogFieldPrefix)
                {
                    return Err(format!(
                        "[keyspace-observability.fields.{}] slow-log-field {:?} must start with {:?}",
                        i, field.SlowLogField, keyspaceObservabilitySlowLogFieldPrefix
                    ));
                }
                let key = field.SlowLogField.to_lowercase();
                if !slowLogFields.insert(key) {
                    return Err(format!(
                        "[keyspace-observability.fields.{}] duplicated slow-log-field {:?}",
                        i, field.SlowLogField
                    ));
                }
            }

            // 语句日志字段仅做重复性检查，无前缀与语法限制。
            if !field.StmtLogField.is_empty() {
                let key = field.StmtLogField.to_lowercase();
                if !stmtLogFields.insert(key) {
                    return Err(format!(
                        "[keyspace-observability.fields.{}] duplicated stmt-log-field {:?}",
                        i, field.StmtLogField
                    ));
                }
            }
        }
        Ok(())
    }
}

// validKeyspaceObservabilityLogFieldName 对应 Go 包内辅助函数，目前沿用 Prometheus label 规则。
/// 校验日志字段名是否合法，目前直接复用 Prometheus label 命名规则。
pub fn validKeyspaceObservabilityLogFieldName(field: &str) -> bool {
    validPrometheusLabelName(field)
}

// validPrometheusLabelName applies the legacy Prometheus label-name grammar.
// The Go implementation requires both IsValid and IsValidLegacy, so a name
// must satisfy [A-Za-z_:][A-Za-z0-9_:]* even when UTF-8 names are enabled.
/// 按传统 Prometheus label 语法校验名称。
///
/// 名称必须匹配 `[A-Za-z_:][A-Za-z0-9_:]*`：首字符为字母、下划线或冒号，
/// 其余字符可再加数字。空字符串因取不到首字符而返回 false。
pub fn validPrometheusLabelName(label: &str) -> bool {
    // 逐字节检查：先验首字节，再验剩余字节。
    let mut bytes = label.bytes();
    matches!(bytes.next(), Some(b'a'..=b'z' | b'A'..=b'Z' | b'_' | b':'))
        && bytes.all(|byte| matches!(byte, b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'_' | b':'))
}

impl Config {
    // ResolveKeyspaceObservability resolves configured output values from metadata.
    // ResolveKeyspaceObservability 对应 Go 指针接收者方法：从 metadata map 中解析并写回 Config 缓存。
    /// 依据映射规则从实际 metadata 中解析出各类输出值并缓存到 `Config`。
    ///
    /// `values` 为 keyspace 的 metadata 键值对。对每条规则：找不到来源键
    /// 且 `Required` 为 true 时返回错误；否则按配置填充指标 label、
    /// 慢日志字段与语句日志字段。
    pub fn ResolveKeyspaceObservability(
        &mut self,
        values: HashMap<String, String>,
    ) -> Result<(), String> {
        let mut resolved = KeyspaceObservabilityValues {
            MetricLabels: HashMap::new(),
            SlowLogFields: Vec::new(),
            StmtLogFields: HashMap::new(),
        };

        for field in &self.keyspace_observability.Fields {
            // 从 metadata 中取来源键的值；缺失时按 Required 决定报错或跳过。
            let Some(value) = values.get(&field.Source) else {
                if field.Required {
                    return Err(format!(
                        "missing required keyspace metadata entry {:?}",
                        field.Source
                    ));
                }
                // Go 这里对非 required 且缺失的 metadata 直接跳过，不生成任何输出。
                continue;
            };

            if !field.MetricLabel.is_empty() {
                resolved
                    .MetricLabels
                    .insert(field.MetricLabel.clone(), value.clone());
            }
            if !field.SlowLogField.is_empty() {
                resolved.SlowLogFields.push(KeyspaceObservabilityLogField {
                    Name: field.SlowLogField.clone(),
                    Value: value.clone(),
                });
            }
            if !field.StmtLogField.is_empty() {
                resolved
                    .StmtLogFields
                    .insert(field.StmtLogField.clone(), value.clone());
            }
        }

        // Go 使用 sort.SliceStable 保证慢日志字段稳定排序；保留按 Name 排序的语义。
        resolved.SlowLogFields.sort_by(|a, b| a.Name.cmp(&b.Name));
        self.keyspace_observability_values = resolved;
        Ok(())
    }

    // GetKeyspaceObservabilityMetricLabels returns resolved metric labels.
    /// 返回已解析的 Prometheus 指标 label 映射。
    pub fn GetKeyspaceObservabilityMetricLabels(&self) -> &HashMap<String, String> {
        &self.keyspace_observability_values.MetricLabels
    }

    // GetKeyspaceObservabilitySlowLogFields returns resolved slow log fields in stable order.
    /// 返回已解析的慢日志字段列表（按字段名稳定排序）。
    pub fn GetKeyspaceObservabilitySlowLogFields(&self) -> &Vec<KeyspaceObservabilityLogField> {
        &self.keyspace_observability_values.SlowLogFields
    }

    // GetKeyspaceObservabilityStmtLogFields returns resolved statement log fields.
    /// 返回已解析的语句日志字段映射。
    pub fn GetKeyspaceObservabilityStmtLogFields(&self) -> &HashMap<String, String> {
        &self.keyspace_observability_values.StmtLogFields
    }
}

impl KeyspaceObservabilityValues {
    // Clone returns a deep copy of resolved metadata observability values.
    // Clone 对应 Go 的深拷贝方法；HashMap/Vec 的 clone 表达 maps.Clone 和 append 复制。
    /// 深拷贝解析结果。
    ///
    /// 与派生的 `Clone` trait 语义等价，保留此方法是为了对应 Go 源码中
    /// 显式的深拷贝实现；空集合保持 default，避免不必要的分配。
    pub fn Clone(&self) -> KeyspaceObservabilityValues {
        let mut res = KeyspaceObservabilityValues::default();
        if !self.MetricLabels.is_empty() {
            res.MetricLabels = self.MetricLabels.clone();
        }
        if !self.SlowLogFields.is_empty() {
            res.SlowLogFields = self.SlowLogFields.clone();
        }
        if !self.StmtLogFields.is_empty() {
            res.StmtLogFields = self.StmtLogFields.clone();
        }
        res
    }
}
