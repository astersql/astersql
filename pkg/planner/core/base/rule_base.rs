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

// 逻辑优化规则（Logical Optimization Rule）抽象。
//
// 规则接收一棵逻辑计划树并产出可能改写后的树；优化器按编排顺序反复应用规则，
// 直至收敛或达到截止条件。接口本身不启动异步任务，也不执行存储 IO。

/// 对应 Go 的 `LogicalOptRule`，覆盖去关联、谓词下推、列裁剪等逻辑优化规则。
pub trait LogicalOptRule {
    /// 对逻辑计划应用规则。
    ///
    /// 返回元组依次保留 Go 语义：优化后的计划、计划是否发生变化，以及潜在错误。
    /// changed 为 true 时优化器可以据此触发后续交互规则；默认 false 表示无需触发。
    /// Context 保存取消和截止信息，接口本身不启动异步任务或外部 IO。
    fn optimize(
        &self,
        ctx: &tokio_util::sync::CancellationToken,
        plan: Box<dyn crate::LogicalPlan>,
    ) -> Result<(Box<dyn crate::LogicalPlan>, bool), crate::Error>;

    /// 返回稳定的规则名称，供规则编排、诊断与跟踪使用。
    fn name(&self) -> &str;
}
