// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// [`IsNullRejected`] 与 builtin 登记表的单元测试。
//
// 安装精简整型 builtin 工厂，构造内表/外表列与 AND/OR/NOT/IN 谓词，
// 校验 null-reject 证明在常见模式下的真假结果。

use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use super::IsNullRejected;
use crate::null_misc_builtins::{
    NULL_REJECT_NULL_PRESERVING_FUNCTIONS, NULL_REJECT_REJECT_NULL_TESTS, NullRejectTestMode,
    is_null_reject_null_preserving, null_reject_test_mode,
};
use expression::{mysql, types};
use parser_ast::functions as parser_ast;
use plan_base::PlanContext as _;

/// 测试用 PlanContext：最小会话变量与表达式上下文。
struct TestPlanContext {
    /// 分配计划节点 ID 的计数器。
    plan_id: AtomicI32,
    /// 会话变量。
    session: planctx::variable::SessionVars,
    /// 表达式求值/构建上下文。
    expression: Arc<exprstatic::ExprContext>,
    /// 构建 protobuf 计划用上下文。
    build_pb: plan_base::BuildPBContext,
    /// builtin 使用计数。
    builtin_function_usage: plan_base::BuiltinFunctionUsageCounter,
}

/// 构造默认测试 PlanContext。
impl TestPlanContext {
    fn new() -> Self {
        let expression = Arc::new(exprstatic::NewExprContext(Vec::new()));
        let expression_for_build: Arc<dyn planctx::exprctx::BuildContext> = expression.clone();
        Self {
            plan_id: AtomicI32::new(0),
            session: planctx::variable::SessionVars::default(),
            expression,
            build_pb: plan_base::BuildPBContext {
                ExprCtx: expression_for_build,
                Client: None,
                TiFlashFastScan: false,
                TiFlashFineGrainedShuffleBatchSize: 0,
                GroupConcatMaxLen: 0,
                InExplainStmt: false,
                WarnHandler: None,
                ExtraWarnghandler: None,
            },
            builtin_function_usage: plan_base::BuiltinFunctionUsageCounter::default(),
        }
    }
}

/// 将测试桩接到 PlanContext trait。
impl plan_base::PlanContext for TestPlanContext {
    fn alloc_plan_id(&self) -> i32 {
        self.plan_id.fetch_add(1, Ordering::Relaxed) + 1
    }

    fn ignore_explain_id_suffix(&self) -> bool {
        false
    }

    fn GetSessionVars(&self) -> &planctx::variable::SessionVars {
        &self.session
    }

    fn GetExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        self.expression.as_ref()
    }

    fn GetRangerCtx(&self) -> &planctx::rangerctx::RangerContext<'_> {
        panic!("null-rejection test does not build ranges")
    }

    fn GetNullRejectCheckExprCtx(&self) -> &dyn planctx::exprctx::ExprContext {
        self.expression.as_ref()
    }

    fn GetBuildPBCtx(&self) -> &plan_base::BuildPBContext {
        &self.build_pb
    }

    fn BuiltinFunctionUsageInc(&self, scalar_func_sig_name: &str) {
        self.builtin_function_usage.Inc(scalar_func_sig_name)
    }
}

/// 测试用整型标量函数实现（覆盖比较、逻辑、IN、IS NULL 等）。
struct IntegerBuiltin {
    name: &'static str,
    arguments: Vec<expression::ExprBox>,
    return_type: expression::types::FieldType,
    collator: Box<dyn expression::collate::Collator>,
    collation: expression::collationInfo,
}

/// 构造与按参数求值辅助。
impl IntegerBuiltin {
    fn new(name: &'static str, arguments: Vec<expression::ExprBox>) -> Self {
        let collation = expression::collationInfo::default();
        collation.SetCoercibility(expression::CoercibilityNumeric);
        Self {
            name,
            arguments,
            return_type: *types::NewFieldType(mysql::TypeTiny),
            collator: expression::collate::GetBinaryCollator(),
            collation,
        }
    }

    fn argument(
        &self,
        index: usize,
        context: &dyn expression::exprctx::EvalContext,
        row: expression::chunk::Row,
    ) -> Result<(i64, bool), expression::Error> {
        self.arguments[index].EvalInt(context, row)
    }
}

/// 透传 collationInfo。
impl expression::CollationInfo for IntegerBuiltin {
    fn HasCoercibility(&self) -> bool {
        self.collation.HasCoercibility()
    }
    fn Coercibility(&self) -> expression::Coercibility {
        self.collation.Coercibility()
    }
    fn SetCoercibility(&self, value: expression::Coercibility) {
        self.collation.SetCoercibility(value)
    }
    fn Repertoire(&self) -> expression::Repertoire {
        self.collation.Repertoire()
    }
    fn SetRepertoire(&mut self, value: expression::Repertoire) {
        self.collation.SetRepertoire(value)
    }
    fn CharsetAndCollation(&self) -> (String, String) {
        self.collation.CharsetAndCollation()
    }
    fn SetCharsetAndCollation(&mut self, charset: String, collation: String) {
        self.collation.SetCharsetAndCollation(charset, collation)
    }
    fn IsExplicitCharset(&self) -> bool {
        self.collation.IsExplicitCharset()
    }
    fn SetExplicitCharset(&mut self, explicit: bool) {
        self.collation.SetExplicitCharset(explicit)
    }
}

