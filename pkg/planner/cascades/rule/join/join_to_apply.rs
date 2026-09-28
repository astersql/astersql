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

// Join→Apply 变换规则占位实现。
//
// Pattern 为 Join(Any, Join)，限 TiDB 引擎。Go 侧 XForm 仍为 TODO，本实现
// 同样返回空替代计划，避免臆造不安全的 Apply 语义。

use cascades_pattern::{EngineAll, EngineTiDBOnly, NewPattern, OperandAny, OperandJoin, Pattern};
use cascades_rule::{BaseRule, BoundPlan, NewBaseRule, Rule, RuleError, XFJoinToApply as RuleType};

/// Join 转 Apply 的 Cascades 变换规则（当前 XForm 为空实现）。
pub struct XFJoinToApply {
    /// 规则元数据（ID 与 Pattern）。
    BaseRule: BaseRule,
}

/// 构造规则：Pattern 为 Join(Any@All, Join@TiDBOnly)，规则类型 XFJoinToApply。
pub fn NewJoinToApply() -> XFJoinToApply {
    let mut pattern = NewPattern(OperandJoin, EngineTiDBOnly);
    pattern.SetChildren(vec![
        NewPattern(OperandAny, EngineAll),
        NewPattern(OperandJoin, EngineTiDBOnly),
    ]);
    XFJoinToApply {
        BaseRule: NewBaseRule(RuleType, pattern),
    }
}

impl Rule for XFJoinToApply {
    fn ID(&self) -> usize {
        self.BaseRule.ID()
    }

    fn String(&self, writer: &mut dyn cascades_util::StrBufferWriter) {
        self.BaseRule.String(writer);
    }

    fn Pattern(&self) -> &Pattern {
        self.BaseRule.Pattern()
    }

    /// 匹配阶段恒通过；具体可行性留给未来 XForm 实现。
    fn Match(&self, _plan: &BoundPlan) -> bool {
        true
    }

    fn XForm(
        &self,
        _plan: &BoundPlan,
    ) -> Result<(Vec<logicalop::LogicalPlanRef>, bool), RuleError> {
        // The Go implementation is intentionally still a TODO and returns no
        // alternatives; retaining that behavior avoids inventing unsafe Apply semantics.
        // Go 实现仍为 TODO 且不返回替代计划；此处保持相同行为。
        Ok((Vec::new(), false))
    }
}
