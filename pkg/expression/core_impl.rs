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

// 表达式核心 trait 实现桥接层。
//
// 为 Column / CorrelatedColumn / Constant / ScalarFunction 统一挂接
// `VecExpr`（向量化求值）、`CollationInfo`（字符集与 coercibility）、
// `SafeToShareAcrossSession`、`StringerWithCtx`、`Hash64`/`Equals`，
// 以及完整的 `Expression` 接口转发；实际算法仍在各类型自身方法中。

use std::any::Any;
use std::collections::HashMap;

use crate::*;

/// 将各表达式类型的向量化方法转发到同名固有实现，避免手写四份样板。
macro_rules! forward_vec_expr {
    ($ty:ty) => {
        impl VecExpr for $ty {
            fn Vectorized(&self) -> bool {
                <$ty>::Vectorized(self)
            }
            fn VecEvalInt(
                &self,
                c: &dyn EvalContext,
                i: &chunk::Chunk,
                r: &mut chunk::Column,
            ) -> Result<(), Error> {
                <$ty>::VecEvalInt(self, c, i, r)
            }
            fn VecEvalReal(
                &self,
                c: &dyn EvalContext,
                i: &chunk::Chunk,
                r: &mut chunk::Column,
            ) -> Result<(), Error> {
                <$ty>::VecEvalReal(self, c, i, r)
            }
            fn VecEvalString(
                &self,
                c: &dyn EvalContext,
                i: &chunk::Chunk,
                r: &mut chunk::Column,
            ) -> Result<(), Error> {
                <$ty>::VecEvalString(self, c, i, r)
            }
            fn VecEvalDecimal(
                &self,
                c: &dyn EvalContext,
                i: &chunk::Chunk,
                r: &mut chunk::Column,
            ) -> Result<(), Error> {
                <$ty>::VecEvalDecimal(self, c, i, r)
            }
            fn VecEvalTime(
                &self,
                c: &dyn EvalContext,
                i: &chunk::Chunk,
                r: &mut chunk::Column,
            ) -> Result<(), Error> {
                <$ty>::VecEvalTime(self, c, i, r)
            }
            fn VecEvalDuration(
                &self,
                c: &dyn EvalContext,
                i: &chunk::Chunk,
                r: &mut chunk::Column,
            ) -> Result<(), Error> {
                <$ty>::VecEvalDuration(self, c, i, r)
            }
            fn VecEvalJSON(
                &self,
                c: &dyn EvalContext,
                i: &chunk::Chunk,
                r: &mut chunk::Column,
            ) -> Result<(), Error> {
                <$ty>::VecEvalJSON(self, c, i, r)
            }
            fn VecEvalVectorFloat32(
                &self,
                c: &dyn EvalContext,
                i: &chunk::Chunk,
                r: &mut chunk::Column,
            ) -> Result<(), Error> {
                <$ty>::VecEvalVectorFloat32(self, c, i, r)
            }
        }
    };
}

forward_vec_expr!(Column);
forward_vec_expr!(CorrelatedColumn);
forward_vec_expr!(Constant);
forward_vec_expr!(ScalarFunction);

/// 通过内嵌 collation_info 字段直接实现 CollationInfo（列与常量）。
macro_rules! direct_collation {
    ($ty:ty, $field:ident) => {
        impl CollationInfo for $ty {
            fn HasCoercibility(&self) -> bool {
                self.$field.HasCoercibility()
            }
            fn Coercibility(&self) -> Coercibility {
                <$ty>::Coercibility(self)
            }
            fn SetCoercibility(&self, value: Coercibility) {
                self.$field.SetCoercibility(value)
            }
            fn Repertoire(&self) -> Repertoire {
                <$ty>::Repertoire(self)
            }
            fn SetRepertoire(&mut self, value: Repertoire) {
                self.$field.SetRepertoire(value)
            }
            fn CharsetAndCollation(&self) -> (String, String) {
                self.$field.CharsetAndCollation()
            }
            fn SetCharsetAndCollation(&mut self, charset: String, collation: String) {
                self.$field.SetCharsetAndCollation(charset, collation)
            }
            fn IsExplicitCharset(&self) -> bool {
                self.$field.IsExplicitCharset()
            }
            fn SetExplicitCharset(&mut self, explicit: bool) {
                self.$field.SetExplicitCharset(explicit)
            }
        }
    };
}

