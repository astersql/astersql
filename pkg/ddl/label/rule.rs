// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// Label Rule（标签规则）的构建与重置逻辑。
//
// Label Rule 由 PD（Placement Driver）消费，把一组 Region Label 绑定到
// 表/分区对应的 key range（键范围）。本模块负责：
// - 将 AST 中的 AttributesSpec 解析为 Labels；
// - 按库名/表名/分区名与 table ID 重置规则 ID、内部标签与 Data；
// - 在 NextGen Keyspace（键空间）部署下生成带 keyspace 前缀的规则 ID 与边界键；
// - 构造批量设置/删除规则的 Patch。

#![allow(non_snake_case, non_camel_case_types, dead_code, unused_variables)]

use crate::attributes::{NewLabels, dbKey, keyspaceKey, partitionKey, tableKey};
use crate::{ast, codec, errors::Error, kerneltype, pd, tablecodec, tikv};

// IDPrefix is the prefix for label rule ID.
/// Label Rule ID 的路径前缀，形如 `schema/...`。
pub const IDPrefix: &str = "schema";
// KeyspacePrefix is the prefix for keyspace in label rule ID.
/// Keyspace 感知规则 ID 的前缀，形如 `keyspace/<id>/schema/...`。
pub const KeyspacePrefix: &str = "keyspace";
/// PD 规则类型：按 key range 匹配 Region。
const ruleType: &str = "key-range";

// RuleIndexDefault is the default index for a rule.
/// 规则优先级索引默认值。
pub const RuleIndexDefault: isize = 0;
// RuleIndexDatabase is the index for a rule of database.
/// 数据库级规则的 Index。
pub const RuleIndexDatabase: isize = 1;
// RuleIndexTable is the index for a rule of table.
/// 表级规则的 Index。
pub const RuleIndexTable: isize = 2;
// RuleIndexPartition is the index for a rule of partition.
/// 分区级规则的 Index（高于表级，更具体）。
pub const RuleIndexPartition: isize = 3;

// TableIDFormat is the format of the label rule ID for a table.
// The format follows "schema/database_name/table_name".
/// 表级规则 ID 的格式模板（占位符形式，实际由 `NewRuleID` 拼接）。
pub const TableIDFormat: &str = "{}/{}/{}";
// PartitionIDFormat is the format of the label rule ID for a partition.
// The format follows "schema/database_name/table_name/partition_name".
/// 分区级规则 ID 的格式模板。
pub const PartitionIDFormat: &str = "{}/{}/{}/{}";

// Rule is used to establish the relationship between labels and a key range.
// Go 使用 type Rule pd.LabelRule 定义新类型；这里用别名保留字段访问形状。
/// PD LabelRule 的类型别名，表示标签与键范围的绑定关系。
pub type Rule = pd::LabelRule;

// NewRule creates a rule.
/// 创建默认空规则（堆分配，对齐 Go 指针语义）。
pub fn NewRule() -> Box<Rule> {
    Box::new(Rule::default())
}

impl Rule {
    // ApplyAttributesSpec will transfer attributes defined in AttributesSpec to the labels.
    /// 将 AttributesSpec 中的属性字符串解析并写入 `Labels`。
    ///
    /// `Default=true` 时清空标签；否则按严格 YAML 数组解析逗号分隔的 `key=value`。
    pub fn ApplyAttributesSpec(&mut self, spec: &ast::AttributesSpec) -> Result<(), Error> {
        if spec.Default {
            self.Labels = Vec::new();
            return Ok(());
        }
        // Go 先把用户输入包进 []，再用 yaml.Strict 解析为字符串数组。
        let attrBytes = format!("[{}]", spec.Attributes);
        let attributes: Vec<String> = serde_yaml::from_str(&attrBytes)
            .map_err(|error| Error::InvalidAttributesSpec(error.to_string()))?;
        self.Labels = NewLabels(attributes)?;
        Ok(())
    }

    // String implements fmt.Stringer.
    /// 序列化为 JSON 字符串；失败时返回空串（对齐 Go Marshal 失败行为）。
    pub fn String(&self) -> String {
        match serde_json::to_string(self) {
            Ok(t) => t,
            // Go 在 Marshal 失败时返回空字符串，错误不再向外传播。
            Err(_) => String::new(),
        }
    }

    // Clone clones a rule.
    /// 深拷贝规则，返回新的 `Box<Rule>`。
    pub fn Clone(&self) -> Box<Rule> {
        let mut newRule = NewRule();
        *newRule = self.clone();
        newRule
    }

