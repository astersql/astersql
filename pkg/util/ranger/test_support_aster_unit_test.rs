// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// ranger 单元测试辅助：构造表达式、列、Range 与 RangerContext。
//
// 注册轻量 TestBuiltin，便于在无完整表达式运行时的环境中拼装 EQ/IN/OR 等谓词。

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Once};

use crate::{ast, collate, errctx, mysql, types};
use rangerctx::RangerContext;

/// 测试用内建函数桩，满足 CollationInfo / builtinFunc 接口。
struct TestBuiltin {
    args: Vec<expression::ExprBox>,
    return_type: types::FieldType,
    pb_code: i32,
    collator: Box<dyn expression::collate::Collator>,
    collation: expression::collationInfo,
}

impl TestBuiltin {
    /// 按参数推导 charset/collation，返回类型固定为 Tiny（布尔比较结果）。
    fn new(context: &dyn expression::BuildContext, args: Vec<expression::ExprBox>) -> Self {
        let mut collation = expression::collationInfo::default();
        collation.SetCoercibility(expression::CoercibilityNumeric);
        if let Some(argument) = args.first() {
            let argument_type = argument.GetType(context.GetEvalCtx());
            collation.SetCharsetAndCollation(
                argument_type.GetCharset().to_owned(),
                argument_type.GetCollate().to_owned(),
            );
        }
        Self {
            args,
            return_type: field_type(mysql::TypeTiny),
            pb_code: 0,
            collator: expression::collate::GetBinaryCollator(),
            collation,
        }
    }
}

impl expression::CollationInfo for TestBuiltin {
    fn HasCoercibility(&self) -> bool {
        self.collation.HasCoercibility()
    }

    fn Coercibility(&self) -> expression::Coercibility {
        self.collation.Coercibility()
    }

    fn SetCoercibility(&self, value: expression::Coercibility) {
        self.collation.SetCoercibility(value);
    }

    fn Repertoire(&self) -> expression::Repertoire {
        self.collation.Repertoire()
    }

    fn SetRepertoire(&mut self, value: expression::Repertoire) {
        self.collation.SetRepertoire(value);
    }

    fn CharsetAndCollation(&self) -> (String, String) {
        self.collation.CharsetAndCollation()
    }

    fn SetCharsetAndCollation(&mut self, charset: String, collation: String) {
        self.collation.SetCharsetAndCollation(charset, collation);
    }

    fn IsExplicitCharset(&self) -> bool {
        self.collation.IsExplicitCharset()
    }

    fn SetExplicitCharset(&mut self, explicit: bool) {
        self.collation.SetExplicitCharset(explicit);
    }
}

impl expression::builtinFunc for TestBuiltin {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn SafeToShareAcrossSession(&self) -> bool {
        true
    }

    fn getArgs(&self) -> &[expression::ExprBox] {
        &self.args
    }

    fn getArgsMut(&mut self) -> &mut [expression::ExprBox] {
        &mut self.args
    }

    fn equal(
        &self,
        context: &dyn expression::EvalContext,
        other: &dyn expression::builtinFunc,
    ) -> bool {
        other.as_any().downcast_ref::<Self>().is_some_and(|other| {
            self.args.len() == other.args.len()
                && self
                    .args
                    .iter()
                    .zip(&other.args)
                    .all(|(left, right)| left.Equal(context, right.as_ref()))
        })
    }

    fn getRetTp(&self) -> &types::FieldType {
        &self.return_type
    }

    fn setPbCode(&mut self, code: i32) {
        self.pb_code = code;
    }

    fn PbCode(&self) -> i32 {
        self.pb_code
    }

    fn setCollator(&mut self, collator: Box<dyn expression::collate::Collator>) {
        self.collator = collator;
    }

    fn collator(&self) -> &dyn expression::collate::Collator {
        self.collator.as_ref()
    }

    fn Clone(&self) -> Box<dyn expression::builtinFunc> {
        Box::new(Self {
            args: self
                .args
                .iter()
                .map(|argument| argument.CloneExpr())
                .collect(),
            return_type: self.return_type.clone(),
            pb_code: self.pb_code,
            collator: self.collator.Clone(),
            collation: self.collation.clone(),
        })
    }

    fn MemoryUsage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
    }

    fn vectorized(&self) -> bool {
        false
    }
}

/// 测试内建工厂：产出名为 builtinEQIntSig 的 TestBuiltin。
fn test_builtin_factory(
    context: &dyn expression::BuildContext,
    args: Vec<expression::ExprBox>,
    _metadata: &expression::formal_registry::FunctionClassMetadata,
) -> Result<expression::formal_registry::GeneratedBuiltinFactoryOutput, expression::Error> {
    expression::formal_registry::GeneratedBuiltinFactoryOutput::new(
        "builtinEQIntSig",
        Box::new(TestBuiltin::new(context, args)),
    )
}