direct_collation!(Column, collation_info);
direct_collation!(Constant, collation_info);

/// 关联列的排序规则委托给内层 `column`。
impl CollationInfo for CorrelatedColumn {
    fn HasCoercibility(&self) -> bool {
        self.column.HasCoercibility()
    }
    fn Coercibility(&self) -> Coercibility {
        self.column.Coercibility()
    }
    fn SetCoercibility(&self, value: Coercibility) {
        self.column.SetCoercibility(value)
    }
    fn Repertoire(&self) -> Repertoire {
        self.column.Repertoire()
    }
    fn SetRepertoire(&mut self, value: Repertoire) {
        self.column.SetRepertoire(value)
    }
    fn CharsetAndCollation(&self) -> (String, String) {
        self.column.CharsetAndCollation()
    }
    fn SetCharsetAndCollation(&mut self, charset: String, collation: String) {
        self.column.SetCharsetAndCollation(charset, collation)
    }
    fn IsExplicitCharset(&self) -> bool {
        self.column.IsExplicitCharset()
    }
    fn SetExplicitCharset(&mut self, explicit: bool) {
        self.column.SetExplicitCharset(explicit)
    }
}

/// 标量函数的 CollationInfo 转发到 ScalarFunction 固有方法。
impl CollationInfo for ScalarFunction {
    fn HasCoercibility(&self) -> bool {
        ScalarFunction::HasCoercibility(self)
    }
    fn Coercibility(&self) -> Coercibility {
        ScalarFunction::Coercibility(self)
    }
    fn SetCoercibility(&self, value: Coercibility) {
        ScalarFunction::SetCoercibility(self, value)
    }
    fn Repertoire(&self) -> Repertoire {
        ScalarFunction::Repertoire(self)
    }
    fn SetRepertoire(&mut self, value: Repertoire) {
        ScalarFunction::SetRepertoire(self, value)
    }
    fn CharsetAndCollation(&self) -> (String, String) {
        ScalarFunction::CharsetAndCollation(self)
    }
    fn SetCharsetAndCollation(&mut self, charset: String, collation: String) {
        ScalarFunction::SetCharsetAndCollation(self, (charset, collation))
    }
    fn IsExplicitCharset(&self) -> bool {
        ScalarFunction::IsExplicitCharset(self)
    }
    fn SetExplicitCharset(&mut self, explicit: bool) {
        ScalarFunction::SetExplicitCharset(self, explicit)
    }
}

/// 将会话共享安全性查询转发到各类型固有方法。
macro_rules! safe_share {
    ($ty:ty) => {
        impl SafeToShareAcrossSession for $ty {
            fn SafeToShareAcrossSession(&self) -> bool {
                <$ty>::SafeToShareAcrossSession(self)
            }
        }
    };
}
safe_share!(Column);
safe_share!(CorrelatedColumn);
safe_share!(Constant);
safe_share!(ScalarFunction);

/// 列的可解释字符串，缺省参数上下文时用 EmptyParamValues。
impl StringerWithCtx for Column {
    fn StringWithCtx(&self, ctx: Option<&dyn ParamValues>, redact: &str) -> String {
        Column::StringWithCtx(self, ctx.unwrap_or(&exprctx::EmptyParamValues), redact)
    }
}
/// 关联列 Explain 文本委托内层列。
impl StringerWithCtx for CorrelatedColumn {
    fn StringWithCtx(&self, ctx: Option<&dyn ParamValues>, redact: &str) -> String {
        self.column
            .StringWithCtx(ctx.unwrap_or(&exprctx::EmptyParamValues), redact)
    }
}
/// 常量的可解释字符串。
impl StringerWithCtx for Constant {
    fn StringWithCtx(&self, ctx: Option<&dyn ParamValues>, redact: &str) -> String {
        Constant::StringWithCtx(self, ctx.unwrap_or(&exprctx::EmptyParamValues), redact)
    }
}
/// 标量函数的可解释字符串。
impl StringerWithCtx for ScalarFunction {
    fn StringWithCtx(&self, ctx: Option<&dyn ParamValues>, redact: &str) -> String {
        ScalarFunction::StringWithCtx(self, ctx.unwrap_or(&exprctx::EmptyParamValues), redact)
    }
}

