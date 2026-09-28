// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// `expression.rs` 中构建选项、表达式切片池与测试辅助转换的单元测试。

use crate::expression_core::*;
use crate::*;

/// With* 选项应独立生效，且 TargetFieldType 保持指向同一目标类型。
#[test]
fn build_options_apply_independently_and_keep_target_reference() {
    let target = *types::NewFieldType(mysql::TypeNewDecimal);
    let mut options = BuildOptions::default();

    WithAllowCastArray(true)(&mut options);
    WithUseNewCollate(true)(&mut options);
    WithCastExprTo(&target)(&mut options);

    assert!(options.AllowCastArray);
    assert!(options.UseNewCollate);
    assert!(std::ptr::eq(options.TargetFieldType.unwrap(), &target));
    assert!(options.InputSchema.is_none());
    assert!(options.SourceTable.is_none());
}

/// GetExpressionSlices 返回空切片但容量满足请求；0 请求至少保留容量 4。
#[test]
fn expression_slice_pool_returns_empty_capacity_for_requested_size() {
    let mut expressions = GetExpressionSlices(9);
    assert!(expressions.is_empty());
    assert!(expressions.capacity() >= 9);
    expressions.push(Box::new(NewOne()));
    expressions.push(Box::new(NewNull()));
    PutExpressionSlices(expressions);

    let minimum = GetExpressionSlices(0);
    assert!(minimum.is_empty());
    assert!(minimum.capacity() >= 4);
}

/// Args2Expressions4Test 按原生值推断 FieldType；MyDecimal 等未知 Kind 保留 None。
#[test]
fn args_to_expressions_infers_supported_types_and_rejects_unknown_type() {
    let args: Vec<types::AnyValue> = vec![
        Box::new(42_i64),
        Box::new(u64::MAX),
        Box::new("hello".to_owned()),
        Box::new(Option::<i64>::None),
        Box::new(types::MyDecimal::default()),
    ];
    let expressions = Args2Expressions4Test(args);
    let field_type = |index: usize| {
        expressions[index]
            .as_ref()
            .unwrap()
            .as_any()
            .downcast_ref::<Constant>()
            .unwrap()
            .RetType
            .as_ref()
            .unwrap()
    };

    assert_eq!(expressions.len(), 5);
    assert_eq!(field_type(0).GetType(), mysql::TypeLong);
    assert_ne!(field_type(1).GetFlag() & mysql::UnsignedFlag, 0);
    assert_eq!(field_type(2).GetType(), mysql::TypeVarString);
    assert_eq!(field_type(3).GetType(), mysql::TypeNull);
    assert!(expressions[4].is_none());
}

/// Go 在构建虚拟生成列表达式时忽略重复的截断告警，且不修改调用方上下文。
#[test]
fn virtual_generated_column_build_context_ignores_truncate_errors() {
    let original = exprstatic::NewExprContext(Vec::new());
    assert!(!original.GetEvalCtx().TypeCtx().Flags().IgnoreTruncateErr());

    let generated = generatedColumnBuildContext(&original);
    assert!(generated.GetEvalCtx().TypeCtx().Flags().IgnoreTruncateErr());
    assert_eq!(
        generated
            .GetEvalCtx()
            .ErrCtx()
            .LevelForGroup(errctx::ErrGroup::ErrGroupTruncate),
        errctx::Level::LevelIgnore
    );
    assert!(!original.GetEvalCtx().TypeCtx().Flags().IgnoreTruncateErr());
}
