// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 物理 Implementation：Memo Group 上带代价的物理计划候选。
//
// Cascades 在探索出逻辑等价类后，为每个物理属性需求选出实现；
// 本 trait 描述代价计算、子实现挂接与代价上限剪枝。

use astersql_planner_core_base::PhysicalPlan;
use std::cell::RefCell;
use std::rc::Rc;

/// 物理实现的共享可变引用（`Rc<RefCell<dyn Implementation>>`）。
pub type ImplementationRef = Rc<RefCell<dyn Implementation>>;

/// A costed physical implementation of a memo group.
/// Memo Group 上带代价的物理实现：绑定物理计划并参与代价枚举。
pub trait Implementation {
    /// 按输出行数与子实现代价计算本实现代价。
    fn CalcCost(&self, out_count: f64, children: &[ImplementationRef]) -> f64;
    /// 写入已算出的代价。
    fn SetCost(&mut self, cost: f64);
    /// 读取已缓存的代价。
    fn GetCost(&self) -> f64;
    /// 返回对应的物理计划（PhysicalPlan）节点。
    fn GetPlan(&self) -> &dyn PhysicalPlan;

    /// Attaches child implementations and returns this implementation.
    /// 挂接子实现并返回自身，供上层继续组树。
    fn AttachChildren(&mut self, children: &[ImplementationRef]) -> &mut dyn Implementation;

    /// Returns the remaining limit before the next child group is implemented.
    /// 在实现下一子 Group 前，根据已实现子节点折算出剩余代价上限（剪枝用）。
    fn GetCostLimit(&self, cost_limit: f64, children: &[ImplementationRef]) -> f64;
}
