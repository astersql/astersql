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

// InfoSchema 上下文侧的“仅元数据”接口与特殊属性过滤器。
//
// InfoSchema（信息模式）是数据库在内存中维护的一套模式/表/策略元数据视图，
// 供解析器、优化器与执行器按名或 ID 查找。本文件定义可独立于完整 InfoSchema
// 实现的窄接口，以及按 TTL、TiFlash 副本、放置策略（Placement Policy）、
// 分区、表锁、亲和性等“特殊属性”筛选表的过滤器。

use crate::{ast, model, placement};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;

/// 特殊属性过滤器：对单张表的 `TableInfo` 做布尔判定。
// SpecialAttributeFilter 对应 Go 的函数类型，用于筛选含特殊属性的表。
pub type SpecialAttributeFilter = fn(&model::TableInfo) -> bool;

/// TTL（Time To Live，存活时间）属性：仅 public 且配置了 TTL 的表匹配。
// TTLAttribute 对应 TTL 过滤器：只有 public 且配置 TTL 的表才匹配。
pub fn TTLAttribute(table: &model::TableInfo) -> bool {
    table.State == model::StatePublic && table.TTLInfo.is_some()
}

/// TiFlash 列存副本属性过滤器。
// TiFlashAttribute 对应 TiFlash 副本过滤器。
pub fn TiFlashAttribute(table: &model::TableInfo) -> bool {
    table.TiFlashReplica.is_some()
}

/// 放置策略属性：检查表级策略及当前启用分区上的策略引用。
// PlacementPolicyAttribute 检查表级策略以及当前启用分区上的策略。
pub fn PlacementPolicyAttribute(table: &model::TableInfo) -> bool {
    if table.PlacementPolicyRef.is_some() {
        return true;
    }
    if let Some(partitionInfo) = table.GetPartitionInfo() {
        for definition in &partitionInfo.Definitions {
            if definition.PlacementPolicyRef.is_some() {
                return true;
            }
        }
    }
    false
}

/// 放置策略属性（忽略分区 Enable 标志）：直接读 `Partition` 字段。
// AllPlacementPolicyAttribute 与上一过滤器不同：直接检查 Partition 字段，忽略分区 Enable 标志。
pub fn AllPlacementPolicyAttribute(table: &model::TableInfo) -> bool {
    if table.PlacementPolicyRef.is_some() {
        return true;
    }
    if let Some(partitionInfo) = &table.Partition {
        for definition in &partitionInfo.Definitions {
            if definition.PlacementPolicyRef.is_some() {
                return true;
            }
        }
    }
    false
}

/// 表锁（Table Lock）属性过滤器。
// TableLockAttribute 对应表锁属性过滤器。
pub fn TableLockAttribute(table: &model::TableInfo) -> bool {
    table.Lock.is_some()
}

/// 分区属性：通过 `GetPartitionInfo` 保留 Go 对“启用分区”的判断语义。
// PartitionAttribute 通过 GetPartitionInfo 保留 Go 对启用分区的判断语义。
pub fn PartitionAttribute(table: &model::TableInfo) -> bool {
    table.GetPartitionInfo().is_some()
}

/// 表亲和性（Affinity）属性过滤器。
// AffinityAttribute 对应亲和性属性过滤器。
pub fn AffinityAttribute(table: &model::TableInfo) -> bool {
    table.Affinity.is_some()
}

/// 组合全部常用特殊属性过滤器；按 Go 短路顺序求或。
// HasSpecialAttributes 按 Go 的短路顺序组合全部常用特殊属性过滤器。
pub fn HasSpecialAttributes(table: &model::TableInfo) -> bool {
    TTLAttribute(table)
        || TiFlashAttribute(table)
        || PlacementPolicyAttribute(table)
        || PartitionAttribute(table)
        || TableLockAttribute(table)
        || AffinityAttribute(table)
}

/// 对应 Go 包级 `AllSpecialAttribute`，指向组合过滤器。
// AllSpecialAttribute 对应 Go 包级变量，函数指针仍指向组合过滤器。
pub const AllSpecialAttribute: SpecialAttributeFilter = HasSpecialAttributes;

/// 一个 schema（数据库）名及其下匹配过滤条件的表元数据列表。
// TableInfoResult 保存一个 schema 名及其中匹配过滤条件的表元数据。
pub struct TableInfoResult {
    pub DBName: ast::CIStr,
    // Go 使用 []*TableInfo 共享元数据，Arc 保留共享且只读的所有权语义。
    pub TableInfos: Vec<Arc<model::TableInfo>>,
}

