// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// Placement Bundle 构建器。
//
// 根据表/分区上绑定的 Placement Policy，在内存中维护 physical_id → Bundle 映射。
// 支持全量重建与增量（delta）更新；Placement Policy 描述数据在 TiKV 上的放置规则。

// Computes and updates in-memory mappings only.
// placement、model 与日志接口保留 Go 外部依赖形状，方便后续模块接线。

/* Mechanical draft retained for migration history.
use std::collections::{HashMap, HashSet};

// policyGetter 对应 Go placement.PolicyGetter 适配器，从当前 InfoSchema 按 ID 取策略。
/// 通过 BundleSchema 按 ID 查找 Placement Policy 的辅助包装。
pub struct policyGetter<'a> {
    pub is: &'a infoSchema,
}

impl placement::PolicyGetter for policyGetter<'_> {
    fn GetPolicy(&self, policy_id: i64) -> Result<&model::PolicyInfo, Error> {
        self.is
            .PolicyByID(policy_id)
            .ok_or_else(|| Error::new(format!("Cannot find placement policy with ID: {policy_id}")))
    }
}

// bundleInfoBuilder 记录本轮构建需要增量刷新的表 ID 与策略 ID。
#[derive(Default)]
/// 累积待更新表/策略，并写出 physical_id → PlacementBundle。
pub struct bundleInfoBuilder {
    pub deltaUpdate: bool,
    pub updateTables: HashSet<i64>,
    pub updatePolicies: HashSet<i64>,
}

impl bundleInfoBuilder {
    // initBundleInfoBuilder 对应 Go 初始化；每次构建清空上轮待更新集合。
    /// 清空待更新表/策略集合，准备新一轮构建。
    pub fn initBundleInfoBuilder(&mut self) {
        self.updateTables = HashSet::new();
        self.updatePolicies = HashSet::new();
    }

    // SetDeltaUpdateBundles 开启按受影响表更新，否则 Build 阶段执行全量重建。
    /// 切换为增量更新模式。
    pub fn SetDeltaUpdateBundles(&mut self) {
        self.deltaUpdate = true;
    }

    // deleteBundle 对应 Go map delete，不存在的键按幂等操作处理。
    pub fn deleteBundle(&mut self, schema: &mut infoSchema, table_id: i64) {
        schema.ruleBundleMap.remove(&table_id);
    }

    /// 标记表需要在增量路径中刷新。
    pub fn markTableBundleShouldUpdate(&mut self, table_id: i64) {
        self.updateTables.insert(table_id);
    }

    /// 标记引用该 policy 的表需要刷新。
    pub fn markBundlesReferPolicyShouldUpdate(&mut self, policy_id: i64) {
        self.updatePolicies.insert(policy_id);
    }

    // updateInfoSchemaBundles 在增量模式只处理闭包后的表集合，全量模式则遍历所有实体表。
    pub fn updateInfoSchemaBundles(&mut self, schema: &mut infoSchema) {
        if self.deltaUpdate {
            self.completeUpdateTables(schema);
            // 克隆 ID 集合，避免更新 bundle 时与待更新集合的可变借用冲突。
            for table_id in self.updateTables.clone() {
                self.updateTableBundles(schema, table_id);
            }
            return;
        }

        schema.ruleBundleMap = HashMap::new();
        let table_ids: Vec<i64> = schema
            .schemaMap
            .values()
            .flat_map(|tables| tables.tables.values())
            .map(|table| table.Meta().ID)
            .collect();
        for table_id in table_ids {
            self.updateTableBundles(schema, table_id);
        }
    }

    // completeUpdateTables 把“策略变化”展开成所有直接引用该策略的表。
    pub fn completeUpdateTables(&mut self, schema: &infoSchema) {
        if self.updatePolicies.is_empty() {
            return;
        }
        for tables in schema.schemaMap.values() {
            for table in tables.tables.values() {
                let info = table.Meta();
                if info
                    .PlacementPolicyRef
                    .as_ref()
                    .is_some_and(|policy| self.updatePolicies.contains(&policy.ID))
                {
                    self.markTableBundleShouldUpdate(info.ID);
                }
            }
        }
    }

    // updateTableBundles 先更新表 bundle，再逐个更新分区 bundle；单项失败只记录日志并继续。
    pub fn updateTableBundles(&mut self, schema: &mut infoSchema, table_id: i64) {
        let Some(table) = schema.TableByID(table_id).cloned() else {
            self.deleteBundle(schema, table_id);
            return;
        };

        let getter = policyGetter { is: schema };
        match placement::NewTableBundle(&getter, table.Meta()) {
            Ok(Some(bundle)) => {
                schema.ruleBundleMap.insert(table_id, bundle);
            }
            Ok(None) => self.deleteBundle(schema, table_id),
            Err(error) => {
                // 与 Go 一致：bundle 构造失败不阻断整个 InfoSchema 构建。
                logutil::BgLogger().Error("create table bundle failed", error);
            }
        }

        let Some(partition) = table.Meta().Partition.as_ref() else {
            return;
        };
        for definition in &partition.Definitions {
            match placement::NewPartitionBundle(&getter, definition) {
                Ok(Some(bundle)) => {
                    schema.ruleBundleMap.insert(definition.ID, bundle);
                }
                Ok(None) => self.deleteBundle(schema, definition.ID),
                Err(error) => {
                    logutil::BgLogger().ErrorWithID(
                        "create partition bundle failed",
                        error,
                        definition.ID,
                    );
                }
            }
        }
    }
}

// Go 通过匿名嵌入提升这些方法；显式转发到 bundleInfoBuilder。
impl Builder {
    pub fn initBundleInfoBuilder(&mut self) {
        self.bundleInfoBuilder.initBundleInfoBuilder();
    }

    pub fn SetDeltaUpdateBundles(&mut self) {
        self.bundleInfoBuilder.SetDeltaUpdateBundles();
    }

    /// 删除指定物理 ID 的 bundle。
    pub fn deleteBundle(&mut self, table_id: i64) {
        self.bundleInfoBuilder
            .deleteBundle(&mut self.infoschemaV2.infoSchema, table_id);
    }

    pub fn markTableBundleShouldUpdate(&mut self, table_id: i64) {
        self.bundleInfoBuilder
            .markTableBundleShouldUpdate(table_id);
    }

    pub fn markBundlesReferPolicyShouldUpdate(&mut self, policy_id: i64) {
        self.bundleInfoBuilder
            .markBundlesReferPolicyShouldUpdate(policy_id);
    }
}

// updateInfoSchemaBundles 对应 Go 嵌入方法调用，在 Build 收尾时应用 bundle 变化。
pub fn updateInfoSchemaBundles(builder: &mut Builder) {
    builder
        .bundleInfoBuilder
        .updateInfoSchemaBundles(&mut builder.infoschemaV2.infoSchema);
}
*/

