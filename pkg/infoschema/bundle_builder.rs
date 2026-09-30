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
    pub fn inherit_bundles(&mut self, bundles: HashMap<i64, Arc<PlacementBundle>>) {
        self.bundles = bundles;
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
