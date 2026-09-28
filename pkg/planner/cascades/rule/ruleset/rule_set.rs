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

// Cascades 按 Operand 组织的规则集合与过滤逻辑。
//
// `OperandRules` 同时持有完整规则列表与按 SetType 划分的子集；当组表达式
// 带有「由解相关规则生成」的 Apply 中间标志时，走解相关专用短路路径，
// 避免对中间态再套用不相关规则。

use cascades_memo::GroupExpressionRef;
use cascades_pattern::Operand;
use cascades_rule::Rule;
use decorrelateapply::NewXFDeCorrelateSimpleApply;
use logicalop::APPLY_GEN_FROM_XF_DECORRELATE_RULE_FLAG;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// 规则子集类别；用 usize 区分默认集与解相关 Apply 集等。
pub type SetType = usize;
/// 未指定/默认规则子集。
pub const DefaultNone: SetType = 0;
/// 解相关 Apply 专用规则子集。
pub const XFSetDeCorrelateApply: SetType = 1;

/// 返回默认的 Operand→规则集映射；当前与 Go 一致为空（Apply 注册被注释）。
pub fn DefaultRuleSets() -> HashMap<Operand, OperandRules> {
    // The Go map is currently empty as well; Apply registration is commented out.
    // Go 侧映射目前同样为空；Apply 相关注册仍注释掉。
    HashMap::new()
}

/// 返回 Apply 根算子的解相关规则子集。
///
/// Go 侧是包级共享 map；Rust 规则对象是拥有型 trait object，因此每次
/// 调用构造等价的新集合，避免全局可变状态，同时保持规则顺序与 ID 不变。
pub fn OperandApplyRulesMap() -> BTreeMap<SetType, ListRules> {
    let mut rules = BTreeMap::new();
    rules.insert(
        XFSetDeCorrelateApply,
        ListRules::New(vec![Box::new(NewXFDeCorrelateSimpleApply())]),
    );
    rules
}

/// 返回 Apply 根算子的完整规则列表。
pub fn OperandApplyRulesList() -> ListRules {
    ListRules::New(vec![Box::new(NewXFDeCorrelateSimpleApply())])
}

/// 规则启用掩码：有序集合记录允许应用的规则 ID。
#[derive(Default)]
pub struct RuleMask(BTreeSet<usize>);

impl RuleMask {
    /// 由一组规则 ID 构造掩码。
    pub fn New(ids: impl IntoIterator<Item = usize>) -> Self {
        Self(ids.into_iter().collect())
    }

    /// 测试指定规则 ID 是否在掩码中。
    pub fn Test(&self, id: usize) -> bool {
        self.0.contains(&id)
    }
}

/// 有序规则列表（保留注册顺序，供 Filter 后仍按原序返回）。
#[derive(Default)]
pub struct ListRules(Vec<Box<dyn Rule>>);

impl ListRules {
    /// 由规则对象列表构造。
    pub fn New(rules: Vec<Box<dyn Rule>>) -> Self {
        Self(rules)
    }
    /// 规则条数。
    pub fn Len(&self) -> usize {
        self.0.len()
    }
    /// 按注册顺序迭代规则。
    pub fn Iter(&self) -> impl Iterator<Item = &dyn Rule> {
        self.0.iter().map(Box::as_ref)
    }

    /// 按掩码过滤，保留原列表中的相对顺序。
    pub fn Filter(&self, mask: &RuleMask) -> Vec<&dyn Rule> {
        self.Iter().filter(|rule| mask.Test(rule.ID())).collect()
    }
}

/// 某一 Operand 下的规则：完整列表 + 按 SetType 索引的子集。
pub struct OperandRules {
    /// SetType → 规则子集。
    setMap: BTreeMap<SetType, ListRules>,
    /// 默认完整规则列表。
    setList: ListRules,
}

/// 构造 OperandRules。
pub fn NewOperandRules(set_map: BTreeMap<SetType, ListRules>, set_list: ListRules) -> OperandRules {
    OperandRules {
        setMap: set_map,
        setList: set_list,
    }
}

/// 构造 Apply 根算子的规则集合。
pub fn OperandApplyRules() -> OperandRules {
    NewOperandRules(OperandApplyRulesMap(), OperandApplyRulesList())
}

impl OperandRules {
    /// 按组表达式选择规则列表：解相关中间 Apply 走子集，否则用完整列表。
    pub fn Filter(&self, expression: &GroupExpressionRef) -> &ListRules {
        // 检查是否带有「由 XF 解相关规则生成」的 Apply 中间标志。
        let is_intermediate_apply = {
            let expression = expression.borrow();
            expression
                .GetWrappedLogicalPlan()
                .base()
                .HasFlag(APPLY_GEN_FROM_XF_DECORRELATE_RULE_FLAG)
        };
        if is_intermediate_apply {
            self.setMap
                .get(&XFSetDeCorrelateApply)
                .expect("de-correlate Apply subset must be registered")
        } else {
            &self.setList
        }
    }
}