    // Reset will reset the label rule for a table/partition with a given ID and names.
    /// 按库/表/分区名与 table ID 列表重置规则 ID、内部标签、类型、Data 与 Index。
    ///
    /// 若当前 `Labels` 为空则只设置 ID 后直接返回；否则注入/更新
    /// keyspace、db、table、partition 等内部标签，并生成排序后的键范围。
    pub fn Reset(
        &mut self,
        tikvCodec: tikv::Codec,
        dbName: String,
        tableName: String,
        partName: String,
        mut ids: Vec<i64>,
    ) -> &mut Rule {
        let isPartition = !partName.is_empty();
        let useKeyspace = UseKeyspaceAwareRules(tikvCodec.clone());
        self.ID = NewRuleID(
            tikvCodec.clone(),
            dbName.clone(),
            tableName.clone(),
            partName.clone(),
        );
        if self.Labels.is_empty() {
            return self;
        }
        // 遍历已有标签，按 key 回填当前库/表/分区/keyspace 的实际取值。
        let mut hasKeyspaceKey = false;
        let mut hasDBKey = false;
        let mut hasTableKey = false;
        let mut hasPartitionKey = false;
        for label in self.Labels.iter_mut() {
            match label.Key.as_str() {
                keyspaceKey => {
                    if useKeyspace {
                        label.Value = tikvCodec.GetKeyspaceID().to_string();
                        hasKeyspaceKey = true;
                    }
                }
                dbKey => {
                    label.Value = dbName.clone();
                    hasDBKey = true;
                }
                tableKey => {
                    label.Value = tableName.clone();
                    hasTableKey = true;
                }
                partitionKey => {
                    if isPartition {
                        label.Value = partName.clone();
                        hasPartitionKey = true;
                    }
                }
                _ => {}
            }
        }

        // 自动补充内部标签，保持 Go 对 keyspace/db/table/partition 的注入顺序。
        if useKeyspace && !hasKeyspaceKey {
            self.Labels.push(pd::RegionLabel {
                Key: keyspaceKey.to_string(),
                Value: tikvCodec.GetKeyspaceID().to_string(),
                ..Default::default()
            });
        }

        if !hasDBKey {
            self.Labels.push(pd::RegionLabel {
                Key: dbKey.to_string(),
                Value: dbName.clone(),
                ..Default::default()
            });
        }

        if !hasTableKey {
            self.Labels.push(pd::RegionLabel {
                Key: tableKey.to_string(),
                Value: tableName.clone(),
                ..Default::default()
            });
        }

        if isPartition && !hasPartitionKey {
            self.Labels.push(pd::RegionLabel {
                Key: partitionKey.to_string(),
                Value: partName.clone(),
                ..Default::default()
            });
        }
        self.RuleType = ruleType.to_string();
        // 按 table ID 排序后，为每个 ID 生成 [start_key, end_key) 的 hex 编码区间。
        let mut dataSlice: Vec<serde_json::Value> = Vec::with_capacity(ids.len());
        ids.sort();
        for id in ids {
            let startPrefix = tablecodec::GenTablePrefix(id);
            // Go 的 int64 加法在边界处按二补码回绕；显式 wrapping 也避免
            // Rust 调试构建在 i64::MAX 表 ID 上 panic。
            let endPrefix = tablecodec::GenTablePrefix(id.wrapping_add(1));
            let (startKey, endKey) = if useKeyspace {
                // Label rules are consumed as region boundary keys, so V2 must encode
                // the whole outer key instead of prefixing a mem-encoded table key.
                tikvCodec.EncodeRegionRange(startPrefix.0, endPrefix.0)
            } else {
                (
                    codec::EncodeBytes(Vec::new(), startPrefix.as_ref()),
                    codec::EncodeBytes(Vec::new(), endPrefix.as_ref()),
                )
            };
            dataSlice.push(serde_json::json!({
                "start_key": hex::encode(startKey),
                "end_key": hex::encode(endKey),
            }));
        }
        self.Data = serde_json::Value::Array(dataSlice);
        // We may support more types later.
        self.Index = RuleIndexTable;
        if isPartition {
            self.Index = RuleIndexPartition;
        }
        self
    }
}

// UseKeyspaceAwareRules returns true when table attribute label rules should be
// scoped by keyspace in NextGen deployments.
/// NextGen 且 Codec 绑定了 KeyspaceMeta 时，规则需按 keyspace 作用域隔离。
pub fn UseKeyspaceAwareRules(tikvCodec: tikv::Codec) -> bool {
    kerneltype::IsNextGen() && !tikvCodec.is_nil() && tikvCodec.GetKeyspaceMeta().is_some()
}

// NewRuleID generates a new rule ID for a table or partition.
/// 生成表或分区的规则 ID；启用 keyspace 时前置 `keyspace/<id>/`。
pub fn NewRuleID(
    tikvCodec: tikv::Codec,
    dbName: String,
    tableName: String,
    partName: String,
) -> String {
    let isPartition = !partName.is_empty();
    let mut id = if isPartition {
        format!("{}/{}/{}/{}", IDPrefix, dbName, tableName, partName)
    } else {
        format!("{}/{}/{}", IDPrefix, dbName, tableName)
    };
    if UseKeyspaceAwareRules(tikvCodec.clone()) {
        id = format!("{}/{}/{}", KeyspacePrefix, tikvCodec.GetKeyspaceID(), id);
    }
    id
}

// RestoreRuleID converts an internal label rule ID to the user-visible form.
/// 将内部规则 ID 还原为用户可见形式（去掉 NextGen 的 keyspace 前缀）。
pub fn RestoreRuleID(ruleID: &str) -> String {
    if !kerneltype::IsNextGen() {
        return ruleID.to_string();
    }
    // 仅当路径形如 keyspace/<id>/schema/... 时裁掉前两段。
    let parts: Vec<&str> = ruleID.split('/').collect();
    if parts.len() >= 3 && parts[0] == KeyspacePrefix && parts[2] == IDPrefix {
        return parts[2..].join("/");
    }
    ruleID.to_string()
}

// NewRulePatch returns a patch of rules which need to be set or deleted.
/// 构造待设置与待删除规则的批量补丁，供 PD 更新 Label Rule。
pub fn NewRulePatch(setRules: Vec<Box<Rule>>, deleteRules: Vec<String>) -> Box<pd::LabelRulePatch> {
    let mut labelRules: Vec<Box<pd::LabelRule>> = Vec::with_capacity(setRules.len());
    for rule in setRules {
        labelRules.push(rule);
    }
    Box::new(pd::LabelRulePatch {
        SetRules: labelRules,
        DeleteRules: deleteRules,
    })
}
