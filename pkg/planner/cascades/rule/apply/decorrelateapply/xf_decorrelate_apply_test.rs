// Copyright 2025 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// Apply 解相关规则的单元测试。
//
// 含：嵌入的 Go 对照参考字符串，以及基于本 crate 桩类型的可执行测试，
// 验证无相关列时 Apply→Join、中间 Apply 移除、有相关列时保留原式。

const GO_REFERENCE: &str = r################"

// XFDeCorrelateSimpleApply 对 intermediary apply 的 memo 清理测试。

// test_xf_de_correlate_should_delete_intermediary_apply 对应 Go 的 TestXFDeCorrelateShouldDeleteIntermediaryApply。
// 原测试当前会 t.Skip；下面仍保留完整构造和断言顺序，方便后续恢复 cascades optimizer 接线时对照。
#[test]
fn test_xf_de_correlate_should_delete_intermediary_apply() {
    testing::skip("skip this test for now, cause decorrelateapply.XFDeCorrelateSimpleApply rule is not applied in the cascades optimizer fully, and this test is not meaningful now.");

    // Go 通过 failpoint 跳过 memo stats 派生，并用 defer 关闭；只保留启停语义。
    require::NoError(failpoint::Enable(
        "github.com/pingcap/tidb/pkg/planner/cascades/memo/MockPlanSkipMemoDeriveStats",
        "return(true)",
    ));
    // defer: failpoint::Disable("github.com/pingcap/tidb/pkg/planner/cascades/memo/MockPlanSkipMemoDeriveStats")

    let ctx = mock::NewContext();

    // new logical schema producer.
    let mut sp = logicalop::LogicalSchemaProducer {};
    let col1 = expression::Column { ID: 1, ..Default::default() };
    sp.SetSchema(expression::NewSchema(vec![col1]));
    let mut name = types::FieldName { ColName: ast::NewCIStr("a"), ..Default::default() };
    sp.SetOutputNames(types::NameSlice(vec![name]));
    sp.BaseLogicalPlan = logicalop::NewBaseLogicalPlan(ctx, "apply", None, 0);

    // 构造左侧 DataSource，列 ID 为 2；Hash64 用来确认两个子计划不是同一个 memo 指纹。
    let mut sp2 = logicalop::LogicalSchemaProducer {};
    let col2 = expression::Column { ID: 2, ..Default::default() };
    sp2.SetSchema(expression::NewSchema(vec![col2]));
    name = types::FieldName { ColName: ast::NewCIStr("b"), ..Default::default() };
    sp2.SetOutputNames(types::NameSlice(vec![name]));
    sp2.BaseLogicalPlan = logicalop::NewBaseLogicalPlan(ctx, "ds1", None, 0);
    let ds2 = logicalop::DataSource { LogicalSchemaProducer: sp2, ..Default::default() };

    // 构造右侧 DataSource，列 ID 为 3；后续断言要求左右孩子顺序不变。
    let mut sp3 = logicalop::LogicalSchemaProducer {};
    let col3 = expression::Column { ID: 3, ..Default::default() };
    sp3.SetSchema(expression::NewSchema(vec![col3]));
    name = types::FieldName { ColName: ast::NewCIStr("c"), ..Default::default() };
    sp3.SetOutputNames(types::NameSlice(vec![name]));
    sp3.BaseLogicalPlan = logicalop::NewBaseLogicalPlan(ctx, "ds2", None, 0);
    let ds3 = logicalop::DataSource { LogicalSchemaProducer: sp3, ..Default::default() };

    let mut hasher = base::NewHashEqualer();
    ds2.Hash64(&mut hasher);
    let ds2_hash64 = hasher.Sum64();
    hasher.Reset();
    ds3.Hash64(&mut hasher);
    let ds3_hash64 = hasher.Sum64();
    require::NotEqual(ds2_hash64, ds3_hash64);

    sp.BaseLogicalPlan.SetChildren(vec![ds2, ds3]);
    let join = logicalop::LogicalJoin { LogicalSchemaProducer: sp, ..Default::default() };
    let mut apply = logicalop::LogicalApply { LogicalJoin: join, NoDecorrelate: false, ..Default::default() };
    apply.SetSelf(&apply);
    apply.SetTP(plancodec::TypeApply);

    let mut mm = memo::NewMemo();
    mm.Init(&apply);
    let mut my_rule = decorrelateapply::NewXFDeCorrelateSimpleApply();
    let mut cas = cascades::NewOptimizer(&apply).unwrap();
    // only allow NewXFDeCorrelateSimpleApply.
    cas.SetRules(vec![my_rule.ID()]);
    // defer: cas.Destroy()
    require::Nil(cas.Execute());

    // 第一次执行时，普通 Apply 应该同时保留原 Apply 与新 Join 两个逻辑计划。
    let mut length = 0;
    cas.GetMemo().NewIterator().Each(|plan: corebase::LogicalPlan| {
        if length == 0 {
            let apply = plan.as_logical_apply();
            require::True(!apply.is_nil());
            require::True(apply.Schema().Columns[0].ID == 1);
            require::True(apply.Self() == apply);
            require::True(apply.TP() == plancodec::TypeApply);

            let ds1 = plan.Children()[0].as_data_source();
            require::True(ds1.Schema().Columns[0].ID == 2);
            let ds2 = plan.Children()[1].as_data_source();
            require::True(ds2.Schema().Columns[0].ID == 3);
        } else if length == 1 {
            let join = plan.as_logical_join();
            require::True(join.Schema().Columns[0].ID == 1);
            require::True(join.Schema().Columns[0] == apply.Schema().Columns[0]); // ref
            require::True(join.Self() == join);
            require::True(join.TP() == plancodec::TypeJoin);

            require::True(plan.Children()[0].as_data_source().Schema().Columns[0].ID == 2);
            require::True(plan.Children()[1].as_data_source().Schema().Columns[0].ID == 3);
        }
        length += 1;
        true
    });
    require::True(length == 2);

    // restore the original plan tree, and mark the Apply generated from xForm rule.
    // 这里的 flag 是本测试核心：标记为中间 Apply 后，再次 decorrelate 时旧 Apply 应从 memo 中移除。
    apply.BaseLogicalPlan.SetChildren(vec![ds2, ds3]);
    apply.SetFlag(logicalop::ApplyGenFromXFDeCorrelateRuleFlag);
    mm.Destroy();
    mm.Init(&apply);
    my_rule = decorrelateapply::NewXFDeCorrelateSimpleApply();
    cas = cascades::NewOptimizer(&apply).unwrap();
    cas.SetRules(vec![my_rule.ID()]);
    // defer: cas.Destroy()
    require::Nil(cas.Execute());

    // 第二次执行时只应剩下 Join，说明 intermediary Apply 已被清理。
    length = 0;
    cas.GetMemo().NewIterator().Each(|plan: corebase::LogicalPlan| {
        if length == 0 {
            let join = plan.as_logical_join();
            require::True(join.Schema().Columns[0].ID == 1);
            require::True(join.Schema().Columns[0] == apply.Schema().Columns[0]); // ref
            require::True(join.Self() == join);
            require::True(join.TP() == plancodec::TypeJoin);

            require::True(plan.Children()[0].as_data_source().Schema().Columns[0].ID == 2);
            require::True(plan.Children()[1].as_data_source().Schema().Columns[0].ID == 3);
        }
        length += 1;
        true
    });
    require::True(length == 1);
}
"################;

