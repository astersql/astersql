// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// `builtin.rs` 的 Aster 单元测试：校验内置函数注册表、构造与向量化过滤语义。
//
// 对应 Go `builtin_test.go` 中与基础 builtin 框架相关的契约。覆盖：
// - 显示名与函数注册表排序；
// - 参数个数校验与返回类型/排序规则（collation）推导；
// - 构造时参数隐式 CAST 与可空性（nullability）标志；
// - 按求值上下文缓存（每上下文只初始化一次，错误不缓存）；
// - 向量化可行性：拒绝有状态/`SET_VAR` 冲突及序列函数冲突；
// - 行过滤保留 NULL，并与 chunk 选择向量（selection）求交；
// - 无符号整数按位模式（bit pattern）写入结果列。

use super::*;
use crate::chunk_executor_kernel::{
    HasAssignSetVarFunc, HasGetSetVarFunc, Vectorizable, VectorizedExecute,
    VectorizedFilterConsiderNull,
};
use std::any::Any;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;

#[derive(Clone)]
/// 测试用表达式桩：持有整型列值，可配置是否声明为向量化可执行。
struct TestExpr {
    field_type: types::FieldType,
    ints: Vec<Option<i64>>,
    vectorized: bool,
}

impl TestExpr {
    /// 构造整型 `TestExpr`，默认标记为可向量化。
    fn int(values: &[Option<i64>]) -> Self {
        Self {
            field_type: types::FieldType::new(mysql::TypeLonglong),
            ints: values.to_vec(),
            vectorized: true,
        }
    }
}

impl Expression for TestExpr {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn GetType(&self, _ctx: &dyn EvalContext) -> &types::FieldType {
        &self.field_type
    }
    fn Vectorized(&self) -> bool {
        self.vectorized
    }
    fn EvalInt(&self, _ctx: &dyn EvalContext, row: chunk::Row) -> (i64, bool, Option<Error>) {
        match self.ints[row.Idx()] {
            Some(value) => (value, false, None),
            None => (0, true, None),
        }
    }
}

/// 用给定名称与参数构造返回 BIGINT 的标量函数表达式。
fn scalar(name: &str, args: Vec<Box<dyn Expression>>) -> Box<dyn Expression> {
    Box::new(ScalarFunction::new(
        name,
        args,
        types::FieldType::new(mysql::TypeLonglong),
    ))
}

#[test]
/// 校验显示名映射与内置函数列表排序/成员与 Go 契约一致。
fn display_names_and_registry_match_go_contract() {
    assert_eq!(GetDisplayName(ast::EQ), "=");
    assert_eq!(GetDisplayName(ast::NullEQ), "<=>");
    assert_eq!(GetDisplayName(ast::IsTruthWithoutNull), "IS TRUE");
    assert_eq!(GetDisplayName("abs"), "abs");
    assert_eq!(GetDisplayName("other_unknown_func"), "other_unknown_func");

    assert!(IsFunctionSupported(ast::EQ));
    assert!(IsFunctionSupported("abs"));
    assert!(!IsFunctionSupported("other_unknown_func"));
    let builtins = GetBuiltinList();
    assert!(builtins.windows(2).all(|pair| pair[0] < pair[1]));
    assert!(builtins.contains(&ast::EQ.to_string()));
}

#[test]
/// 校验参数个数错误信息及按 EvalType 生成返回字段类型的规则。
fn argument_count_and_return_type_rules_match_go() {
    let class = baseFunctionClass::new("demo", 1, 2);
    assert!(class.verifyArgsByCount(1).is_ok());
    assert!(class.verifyArgsByCount(2).is_ok());
    assert_eq!(
        class.verifyArgsByCount(0).unwrap_err().to_string(),
        "Incorrect parameter count in the call to native function 'demo'"
    );

    let collation = ExprCollation::default();
    let int_type = newReturnFieldTypeForBaseBuiltinFunc(ast::EQ, types::ETInt, &collation);
    assert_eq!(int_type.GetType(), mysql::TypeLonglong);
    assert!(mysql::HasBinaryFlag(int_type.GetFlag()));
    assert!(int_type.GetFlag() & mysql::IsBooleanFlag != 0);

    let string_type = newReturnFieldTypeForBaseBuiltinFunc("concat", types::ETString, &collation);
    assert_eq!(string_type.GetCharset(), charset::DEFAULT_CHARSET);
    assert_eq!(string_type.GetCollate(), charset::DEFAULT_COLLATION);
}

