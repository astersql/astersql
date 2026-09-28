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

// 逻辑 Schema 生产者基类（LogicalSchemaProducer）。
//
// 多数产出列的逻辑算子内嵌本结构，统一管理 Schema（输出列集合）、
// OutputNames（列显示名）以及主键/唯一键（PKOrUK）在列裁剪后的继承。

use crate::{BaseLogicalPlan, Column, LogicalPlan, NameSlice, Schema};
use std::any::Any;
use std::collections::HashSet;

/// 带独立 Schema/列名的逻辑算子公共基座。
#[derive(Default)]
pub struct LogicalSchemaProducer {
    /// 基类逻辑计划（ID、子节点、统计等）。
    pub BaseLogicalPlan: BaseLogicalPlan,
}

impl LogicalSchemaProducer {
    /// 只读访问输出 Schema。
    pub fn Schema(&self) -> &Schema {
        LogicalPlan::Schema(self)
    }

    /// 可变访问输出 Schema。
    pub fn Schema_mut(&mut self) -> &mut Schema {
        LogicalPlan::Schema_mut(self)
    }

    /// 输出列名切片。
    pub fn OutputNames(&self) -> &NameSlice {
        LogicalPlan::OutputNames(self)
    }

    /// 设置输出列名。
    pub fn SetOutputNames(&mut self, names: NameSlice) {
        LogicalPlan::SetOutputNames(self, names);
    }

    /// 设置 Schema。
    pub fn SetSchema(&mut self, schema: Schema) {
        self.BaseLogicalPlan.SetSchema(schema);
    }

    /// 同时设置 Schema 与输出列名。
    pub fn SetSchemaAndNames(&mut self, schema: Schema, names: NameSlice) {
        self.SetSchema(schema);
        self.SetOutputNames(names);
    }

    /// 按父节点用列内联裁剪本节点 Schema（列裁剪的简化路径）。
    pub fn InlineProjection(&mut self, parent_used_cols: &[Column]) {
        let used = self
            .SCtx()
            .map(|context| {
                expression::GetUsedList(
                    context.GetExprCtx().GetEvalCtx(),
                    parent_used_cols.to_vec(),
                    self.Schema(),
                )
            })
            .unwrap_or_else(|| {
                let used_ids: HashSet<_> = parent_used_cols
                    .iter()
                    .map(|column| column.UniqueID)
                    .collect();
                self.Schema()
                    .Columns
                    .iter()
                    .map(|column| used_ids.contains(&column.UniqueID))
                    .collect()
            });
        let mut used = used;
        if parent_used_cols.is_empty()
            && !used.is_empty()
            && let Some(chosen) = self
                .Schema()
                .Columns
                .iter()
                .enumerate()
                .min_by_key(|(_, column)| {
                    column
                        .RetType
                        .as_ref()
                        .map_or(isize::MAX, |field_type| field_type.GetFlen())
                })
                .map(|(index, _)| index)
        {
            used.fill(false);
            used[chosen] = true;
        }
        let schema = self.Schema_mut();
        let columns = std::mem::take(&mut schema.Columns);
        schema.Columns = columns
            .into_iter()
            .enumerate()
            .filter_map(|(index, column)| {
                used.get(index).copied().unwrap_or(false).then_some(column)
            })
            .collect();
    }

    /// 从子节点继承仍出现在输出中的主键/唯一键信息。
    pub fn BuildKeyInfo(&mut self) {
        self.Schema_mut().PKOrUK.clear();
        self.BaseLogicalPlan.BuildKeyInfo();
        let [child] = self.BaseLogicalPlan.Children() else {
            return;
        };
        let child_keys = child.Schema().PKOrUK.clone();
        let keys = child_keys
            .iter()
            .filter_map(|key| {
                self.Schema().ColumnsIndices(key).map(|indices| {
                    indices
                        .into_iter()
                        .map(|index| self.Schema().Columns[index].clone())
                        .collect()
                })
            })
            .collect();
        self.Schema_mut().PKOrUK = keys;
    }
}

impl LogicalPlan for LogicalSchemaProducer {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn base(&self) -> &BaseLogicalPlan {
        &self.BaseLogicalPlan
    }

    fn base_mut(&mut self) -> &mut BaseLogicalPlan {
        &mut self.BaseLogicalPlan
    }
}
