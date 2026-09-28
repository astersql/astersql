// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 物理 SHOW 算子：执行 SHOW 类语句（如 SHOW TABLES）并输出元数据结果集。
//
// 内容由 `ShowContents` 描述；可选的谓词抽取器（Extractor）用于把 WHERE 下推到 SHOW 扫描侧。

use base::ContextRef;

use crate::{BasePhysicalPlan, PhysicalSchemaProducer};

/// 物理 Show：组合 Schema 生产者、SHOW 内容描述与可选谓词抽取器。
pub struct PhysicalShow {
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    /// SHOW 语句的具体内容（类型、目标库表、模式等）。
    pub ShowContents: logicalop::ShowContents,
    /// 可选：从 SHOW 结果上抽取/下推谓词的提取器。
    pub Extractor: Option<Box<dyn logicalop::ShowPredicateExtractor>>,
}

impl PhysicalShow {
    /// 构造空内容、无抽取器的 PhysicalShow，计划类型为 TypeShow。
    pub fn New(ctx: ContextRef) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeShow,
                0,
            )),
            ShowContents: logicalop::ShowContents::default(),
            Extractor: None,
        }
    }

    /// 重新绑定基础计划，并将统计行数固定为 1（SHOW 结果集规模由执行器决定）。
    pub fn Init(mut self, ctx: ContextRef) -> Self {
        self.PhysicalSchemaProducer.BasePhysicalPlan =
            BasePhysicalPlan::New(ctx, plancodec::TypeShow, 0);
        let mut stats = property::StatsInfo::default();
        stats.RowCount = 1.0;
        base::PhysicalPlan::set_stats(&mut self.PhysicalSchemaProducer.BasePhysicalPlan, stats);
        self
    }

    /// 克隆基础计划、ShowContents 与 Extractor 到新会话上下文。
    pub fn Clone(&self, new_ctx: ContextRef) -> Result<Self, expression::Error> {
        let mut producer = PhysicalSchemaProducer::New(
            self.PhysicalSchemaProducer
                .BasePhysicalPlan
                .CloneWithNewCtx(new_ctx)?,
        );
        if let Some(schema) = self.PhysicalSchemaProducer.SchemaRef() {
            producer.SetSchema(schema.Clone());
        }
        Ok(Self {
            PhysicalSchemaProducer: producer,
            ShowContents: self.ShowContents.clone(),
            Extractor: self.Extractor.clone(),
        })
    }

    /// EXPLAIN 信息委托给谓词抽取器；无抽取器时返回空串。
    pub fn ExplainInfo(&self) -> String {
        self.Extractor
            .as_ref()
            .map_or_else(String::new, |extractor| extractor.ExplainInfo())
    }

    /// 估算内存：Schema 生产者 + ShowContents 字段占用。
    pub fn MemoryUsage(&self) -> i64 {
        self.PhysicalSchemaProducer.MemoryUsage()
            + self.ShowContents.MemoryUsage()
            + std::mem::size_of::<[usize; 2]>() as i64
    }
}
