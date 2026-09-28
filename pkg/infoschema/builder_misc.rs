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
// Placement Policy：表/分区数据放置策略；Resource Group：资源隔离组。

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
