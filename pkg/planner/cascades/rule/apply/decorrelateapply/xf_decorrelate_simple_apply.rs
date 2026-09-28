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

// 简单 Apply 解相关变换规则：无相关列时将 Apply 改写为普通 Join。
//
// 匹配模式为 Apply(Any, Any)。若内层相对外层 schema 无相关列，则浅拷贝
// Join 骨架并 `realloc_for_cascades`；若 Apply 带有解相关规则生成标志，
// 则同时指示调用方从 memo 移除原中间 Apply。

use crate::xf_decorrelate_apply_base::{
    APPLY_GEN_FROM_XF_DECORRELATE_RULE_FLAG, BaseRule, Engine, GroupExpression, LogicalPlan,
    Operand, Pattern, Result, Rule, XF_DECORRELATE_SIMPLE_APPLY_ID, XFDeCorrelateApplyBase,
    extract_correlated_columns,
};
use cascades_pattern::{
    BuildPattern as BuildCascadesPattern, EngineTiDBOnly, NewPattern as NewCascadesPattern,
    OperandAny as CascadesOperandAny, OperandApply as CascadesOperandApply,
};
use cascades_rule::{
    BaseRule as CascadesBaseRule, BoundPlan, NewBaseRule as NewCascadesBaseRule,
    Rule as CascadesRule, RuleError as CascadesRuleError,
    XFDeCorrelateSimpleApply as CascadesRuleType,
};
use logicalop::LogicalPlan as CascadesLogicalPlan;

/// 简单 Apply 解相关规则：组合共享基类完成 PreCheck 与元数据。
pub struct XFDeCorrelateSimpleApply {
    /// 共享解相关基类（含 BaseRule / PreCheck）。
    pub apply_base: XFDeCorrelateApplyBase,
    /// 真实 Cascades 规则元数据；与轻量迁移桩并存，供规则集注册。
    pub cascades_base: CascadesBaseRule,
}

/// 构造规则：Pattern 为 Apply 下挂两个 Any 孩子，引擎限 TiDB。
pub fn new_xf_decorrelate_simple_apply() -> XFDeCorrelateSimpleApply {
    let mut pattern = Pattern::new(Operand::Apply, Engine::TiDbOnly);
    pattern.set_children(vec![
        Pattern::new(Operand::Any, Engine::TiDbOnly),
        Pattern::new(Operand::Any, Engine::TiDbOnly),
    ]);
    XFDeCorrelateSimpleApply {
        apply_base: XFDeCorrelateApplyBase {
            base_rule: BaseRule::new(XF_DECORRELATE_SIMPLE_APPLY_ID, pattern),
        },
        cascades_base: NewCascadesBaseRule(
            CascadesRuleType,
            BuildCascadesPattern(
                CascadesOperandApply,
                EngineTiDBOnly,
                vec![
                    NewCascadesPattern(CascadesOperandAny, EngineTiDBOnly),
                    NewCascadesPattern(CascadesOperandAny, EngineTiDBOnly),
                ],
            ),
        ),
    }
}

impl XFDeCorrelateSimpleApply {
    /// 执行变换：无相关列则产出 Join；有相关列则返回空列表。
    ///
    /// 返回值第二项表示是否应移除原 Apply（中间 Apply 标志为真时）。
    pub fn xform(&self, apply_expression: &GroupExpression) -> Result<(Vec<LogicalPlan>, bool)> {
        let [outer_expression, inner_expression] = apply_expression.children.as_slice() else {
            return Err(crate::xf_decorrelate_apply_base::Error::new(
                "decorrelate Apply requires exactly two children",
            ));
        };
        let apply = apply_expression
            .wrapped_logical_plan()
            .as_apply()
            .expect("Apply pattern must bind LogicalApply");
        // 中间 Apply（由本规则生成）在再次变换后应从 memo 删除。
        let remove = apply.has_flag(APPLY_GEN_FROM_XF_DECORRELATE_RULE_FLAG);

        // Never mutate apply's correlated-column state in place: doing so would
        // change its memo hash and require reinsertion into the group.
        // 不就地修改相关列状态，以免改变 memo 哈希而需重新插入 Group。
        let correlated = extract_correlated_columns(
            inner_expression.wrapped_logical_plan(),
            outer_expression.wrapped_logical_plan().schema(),
        );
        if !correlated.is_empty() {
            // 仍有相关列，无法安全地简单解相关。
            return Ok((Vec::new(), false));
        }

        // 无相关列：复制 Join 并重新分配 Cascades 元数据后作为替代计划。
        let mut join = apply.logical_join.shallow_ref();
        join.realloc_for_cascades();
        assert!(
            !join.children.is_empty(),
            "Apply's cloned LogicalJoin must retain children"
        );
        Ok((vec![LogicalPlan::Join(join)], remove))
    }

