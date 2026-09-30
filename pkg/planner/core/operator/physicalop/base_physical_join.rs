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

// 物理连接算子公共基座。
// 承载 Join 类型、左右条件、连接键与 Schema 组装逻辑，供 HashJoin/IndexJoin 等复用。

use base::{ContextRef, JoinType, PhysicalPlan};
use expression::{Column, CorrelatedColumn, ExprBox, Schema};
use types::datum::Datum;

use crate::PhysicalSchemaProducer;

/// 物理连接算子共享状态：条件、连接键、内表下标与外连接默认值等。
pub struct BasePhysicalJoin {
    pub PhysicalSchemaProducer: PhysicalSchemaProducer,
    pub JoinType: JoinType,
    pub LeftConditions: Vec<ExprBox>,
    pub RightConditions: Vec<ExprBox>,
    pub OtherConditions: Vec<ExprBox>,
    pub InnerChildIdx: usize,
    pub OuterJoinKeys: Vec<Column>,
    pub InnerJoinKeys: Vec<Column>,
    pub LeftJoinKeys: Vec<Column>,
    pub RightJoinKeys: Vec<Column>,
    pub IsNullEQ: Vec<bool>,
    pub DefaultValues: Vec<Datum>,
    pub LeftNAJoinKeys: Vec<Column>,
    pub RightNAJoinKeys: Vec<Column>,
}

impl BasePhysicalJoin {
    /// 构造空条件列表的基座，默认内表为右孩子（下标 1）。
    pub fn New(producer: PhysicalSchemaProducer, join_type: JoinType) -> Self {
        Self {
            PhysicalSchemaProducer: producer,
            JoinType: join_type,
            LeftConditions: Vec::new(),
            RightConditions: Vec::new(),
            OtherConditions: Vec::new(),
            InnerChildIdx: 1,
            OuterJoinKeys: Vec::new(),
            InnerJoinKeys: Vec::new(),
            LeftJoinKeys: Vec::new(),
            RightJoinKeys: Vec::new(),
            IsNullEQ: Vec::new(),
            DefaultValues: Vec::new(),
            LeftNAJoinKeys: Vec::new(),
            RightNAJoinKeys: Vec::new(),
        }
    }

    /// 返回连接类型（内连接、外连接、半连接等）。
    pub fn GetJoinType(&self) -> JoinType {
        self.JoinType
    }
    /// 标记该类型实现物理 Join 接口的占位方法。
    pub fn PhysicalJoinImplement(&self) {}
    /// 返回内表孩子下标（IndexJoin/HashJoin 探测侧）。
    pub fn GetInnerChildIdx(&self) -> usize {
        self.InnerChildIdx
    }

    /// 计划缓存场景下克隆；失败时返回 None。
    pub fn CloneForPlanCacheWithSelf(&self, new_ctx: ContextRef) -> Option<Self> {
        Some(Self {
            PhysicalSchemaProducer: self
                .PhysicalSchemaProducer
                .CloneForPlanCacheWithSelf(new_ctx)?,
            JoinType: self.JoinType,
            LeftConditions: clone_exprs(&self.LeftConditions),
            RightConditions: clone_exprs(&self.RightConditions),
            OtherConditions: clone_exprs(&self.OtherConditions),
            InnerChildIdx: self.InnerChildIdx,
            OuterJoinKeys: self.OuterJoinKeys.iter().map(Column::Clone).collect(),
            InnerJoinKeys: self.InnerJoinKeys.iter().map(Column::Clone).collect(),
            LeftJoinKeys: self.LeftJoinKeys.iter().map(Column::Clone).collect(),
            RightJoinKeys: self.RightJoinKeys.iter().map(Column::Clone).collect(),
            IsNullEQ: self.IsNullEQ.clone(),
            DefaultValues: self.DefaultValues.clone(),
            LeftNAJoinKeys: self.LeftNAJoinKeys.iter().map(Column::Clone).collect(),
            RightNAJoinKeys: self.RightNAJoinKeys.iter().map(Column::Clone).collect(),
        })
    }