/// 按函数名实现 evalInt 语义（含三值逻辑）。
impl expression::builtinFunc for IntegerBuiltin {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn SafeToShareAcrossSession(&self) -> bool {
        self.arguments
            .iter()
            .all(|argument| argument.SafeToShareAcrossSession())
    }

    fn evalInt(
        &self,
        context: &dyn expression::exprctx::EvalContext,
        row: expression::chunk::Row,
    ) -> Result<(i64, bool), expression::Error> {
        let first = || self.argument(0, context, row.clone());
        let second = || self.argument(1, context, row.clone());
        // 按函数名分支实现三值逻辑求值。
        match self.name {
            parser_ast::IsNull => {
                let (_, is_null) = first()?;
                Ok((i64::from(is_null), false))
            }
            parser_ast::IsTruthWithNull => {
                let (value, is_null) = first()?;
                Ok((i64::from(value != 0), is_null))
            }
            parser_ast::UnaryNot => {
                let (value, is_null) = first()?;
                Ok((i64::from(value == 0), is_null))
            }
            parser_ast::LogicAnd => {
                let (left, left_null) = first()?;
                if !left_null && left == 0 {
                    return Ok((0, false));
                }
                let (right, right_null) = second()?;
                if !right_null && right == 0 {
                    Ok((0, false))
                } else if left_null || right_null {
                    Ok((0, true))
                } else {
                    Ok((1, false))
                }
            }
            parser_ast::LogicOr => {
                let (left, left_null) = first()?;
                if !left_null && left != 0 {
                    return Ok((1, false));
                }
                let (right, right_null) = second()?;
                if !right_null && right != 0 {
                    Ok((1, false))
                } else if left_null || right_null {
                    Ok((0, true))
                } else {
                    Ok((0, false))
                }
            }
            parser_ast::LT
            | parser_ast::LE
            | parser_ast::GT
            | parser_ast::GE
            | parser_ast::EQ
            | parser_ast::NE => {
                let (left, left_null) = first()?;
                let (right, right_null) = second()?;
                if left_null || right_null {
                    Ok((0, true))
                } else {
                    let result = match self.name {
                        parser_ast::LT => left < right,
                        parser_ast::LE => left <= right,
                        parser_ast::GT => left > right,
                        parser_ast::GE => left >= right,
                        parser_ast::EQ => left == right,
                        parser_ast::NE => left != right,
                        _ => unreachable!(),
                    };
                    Ok((i64::from(result), false))
                }
            }
            parser_ast::In => {
                let (needle, needle_null) = first()?;
                if needle_null {
                    return Ok((0, true));
                }
                let mut has_null = false;
                for argument in self.arguments.iter().skip(1) {
                    let (candidate, candidate_null) = argument.EvalInt(context, row.clone())?;
                    has_null |= candidate_null;
                    if !candidate_null && candidate == needle {
                        return Ok((1, false));
                    }
                }
                Ok((0, has_null))
            }
            name => Err(expression::errors::New(format!(
                "unsupported integer test builtin {name}"
            ))),
        }
    }

    fn getArgs(&self) -> &[expression::ExprBox] {
        &self.arguments
    }
    fn getArgsMut(&mut self) -> &mut [expression::ExprBox] {
        &mut self.arguments
    }
    fn equal(
        &self,
        context: &dyn expression::exprctx::EvalContext,
        other: &dyn expression::builtinFunc,
    ) -> bool {
        other.as_any().downcast_ref::<Self>().is_some_and(|other| {
            self.name == other.name
                && self.arguments.len() == other.arguments.len()
                && self
                    .arguments
                    .iter()
                    .zip(&other.arguments)
                    .all(|(left, right)| left.Equal(context, right.as_ref()))
        })
    }
    fn getRetTp(&self) -> &expression::types::FieldType {
        &self.return_type
    }
    fn setPbCode(&mut self, _code: i32) {}
    fn PbCode(&self) -> i32 {
        0
    }
    fn setCollator(&mut self, collator: Box<dyn expression::collate::Collator>) {
        self.collator = collator;
    }
    fn collator(&self) -> &dyn expression::collate::Collator {
        self.collator.as_ref()
    }
    fn Clone(&self) -> Box<dyn expression::builtinFunc> {
        Box::new(Self {
            name: self.name,
            arguments: self.arguments.clone(),
            return_type: self.return_type.clone(),
            collator: expression::collate::GetBinaryCollator(),
            collation: self.collation.clone(),
        })
    }
    fn MemoryUsage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
            + self
                .arguments
                .iter()
                .map(|argument| argument.MemoryUsage())
                .sum::<i64>()
    }
    fn vectorized(&self) -> bool {
        false
    }
}