#[test]
/// 校验 `newBaseBuiltinFuncWithTp` 会对实参做类型转换并设置可空标志。
fn builtin_constructor_casts_arguments_and_applies_nullability() {
    let build_ctx = SimpleBuildContext::new(11);
    let args: Vec<Box<dyn Expression>> = vec![Box::new(TestExpr::int(&[Some(42)]))];
    let mut builtin = newBaseBuiltinFuncWithTp(
        Some(&build_ctx),
        "concat",
        args,
        types::ETString,
        &[types::ETString],
    )
    .unwrap();

    assert_eq!(builtin.getArgs().len(), 1);
    assert_eq!(
        builtin.getArgs()[0]
            .EvalString(build_ctx.GetEvalCtx(), chunk::Row { index: 0 })
            .0,
        "42"
    );
    assert_eq!(builtin.getRetTp().EvalType(), types::ETString);
    assert!(!mysql::HasNotNullFlag(builtin.getRetTp().GetFlag()));

    assert_eq!(
        newBaseBuiltinFuncWithTp(None, "concat", Vec::new(), types::ETString, &[],)
            .err()
            .unwrap()
            .to_string(),
        "unexpected nil session ctx"
    );
}

#[test]
/// 校验 `builtinFuncCache`：同上下文只初始化一次，失败结果不会被缓存。
// 并发下同一 EvalContext 只初始化一次；首次返回 Err 不得写入缓存。
fn cache_is_once_per_context_and_does_not_cache_errors() {
    let cache = Arc::new(builtinFuncCache::<usize>::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let mut joins = Vec::new();
    for _ in 0..8 {
        let cache = Arc::clone(&cache);
        let calls = Arc::clone(&calls);
        joins.push(thread::spawn(move || {
            cache
                .getOrInitCache(&SimpleEvalContext::new(7), || {
                    Ok(calls.fetch_add(1, Ordering::SeqCst) + 101)
                })
                .unwrap()
        }));
    }
    for join in joins {
        assert_eq!(join.join().unwrap(), 101);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let fail = AtomicBool::new(true);
    assert!(
        cache
            .getOrInitCache(&SimpleEvalContext::new(8), || {
                if fail.swap(false, Ordering::SeqCst) {
                    Err(Error::new("mockError"))
                } else {
                    Ok(128)
                }
            })
            .is_err()
    );
    assert_eq!(
        cache
            .getOrInitCache(&SimpleEvalContext::new(8), || Ok(128))
            .unwrap(),
        128
    );
}

#[test]
/// 校验向量化拒绝 `SET_VAR`/`GET_VAR` 及冲突的序列（sequence）函数组合。
///
/// 序列函数如 `NEXTVAL` 带副作用，同一批向量化执行中若出现冲突用法会破坏结果确定性。
fn vectorizable_rejects_stateful_and_conflicting_sequence_functions() {
    /// 构造带指定函数名的正式标量表达式，用于向量化可行性探测。
    fn formal_scalar(name: &str, argument: Option<crate::ExprBox>) -> crate::ExprBox {
        let ctx = exprstatic::NewExprContext(Vec::new());
        let argument = argument.unwrap_or_else(|| Box::new(crate::NewInt64Const(0)));
        let mut expression = crate::BuildGetVarFunction(
            &ctx,
            &argument,
            &*crate::types::NewFieldType(crate::mysql::TypeLonglong),
        )
        .unwrap();
        expression
            .as_any_mut()
            .downcast_mut::<crate::ScalarFunction>()
            .unwrap()
            .FuncName = crate::ast::NewCIStr(name);
        expression.SetCoercibility(crate::CoercibilityNumeric);
        expression.SetCharsetAndCollation(
            crate::charset::CharsetBin.to_owned(),
            crate::charset::CollationBin.to_owned(),
        );
        expression
    }

    assert!(!Vectorizable(&[formal_scalar(ast::SetVar, None)]));
    let nested = formal_scalar("plus", Some(formal_scalar(ast::GetVar, None)));
    assert!(HasGetSetVarFunc(nested.as_ref()));

    assert!(!Vectorizable(&[
        formal_scalar(ast::NextVal, None),
        formal_scalar(ast::LastVal, None)
    ]));
    assert!(!Vectorizable(&[
        formal_scalar(ast::NextVal, None),
        formal_scalar(ast::NextVal, None)
    ]));
    assert!(Vectorizable(&[
        formal_scalar(ast::LastVal, None),
        formal_scalar(ast::SetVal, None)
    ]));

    let assigned = formal_scalar(ast::SetVar, Some(formal_scalar("plus", None)));
    assert!(HasAssignSetVarFunc(assigned.as_ref()));
}

#[test]
/// 校验向量化过滤保留 NULL 位，并与 chunk 的 selection 向量求交。
///
/// Chunk 是列式批处理容器；selection 表示当前活跃行下标集合。
// selection 与过滤结果求交后，NULL 行仍出现在 nulls 位图中对应位置。
fn row_filter_preserves_nulls_and_intersects_chunk_selection() {
    let ctx = exprstatic::NewEvalContext(Vec::new());
    let field_type = *crate::types::NewFieldType(crate::mysql::TypeLonglong);
    let mut input = crate::chunk::NewChunkWithCapacity(vec![field_type.clone()], 4);
    input.AppendInt64(0, 1);
    input.AppendInt64(0, 1);
    input.AppendNull(0);
    input.AppendInt64(0, 0);
    input.SetSel(Some(vec![0, 2, 3]));
    let mut iterator = crate::chunk::NewIterator4Chunk(input);
    let filters: Vec<crate::ExprBox> = vec![Box::new(crate::Column::new(field_type, 1, 1, 0))];

    let (selected, nulls, err) = VectorizedFilterConsiderNull(
        &ctx,
        false,
        &filters,
        &mut iterator,
        Vec::new(),
        Some(Vec::new()),
    );
    assert!(err.is_none());
    assert_eq!(selected, vec![true, false, false, false]);
    assert_eq!(nulls.unwrap(), vec![false, false, true, false]);
    assert_eq!(iterator.GetChunk().Sel(), Some(&[0, 2, 3][..]));
}

#[test]
/// 校验无符号整型在向量化写出时保留底层位模式（例如 -1 位型为 u64::MAX）。
fn row_execution_keeps_unsigned_integer_bit_pattern() {
    let ctx = exprstatic::NewEvalContext(Vec::new());
    let mut field_type = crate::types::NewFieldType(crate::mysql::TypeLonglong);
    field_type.AddFlag(crate::mysql::UnsignedFlag);
    let mut input = crate::chunk::NewChunkWithCapacity(vec![(*field_type).clone()], 2);
    input.AppendInt64(0, -1);
    input.AppendNull(0);
    let expressions: Vec<crate::ExprBox> =
        vec![Box::new(crate::Column::new((*field_type).clone(), 1, 1, 0))];
    let mut iterator = crate::chunk::NewIterator4Chunk(input);
    let mut output = crate::chunk::NewChunkWithCapacity(vec![*field_type], 2);
    VectorizedExecute(&ctx, &expressions, &mut iterator, &mut output).unwrap();
    assert_eq!(output.Column(0).GetUint64(0), u64::MAX);
    assert!(output.Column(0).IsNull(1));
}
