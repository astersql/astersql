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

// TiKV Single Gather 逻辑算子：从 TiKV（行存 KV 引擎）收集扫描结果。
//
// 在分布式执行中，下推到存储层的扫描需要在 TiDB 侧“汇集”（Gather）数据。
// 本算子描述单次汇集：可覆盖表扫描或索引扫描（Index Gather），以及是否双读
// （Index Lookup：先读索引再回表）。

use crate::*;
use std::any::Any;

/// 从 TiKV 汇集数据的逻辑算子。
pub struct TiKVSingleGather {
    /// Schema 与基类逻辑计划。
    pub LogicalSchemaProducer: LogicalSchemaProducer,
    /// 关联的数据源（表元信息与访问路径）。
    pub Source: Option<DataSourceRef>,
    /// 是否为索引汇集（相对表主键扫描）。
    pub IsIndexGather: bool,
    /// 索引汇集时对应的索引元信息。
    pub Index: Option<model::IndexInfo>,
    /// 存储类型（通常为 TiKV）。
    pub StoreType: kv::StoreType,
    /// 是否需要双读（索引 + 回表）。
    pub IsDoubleRead: bool,
    /// 附加在表侧的过滤条件。
    pub TableFilters: Vec<Expression>,
}

impl Default for TiKVSingleGather {
    fn default() -> Self {
        Self {
            LogicalSchemaProducer: LogicalSchemaProducer::default(),
            Source: None,
            IsIndexGather: false,
            Index: None,
            StoreType: kv::StoreType::TiKV,
            IsDoubleRead: false,
            TableFilters: Vec::new(),
        }
    }
}

impl TiKVSingleGather {
    /// 初始化算子名为 `"TiKVSingleGather"`。
    pub fn Init(mut self, ctx: base::ContextRef, offset: i32) -> Self {
        self.LogicalSchemaProducer.BaseLogicalPlan =
            NewBaseLogicalPlan(ctx, "TiKVSingleGather", offset);
        self
    }

    /// EXPLAIN：展示数据源信息，索引汇集时追加索引名。
    pub fn ExplainInfo(&self) -> String {
        let mut result = self
            .Source
            .as_ref()
            .map(|source| source.borrow().ExplainInfo())
            .unwrap_or_else(|| "gather".to_owned());
        if self.IsIndexGather
            && let Some(index) = &self.Index
        {
            result.push_str(&format!(", index:{}", index.Name.O));
        }
        result
    }

    /// 委托 schema 生产者构建唯一键信息。
    pub fn BuildKeyInfo(&mut self) {
        self.LogicalSchemaProducer.BuildKeyInfo();
    }

    /// 与 Go 的基类实现一致，直接继承唯一扫描子节点的统计信息。
    pub fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        self.LogicalSchemaProducer
            .BaseLogicalPlan
            .DeriveStats(reload)
    }

    /// 有序属性直接继承第一个子节点（汇集本身不改变序）。
    pub fn PreparePossibleProperties(
        &mut self,
        children: &[PossiblePropertiesInfo],
    ) -> PossiblePropertiesInfo {
        let Some(first) = children.first() else {
            self.LogicalSchemaProducer
                .BaseLogicalPlan
                .PreparePossibleProperties(&[]);
            return PossiblePropertiesInfo::default();
        };
        self.LogicalSchemaProducer
            .BaseLogicalPlan
            .PreparePossibleProperties(&[first.HasTiFlash]);
        first.clone()
    }
}

impl LogicalPlan for TiKVSingleGather {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn base(&self) -> &BaseLogicalPlan {
        &self.LogicalSchemaProducer.BaseLogicalPlan
    }
    fn base_mut(&mut self) -> &mut BaseLogicalPlan {
        &mut self.LogicalSchemaProducer.BaseLogicalPlan
    }
    fn ExplainInfo(&self) -> String {
        TiKVSingleGather::ExplainInfo(self)
    }
    fn BuildKeyInfo(&mut self) {
        TiKVSingleGather::BuildKeyInfo(self)
    }
    fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        TiKVSingleGather::DeriveStats(self, reload)
    }
}