/// “仅元数据” InfoSchema 接口：打破循环依赖，组合 SchemaAndTable 与 Misc。
// MetaOnlyInfoSchema 对应 Go 为打破循环依赖而提供的“仅元数据”接口。
// 它组合 SchemaAndTable 与 Misc，并保留查询失败和未命中的不同返回形状。
pub trait MetaOnlyInfoSchema: SchemaAndTable + Misc {
    fn SchemaMetaVersion(&self) -> i64;
    fn SchemaByName(&self, schema: &ast::CIStr) -> Option<Arc<model::DBInfo>>;
    fn SchemaExists(&self, schema: &ast::CIStr) -> bool;
    fn TableInfoByName(
        &self,
        schema: &ast::CIStr,
        table: &ast::CIStr,
    ) -> Result<Arc<model::TableInfo>, Self::Error>;
    fn TableInfoByID(&self, id: i64) -> Option<Arc<model::TableInfo>>;
    fn FindTableInfoByPartitionID(
        &self,
        partitionID: i64,
    ) -> Option<(
        Arc<model::TableInfo>,
        Arc<model::DBInfo>,
        Arc<model::PartitionDefinition>,
    )>;
    fn TableExists(&self, schema: &ast::CIStr, table: &ast::CIStr) -> bool;
    fn SchemaByID(&self, id: i64) -> Option<Arc<model::DBInfo>>;
    fn AllSchemaNames(&self) -> Vec<ast::CIStr>;

    // 关联类型允许具体实现沿用自己的可取消上下文和错误类型。
    fn SchemaSimpleTableInfos(
        &self,
        ctx: &Self::Context,
        schema: &ast::CIStr,
    ) -> Result<Vec<Arc<model::TableNameInfo>>, Self::Error>;
    fn ListTablesWithSpecialAttribute(
        &self,
        filter: SpecialAttributeFilter,
    ) -> Vec<TableInfoResult>;

    // 使用小写 schema/table 名查找引用当前表的外键元数据。
    fn GetTableReferredForeignKeys(
        &self,
        schema: &str,
        table: &str,
    ) -> Vec<Arc<model::ReferredFKInfo>>;
}

/// 遍历全部 schema 及指定 schema 下表元数据的窄接口。
// SchemaAndTable 对应遍历全部 schema 和指定 schema 下表元数据的窄接口。
pub trait SchemaAndTable {
    type Context: ?Sized;
    type Error;

    fn AllSchemas(&self) -> Vec<Arc<model::DBInfo>>;
    fn SchemaTableInfos(
        &self,
        ctx: &Self::Context,
        schema: &ast::CIStr,
    ) -> Result<Vec<Arc<model::TableInfo>>, Self::Error>;
}

/// 与核心查表关系较弱的杂项查询：策略、资源组、脱敏、placement bundle、临时表。
// Misc 汇总与核心 InfoSchema 查表关系较弱的策略、资源组、脱敏和临时表查询。
pub trait Misc {
    fn PolicyByName(&self, name: &ast::CIStr) -> Option<Arc<model::PolicyInfo>>;
    fn ResourceGroupByName(&self, name: &ast::CIStr) -> Option<Arc<model::ResourceGroupInfo>>;
    fn MaskingPolicyByName(&self, name: &ast::CIStr) -> Option<Arc<model::MaskingPolicyInfo>>;
    fn MaskingPolicyByTableColumn(
        &self,
        tableID: i64,
        columnID: i64,
    ) -> Option<Arc<model::MaskingPolicyInfo>>;
    // PlacementBundleByPhysicalTableID 按物理表 ID 返回 placement rule bundle。
    fn PlacementBundleByPhysicalTableID(&self, id: i64) -> Option<Arc<placement::Bundle>>;
    fn AllPlacementBundles(&self) -> Vec<Arc<placement::Bundle>>;
    fn AllPlacementPolicies(&self) -> Vec<Arc<model::PolicyInfo>>;
    fn ClonePlacementPolicies(&self) -> HashMap<String, Arc<model::PolicyInfo>>;
    fn AllMaskingPolicies(&self) -> Vec<Arc<model::MaskingPolicyInfo>>;
    fn AllResourceGroups(&self) -> Vec<Arc<model::ResourceGroupInfo>>;
    fn CloneResourceGroups(&self) -> HashMap<String, Arc<model::ResourceGroupInfo>>;
    fn HasTemporaryTable(&self) -> bool;
}

/// 测试用轻量适配器：直接包装一组 `DBInfo`，实现 `SchemaAndTable`。
// DBInfoAsInfoSchema 是测试场景使用的轻量适配器，直接包装 DBInfo 列表。
pub struct DBInfoAsInfoSchema(pub Vec<Arc<model::DBInfo>>);

impl SchemaAndTable for DBInfoAsInfoSchema {
    type Context = ();
    type Error = Infallible;

    // AllSchemas 对应 Go 的零拷贝类型转换；克隆共享引用，不复制 DBInfo 内容。
    fn AllSchemas(&self) -> Vec<Arc<model::DBInfo>> {
        self.0.clone()
    }

    // SchemaTableInfos 线性查找 schema；命中后返回其 Deprecated.Tables，未命中返回空列表。
    fn SchemaTableInfos(
        &self,
        _ctx: &Self::Context,
        schema: &ast::CIStr,
    ) -> Result<Vec<Arc<model::TableInfo>>, Self::Error> {
        for db in &self.0 {
            if &db.Name == schema {
                return Ok(db.Deprecated.Tables.clone());
            }
        }
        Ok(Vec::new())
    }
}
