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

// `evaluator.rs` 求值套件的单元测试。
//
// 覆盖列交换快路径、禁用列求值、常量的向量/标量一致性，以及可选属性收集边界。

use crate::evaluator_kernel::{GetOptionalEvalPropsForExpr, NewEvaluatorSuite};
use crate::*;
use std::any::Any;

struct OptionalPropsBuiltin {
    required: exprctx::OptionalEvalPropKeySet,
    args: Vec<Box<dyn Expression>>,
    collation: collationInfo,
    ret_type: types::FieldType,
    collator: Box<dyn collate::Collator>,
}

impl OptionalPropsBuiltin {
    fn new(required: exprctx::OptionalEvalPropKeySet, args: Vec<Box<dyn Expression>>) -> Self {
        Self {
            required,
            args,
            collation: collationInfo::default(),
            ret_type: int_type(),
            collator: collate::GetBinaryCollator(),
        }
    }
}

impl CollationInfo for OptionalPropsBuiltin {
    fn HasCoercibility(&self) -> bool {
        self.collation.HasCoercibility()
    }
    fn Coercibility(&self) -> Coercibility {
        self.collation.Coercibility()
    }
    fn Repertoire(&self) -> Repertoire {
        self.collation.Repertoire()
    }
    fn CharsetAndCollation(&self) -> (String, String) {
        self.collation.CharsetAndCollation()
    }
    fn SetCoercibility(&self, value: Coercibility) {
        self.collation.SetCoercibility(value);
    }
    fn SetRepertoire(&mut self, value: Repertoire) {
        self.collation.SetRepertoire(value);
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

impl builtinFunc for OptionalPropsBuiltin {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn RequiredOptionalEvalProps(&self) -> exprctx::OptionalEvalPropKeySet {
        self.required
    }
    fn SafeToShareAcrossSession(&self) -> bool {
        true
    }
    fn getArgs(&self) -> &[Box<dyn Expression>] {
        &self.args
    }
    fn getArgsMut(&mut self) -> &mut [Box<dyn Expression>] {
        &mut self.args
    }
    fn equal(&self, _ctx: &dyn EvalContext, other: &dyn builtinFunc) -> bool {
        other.as_any().is::<Self>()
    }
    fn getRetTp(&self) -> &types::FieldType {
        &self.ret_type
    }
    fn setPbCode(&mut self, _code: i32) {}
    fn PbCode(&self) -> i32 {
        0
    }
    fn setCollator(&mut self, collator: Box<dyn collate::Collator>) {
        self.collator = collator;
    }
    fn collator(&self) -> &dyn collate::Collator {
        self.collator.as_ref()
    }
    fn Clone(&self) -> Box<dyn builtinFunc> {
        Box::new(Self::new(self.required, Vec::new()))
    }
    fn MemoryUsage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
    }
    fn vectorized(&self) -> bool {
        true
    }
}

fn scalar_with_props(
    required: exprctx::OptionalEvalPropKeySet,
    args: Vec<Box<dyn Expression>>,
) -> Box<dyn Expression> {
    Box::new(ScalarFunction {
        FuncName: ast::NewCIStr("optional_props_test"),
        RetType: Some(int_type()),
        Function: Box::new(OptionalPropsBuiltin::new(required, args)),
        hashcode: Vec::new(),
        canonicalhashcode: Vec::new(),
    })
}

/// 构造 BIGINT 字段类型。
fn int_type() -> types::FieldType {
    *types::NewFieldType(mysql::TypeLonglong)
}

/// 构造指向 input 第 `index` 列的整型 Column 表达式。
fn int_column(index: isize) -> Box<dyn Expression> {
    Box::new(Column::new(
        int_type(),
        index as i64 + 1,
        index as i64 + 1,
        index,
    ))
}

/// 构造整型常量表达式。
fn int_constant(value: i64) -> Box<dyn Expression> {
    Box::new(Constant::with_type(types::NewIntDatum(value), int_type()))
}

/// 用给定整数值填充单列 chunk。
fn int_input(values: &[i64]) -> Box<chunk::Chunk> {
    let mut input = chunk::NewChunkWithCapacity(vec![int_type()], values.len());
    for value in values {
        input.AppendInt64(0, *value);
    }
    input
}

/// 列求值走交换：两列共享同一输入列引用，输入列被掏空。
#[test]
fn column_evaluator_swaps_and_reuses_the_input_column() {
    let ctx = exprstatic::NewEvalContext(Vec::new());
    let suite = NewEvaluatorSuite(vec![int_column(0), int_column(0)], false);
    assert!(suite.Vectorizable());
    assert!(suite.ColumnSwapHelper.is_some());
    assert_eq!(
        suite.RequiredOptionalEvalProps(),
        exprctx::OptionalEvalPropKeySet::default()
    );

    let mut input = int_input(&[1, 2, 3]);
    let mut output = chunk::NewChunkWithCapacity(vec![int_type(), int_type()], 3);
    suite.Run(&ctx, true, &mut input, &mut output).unwrap();

    assert_eq!(output.Column(0).Int64s(), vec![1, 2, 3]);
    assert_eq!(output.Column(1).Int64s(), vec![1, 2, 3]);
    assert!(output.Column(0).same_ref(output.Column(1)));
    assert_eq!(input.Column(0).Rows(), 0);
}

/// 禁用列求值时保留输入列，按行复制到输出。
#[test]
fn avoid_column_evaluator_keeps_input_and_evaluates_by_rows() {
    let ctx = exprstatic::NewEvalContext(Vec::new());
    let suite = NewEvaluatorSuite(vec![int_column(0)], true);
    assert!(suite.Vectorizable());
    assert!(suite.ColumnSwapHelper.is_none());

    let mut input = int_input(&[-5, 0, 8]);
    let mut output = chunk::NewChunkWithCapacity(vec![int_type()], 3);
    suite.Run(&ctx, false, &mut input, &mut output).unwrap();

    assert_eq!(input.Column(0).Int64s(), vec![-5, 0, 8]);
    assert_eq!(output.Column(0).Int64s(), vec![-5, 0, 8]);
}

/// 常量在向量化开关开/关时结果一致。
#[test]
fn default_evaluator_matches_vectorized_and_scalar_constant_paths() {
    let ctx = exprstatic::NewEvalContext(Vec::new());
    for vec_enabled in [false, true] {
        let suite = NewEvaluatorSuite(vec![int_constant(42)], false);
        assert!(suite.Vectorizable());
        assert!(suite.ColumnSwapHelper.is_none());
        assert_eq!(
            suite.RequiredOptionalEvalProps(),
            exprctx::OptionalEvalPropKeySet::default()
        );

        let mut input = chunk::NewChunkWithCapacity(Vec::<types::FieldType>::new(), 4);
        input.SetNumVirtualRows(4);
        let mut output = chunk::NewChunkWithCapacity(vec![int_type()], 4);
        suite
            .Run(&ctx, vec_enabled, &mut input, &mut output)
            .unwrap();

        assert_eq!(output.Column(0).Int64s(), vec![42, 42, 42, 42]);
        assert!((0..4).all(|row| !output.Column(0).IsNull(row)));
    }
}

/// 空套件视为可向量化且无可选属性。
#[test]
fn empty_suite_is_vectorizable_and_has_no_optional_properties() {
    let suite = NewEvaluatorSuite(Vec::new(), false);
    assert!(suite.Vectorizable());
    assert!(suite.ColumnSwapHelper.is_none());
    assert_eq!(
        suite.RequiredOptionalEvalProps(),
        exprctx::OptionalEvalPropKeySet::default()
    );
}

/// 常量与列不是 ScalarFunction，可选属性为空。
#[test]
fn non_scalar_expressions_have_no_optional_properties() {
    let constant = int_constant(7);
    let column = int_column(0);
    assert_eq!(
        GetOptionalEvalPropsForExpr(constant.as_ref()),
        exprctx::OptionalEvalPropKeySet::default()
    );
    assert_eq!(
        GetOptionalEvalPropsForExpr(column.as_ref()),
        exprctx::OptionalEvalPropKeySet::default()
    );
}

/// 对齐 Go TestOptionalProp：递归收集父函数、子函数及套件内兄弟表达式的属性并集。
#[test]
fn optional_properties_are_collected_recursively_across_the_suite() {
    let current_user = exprctx::OptPropCurrentUser.AsPropKeySet();
    let ddl_owner = exprctx::OptPropDDLOwnerInfo.AsPropKeySet();
    let advisory_lock = exprctx::OptPropAdvisoryLock.AsPropKeySet();

    let nested = scalar_with_props(
        current_user,
        vec![scalar_with_props(ddl_owner, vec![int_constant(1)])],
    );
    assert_eq!(
        GetOptionalEvalPropsForExpr(nested.as_ref()),
        exprctx::OptionalEvalPropKeySet(current_user.0 | ddl_owner.0)
    );

    let suite = NewEvaluatorSuite(
        vec![nested, scalar_with_props(advisory_lock, Vec::new())],
        false,
    );
    assert_eq!(
        suite.RequiredOptionalEvalProps(),
        exprctx::OptionalEvalPropKeySet(current_user.0 | ddl_owner.0 | advisory_lock.0)
    );
}
