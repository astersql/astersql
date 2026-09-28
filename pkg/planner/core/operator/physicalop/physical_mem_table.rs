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

// 内存表（MemTable）物理扫描算子：读取 INFORMATION_SCHEMA 等非持久化表数据。
//
// 与普通 TableScan 不同，数据来自进程内构造/缓存；可选 Extractor 将谓词下推到提取器，
// QueryTimeRange 用于按时间窗口裁剪诊断类表。

use base::{ContextRef, PhysicalPlan};
use logicalop::LogicalPlan as _;

use crate::{BasePhysicalPlan, PhysicalSchemaProducer};

/// 对应 Go `PhysicalMemTable`：描述要扫描的内存表元数据与可选谓词提取器。
pub struct PhysicalMemTable {
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    /// 库名（大小写不敏感标识 CIStr）。
    pub DBName: parser_ast::CIStr,
    /// 表元信息。
    pub Table: model::TableInfo,
    /// 投影后的列定义。
    pub Columns: Vec<model::ColumnInfo>,
    /// 谓词提取器：把 Selection 条件转成内存表侧过滤。
    pub Extractor: Option<Box<dyn logicalop::MemTablePredicateExtractor>>,
    /// 查询时间范围 `[start, end)`，单位依具体表约定（常为 unix 时间戳）。
    pub QueryTimeRange: Option<(i64, i64)>,
}

impl PhysicalMemTable {
    /// 构造 TypeMemTableScan 空壳。
    pub fn New(ctx: ContextRef) -> Self {
        Self {
            PhysicalSchemaProducer: PhysicalSchemaProducer::New(BasePhysicalPlan::New(
                ctx,
                plancodec::TypeMemTableScan,
                0,
            )),
            DBName: parser_ast::CIStr::default(),
            Table: model::TableInfo::default(),
            Columns: Vec::new(),
            Extractor: None,
            QueryTimeRange: None,
        }
    }

    /// 绑定统计与查询块偏移。
    pub fn Init(mut self, ctx: ContextRef, stats: property::StatsInfo, offset: i32) -> Self {
        self.PhysicalSchemaProducer.BasePhysicalPlan =
            BasePhysicalPlan::New(ctx, plancodec::TypeMemTableScan, offset);
        base::PhysicalPlan::set_stats(&mut self.PhysicalSchemaProducer.BasePhysicalPlan, stats);
        self
    }

    /// 克隆元数据、列与 Extractor 到新上下文。
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
            DBName: self.DBName.clone(),
            Table: self.Table.clone(),
            Columns: self.Columns.clone(),
            Extractor: self.Extractor.clone(),
            QueryTimeRange: self.QueryTimeRange,
        })
    }

    /// 返回 Go `ScanAccessObject.String()` 对应的表访问描述。
    pub fn AccessObject(&self) -> String {
        format!("table:{}", self.Table.Name.O)
    }

    /// 当前 Rust extractor 契约不携带额外 EXPLAIN 文本。
    pub fn OperatorInfo(&self, _normalized: bool) -> String {
        String::new()
    }

    /// EXPLAIN 依次拼接 access object 与非空 operator info。
    pub fn ExplainInfo(&self) -> String {
        let access_object = self.AccessObject();
        let operator_info = self.OperatorInfo(false);
        if operator_info.is_empty() {
            access_object
        } else {
            format!("{access_object}, {operator_info}")
        }
    }

    /// 按 Go 字段布局估算：指针、slice/interface 头和其引用的列指针。
    pub fn MemoryUsage(&self) -> i64 {
        let pointer = std::mem::size_of::<*const ()>() as i64;
        let slice = std::mem::size_of::<[usize; 3]>() as i64;
        let interface = std::mem::size_of::<[usize; 2]>() as i64;
        self.PhysicalSchemaProducer.MemoryUsage()
            + self.DBName.memory_usage()
            + pointer
            + slice
            + self.Columns.capacity() as i64 * pointer
            + interface
            + std::mem::size_of::<Option<(i64, i64)>>() as i64
    }
}

/// 从 LogicalMemTable 枚举物理候选；不支持 IndexJoin、特定 MPP 分区或排序需求。
pub fn ExhaustPhysicalPlans4LogicalMemTable(
    logical: &logicalop::LogicalMemTable,
    required: &property::PhysicalProperty,
) -> Vec<Box<dyn PhysicalPlan>> {
    // IndexJoin / 强制 MPP 分区 / 排序属性均无法由内存表扫描满足。
    if required.IndexJoinProp.is_some()
        || required.MPPPartitionTp != property::AnyType
        || !required.IsSortItemEmpty()
    {
        return Vec::new();
    }
    let Some(ctx) = logical.SCtx().cloned() else {
        return Vec::new();
    };
    let mut plan = PhysicalMemTable::New(ctx.clone());
    plan.DBName = logical.DBName.clone();
    plan.Table = logical.TableInfo.clone();
    plan.Columns = logical.Columns.clone();
    plan.Extractor = logical.Extractor.clone();
    plan.QueryTimeRange = logical.QueryTimeRange;
    plan.PhysicalSchemaProducer
        .SetSchema(logical.Schema().Clone());
    vec![Box::new(plan.Init(
        ctx,
        logical.StatsInfo().cloned().unwrap_or_default(),
        logical.QueryBlockOffset(),
    ))]
}
