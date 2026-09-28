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

// 可能属性（possible properties）列剪枝信息收集。
//
// 与 Go 一致，以后序遍历递归准备子计划的属性，再把每个子树的完整结果及当前
// schema 交给当前逻辑算子。属性的生成、过滤与 TiFlash 标记均由具体算子决定。

use base_dependency::{LogicalPlan, PossiblePropertiesInfo};

pub(crate) fn prepare_possible_properties_for<T: ?Sized, Children, Prepare>(
    plan: &T,
    children: Children,
    prepare: Prepare,
) -> PossiblePropertiesInfo
where
    Children: for<'a> Fn(&'a T) -> Vec<&'a T> + Copy,
    Prepare: Fn(&T, &[PossiblePropertiesInfo]) -> PossiblePropertiesInfo + Copy,
{
    let children_properties = children(plan)
        .into_iter()
        .map(|child| prepare_possible_properties_for(child, children, prepare))
        .collect::<Vec<_>>();
    prepare(plan, &children_properties)
}

/// 自底向上收集整棵逻辑计划树的可能属性信息。
pub fn preparePossibleProperties(plan: &dyn LogicalPlan) -> PossiblePropertiesInfo {
    prepare_possible_properties_for(plan, LogicalPlan::logical_children, |node, children| {
        node.prepare_possible_properties(node.schema(), children)
    })
}