/// 生成注册到 expression 的整型 builtin 工厂。
macro_rules! integer_factory {
    ($factory:ident, $name:expr, $signature:expr) => {
        fn $factory(
            _context: &dyn expression::exprctx::BuildContext,
            arguments: Vec<expression::ExprBox>,
            _metadata: &expression::FunctionClassMetadata,
        ) -> Result<expression::GeneratedBuiltinFactoryOutput, expression::Error> {
            expression::GeneratedBuiltinFactoryOutput::new(
                $signature,
                Box::new(IntegerBuiltin::new($name, arguments)),
            )
        }
    };
}

integer_factory!(gt_factory, parser_ast::GT, "builtinGTIntSig");
integer_factory!(lt_factory, parser_ast::LT, "builtinLTIntSig");
integer_factory!(le_factory, parser_ast::LE, "builtinLEIntSig");
integer_factory!(ge_factory, parser_ast::GE, "builtinGEIntSig");
integer_factory!(eq_factory, parser_ast::EQ, "builtinEQIntSig");
integer_factory!(ne_factory, parser_ast::NE, "builtinNEIntSig");
integer_factory!(and_factory, parser_ast::LogicAnd, "builtinLogicAndSig");
integer_factory!(or_factory, parser_ast::LogicOr, "builtinLogicOrSig");
integer_factory!(not_factory, parser_ast::UnaryNot, "builtinUnaryNotIntSig");
integer_factory!(is_null_factory, parser_ast::IsNull, "builtinIntIsNullSig");
integer_factory!(in_factory, parser_ast::In, "builtinInIntSig");
integer_factory!(
    is_truth_with_null_factory,
    parser_ast::IsTruthWithNull,
    "builtinIntIsTrueWithNullSig"
);

/// 本测试需要注册的函数名与工厂表。
const INTEGER_FACTORIES: &[(&str, expression::BuiltinFactory)] = &[
    (parser_ast::GT, gt_factory),
    (parser_ast::LT, lt_factory),
    (parser_ast::LE, le_factory),
    (parser_ast::GE, ge_factory),
    (parser_ast::EQ, eq_factory),
    (parser_ast::NE, ne_factory),
    (parser_ast::LogicAnd, and_factory),
    (parser_ast::LogicOr, or_factory),
    (parser_ast::UnaryNot, not_factory),
    (parser_ast::IsNull, is_null_factory),
    (parser_ast::In, in_factory),
    (parser_ast::IsTruthWithNull, is_truth_with_null_factory),
];

/// 注册测试用整型工厂。
fn install_integer_factories() {
    for (name, factory) in INTEGER_FACTORIES {
        expression::registerBuiltinFactory(name, *factory)
            .unwrap_or_else(|error| panic!("register {name}: {error}"));
    }
}

/// 卸载测试用整型工厂。
fn remove_integer_factories() {
    for (name, _) in INTEGER_FACTORIES {
        expression::removeBuiltinFactory(name);
    }
}

/// RAII：构造时注册、析构时卸载整型工厂。
struct InstalledIntegerFactories;

/// 进入作用域即安装工厂。
impl InstalledIntegerFactories {
    fn new() -> Self {
        install_integer_factories();
        Self
    }
}

/// 离开作用域卸载工厂，避免污染其他测试。
impl Drop for InstalledIntegerFactories {
    fn drop(&mut self) {
        remove_integer_factories();
    }
}

/// 构造带给定 UniqueID 的 BIGINT 列。
fn int_column(id: i64) -> expression::Column {
    expression::Column::new(
        *types::NewFieldType(mysql::TypeLonglong),
        id,
        id,
        id as isize,
    )
}

/// 构造 BIGINT 常量表达式。
fn int_constant(value: i64) -> expression::ExprBox {
    Box::new(expression::Constant::with_type(
        types::NewIntDatum(value),
        *types::NewFieldType(mysql::TypeLonglong),
    ))
}

/// 用测试上下文构造标量函数表达式。
fn function(
    context: &TestPlanContext,
    name: &str,
    arguments: Vec<expression::ExprBox>,
) -> expression::ExprBox {
    expression::NewFunctionBase(
        context.GetExprCtx(),
        name,
        *types::NewFieldType(mysql::TypeTiny),
        arguments,
    )
    .unwrap_or_else(|error| panic!("construct {name}: {error}"))
}

