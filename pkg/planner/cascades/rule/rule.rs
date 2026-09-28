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

// Cascades 变换规则的核心 trait 与基类。
//
// 规则（Rule）描述「如何把一种逻辑计划改写成等价形式」：先用 Pattern
// 描述可匹配的算子形状，再经 PreCheck / Match 过滤，最后由 XForm 产出
// 新的逻辑计划（LogicalPlan）列表。BaseRule 提供仅含类型与 Pattern 的骨架。

use crate::{BoundPlan, Type};
use cascades_pattern::Pattern;
use std::fmt;

/// 规则应用过程中的错误；载荷为可读消息字符串。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuleError(pub String);

impl fmt::Display for RuleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for RuleError {}

/// Cascades 逻辑变换规则契约。
///
/// 调度器对候选 BoundPlan（已绑定到 Memo 的表达式树）依次调用本 trait：
/// Pattern 决定结构匹配，PreCheck/Match 做语义过滤，XForm 生成等价改写。
pub trait Rule {
    /// 规则唯一 ID，用于规则掩码（RuleMask）过滤。
    fn ID(&self) -> usize;
    /// 将规则可读名称写入缓冲区。
    fn String(&self, writer: &mut dyn cascades_util::StrBufferWriter);
    /// 返回该规则用于结构匹配的 Pattern。
    fn Pattern(&self) -> &Pattern;
    /// 轻量前置检查；默认恒为 true。
    fn PreCheck(&self, _plan: &BoundPlan) -> bool {
        true
    }
    /// 语义级匹配；默认恒为 true。
    fn Match(&self, _plan: &BoundPlan) -> bool {
        true
    }
    /// 执行变换：返回新逻辑计划列表，以及是否需要继续探索等标志。
    fn XForm(
        &self,
        _plan: &BoundPlan,
    ) -> Result<(Vec<logicalop::LogicalPlanRef>, bool), RuleError> {
        Ok((Vec::new(), false))
    }
}

/// 仅携带规则类型与 Pattern 的基类；具体规则可组合或包装它。
pub struct BaseRule {
    /// 规则类型枚举（见 rule_type）。
    tp: Type,
    /// 结构匹配模式。
    pattern: Pattern,
}

/// 构造 BaseRule。
pub fn NewBaseRule(tp: Type, pattern: Pattern) -> BaseRule {
    BaseRule { tp, pattern }
}

impl Rule for BaseRule {
    fn ID(&self) -> usize {
        0
    }

    fn String(&self, writer: &mut dyn cascades_util::StrBufferWriter) {
        writer.WriteString(self.tp.String());
    }

    fn Pattern(&self) -> &Pattern {
        &self.pattern
    }
}
