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

// 规划器遥测（telemetry）辅助：检测执行计划树是否涉及 TiFlash。
//
// TiFlash 是列存分析引擎；ExchangeSender/Receiver 是 MPP（大规模并行处理）
// 任务间的数据交换算子。遥测用这两个布尔结果统计 TiFlash/MPP 使用情况。

use crate::{PlanNode, StoreType};

/// 递归扫描物理计划树，返回 `(含 TiFlash TableReader, 其表计划根为 ExchangeSender)`。
pub fn IsTiFlashContained(plan: &PlanNode) -> (bool, bool) {
    // Go 的 base.PhysicalPlan 断言会在逻辑节点处终止遍历。
    if matches!(
        plan.kind,
        crate::PlanKind::DataSource { .. } | crate::PlanKind::Join { .. }
    ) {
        return (false, false);
    }

    // Go 只按 PhysicalTableReader.StoreType 统计 TiFlash；其内部 table plan
    // 不作为普通子树递归，而只检查根算子是否为 ExchangeSender。
    if matches!(plan.kind, crate::PlanKind::TableReader) {
        let tiflash = plan.store_type == StoreType::TiFlash;
        let exchange = tiflash
            && plan
                .children
                .first()
                .is_some_and(|child| matches!(child.kind, crate::PlanKind::ExchangeSender { .. }));
        return (tiflash, exchange);
    }

    // 与 Go 一致，在发现首个 TiFlash reader 后停止扫描。
    for child in &plan.children {
        let result = IsTiFlashContained(child);
        if result.0 {
            return result;
        }
    }
    (false, false)
}
