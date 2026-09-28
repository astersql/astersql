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

// Runtime Filter（运行时过滤器）生成器。
//
// 在物理执行计划（Physical Plan）树上遍历 HashJoin，把等值连接条件下推到
// 下游 TableScan，生成 Runtime Filter 描述。执行期由 build 侧产出过滤值，
// probe/scan 侧据此减少扫描行数。TiFlash（列存加速副本）上使用 Global 模式，
// 其余存储引擎使用 Local 模式。

use crate::{JoinType, PlanKind, PlanNode};

/// Runtime Filter 作用域：Local 仅本 Fragment，Global 可跨 Fragment 传递。
/// Fragment 是分布式执行计划中的一段可独立调度的算子子树。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeFilterMode {
    /// 同一 Fragment 内生效。
    Local,
    /// 可跨 Fragment（常见于 TiFlash）生效。
    Global,
}

/// 一条 Runtime Filter：记录源 Join、目标 Scan 及 build/probe 表达式。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeFilter {
    /// 过滤器唯一编号。
    pub id: usize,
    /// 产生过滤条件的 HashJoin 计划节点 ID。
    pub source_join: i32,
    /// 接收过滤条件的 TableScan 计划节点 ID。
    pub target_scan: i32,
    /// Build 侧等值表达式文本。
    pub build_expr: String,
    /// Probe 侧等值表达式文本。
    pub probe_expr: String,
    /// Local 或 Global 作用域。
    pub mode: RuntimeFilterMode,
}

/// 遍历计划树并收集 RuntimeFilter 的状态机。
#[derive(Default)]
pub struct RuntimeFilterGenerator {
    /// 下一个可用过滤器 ID。
    next_id: usize,
    /// 已生成的过滤器列表。
    pub filters: Vec<RuntimeFilter>,
}

impl RuntimeFilterGenerator {
    /// 从根计划节点开始生成 Runtime Filter。
    pub fn GenerateRuntimeFilter(&mut self, plan: &PlanNode) {
        self.visit(plan, Vec::new());
    }

    /// 深度优先遍历：TiFlash HashJoin 为每条等值条件构造候选，只将其传给
    /// probe 子树；ExchangeReceiver 是 Fragment 边界，当前 Go 实现会丢弃跨
    /// Fragment 的 Global RF。
    fn visit(
        &mut self,
        plan: &PlanNode,
        mut sources: Vec<(i32, String, String, RuntimeFilterMode)>,
    ) {
        if matches!(plan.kind, PlanKind::ExchangeReceiver { .. }) {
            for source in &mut sources {
                source.3 = RuntimeFilterMode::Global;
            }
        }

        if matches!(plan.kind, PlanKind::TableScan { .. }) {
            for (join, build, probe, mode) in sources {
                // Go `assignRuntimeFilter` currently removes unsupported Global RFs.
                if mode == RuntimeFilterMode::Global {
                    continue;
                }
                self.filters.push(RuntimeFilter {
                    id: self.next_id,
                    source_join: join,
                    target_scan: plan.id,
                    build_expr: build,
                    probe_expr: probe,
                    mode,
                });
                self.next_id += 1;
            }
            return;
        }

        if let PlanKind::HashJoin {
            inner_child,
            equal_conditions,
        } = &plan.kind
        {
            for (child_idx, child) in plan.children.iter().enumerate() {
                let mut child_sources = sources.clone();
                if plan.store_type == crate::StoreType::TiFlash && child_idx != *inner_child {
                    child_sources.extend(equal_conditions.iter().map(|(build, probe)| {
                        (
                            plan.id,
                            build.clone(),
                            probe.clone(),
                            RuntimeFilterMode::Local,
                        )
                    }));
                }
                self.visit(child, child_sources);
            }
            return;
        }

        for child in &plan.children {
            self.visit(child, sources.clone());
        }
    }

    /// 按 build 侧位置判断连接类型是否允许生成 Runtime Filter。
    pub fn matchRFJoinType(join: JoinType, right_is_build_side: bool) -> bool {
        if right_is_build_side {
            !matches!(
                join,
                JoinType::LeftOuterJoin
                    | JoinType::AntiSemiJoin
                    | JoinType::LeftOuterSemiJoin
                    | JoinType::AntiLeftOuterSemiJoin
            )
        } else {
            join != JoinType::RightOuterJoin
        }
    }

    /// 从 build 节点向下查找目标 Scan；ExchangeReceiver 是 Fragment 边界。
    pub fn belongsToSameFragment(source: &PlanNode, target: &PlanNode) -> bool {
        if matches!(source.kind, PlanKind::ExchangeReceiver { .. }) {
            return false;
        }
        if matches!(source.kind, PlanKind::TableScan { .. }) {
            return source.id == target.id;
        }
        source
            .children
            .iter()
            .any(|child| Self::belongsToSameFragment(child, target))
    }
}