#![allow(non_camel_case_types, non_snake_case)]

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::infoschema::{PlacementBundle, PolicyInfo};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 分区级 bundle 规格：分区 ID 与可选 policy。
pub struct PartitionBundleSpec {
    pub partition_id: i64,
    pub policy_id: Option<i64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
/// 表级 bundle 规格：表 ID、表级 policy 与分区列表。
pub struct TableBundleSpec {
    pub table_id: i64,
    pub policy_id: Option<i64>,
    pub partitions: Vec<PartitionBundleSpec>,
}

/// 供 bundle 构建读取 policy 与表规格的抽象视图。
pub trait BundleSchema {
    fn policy_by_id(&self, policy_id: i64) -> Option<Arc<PolicyInfo>>;
    fn table_bundle_spec(&self, table_id: i64) -> Option<TableBundleSpec>;
    fn all_table_bundle_specs(&self) -> Vec<TableBundleSpec>;
}

pub struct policyGetter<'a> {
    schema: &'a dyn BundleSchema,
}

impl policyGetter<'_> {
    /// 查找 policy；不存在则返回错误字符串（对齐 Go 报错文案）。
    pub fn GetPolicy(&self, policy_id: i64) -> Result<Arc<PolicyInfo>, String> {
        self.schema
            .policy_by_id(policy_id)
            .ok_or_else(|| format!("Cannot find placement policy with ID: {policy_id}"))
    }
}