    /// 深拷贝连接条件与键列，并换绑新的执行上下文。
    pub fn CloneWithSelf(&self, new_ctx: ContextRef) -> Result<Self, expression::Error> {
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
            JoinType: self.JoinType,
            LeftConditions: clone_exprs(&self.LeftConditions),
            RightConditions: clone_exprs(&self.RightConditions),
            OtherConditions: clone_exprs(&self.OtherConditions),
            InnerChildIdx: self.InnerChildIdx,
            OuterJoinKeys: self.OuterJoinKeys.iter().map(Column::Clone).collect(),
            InnerJoinKeys: self.InnerJoinKeys.iter().map(Column::Clone).collect(),
            LeftJoinKeys: self.LeftJoinKeys.iter().map(Column::Clone).collect(),
            RightJoinKeys: self.RightJoinKeys.iter().map(Column::Clone).collect(),
            // Match Go: ordinary cloning does not carry the plan-cache-only
            // NULL-equality bitmap; CloneForPlanCacheWithSelf does.
            IsNullEQ: Vec::new(),
            DefaultValues: self.DefaultValues.clone(),
            LeftNAJoinKeys: self.LeftNAJoinKeys.iter().map(Column::Clone).collect(),
            RightNAJoinKeys: self.RightNAJoinKeys.iter().map(Column::Clone).collect(),
        })
    }

    /// 从左右及其他条件中抽取相关列（关联子查询外层引用）。
    pub fn ExtractCorrelatedCols(&self) -> Vec<CorrelatedColumn> {
        self.LeftConditions
            .iter()
            .chain(&self.RightConditions)
            .chain(&self.OtherConditions)
            .flat_map(|expr| {
                expression::ExtractCorColumns(expr.as_ref())
                    .into_iter()
                    .map(CorrelatedColumn::Clone)
            })
            .collect()
    }

    /// 估算本节点及持有表达式、列、默认值的内存占用。
    pub fn MemoryUsage(&self) -> i64 {
        let expression_capacity = self.LeftConditions.capacity()
            + self.RightConditions.capacity()
            + self.OtherConditions.capacity();
        let column_capacity = self.OuterJoinKeys.capacity()
            + self.InnerJoinKeys.capacity()
            + self.LeftJoinKeys.capacity()
            + self.RightJoinKeys.capacity()
            + self.LeftNAJoinKeys.capacity()
            + self.RightNAJoinKeys.capacity();
        std::mem::size_of::<Self>() as i64
            + self.PhysicalSchemaProducer.MemoryUsage()
            + (expression_capacity * std::mem::size_of::<ExprBox>()) as i64
            + (column_capacity * std::mem::size_of::<Column>()) as i64
            + (self.DefaultValues.capacity() * std::mem::size_of::<Datum>()) as i64
            + self
                .LeftConditions
                .iter()
                .chain(&self.RightConditions)
                .chain(&self.OtherConditions)
                .map(|expr| expr.MemoryUsage())
                .sum::<i64>()
            + self
                .OuterJoinKeys
                .iter()
                .chain(&self.InnerJoinKeys)
                .chain(&self.LeftJoinKeys)
                .chain(&self.RightJoinKeys)
                .chain(&self.LeftNAJoinKeys)
                .chain(&self.RightNAJoinKeys)
                .map(|column| column.MemoryUsage() - std::mem::size_of::<Column>() as i64)
                .sum::<i64>()
            + self
                .DefaultValues
                .iter()
                .map(|datum| datum.MemUsage() - std::mem::size_of::<Datum>() as i64)
                .sum::<i64>()
            + self.IsNullEQ.capacity() as i64
    }
}

/// 深拷贝表达式列表。
fn clone_exprs(expressions: &[ExprBox]) -> Vec<ExprBox> {
    expressions.iter().map(|expr| expr.CloneExpr()).collect()
}

/// 空 `BasePhysicalJoin` 结构体本身的字节大小常量。
pub const EMPTY_BASE_PHYSICAL_JOIN_SIZE: i64 = std::mem::size_of::<BasePhysicalJoin>() as i64;

/// 按连接类型组装输出 Schema：半连接仅保留左表；外半连接追加标记列；外连接清除可空侧 NotNull。
pub fn BuildPhysicalJoinSchema(join_type: JoinType, join: &dyn PhysicalPlan) -> Schema {
    let children = join.children();
    let left = children
        .first()
        .map(|child| child.schema().Clone())
        .unwrap_or_else(|| expression::NewSchema(Vec::new()));
    // 半连接/反半连接：输出与左表相同，不拼接右表列。
    if matches!(join_type, JoinType::SemiJoin | JoinType::AntiSemiJoin) {
        return left;
    }
    if matches!(
        join_type,
        JoinType::LeftOuterSemiJoin | JoinType::AntiLeftOuterSemiJoin
    ) {
        let mut result = left;
        if let Some(column) = join.schema().Columns.last() {
            result.Append([column.Clone()]);
        }
        return result;
    }
    let Some(right) = children.get(1).map(|child| child.schema().Clone()) else {
        return left;
    };
    let left_len = left.Len();
    let mut columns = left.Columns;
    columns.extend(right.Columns);
    let mut result = expression::NewSchema(columns);
    // 外连接：可空侧列去掉 NotNull 标志。
    let nullable = match join_type {
        JoinType::LeftOuterJoin => left_len..result.Len(),
        JoinType::RightOuterJoin => 0..left_len,
        JoinType::FullOuterJoin => 0..result.Len(),
        _ => 0..0,
    };
    for column in &mut result.Columns[nullable] {
        if let Some(field_type) = &mut column.RetType {
            field_type.DelFlag(mysql::r#type::NotNullFlag);
        }
    }
    result
}
