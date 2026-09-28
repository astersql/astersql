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

// InfoSchema Builder 杂项入口：placement policy、资源组与临时表辅助。
//
// 活跃实现将各类 DDL diff 转译为对应 `ActionType` 后委托 `Builder::ApplyDiff`；
// 上方机械草稿块保留 Go 侧更细粒度逻辑对照。
// Placement Policy：表/分区数据放置策略；Resource Group：资源隔离组。

// Reader、SchemaDiff 与错误类型继续保持 Go 依赖形状，等待跨文件模块接线。

/* Mechanical draft retained for migration history.
// applyCreatePolicy 对应创建 placement policy；同 ID 已存在表示替换，需要刷新引用它的 bundle。
/// 应用创建 Placement Policy 的 schema diff。
pub fn applyCreatePolicy(
    builder: &mut Builder,
    reader: &dyn meta::Reader,
    diff: &model::SchemaDiff,
) -> Result<(), Error> {
    let policy = reader
        .GetPolicy(diff.SchemaID)?
        .ok_or_else(|| Error::placement_policy_not_exists(diff.SchemaID))?;

    if builder.infoSchema().PolicyByID(policy.ID).is_some() {
        builder.markBundlesReferPolicyShouldUpdate(policy.ID);
    }
    builder.infoSchema_mut().setPolicy(policy);
    Ok(())
}

// applyAlterPolicy 对应修改策略：替换缓存对象并标记所有引用 bundle 待更新。
/// 应用修改 Placement Policy 的 schema diff，返回受影响表 ID 列表。
pub fn applyAlterPolicy(
    builder: &mut Builder,
    reader: &dyn meta::Reader,
    diff: &model::SchemaDiff,
) -> Result<Vec<i64>, Error> {
    let policy = reader
        .GetPolicy(diff.SchemaID)?
        .ok_or_else(|| Error::placement_policy_not_exists(diff.SchemaID))?;
    let policy_id = policy.ID;
    builder.infoSchema_mut().setPolicy(policy);
    builder.markBundlesReferPolicyShouldUpdate(policy_id);

    // Go 当前尚未返回策略关联的表 ID，因此保留空切片结果，而不是虚构扫描行为。
    Ok(Vec::new())
}

// applyDropPolicy 按 ID 查找、按小写名称删除；不存在时与 Go 一样幂等返回。
pub fn applyDropPolicy(builder: &mut Builder, policy_id: i64) -> Vec<i64> {
    let Some(policy) = builder.infoSchema().PolicyByID(policy_id).cloned() else {
        return Vec::new();
    };
    builder.infoSchema_mut().deletePolicy(&policy.Name.L);

    // 原实现没有计算关联表，返回空集合保持现有调用约定。
    Vec::new()
}

// applyCreateOrAlterResourceGroup 对创建和修改共用一次覆盖写入。
/// 按 `alter` 标志分派创建或修改 Resource Group。
pub fn applyCreateOrAlterResourceGroup(
    builder: &mut Builder,
    reader: &dyn meta::Reader,
    diff: &model::SchemaDiff,
) -> Result<(), Error> {
    let group = reader
        .GetResourceGroup(diff.SchemaID)?
        .ok_or_else(|| Error::resource_group_not_exists(diff.SchemaID))?;
    builder.infoSchema_mut().setResourceGroup(group);
    Ok(())
}

// applyDropResourceGroup 对应 Go 删除路径；reader 参数在源实现中保留但不参与读取。
/// 应用删除 Resource Group 的 schema diff。
pub fn applyDropResourceGroup(
    builder: &mut Builder,
    _reader: &dyn meta::Reader,
    diff: &model::SchemaDiff,
) -> Vec<i64> {
    let Some(group) = builder
        .infoSchema()
        .ResourceGroupByID(diff.SchemaID)
        .cloned()
    else {
        return Vec::new();
    };
    builder.infoSchema_mut().deleteResourceGroup(&group.Name.L);

    Vec::new()
}

impl Builder {
    // addTemporaryTable 对应 Go 的惰性集合初始化；Rust 集合在 Builder 初始化时已可用。
    pub fn addTemporaryTable(&mut self, table_id: i64) {
        self.infoSchema_mut().temporaryTableIDs.insert(table_id);
    }

    // initMisc 在实体库表建立后加载策略和资源组；masking policy 参数按 Go 现状暂不消费。
    pub fn initMisc(
        &mut self,
        policies: Vec<model::PolicyInfo>,
        resource_groups: Vec<model::ResourceGroupInfo>,
        _masking_policies: Vec<model::MaskingPolicyInfo>,
    ) {
        for policy in policies {
            self.infoSchema_mut().setPolicy(policy);
        }
        for group in resource_groups {
            self.infoSchema_mut().setResourceGroup(group);
        }
    }
}
*/

#![allow(non_snake_case)]

use crate::builder::{ActionType, Builder, MetadataReader, SchemaDiff};
use crate::infoschema::{PolicyInfo, ResourceGroupInfo};

/// 将策略/资源组类 diff 的 `table_id` 改写为源 `schema_id`，并替换动作类型。
fn policy_diff(source: &SchemaDiff, action_type: ActionType) -> SchemaDiff {
    SchemaDiff {
        action_type,
        table_id: source.schema_id,
        ..source.clone()
    }
}

pub fn applyCreatePolicy(
    builder: &mut Builder,
    metadata: &dyn MetadataReader,
    diff: &SchemaDiff,
) -> Result<(), String> {
    builder
        .ApplyDiff(
            metadata,
            &policy_diff(diff, ActionType::CreatePlacementPolicy),
        )
        .map(|_| ())
}

pub fn applyAlterPolicy(
    builder: &mut Builder,
    metadata: &dyn MetadataReader,
    diff: &SchemaDiff,
) -> Result<Vec<i64>, String> {
    builder.ApplyDiff(
        metadata,
        &policy_diff(diff, ActionType::AlterPlacementPolicy),
    )
}

/// 应用删除 Placement Policy 的 schema diff。
pub fn applyDropPolicy(
    builder: &mut Builder,
    metadata: &dyn MetadataReader,
    diff: &SchemaDiff,
) -> Result<Vec<i64>, String> {
    builder.ApplyDiff(
        metadata,
        &policy_diff(diff, ActionType::DropPlacementPolicy),
    )
}

pub fn applyCreateOrAlterResourceGroup(
    builder: &mut Builder,
    metadata: &dyn MetadataReader,
    diff: &SchemaDiff,
    alter: bool,
) -> Result<(), String> {
    // alter=true 走 AlterResourceGroup，否则 CreateResourceGroup。
    let action_type = if alter {
        ActionType::AlterResourceGroup
    } else {
        ActionType::CreateResourceGroup
    };
    builder
        .ApplyDiff(metadata, &policy_diff(diff, action_type))
        .map(|_| ())
}

pub fn applyDropResourceGroup(
    builder: &mut Builder,
    metadata: &dyn MetadataReader,
    diff: &SchemaDiff,
) -> Result<Vec<i64>, String> {
    builder.ApplyDiff(metadata, &policy_diff(diff, ActionType::DropResourceGroup))
}

/// 将表 ID 登记为临时表。
pub fn addTemporaryTable(builder: &mut Builder, table_id: i64) {
    builder.addTemporaryTable(table_id);
}

/// 初始化 Builder 中的 policy 与 resource group 集合。
pub fn initMisc(
    builder: &mut Builder,
    policies: Vec<PolicyInfo>,
    resource_groups: Vec<ResourceGroupInfo>,
) {
    builder.initMisc(policies, resource_groups);
}