/// 统一注册 Hash64 / Equals，供表达式树结构比较与去重。
macro_rules! base_hash {
    ($ty:ty, $hash:path, $equals:expr) => {
        impl base::Hash64 for $ty {
            fn Hash64(&self, h: &mut dyn base::Hasher) {
                $hash(self, h)
            }
        }
        impl base::Equals for $ty {
            fn Equals(&self, other: &dyn Any) -> bool {
                ($equals)(self, other)
            }
        }
    };
}
base_hash!(Column, Column::Hash64, |this: &Column, other: &dyn Any| {
    other
        .downcast_ref::<Column>()
        .is_some_and(|rhs| Column::Equals(this, rhs))
});
base_hash!(
    Constant,
    Constant::Hash64,
    |this: &Constant, other: &dyn Any| other
        .downcast_ref::<Constant>()
        .is_some_and(|rhs| Constant::Equals(this, rhs))
);
base_hash!(
    ScalarFunction,
    ScalarFunction::Hash64,
    |this: &ScalarFunction, other: &dyn Any| { ScalarFunction::Equals(this, other) }
);
base_hash!(
    CorrelatedColumn,
    CorrelatedColumn::Hash64,
    |this: &CorrelatedColumn, other: &dyn Any| other
        .downcast_ref::<CorrelatedColumn>()
        .is_some_and(|rhs| CorrelatedColumn::Equals(this, rhs))
);

/// 无 redact 的基础 Explain 文本，供调试路径使用。
fn basic_explain(expression: &dyn StringerWithCtx) -> String {
    expression.StringWithCtx(Some(&exprctx::EmptyParamValues), errors::RedactLogDisable)
}