#[derive(Default)]
pub struct bundleInfoBuilder {
    /// true 时只更新标记过的表；false 时全量重建。
    delta_update: bool,
    /// 增量模式下需要刷新 bundle 的表 ID。
    update_tables: HashSet<i64>,
    /// 变更过的 policy ID；会扩展到引用它们的表。
    update_policies: HashSet<i64>,
    /// 构建结果：物理表/分区 ID 到 bundle。
    bundles: HashMap<i64, Arc<PlacementBundle>>,
}

impl bundleInfoBuilder {
    /// 创建空的 bundle 构建器。
    pub fn new() -> Self {
        Self::default()
    }
    pub fn initBundleInfoBuilder(&mut self) {
        self.update_tables.clear();
        self.update_policies.clear();
    }
    pub fn SetDeltaUpdateBundles(&mut self) {
        self.delta_update = true;
    }
    pub fn deleteBundle(&mut self, table_id: i64) {
        self.bundles.remove(&table_id);
    }
    pub fn markTableBundleShouldUpdate(&mut self, table_id: i64) {
        self.update_tables.insert(table_id);
    }
    pub fn markBundlesReferPolicyShouldUpdate(&mut self, policy_id: i64) {
        self.update_policies.insert(policy_id);
    }
    /// 只读访问当前已构建的 bundles。
    pub fn bundles(&self) -> &HashMap<i64, Arc<PlacementBundle>> {
        &self.bundles
    }

    /// 按增量或全量策略更新 bundles，收集过程中的错误信息。
    pub fn updateInfoSchemaBundles(&mut self, schema: &dyn BundleSchema) -> Vec<String> {
        let mut errors = Vec::new();
        // 增量：先把变更 policy 关联到表，再逐表更新。
        if self.delta_update {
            self.completeUpdateTables(schema);
            let updates: Vec<i64> = self.update_tables.iter().copied().collect();
            for table_id in updates {
                errors.extend(self.updateTableBundles(schema, table_id));
            }
        } else {
            // 全量：清空后对所有表规格重建。
            self.bundles.clear();
            for table in schema.all_table_bundle_specs() {
                errors.extend(self.updateTableBundles(schema, table.table_id));
            }
        }
        errors
    }

    /// 将引用了 `update_policies` 的表加入 `update_tables`。
    fn completeUpdateTables(&mut self, schema: &dyn BundleSchema) {
        if self.update_policies.is_empty() {
            return;
        }
        for table in schema.all_table_bundle_specs() {
            if table
                .policy_id
                .is_some_and(|policy| self.update_policies.contains(&policy))
            {
                self.update_tables.insert(table.table_id);
            }
        }
    }

    /// 更新单表及其分区的 bundle；表不存在则删除旧条目。
    fn updateTableBundles(&mut self, schema: &dyn BundleSchema, table_id: i64) -> Vec<String> {
        let Some(table) = schema.table_bundle_spec(table_id) else {
            self.deleteBundle(table_id);
            return Vec::new();
        };
        let getter = policyGetter { schema };
        let mut errors = Vec::new();
        if let Err(error) = self.upsert_bundle(&getter, table.table_id, table.policy_id) {
            errors.push(error);
        }
        for partition in table.partitions {
            if let Err(error) =
                self.upsert_bundle(&getter, partition.partition_id, partition.policy_id)
            {
                errors.push(error);
            }
        }
        errors
    }

    /// 有 policy 则写入规则占位串，无 policy 则删除该物理 ID。
    fn upsert_bundle(
        &mut self,
        getter: &policyGetter<'_>,
        physical_id: i64,
        policy_id: Option<i64>,
    ) -> Result<(), String> {
        let Some(policy_id) = policy_id else {
            self.deleteBundle(physical_id);
            return Ok(());
        };
        let policy = getter.GetPolicy(policy_id)?;
        self.bundles.insert(
            physical_id,
            Arc::new(PlacementBundle {
                physical_id,
                rules: vec![format!("policy:{}:{}", policy.id, policy.name.original)],
            }),
        );
        Ok(())
    }
}

/// 包级入口：委托 `bundleInfoBuilder::updateInfoSchemaBundles`。
pub fn updateInfoSchemaBundles(
    builder: &mut bundleInfoBuilder,
    schema: &dyn BundleSchema,
) -> Vec<String> {
    builder.updateInfoSchemaBundles(schema)
}
