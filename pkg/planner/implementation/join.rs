// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 二元 Join 物理实现的 Implementation：HashJoin 与 MergeJoin。
//
// 两者共用同一代价公式：自身 Join 代价（由左右行数决定）+ 左右子树代价。
// HashJoin 用哈希表匹配，MergeJoin 要求两侧有序后归并匹配。

use astersql_planner_core_base::PhysicalPlan;
use astersql_planner_memo::ImplementationRef;

use crate::{
    AttachChildren, BaseImpl, BinaryJoinCostPlan, ChildCost, ChildRows, impl_implementation,
};

/// 生成二元 Join Implementation：代价 = SelfCost(左行, 右行) + 左代价 + 右代价。
macro_rules! binary_join_implementation {
    ($name:ident, $constructor:ident) => {
        /// 二元 Join 物理实现包装。
        pub struct $name {
            pub(crate) base: BaseImpl,
            plan_node: Box<dyn BinaryJoinCostPlan>,
        }

        /// 由实现了 `BinaryJoinCostPlan` 的物理计划构造 Implementation。
        pub fn $constructor(plan: Box<dyn BinaryJoinCostPlan>) -> $name {
            $name {
                base: BaseImpl::default(),
                plan_node: plan,
            }
        }

        impl $name {
            /// 汇总 Join 自身代价与两侧子 Implementation 代价。
            fn calc_cost(&self, _out_count: f64, children: &[ImplementationRef]) -> f64 {
                let self_cost = self
                    .plan_node
                    .SelfCost(ChildRows(children, 0), ChildRows(children, 1));
                let cost = self_cost + ChildCost(children, 0) + ChildCost(children, 1);
                self.base.SetCost(cost);
                cost
            }
            fn plan(&self) -> &dyn PhysicalPlan {
                self.plan_node.Plan()
            }
            fn attach_children(&mut self, children: &[ImplementationRef]) {
                AttachChildren(self.plan_node.PlanMut(), children);
            }
            fn cost_limit(&self, cost_limit: f64, children: &[ImplementationRef]) -> f64 {
                self.base.GetCostLimit(cost_limit, children)
            }
        }

        impl_implementation!($name);
    };
}

// HashJoin：构建侧建哈希表，探测侧探测匹配。
binary_join_implementation!(HashJoinImpl, NewHashJoinImpl);
// MergeJoin：两侧按连接键有序后归并连接。
binary_join_implementation!(MergeJoinImpl, NewMergeJoinImpl);