    /// Go 风格 ID() 别名。
    #[allow(non_snake_case)]
    pub fn ID(&self) -> usize {
        self.id()
    }

    /// Go 风格 XForm() 别名。
    #[allow(non_snake_case)]
    pub fn XForm(&self, expression: &GroupExpression) -> Result<(Vec<LogicalPlan>, bool)> {
        self.xform(expression)
    }
}

impl Rule for XFDeCorrelateSimpleApply {
    fn id(&self) -> usize {
        XF_DECORRELATE_SIMPLE_APPLY_ID
    }

    fn base_rule(&self) -> &BaseRule {
        &self.apply_base.base_rule
    }

    fn pre_check(&self, expression: &GroupExpression) -> bool {
        self.apply_base.pre_check(expression)
    }

    fn xform(&self, expression: &GroupExpression) -> Result<(Vec<LogicalPlan>, bool)> {
        XFDeCorrelateSimpleApply::xform(self, expression)
    }
}

/// 将同一个规则接入真实 Cascades 规则接口。
///
/// `Memo::CopyIn` 已将逻辑计划的子树拆到 Group 输入中，因此 Apply→Join
/// 输出只需携带 Join 本体；CopyIn 会沿输入 Group 重新建立子关系，和 Go
/// `LogicalJoinShallowRef`/`NewGroupExprWithChildren` 的所有权模型一致。
impl CascadesRule for XFDeCorrelateSimpleApply {
    fn ID(&self) -> usize {
        CascadesRuleType as usize
    }

    fn String(&self, writer: &mut dyn cascades_util::StrBufferWriter) {
        // Keep the rule name identical to Go's BaseRule.String: Type 2 is
        // intentionally represented as "default_none" there.
        self.cascades_base.String(writer);
    }

    fn Pattern(&self) -> &cascades_pattern::Pattern {
        self.cascades_base.Pattern()
    }

    fn PreCheck(&self, plan: &BoundPlan) -> bool {
        plan.WithWrappedLogicalPlan(|logical_plan| {
            logical_plan
                .as_any()
                .downcast_ref::<logicalop::LogicalApply>()
                .is_some_and(|apply| !apply.NoDecorrelate)
        })
    }

    fn XForm(
        &self,
        plan: &BoundPlan,
    ) -> std::result::Result<(Vec<logicalop::LogicalPlanRef>, bool), CascadesRuleError> {
        let children = plan.Children();
        if children.len() != 2 {
            return Err(CascadesRuleError(
                "decorrelate Apply requires exactly two children".to_owned(),
            ));
        }

        let outer_schema = children[0].WithWrappedLogicalPlan(|outer| outer.Schema().Clone());
        let correlated = children[1].WithWrappedLogicalPlan(|inner| {
            coreusage::ExtractCorColumnsBySchema4LogicalPlan(inner, &outer_schema)
        });
        let (remove, mut join, schema, output_names) = plan.WithWrappedLogicalPlan(|logical| {
            let Some(apply) = logical.as_any().downcast_ref::<logicalop::LogicalApply>() else {
                return Err(CascadesRuleError(
                    "decorrelate Apply requires a LogicalApply root".to_owned(),
                ));
            };
            Ok((
                apply
                    .base()
                    .HasFlag(logicalop::APPLY_GEN_FROM_XF_DECORRELATE_RULE_FLAG),
                apply.LogicalJoin.LogicalJoinShallowRef(),
                apply.Schema().Clone(),
                apply.OutputNames().Shallow(),
            ))
        })?;

        if !correlated.is_empty() {
            return Ok((Vec::new(), false));
        }

        if let Some(ctx) = plan.WithWrappedLogicalPlan(|logical| logical.SCtx().cloned()) {
            join.LogicalSchemaProducer.BaseLogicalPlan = logicalop::NewBaseLogicalPlan(
                ctx,
                "Join",
                plan.WithWrappedLogicalPlan(|logical| logical.QueryBlockOffset()),
            );
        }
        join.SetSchema(schema);
        join.SetOutputNames(output_names);
        join.base_mut().ReAlloc4Cascades("Join");
        Ok((vec![Box::new(join)], remove))
    }
}

/// Go 风格构造函数别名。
#[allow(non_snake_case)]
pub fn NewXFDeCorrelateSimpleApply() -> XFDeCorrelateSimpleApply {
    new_xf_decorrelate_simple_apply()
}