/// Column 的 Expression 实现：按行/向量求值、解析下标、重映射列身份。
impl Expression for Column {
    fn Traverse(&self, action: &dyn TraverseAction) -> ExprBox {
        action.Transform(Box::new(self.clone()))
    }
    fn Eval(&self, c: &dyn EvalContext, r: chunk::Row) -> Result<types::Datum, Error> {
        Column::Eval(self, c, r)
    }
    fn EvalInt(&self, c: &dyn EvalContext, r: chunk::Row) -> Result<(i64, bool), Error> {
        Column::EvalInt(self, c, r)
    }
    fn EvalReal(&self, c: &dyn EvalContext, r: chunk::Row) -> Result<(f64, bool), Error> {
        Column::EvalReal(self, c, r)
    }
    fn EvalString(&self, c: &dyn EvalContext, r: chunk::Row) -> Result<(String, bool), Error> {
        Column::EvalString(self, c, r)
    }
    fn EvalDecimal(
        &self,
        c: &dyn EvalContext,
        r: chunk::Row,
    ) -> Result<(types::MyDecimal, bool), Error> {
        Column::EvalDecimal(self, c, r)
    }
    fn EvalTime(&self, c: &dyn EvalContext, r: chunk::Row) -> Result<(types::Time, bool), Error> {
        Column::EvalTime(self, c, r)
    }
    fn EvalDuration(
        &self,
        c: &dyn EvalContext,
        r: chunk::Row,
    ) -> Result<(types::Duration, bool), Error> {
        Column::EvalDuration(self, c, r)
    }
    fn EvalJSON(
        &self,
        c: &dyn EvalContext,
        r: chunk::Row,
    ) -> Result<(types::BinaryJSON, bool), Error> {
        Column::EvalJSON(self, c, r)
    }
    fn EvalVectorFloat32(
        &self,
        c: &dyn EvalContext,
        r: chunk::Row,
    ) -> Result<(types::VectorFloat32, bool), Error> {
        Column::EvalVectorFloat32(self, c, r)
    }
    fn GetType(&self, c: &dyn EvalContext) -> &types::FieldType {
        Column::GetType(self, c)
    }
    fn GetTypeMut(&mut self) -> &mut types::FieldType {
        self.RetType.as_mut().unwrap()
    }
    fn CloneExpr(&self) -> ExprBox {
        Box::new(self.clone())
    }
    fn Equal(&self, c: &dyn EvalContext, o: &dyn Expression) -> bool {
        Column::Equal(self, c, o)
    }
    fn IsCorrelated(&self) -> bool {
        false
    }
    fn ConstLevel(&self) -> ConstLevel {
        ConstNone
    }
    fn Decorrelate(&self, _: &Schema) -> ExprBox {
        Box::new(self.clone())
    }
    fn ResolveIndices(&self, s: &Schema) -> Result<ExprBox, Error> {
        Ok(Box::new(Column::ResolveIndices(self, s)?))
    }
    fn resolveIndices(&mut self, s: &Schema) -> Result<(), Error> {
        *self = Column::ResolveIndices(self, s)?;
        Ok(())
    }
    fn ResolveIndicesByVirtualExpr(&self, c: &dyn EvalContext, s: &Schema) -> (ExprBox, bool) {
        let (v, ok) = Column::ResolveIndicesByVirtualExpr(self, c, s);
        (Box::new(v), ok)
    }
    fn resolveIndicesByVirtualExpr(&mut self, c: &dyn EvalContext, s: &Schema) -> bool {
        let (v, ok) = Column::ResolveIndicesByVirtualExpr(self, c, s);
        *self = v;
        ok
    }
    fn RemapColumn(&self, m: &HashMap<i64, Column>) -> Result<ExprBox, Error> {
        Ok(Box::new(Column::RemapColumn(self, m)?))
    }
    fn ExplainInfo(&self, ctx: &dyn EvalContext) -> String {
        Column::ExplainInfo(self, ctx)
    }
    fn ExplainNormalizedInfo(&self) -> String {
        Column::ExplainNormalizedInfo(self)
    }
    fn ExplainNormalizedInfo4InList(&self) -> String {
        Column::ExplainNormalizedInfo4InList(self)
    }
    fn HashCode(&self) -> Vec<u8> {
        let mut v = self.clone();
        Column::HashCode(&mut v)
    }
    fn CanonicalHashCode(&self) -> Vec<u8> {
        let mut v = self.clone();
        Column::CanonicalHashCode(&mut v)
    }
    fn MemoryUsage(&self) -> i64 {
        Column::MemoryUsage(self)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// Constant 的 Expression 实现：求值转发固有方法；规范化 Explain 以 `?` 占位。
impl Expression for Constant {
    fn Traverse(&self, action: &dyn TraverseAction) -> ExprBox {
        action.Transform(Box::new(self.Clone()))
    }
    fn Eval(&self, c: &dyn EvalContext, r: chunk::Row) -> Result<types::Datum, Error> {
        Constant::Eval(self, c, r)
    }
    fn EvalInt(&self, c: &dyn EvalContext, r: chunk::Row) -> Result<(i64, bool), Error> {
        Constant::EvalInt(self, c, r)
    }
    fn EvalReal(&self, c: &dyn EvalContext, r: chunk::Row) -> Result<(f64, bool), Error> {
        Constant::EvalReal(self, c, r)
    }
    fn EvalString(&self, c: &dyn EvalContext, r: chunk::Row) -> Result<(String, bool), Error> {
        Constant::EvalString(self, c, r)
    }
    fn EvalDecimal(
        &self,
        c: &dyn EvalContext,
        r: chunk::Row,
    ) -> Result<(types::MyDecimal, bool), Error> {
        Constant::EvalDecimal(self, c, r)
    }
    fn EvalTime(&self, c: &dyn EvalContext, r: chunk::Row) -> Result<(types::Time, bool), Error> {
        Constant::EvalTime(self, c, r)
    }
    fn EvalDuration(
        &self,
        c: &dyn EvalContext,
        r: chunk::Row,
    ) -> Result<(types::Duration, bool), Error> {
        Constant::EvalDuration(self, c, r)
    }
    fn EvalJSON(
        &self,
        c: &dyn EvalContext,
        r: chunk::Row,
    ) -> Result<(types::BinaryJSON, bool), Error> {
        Constant::EvalJSON(self, c, r)
    }
    fn EvalVectorFloat32(
        &self,
        c: &dyn EvalContext,
        r: chunk::Row,
    ) -> Result<(types::VectorFloat32, bool), Error> {
        Constant::EvalVectorFloat32(self, c, r)
    }
    fn GetType(&self, _: &dyn EvalContext) -> &types::FieldType {
        self.RetType.as_ref().unwrap()
    }
    fn GetTypeMut(&mut self) -> &mut types::FieldType {
        self.RetType.as_mut().unwrap()
    }
    fn CloneExpr(&self) -> ExprBox {
        Box::new(self.Clone())
    }
    fn Equal(&self, c: &dyn EvalContext, o: &dyn Expression) -> bool {
        Constant::Equal(self, c, o)
    }
    fn IsCorrelated(&self) -> bool {
        false
    }
    fn ConstLevel(&self) -> ConstLevel {
        Constant::ConstLevel(self)
    }
    fn Decorrelate(&self, _: &Schema) -> ExprBox {
        Box::new(self.Clone())
    }
    fn ResolveIndices(&self, _: &Schema) -> Result<ExprBox, Error> {
        Ok(Box::new(self.Clone()))
    }
    fn resolveIndices(&mut self, _: &Schema) -> Result<(), Error> {
        Ok(())
    }
    fn ResolveIndicesByVirtualExpr(&self, _: &dyn EvalContext, _: &Schema) -> (ExprBox, bool) {
        (Box::new(self.Clone()), true)
    }
    fn resolveIndicesByVirtualExpr(&mut self, _: &dyn EvalContext, _: &Schema) -> bool {
        true
    }
    fn RemapColumn(&self, _: &HashMap<i64, Column>) -> Result<ExprBox, Error> {
        Ok(Box::new(self.Clone()))
    }
    fn ExplainInfo(&self, ctx: &dyn EvalContext) -> String {
        Constant::ExplainInfo(self, ctx)
    }
    fn ExplainNormalizedInfo(&self) -> String {
        "?".to_owned()
    }
    fn ExplainNormalizedInfo4InList(&self) -> String {
        "?".to_owned()
    }
    fn HashCode(&self) -> Vec<u8> {
        let mut v = self.Clone();
        Constant::HashCode(&mut v)
    }
    fn CanonicalHashCode(&self) -> Vec<u8> {
        let mut v = self.Clone();
        Constant::CanonicalHashCode(&mut v)
    }
    fn MemoryUsage(&self) -> i64 {
        Constant::MemoryUsage(self)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// CorrelatedColumn：`IsCorrelated` 为真；Decorrelate 在 schema 含该列时退化为普通列。
impl Expression for CorrelatedColumn {
    fn Traverse(&self, action: &dyn TraverseAction) -> ExprBox {
        action.Transform(Box::new(self.Clone()))
    }
    fn Eval(&self, c: &dyn EvalContext, r: chunk::Row) -> Result<types::Datum, Error> {
        CorrelatedColumn::Eval(self, c, r)
    }
    fn EvalInt(&self, c: &dyn EvalContext, r: chunk::Row) -> Result<(i64, bool), Error> {
        CorrelatedColumn::EvalInt(self, c, r)
    }
    fn EvalReal(&self, c: &dyn EvalContext, r: chunk::Row) -> Result<(f64, bool), Error> {
        CorrelatedColumn::EvalReal(self, c, r)
    }
    fn EvalString(&self, c: &dyn EvalContext, r: chunk::Row) -> Result<(String, bool), Error> {
        CorrelatedColumn::EvalString(self, c, r)
    }
    fn EvalDecimal(
        &self,
        c: &dyn EvalContext,
        r: chunk::Row,
    ) -> Result<(types::MyDecimal, bool), Error> {
        CorrelatedColumn::EvalDecimal(self, c, r)
    }
    fn EvalTime(&self, c: &dyn EvalContext, r: chunk::Row) -> Result<(types::Time, bool), Error> {
        CorrelatedColumn::EvalTime(self, c, r)
    }
    fn EvalDuration(
        &self,
        c: &dyn EvalContext,
        r: chunk::Row,
    ) -> Result<(types::Duration, bool), Error> {
        CorrelatedColumn::EvalDuration(self, c, r)
    }
    fn EvalJSON(
        &self,
        c: &dyn EvalContext,
        r: chunk::Row,
    ) -> Result<(types::BinaryJSON, bool), Error> {
        CorrelatedColumn::EvalJSON(self, c, r)
    }
    fn EvalVectorFloat32(
        &self,
        c: &dyn EvalContext,
        r: chunk::Row,
    ) -> Result<(types::VectorFloat32, bool), Error> {
        CorrelatedColumn::EvalVectorFloat32(self, c, r)
    }
    fn GetType(&self, c: &dyn EvalContext) -> &types::FieldType {
        self.column.GetType(c)
    }
    fn GetTypeMut(&mut self) -> &mut types::FieldType {
        self.column.RetType.as_mut().unwrap()
    }
    fn CloneExpr(&self) -> ExprBox {
        Box::new(self.Clone())
    }
    fn Equal(&self, c: &dyn EvalContext, o: &dyn Expression) -> bool {
        CorrelatedColumn::Equal(self, c, o)
    }
    fn IsCorrelated(&self) -> bool {
        true
    }
    fn ConstLevel(&self) -> ConstLevel {
        ConstNone
    }
    fn Decorrelate(&self, s: &Schema) -> ExprBox {
        // 外层 schema 已包含该列时，关联依赖可消除，降为普通列引用。
        if s.Contains(&self.column) {
            Box::new(self.column.clone())
        } else {
            Box::new(self.Clone())
        }
    }
    fn ResolveIndices(&self, _: &Schema) -> Result<ExprBox, Error> {
        Ok(Box::new(self.Clone()))
    }
    fn resolveIndices(&mut self, _: &Schema) -> Result<(), Error> {
        Ok(())
    }
    fn ResolveIndicesByVirtualExpr(&self, _: &dyn EvalContext, _: &Schema) -> (ExprBox, bool) {
        (Box::new(self.Clone()), true)
    }
    fn resolveIndicesByVirtualExpr(&mut self, _: &dyn EvalContext, _: &Schema) -> bool {
        true
    }
    fn RemapColumn(&self, m: &HashMap<i64, Column>) -> Result<ExprBox, Error> {
        Ok(Box::new(CorrelatedColumn::RemapColumn(self, m)?))
    }
    fn ExplainInfo(&self, ctx: &dyn EvalContext) -> String {
        self.column.ExplainInfo(ctx)
    }
    fn ExplainNormalizedInfo(&self) -> String {
        self.column.ExplainNormalizedInfo()
    }
    fn ExplainNormalizedInfo4InList(&self) -> String {
        self.column.ExplainNormalizedInfo4InList()
    }
    fn HashCode(&self) -> Vec<u8> {
        let mut v = vec![correlatedColumn];
        v = codec::EncodeInt(v, self.column.UniqueID);
        v
    }
    fn CanonicalHashCode(&self) -> Vec<u8> {
        self.HashCode()
    }
    fn MemoryUsage(&self) -> i64 {
        CorrelatedColumn::MemoryUsage(self)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// ScalarFunction：对参数递归 Decorrelate / ResolveIndices；哈希走标量固有实现。
impl Expression for ScalarFunction {
    fn Traverse(&self, action: &dyn TraverseAction) -> ExprBox {
        action.Transform(self.Clone())
    }
    fn Eval(&self, c: &dyn EvalContext, r: chunk::Row) -> Result<types::Datum, Error> {
        ScalarFunction::Eval(self, c, r)
    }
    fn EvalInt(&self, c: &dyn EvalContext, r: chunk::Row) -> Result<(i64, bool), Error> {
        ScalarFunction::EvalInt(self, c, r)
    }
    fn EvalReal(&self, c: &dyn EvalContext, r: chunk::Row) -> Result<(f64, bool), Error> {
        ScalarFunction::EvalReal(self, c, r)
    }
    fn EvalString(&self, c: &dyn EvalContext, r: chunk::Row) -> Result<(String, bool), Error> {
        ScalarFunction::EvalString(self, c, r)
    }
    fn EvalDecimal(
        &self,
        c: &dyn EvalContext,
        r: chunk::Row,
    ) -> Result<(types::MyDecimal, bool), Error> {
        ScalarFunction::EvalDecimal(self, c, r)
    }
    fn EvalTime(&self, c: &dyn EvalContext, r: chunk::Row) -> Result<(types::Time, bool), Error> {
        ScalarFunction::EvalTime(self, c, r)
    }
    fn EvalDuration(
        &self,
        c: &dyn EvalContext,
        r: chunk::Row,
    ) -> Result<(types::Duration, bool), Error> {
        ScalarFunction::EvalDuration(self, c, r)
    }
    fn EvalJSON(
        &self,
        c: &dyn EvalContext,
        r: chunk::Row,
    ) -> Result<(types::BinaryJSON, bool), Error> {
        ScalarFunction::EvalJSON(self, c, r)
    }
    fn EvalVectorFloat32(
        &self,
        c: &dyn EvalContext,
        r: chunk::Row,
    ) -> Result<(types::VectorFloat32, bool), Error> {
        ScalarFunction::EvalVectorFloat32(self, c, r)
    }
    fn GetType(&self, c: &dyn EvalContext) -> &types::FieldType {
        ScalarFunction::GetType(self, c)
    }
    fn GetTypeMut(&mut self) -> &mut types::FieldType {
        self.RetType.as_mut().unwrap()
    }
    fn CloneExpr(&self) -> ExprBox {
        self.Clone()
    }
    fn Equal(&self, c: &dyn EvalContext, o: &dyn Expression) -> bool {
        ScalarFunction::Equal(self, c, o)
    }
    fn IsCorrelated(&self) -> bool {
        ScalarFunction::IsCorrelated(self)
    }
    fn ConstLevel(&self) -> ConstLevel {
        ScalarFunction::ConstLevel(self)
    }
    fn Decorrelate(&self, s: &Schema) -> ExprBox {
        let mut v = self.clone_scalar();
        for arg in v.GetArgsMut() {
            *arg = arg.Decorrelate(s)
        }
        Box::new(v)
    }
    fn ResolveIndices(&self, s: &Schema) -> Result<ExprBox, Error> {
        let mut v = self.clone_scalar();
        for arg in v.GetArgsMut() {
            arg.resolveIndices(s)?
        }
        Ok(Box::new(v))
    }
    fn resolveIndices(&mut self, s: &Schema) -> Result<(), Error> {
        for arg in self.GetArgsMut() {
            arg.resolveIndices(s)?
        }
        Ok(())
    }
    fn ResolveIndicesByVirtualExpr(&self, c: &dyn EvalContext, s: &Schema) -> (ExprBox, bool) {
        let mut v = self.clone_scalar();
        let ok = v
            .GetArgsMut()
            .iter_mut()
            .all(|arg| arg.resolveIndicesByVirtualExpr(c, s));
        (Box::new(v), ok)
    }
    fn resolveIndicesByVirtualExpr(&mut self, c: &dyn EvalContext, s: &Schema) -> bool {
        self.GetArgsMut()
            .iter_mut()
            .all(|arg| arg.resolveIndicesByVirtualExpr(c, s))
    }
    fn RemapColumn(&self, m: &HashMap<i64, Column>) -> Result<ExprBox, Error> {
        ScalarFunction::RemapColumn(self, m)
    }
    fn ExplainInfo(&self, ctx: &dyn EvalContext) -> String {
        ScalarFunction::ExplainInfo(self, ctx)
    }
    fn ExplainNormalizedInfo(&self) -> String {
        ScalarFunction::ExplainNormalizedInfo(self)
    }
    fn ExplainNormalizedInfo4InList(&self) -> String {
        ScalarFunction::ExplainNormalizedInfo4InList(self)
    }
    fn HashCode(&self) -> Vec<u8> {
        let mut v = self.clone_scalar();
        ScalarFunction::HashCode(&mut v).to_vec()
    }
    fn CanonicalHashCode(&self) -> Vec<u8> {
        let mut v = self.clone_scalar();
        ScalarFunction::CanonicalHashCode(&mut v).to_vec()
    }
    fn MemoryUsage(&self) -> i64 {
        ScalarFunction::MemoryUsage(self)
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}