use super::*;
use cascades_rule::Rule as CascadesRule;

struct StringWriter(String);

impl cascades_util::StrBufferWriter for StringWriter {
    fn WriteString(&mut self, text: &str) {
        self.0.push_str(text);
    }

    fn Flush(&mut self) {}
}

/// 构造带指定列名与相关列的叶子 LogicalPlan::Node。
fn node(name: &str, columns: &[&str], correlated: &[&str]) -> LogicalPlan {
    LogicalPlan::Node(LogicalNode {
        name: name.to_owned(),
        schema: Schema {
            columns: columns.iter().map(|column| (*column).to_owned()).collect(),
        },
        correlated_columns: correlated
            .iter()
            .map(|column| (*column).to_owned())
            .collect(),
    })
}

/// 构造外层/内层双子树的 Apply 组表达式，便于驱动 XForm 单测。
fn apply_expression(flags: u64, inner_correlated: &[&str]) -> GroupExpression {
    let outer = node("outer", &["a", "b"], &[]);
    let inner = node("inner", &["c"], inner_correlated);
    let join = LogicalJoin {
        plan_id: 99,
        plan_type: "Apply".to_owned(),
        schema: Schema {
            columns: vec!["a".to_owned(), "c".to_owned()],
        },
        children: vec![outer.clone(), inner.clone()],
        statistics: Some("derived".to_owned()),
    };
    GroupExpression {
        logical_plan: LogicalPlan::Apply(LogicalApply {
            logical_join: join,
            no_decorrelate: false,
            flags,
        }),
        children: vec![
            GroupExpression {
                logical_plan: outer,
                children: Vec::new(),
            },
            GroupExpression {
                logical_plan: inner,
                children: Vec::new(),
            },
        ],
    }
}

/// 无相关列的 Apply 应改写为 Join，并保留原孩子；remove 为 false。
#[test]
fn decorrelation_rewrites_uncorrelated_apply_and_preserves_children() {
    let rule = NewXFDeCorrelateSimpleApply();
    let expression = apply_expression(0, &[]);
    assert!(rule.apply_base.PreCheck(&expression));
    let (plans, remove) = rule.XForm(&expression).unwrap();
    assert!(!remove);
    let [LogicalPlan::Join(join)] = plans.as_slice() else {
        panic!("expected one logical join");
    };
    assert_eq!(join.plan_type, "Join");
    assert_eq!(
        join.children,
        expression
            .logical_plan
            .as_apply()
            .unwrap()
            .logical_join
            .children
    );
    assert_eq!(join.statistics, None);
}

/// 带中间标志的 Apply 变换后应 remove；有相关列则不产生替代计划。
#[test]
fn intermediary_apply_is_removed_and_correlated_apply_is_retained() {
    let rule = NewXFDeCorrelateSimpleApply();
    let intermediary = apply_expression(APPLY_GEN_FROM_XF_DECORRELATE_RULE_FLAG, &[]);
    let (plans, remove) = rule.XForm(&intermediary).unwrap();
    assert_eq!(plans.len(), 1);
    assert!(remove);

    let correlated = apply_expression(0, &["a"]);
    let (plans, remove) = rule.XForm(&correlated).unwrap();
    assert!(plans.is_empty());
    assert!(!remove);
}

/// The Go BaseRule.String implementation maps rule type 2 to default_none.
#[test]
fn rule_string_matches_go_base_rule() {
    let rule = NewXFDeCorrelateSimpleApply();
    let mut writer = StringWriter(String::new());
    CascadesRule::String(&rule, &mut writer);
    assert_eq!(writer.0, "default_none");
}