/// 一次性注册比较/逻辑/IN/IS NULL 等测试内建，供 NewFunctionBase 使用。
fn register_test_builtins() {
    static REGISTER: Once = Once::new();
    REGISTER.call_once(|| {
        for name in [
            ast::EQ,
            ast::NE,
            ast::LT,
            ast::LE,
            ast::GT,
            ast::GE,
            ast::In,
            ast::IsNull,
            ast::LogicAnd,
            ast::LogicOr,
        ] {
            expression::formal_registry::registerBuiltinFactory(name, test_builtin_factory)
                .unwrap_or_else(|error| panic!("{name} test builtin registration failed: {error}"));
        }
    });
}

/// 按 MySQL 类型码构造默认 FieldType。
pub(crate) fn field_type(mysql_type: u8) -> types::FieldType {
    (*types::NewFieldType(mysql_type)).clone()
}

/// 构造无会话绑定的静态 RangerContext，默认 RegardNULLAsPoint=true。
pub(crate) fn ranger_context() -> RangerContext<'static> {
    let expression_context: Arc<dyn expression::exprctx::BuildContext> =
        Arc::new(exprstatic::NewExprContext(Vec::new()));
    RangerContext {
        TypeCtx: types::DefaultStmtNoWarningContext.clone(),
        ErrCtx: errctx::StrictNoWarningContext.clone(),
        ExprCtx: expression_context,
        RangeFallbackHandler: None,
        PlanCacheTracker: None,
        OptimizerFixControl: HashMap::new(),
        UseCache: false,
        RegardNULLAsPoint: true,
        OptPrefixIndexSingleScan: false,
    }
}

/// 构造 Longlong 测试列。
pub(crate) fn column(id: i64, index: usize) -> expression::Column {
    expression::Column::new(field_type(mysql::TypeLonglong), id, id, index as isize)
}

/// 构造与列类型一致的整型常量表达式。
pub(crate) fn int_constant(column: &expression::Column, value: i64) -> expression::ExprBox {
    Box::new(expression::Constant::with_type(
        types::NewIntDatum(value),
        column.RetType.clone().expect("test column has a type"),
    ))
}

/// 通过已注册的测试内建构造标量函数表达式。
pub(crate) fn scalar(
    context: &RangerContext<'_>,
    name: &str,
    args: Vec<expression::ExprBox>,
) -> expression::ExprBox {
    register_test_builtins();
    expression::NewFunctionBase(
        context.ExprCtx.as_ref(),
        name,
        field_type(mysql::TypeTiny),
        args,
    )
    .unwrap_or_else(|error| panic!("{name} must be constructible: {error}"))
}

/// 构造 `col op const` 形式的比较谓词。
pub(crate) fn comparison(
    context: &RangerContext<'_>,
    name: &str,
    column: &expression::Column,
    value: i64,
) -> expression::ExprBox {
    scalar(
        context,
        name,
        vec![Box::new(column.clone()), int_constant(column, value)],
    )
}

/// 构造等值谓词 `col = const`。
pub(crate) fn equality(
    context: &RangerContext<'_>,
    column: &expression::Column,
    value: i64,
) -> expression::ExprBox {
    comparison(context, ast::EQ, column, value)
}

/// 用 LogicOr 折叠多条件，得到 DNF（析取范式）表达式。
pub(crate) fn disjunction(
    context: &RangerContext<'_>,
    conditions: Vec<expression::ExprBox>,
) -> expression::ExprBox {
    let mut conditions = conditions.into_iter();
    let first = conditions.next().expect("DNF requires at least one branch");
    conditions.fold(first, |left, right| {
        scalar(context, ast::LogicOr, vec![left, right])
    })
}

/// 构造单列整型 Range，可指定开闭区间。
pub(crate) fn int_range(
    low: i64,
    high: i64,
    low_exclude: bool,
    high_exclude: bool,
) -> crate::Range {
    crate::Range {
        LowVal: vec![types::NewIntDatum(low)],
        HighVal: vec![types::NewIntDatum(high)],
        Collators: collate::GetBinaryCollatorSlice(1),
        LowExclude: low_exclude,
        HighExclude: high_exclude,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use expression::builtinFunc;

    #[test]
    fn test_builtin_equality_compares_arguments() {
        let context = ranger_context();
        let column = column(1, 0);
        let left = TestBuiltin::new(context.ExprCtx.as_ref(), vec![int_constant(&column, 1)]);
        let same = TestBuiltin::new(context.ExprCtx.as_ref(), vec![int_constant(&column, 1)]);
        let different = TestBuiltin::new(context.ExprCtx.as_ref(), vec![int_constant(&column, 2)]);

        assert!(left.equal(context.ExprCtx.GetEvalCtx(), &same));
        assert!(!left.equal(context.ExprCtx.GetEvalCtx(), &different));
    }
}
