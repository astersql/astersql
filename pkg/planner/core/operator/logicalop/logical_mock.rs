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

// 逻辑算子：测试用 Mock 数据源（MockDataSource）。
//
// 仅用于单元测试/规则测试中构造最小逻辑计划树，类型名为 `mockDS`。
// 不实现真实扫描，只提供 BaseLogicalPlan 挂钩。

use crate::{BaseLogicalPlan, LogicalPlan};
use std::any::Any;

/// 测试桩数据源：无真实表扫描语义。
#[derive(Default)]
pub struct MockDataSource {
    /// 基类逻辑计划字段。
    pub BaseLogicalPlan: BaseLogicalPlan,
}

impl MockDataSource {
    /// 初始化为 mockDS 节点。
    pub fn Init(mut self, ctx: base::ContextRef) -> Self {
        self.BaseLogicalPlan = crate::NewBaseLogicalPlan(ctx, "mockDS", 0);
        self
    }
}

impl LogicalPlan for MockDataSource {
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
