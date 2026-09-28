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

// 逻辑算子：分区表 UNION ALL（LogicalPartitionUnionAll / PartitionUnion）。
//
// 分区表（Partitioned Table）按分区拆成多路扫描后，用本节点做无去重合并。
// 内部复用 LogicalUnionAll 的列裁剪与统计推导，类型标识为 PartitionUnion。

use crate::*;
use std::any::Any;

/// 分区 UNION ALL：包装 LogicalUnionAll，语义为分区并行扫描合并。
#[derive(Default)]
pub struct LogicalPartitionUnionAll {
    /// 底层普通 UNION ALL 实现。
    pub LogicalUnionAll: LogicalUnionAll,
}

impl LogicalPartitionUnionAll {
    /// 初始化为 PartitionUnion 节点。
    pub fn Init(mut self, ctx: base::ContextRef, offset: i32) -> Self {
        self.LogicalUnionAll.LogicalSchemaProducer.BaseLogicalPlan =
            NewBaseLogicalPlan(ctx, "PartitionUnion", offset);
        self
    }
    /// 列裁剪委托给内部 UnionAll。
    pub fn PruneColumns(&mut self, columns: &[Column]) -> Result<()> {
        self.LogicalUnionAll.PruneColumns(columns)
    }
    /// 为每个分区分支下推 `offset + count` 的 TopN，并保留原 TopN 在 Union 之上。
    pub fn PushDownTopN(&mut self, mut top_n: Option<LogicalPlanRef>) -> Option<LogicalPlanRef> {
        let pushed_top_n = top_n.as_ref().map(|plan| {
            let top_n = plan
                .as_any()
                .downcast_ref::<LogicalTopN>()
                .expect("LogicalPartitionUnionAll::PushDownTopN expects LogicalTopN");
            (
                top_n.Count.wrapping_add(top_n.Offset),
                top_n.PreferLimitToCop,
                top_n.ByItems.clone(),
                top_n
                    .SCtx()
                    .cloned()
                    .expect("initialized TopN must retain planner context"),
                top_n.QueryBlockOffset(),
            )
        });

        for child in self.Children_mut() {
            let branch_top_n = pushed_top_n.as_ref().map(
                |(count, prefer_limit_to_cop, by_items, ctx, query_block_offset)| {
                    Box::new(
                        LogicalTopN {
                            Count: *count,
                            PreferLimitToCop: *prefer_limit_to_cop,
                            ByItems: by_items.clone(),
                            ..LogicalTopN::default()
                        }
                        .Init(ctx.clone(), *query_block_offset),
                    ) as LogicalPlanRef
                },
            );
            if let Some(pushed) = child.PushDownTopN(branch_top_n) {
                *child = pushed;
            }
        }

        let current = std::mem::take(self);
        if let Some(plan) = top_n.as_mut() {
            plan.SetChildren(vec![Box::new(current)]);
            top_n
        } else {
            Some(Box::new(current))
        }
    }
}

impl LogicalPlan for LogicalPartitionUnionAll {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
    fn base(&self) -> &BaseLogicalPlan {
        self.LogicalUnionAll.base()
    }
    fn base_mut(&mut self) -> &mut BaseLogicalPlan {
        self.LogicalUnionAll.base_mut()
    }
    fn PruneColumns(&mut self, columns: &[Column]) -> Result<()> {
        Self::PruneColumns(self, columns)
    }
    fn PushDownTopN(&mut self, top_n: Option<LogicalPlanRef>) -> Option<LogicalPlanRef> {
        Self::PushDownTopN(self, top_n)
    }
    fn DeriveStats(&mut self, reload: bool) -> Result<(StatsInfo, bool)> {
        self.LogicalUnionAll.DeriveStats(reload)
    }
}