/// 列装箱为 ExprBox。
fn column_expr(column: &expression::Column) -> expression::ExprBox {
    Box::new(column.clone())
}

#[test]
/// 校验登记表长度、去重与若干查表结果。
fn test_null_reject_builtin_registry_snapshot() {
    crate::main_test::setup_for_planner_util_test();
    assert_eq!(NULL_REJECT_NULL_PRESERVING_FUNCTIONS.len(), 174);
    assert_eq!(NULL_REJECT_REJECT_NULL_TESTS.len(), 3);

    let mut names = NULL_REJECT_NULL_PRESERVING_FUNCTIONS.to_vec();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), NULL_REJECT_NULL_PRESERVING_FUNCTIONS.len());
    assert!(is_null_reject_null_preserving(parser_ast::GT));
    assert!(is_null_reject_null_preserving(parser_ast::Cast));
    assert!(matches!(
        null_reject_test_mode(parser_ast::IsTruthWithNull),
        Some(NullRejectTestMode::KeepsNull)
    ));
    assert!(matches!(
        null_reject_test_mode(parser_ast::IsTruthWithoutNull),
        Some(NullRejectTestMode::ReturnsFalse)
    ));
}

#[test]
/// 覆盖内表比较、外表比较、IS NULL、NOT、AND/OR、IN 等证明模式。
fn test_is_null_rejected_proof_modes() {
    crate::main_test::setup_for_planner_util_test();
    let _factories = InstalledIntegerFactories::new();
    let context = TestPlanContext::new();
    let inner_a = int_column(1);
    let inner_b = int_column(2);
    let outer = int_column(3);
    let schema = expression::NewSchema(vec![inner_a.clone(), inner_b.clone()]);

    // 内表列 > 0：内表列 NULL 时结果非 TRUE → 应拒绝。
    let gt_inner = || {
        function(
            &context,
            parser_ast::GT,
            vec![column_expr(&inner_a), int_constant(0)],
        )
    };
    let gt_outer = || {
        function(
            &context,
            parser_ast::GT,
            vec![column_expr(&outer), int_constant(0)],
        )
    };
    let is_null_inner = || function(&context, parser_ast::IsNull, vec![column_expr(&inner_a)]);

    assert!(IsNullRejected(&context, &schema, gt_inner()));
    assert!(!IsNullRejected(&context, &schema, gt_outer()));
    assert!(IsNullRejected(&context, &schema, column_expr(&inner_a)));
    assert!(!IsNullRejected(&context, &schema, int_constant(1)));
    assert!(IsNullRejected(&context, &schema, int_constant(0)));
    assert!(!IsNullRejected(&context, &schema, is_null_inner()));
    assert!(IsNullRejected(
        &context,
        &schema,
        function(
            &context,
            parser_ast::IsTruthWithNull,
            vec![column_expr(&inner_a)]
        )
    ));
    assert!(IsNullRejected(
        &context,
        &schema,
        function(&context, parser_ast::UnaryNot, vec![is_null_inner()])
    ));
    assert!(IsNullRejected(
        &context,
        &schema,
        function(&context, parser_ast::UnaryNot, vec![gt_inner()])
    ));

    let and = function(
        &context,
        parser_ast::LogicAnd,
        vec![
            function(
                &context,
                parser_ast::EQ,
                vec![column_expr(&inner_a), int_constant(0)],
            ),
            gt_outer(),
        ],
    );
    let or = function(&context, parser_ast::LogicOr, vec![gt_inner(), and]);
    assert!(IsNullRejected(&context, &schema, or));

    // OR 只有两侧都恒非 TRUE 才能证明；外表条件仍可能为 TRUE。
    assert!(!IsNullRejected(
        &context,
        &schema,
        function(&context, parser_ast::LogicOr, vec![gt_inner(), gt_outer()])
    ));

    // AND 任一侧恒非 TRUE 即可证明，即使另一侧来自外表。
    assert!(IsNullRejected(
        &context,
        &schema,
        function(&context, parser_ast::LogicAnd, vec![gt_inner(), gt_outer()])
    ));

    // IN 列表全为内表列：置 NULL 后 IN 必 NULL → 应拒绝。
    let all_null_candidates = function(
        &context,
        parser_ast::In,
        vec![
            int_constant(1),
            column_expr(&inner_a),
            column_expr(&inner_b),
        ],
    );
    assert!(IsNullRejected(&context, &schema, all_null_candidates));

    let null_needle = function(
        &context,
        parser_ast::In,
        vec![column_expr(&inner_a), int_constant(1)],
    );
    assert!(IsNullRejected(&context, &schema, null_needle));

    let non_null_candidate = function(
        &context,
        parser_ast::In,
        vec![int_constant(1), column_expr(&inner_a), int_constant(1)],
    );
    assert!(!IsNullRejected(&context, &schema, non_null_candidate));
}
