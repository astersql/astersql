// Copyright 2017 PingCAP, Inc.
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

// 常量表达式与 prepared 参数标记。
//
// 对应 Go `constant.go`：构造各类常量、实现 Constant 的行/向量求值，以及
// ParamMarker / DeferredExpr 在执行上下文中的惰性取值。常量折叠会把可确定子树替换为本类型。

use crate::*;

/// 构造器先设置 MySQL 字段元数据，再装入 Datum；无符号 0/1 用窄类型避免整数提升。
fn tiny_integer(value: i64, unsigned: bool) -> Constant {
    let mut ret_type = types::NewFieldType(mysql::TypeTiny);
    if unsigned {
        ret_type.AddFlag(mysql::UnsignedFlag);
    }
    ret_type.SetFlen(1);
    ret_type.SetDecimal(0);
    Constant::new(types::NewDatum(&value), *ret_type)
}

/// 无符号整型常量 1，常用于谓词改写与 NULL-reject 探测。
pub fn NewOne() -> Constant {
    tiny_integer(1, true)
}
/// 有符号整型常量 1。
pub fn NewSignedOne() -> Constant {
    tiny_integer(1, false)
}
/// 无符号整型常量 0。
pub fn NewZero() -> Constant {
    tiny_integer(0, true)
}
/// 有符号整型常量 0。
pub fn NewSignedZero() -> Constant {
    tiny_integer(0, false)
}

/// 无符号 64 位整数常量，字段宽度取 MaxIntWidth。
pub fn NewUInt64Const(num: usize) -> Constant {
    let mut ret_type = types::NewFieldType(mysql::TypeLonglong);
    ret_type.AddFlag(mysql::UnsignedFlag);
    ret_type.SetFlen(mysql::MaxIntWidth as isize);
    ret_type.SetDecimal(0);
    Constant::new(types::NewDatum(&num), *ret_type)
}
/// 使用调用方给定 FieldType 的无符号 64 位常量。
pub fn NewUInt64ConstWithFieldType(num: u64, field_type: types::FieldType) -> Constant {
    Constant::new(types::NewDatum(&num), field_type)
}
/// 有符号 64 位整数常量。
pub fn NewInt64Const(num: i64) -> Constant {
    let mut ret_type = types::NewFieldType(mysql::TypeLonglong);
    ret_type.SetFlen(mysql::MaxIntWidth as isize);
    ret_type.SetDecimal(0);
    Constant::new(types::NewDatum(&num), *ret_type)
}
/// 变长字符串常量，Flen 取字节长度。
pub fn NewStrConst(value: &str) -> Constant {
    let mut ret_type = types::NewFieldType(mysql::TypeVarString);
    ret_type.SetFlen(value.len() as isize);
    let value = value.to_owned();
    Constant::new(types::NewDatum(&value), *ret_type)
}
/// SQL NULL 常量，底层类型为 TINY。
pub fn NewNull() -> Constant {
    tiny_integer_datum(types::NewDatum(&Option::<i64>::None), true)
}
/// 以给定 Datum 构造 TINY 类型常量（用于 NULL）。
fn tiny_integer_datum(value: types::Datum, _null: bool) -> Constant {
    let mut ret_type = types::NewFieldType(mysql::TypeTiny);
    ret_type.SetFlen(1);
    ret_type.SetDecimal(0);
    Constant::new(value, *ret_type)
}
/// 使用调用方 FieldType 的 SQL NULL 常量。
pub fn NewNullWithFieldType(field_type: types::FieldType) -> Constant {
    Constant::new(types::NewDatum(&Option::<i64>::None), field_type)
}

/// Constant 对应 Go 常量表达式。DeferredExpr 和 ParamMarker 使其值依赖当前执行上下文。
pub struct Constant {
    pub Value: types::Datum,
    pub RetType: Option<types::FieldType>,
    pub DeferredExpr: Option<Box<dyn Expression>>,
    pub ParamMarker: Option<ParamMarker>,
    hashcode: Vec<u8>,
    pub SubqueryRefID: i64,
    pub collation_info: collationInfo,
}

/// ParamMarker 保存 prepared 参数序号，不自行持有用户变量。
#[derive(Clone)]
pub struct ParamMarker {
    order: usize,
}

impl ParamMarker {
    /// 创建一个按 prepared-statement 参数序号取值的标记。
    pub fn new(order: usize) -> Self {
        Self { order }
    }

    /// 返回该标记在 prepared-statement 参数数组中的序号。
    pub fn order(&self) -> usize {
        self.order
    }

    /// 参数解析发生在调用时，以保留 EXECUTE/COM_EXECUTE 每次绑定的新值及错误。
    pub fn GetUserVar<T: ParamValues + ?Sized>(&self, ctx: &T) -> Result<types::Datum, Error> {
        ctx.GetParamValue(self.order)
            .map_err(|error| errors::New(error.to_string()))
    }
}

impl Constant {
    /// 创建不含 DeferredExpr/ParamMarker 的普通常量。
    fn new(value: types::Datum, ret_type: types::FieldType) -> Self {
        Self {
            Value: value,
            RetType: Some(ret_type),
            DeferredExpr: None,
            ParamMarker: None,
            hashcode: Vec::new(),
            SubqueryRefID: 0,
            collation_info: collationInfo::default(),
        }
    }

    /// 指定返回类型的常量。
    pub fn with_type(value: types::Datum, ret_type: types::FieldType) -> Self {
        Self::new(value, ret_type)
    }
    /// 折叠结果仍依赖原标量函数（DeferredExpr），值可能随执行上下文变化。
    pub fn with_deferred(
        value: types::Datum,
        ret_type: types::FieldType,
        expression: ScalarFunction,
    ) -> Self {
        let mut constant = Self::new(value, ret_type);
        constant.DeferredExpr = Some(Box::new(expression));
        constant
    }
    /// 附带子查询展示 ID，便于 EXPLAIN 关联折叠来源。
    pub fn with_subquery(value: types::Datum, ret_type: types::FieldType, id: i64) -> Self {
        let mut constant = Self::new(value, ret_type);
        constant.SubqueryRefID = id;
        constant
    }
    /// 带预置排序规则元数据的常量。
    pub fn with_collation(
        value: types::Datum,
        ret_type: types::FieldType,
        collation_info: collationInfo,
    ) -> Self {
        let mut constant = Self::new(value, ret_type);
        constant.collation_info = collation_info;
        constant
    }
    /// 复制元数据并替换 Datum 值，用于折叠后更新惰性常量。
    pub fn clone_with_value(&self, value: types::Datum) -> Self {
        let mut cloned = self.Clone();
        cloned.Value = value;
        cloned
    }

    /// DeferredExpr 可跨会话共享时，常量本身也可共享。
    pub fn SafeToShareAcrossSession(&self) -> bool {
        self.DeferredExpr
            .as_ref()
            .map_or(true, |expr| expr.SafeToShareAcrossSession())
    }

    /// 按 redact 策略格式化常量；ParamMarker 取值失败时显示为 `?`。
    pub fn StringWithCtx(&self, ctx: &dyn ParamValues, redact: &str) -> String {
        let value = if let Some(marker) = &self.ParamMarker {
            // Go 在调试构建断言错误为空；生产路径取值失败时仍返回问号，避免泄露或 panic。
            match marker.GetUserVar(ctx) {
                Ok(value) => value,
                Err(_) => return "?".to_owned(),
            }
        } else if let Some(expr) = &self.DeferredExpr {
            return expr.StringWithCtx(Some(ctx), redact);
        } else {
            self.Value.clone()
        };
        let stringify = || {
            if value.Kind() == types::KindMysqlTime {
                let mut rendered = value.GetMysqlTime().String();
                let decimal = self
                    .RetType
                    .as_ref()
                    .map_or(0, |field_type| field_type.GetDecimal())
                    .clamp(0, types::MaxFsp as isize) as usize;
                if decimal > 0 && !rendered.contains('.') {
                    rendered.push('.');
                    rendered.push_str(&"0".repeat(decimal));
                }
                rendered
            } else {
                value.TruncatedStringify()
            }
        };
        let rendered = match redact {
            errors::RedactLogDisable => stringify(),
            errors::RedactLogMarker => format!("‹{}›", stringify()),
            _ => "?".to_owned(),
        };
        if self.SubqueryRefID > 0 {
            format!("ScalarQueryCol#{}({})", self.SubqueryRefID, rendered)
        } else {
            rendered
        }
    }

    /// Clone 深拷贝类型、参数标记、延迟表达式与哈希切片，避免计划缓存副本互相写入。
    pub fn Clone(&self) -> Constant {
        Constant {
            Value: self.Value.clone(),
            RetType: self.RetType.clone(),
            DeferredExpr: self.DeferredExpr.as_ref().map(|expr| expr.CloneExpr()),
            ParamMarker: self.ParamMarker.clone(),
            hashcode: self.hashcode.clone(),
            SubqueryRefID: self.SubqueryRefID,
            collation_info: self.collation_info.clone(),
        }
    }

    pub fn GetType(&self, ctx: &dyn EvalContext) -> Option<types::FieldType> {
        if let Some(marker) = &self.ParamMarker {
            // GetType 可能由 IndexJoin 多线程调用；每次新建 FieldType，避免共享指针数据竞争。
            let mut field_type = types::NewFieldType(mysql::TypeUnspecified);
            let datum = match marker.GetUserVar(ctx) {
                Ok(value) => value,
                Err(_) => return None,
            };
            types::InferParamTypeFromDatum(&datum, &mut field_type);
            return Some(*field_type);
        }
        self.RetType.clone()
    }

    // 非延迟常量由公共函数批量填充；延迟表达式必须走自身向量化路径，以便每次执行重新求值。
    pub fn VecEvalInt(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        match &self.DeferredExpr {
            None => genVecFromConstExpr(ctx, self, types::ETInt, input, result),
            Some(e) => e.VecEvalInt(ctx, input, result),
        }
    }
    pub fn VecEvalReal(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        match &self.DeferredExpr {
            None => genVecFromConstExpr(ctx, self, types::ETReal, input, result),
            Some(e) => e.VecEvalReal(ctx, input, result),
        }
    }
    pub fn VecEvalString(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        match &self.DeferredExpr {
            None => genVecFromConstExpr(ctx, self, types::ETString, input, result),
            Some(e) => e.VecEvalString(ctx, input, result),
        }
    }
    pub fn VecEvalDecimal(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        match &self.DeferredExpr {
            None => genVecFromConstExpr(ctx, self, types::ETDecimal, input, result),
            Some(e) => e.VecEvalDecimal(ctx, input, result),
        }
    }
    pub fn VecEvalTime(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        match &self.DeferredExpr {
            None => genVecFromConstExpr(ctx, self, types::ETTimestamp, input, result),
            Some(e) => e.VecEvalTime(ctx, input, result),
        }
    }
    pub fn VecEvalDuration(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        match &self.DeferredExpr {
            None => genVecFromConstExpr(ctx, self, types::ETDuration, input, result),
            Some(e) => e.VecEvalDuration(ctx, input, result),
        }
    }
    pub fn VecEvalJSON(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        match &self.DeferredExpr {
            None => genVecFromConstExpr(ctx, self, types::ETJson, input, result),
            Some(e) => e.VecEvalJSON(ctx, input, result),
        }
    }
    pub fn VecEvalVectorFloat32(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        match &self.DeferredExpr {
            None => genVecFromConstExpr(ctx, self, types::ETVectorFloat32, input, result),
            Some(e) => e.VecEvalVectorFloat32(ctx, input, result),
        }
    }

    /// getLazyDatum 区分上下文值与结构中固定 Value；bool 为 true 时调用方必须使用返回 Datum。
    fn getLazyDatum(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::Datum, bool), Error> {
        if let Some(marker) = &self.ParamMarker {
            return marker.GetUserVar(ctx).map(|value| (value, true));
        }
        if let Some(expr) = &self.DeferredExpr {
            return expr.Eval(ctx, row).map(|value| (value, true));
        }
        Ok((types::Datum::default(), false))
    }

    pub fn Traverse(&self, action: &mut dyn TraverseAction) -> Box<dyn Expression> {
        action.Transform(Box::new(self.Clone()))
    }
    pub fn Eval(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<types::Datum, Error> {
        let (datum, lazy) = self.getLazyDatum(ctx, row)?;
        if !lazy {
            return Ok(self.Value.clone());
        }
        if datum.IsNull() {
            return Ok(datum);
        }
        if self.DeferredExpr.is_some() {
            if datum.Kind() != types::KindMysqlDecimal {
                return datum.ConvertTo(typeCtx(ctx), self.RetType.as_ref().unwrap());
            }
            let mut decimal = datum.GetMysqlDecimal();
            self.adjustDecimal(ctx, &mut decimal)?;
            let mut adjusted = datum;
            adjusted.SetMysqlDecimal(decimal);
            return Ok(adjusted);
        }
        Ok(datum)
    }

    fn effectiveDatum(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<types::Datum, Error> {
        let (lazy, is_lazy) = self.getLazyDatum(ctx, row)?;
        Ok(if is_lazy { lazy } else { self.Value.clone() })
    }

    pub fn EvalInt(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<(i64, bool), Error> {
        let datum = self.effectiveDatum(ctx, row)?;
        if self
            .GetType(ctx)
            .is_some_and(|t| t.GetType() == mysql::TypeNull)
            || datum.IsNull()
        {
            return Ok((0, true));
        }
        match datum.Kind() {
            types::KindBinaryLiteral | types::KindMysqlBit => datum
                .GetBinaryLiteral()
                .ToInt(typeCtx(ctx))
                .map(|v| (v as i64, false))
                .map_err(Into::into),
            types::KindString => datum.ToInt64(typeCtx(ctx)).map(|v| (v, false)),
            _ if self.GetType(ctx).is_some_and(|t| t.Hybrid()) => {
                datum.ToInt64(typeCtx(ctx)).map(|v| (v, false))
            }
            _ => Ok((datum.GetInt64(), false)),
        }
    }
    pub fn EvalReal(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<(f64, bool), Error> {
        let datum = self.effectiveDatum(ctx, row)?;
        if self
            .GetType(ctx)
            .is_some_and(|t| t.GetType() == mysql::TypeNull)
            || datum.IsNull()
        {
            return Ok((0.0, true));
        }
        if self.GetType(ctx).is_some_and(|t| t.Hybrid())
            || matches!(datum.Kind(), types::KindBinaryLiteral | types::KindString)
        {
            return datum.ToFloat64(typeCtx(ctx)).map(|v| (v, false));
        }
        Ok((datum.GetFloat64(), false))
    }
    pub fn EvalString(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(String, bool), Error> {
        let datum = self.effectiveDatum(ctx, row)?;
        if self
            .GetType(ctx)
            .is_some_and(|t| t.GetType() == mysql::TypeNull)
            || datum.IsNull()
        {
            return Ok((String::new(), true));
        }
        datum.ToString().map(|v| (v, false))
    }
    pub fn EvalDecimal(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::MyDecimal, bool), Error> {
        let datum = self.effectiveDatum(ctx, row)?;
        if self
            .GetType(ctx)
            .is_some_and(|t| t.GetType() == mysql::TypeNull)
            || datum.IsNull()
        {
            return Ok((types::MyDecimal::default(), true));
        }
        let mut decimal = datum.ToDecimal(typeCtx(ctx))?;
        self.adjustDecimal(ctx, &mut decimal)?;
        Ok((decimal, false))
    }
    fn adjustDecimal(
        &self,
        ctx: &dyn EvalContext,
        decimal: &mut types::MyDecimal,
    ) -> Result<(), Error> {
        let (_, fraction) = decimal.PrecisionAndFrac();
        let target = self.GetType(ctx).unwrap().GetDecimal();
        // 计划构建可能提高目标小数位；仅在当前精度不足时按 half-up 补齐，避免重复舍入。
        if (fraction as isize) < target {
            let mut rounded = types::MyDecimal::default();
            decimal.Round(&mut rounded, target, types::ModeHalfUp)?;
            *decimal = rounded;
        }
        Ok(())
    }
    pub fn EvalTime(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::Time, bool), Error> {
        let datum = self.effectiveDatum(ctx, row)?;
        if self
            .GetType(ctx)
            .is_some_and(|field_type| field_type.GetType() == mysql::TypeNull)
            || datum.IsNull()
        {
            return Ok((types::ZeroTime, true));
        }
        Ok((datum.GetMysqlTime(), false))
    }
    pub fn EvalDuration(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::Duration, bool), Error> {
        let datum = self.effectiveDatum(ctx, row)?;
        if self
            .GetType(ctx)
            .is_some_and(|field_type| field_type.GetType() == mysql::TypeNull)
            || datum.IsNull()
        {
            return Ok((types::Duration::default(), true));
        }
        Ok((datum.GetMysqlDuration(), false))
    }
    pub fn EvalJSON(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::BinaryJSON, bool), Error> {
        let datum = self.effectiveDatum(ctx, row)?;
        if self
            .GetType(ctx)
            .is_some_and(|field_type| field_type.GetType() == mysql::TypeNull)
            || datum.IsNull()
        {
            return Ok((types::BinaryJSON::default(), true));
        }
        Ok((datum.GetMysqlJSON(), false))
    }
    pub fn EvalVectorFloat32(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::VectorFloat32, bool), Error> {
        let datum = self.effectiveDatum(ctx, row)?;
        if self
            .GetType(ctx)
            .is_some_and(|field_type| field_type.GetType() == mysql::TypeNull)
            || datum.IsNull()
        {
            return Ok((types::ZeroVectorFloat32(), true));
        }
        Ok((datum.GetVectorFloat32(), false))
    }

    pub fn Equal(&self, ctx: &dyn EvalContext, other: &dyn Expression) -> bool {
        let Some(rhs) = other.as_constant() else {
            return false;
        };
        if self.Eval(ctx, chunk::Row::default()).is_err()
            || rhs.Eval(ctx, chunk::Row::default()).is_err()
        {
            return false;
        }
        self.Value
            .Compare(
                typeCtx(ctx),
                &rhs.Value,
                collate::GetBinaryCollator().as_ref(),
            )
            .is_ok_and(|order| order == 0)
    }
    pub fn IsCorrelated(&self) -> bool {
        false
    }
    pub fn ConstLevel(&self) -> ConstLevel {
        if self.DeferredExpr.is_some() || self.ParamMarker.is_some() {
            ConstLevel::ConstOnlyInContext
        } else {
            ConstLevel::ConstStrict
        }
    }
    pub fn Decorrelate(&self, _schema: &Schema) -> Constant {
        self.Clone()
    }
    pub fn HashCode(&mut self) -> Vec<u8> {
        self.getHashCode(false)
    }
    pub fn CanonicalHashCode(&mut self) -> Vec<u8> {
        self.getHashCode(true)
    }

    pub fn Hash64(&self, h: &mut dyn base::Hasher) {
        if let Some(tp) = &self.RetType {
            h.HashByte(base::NotNilFlag);
            hashFieldType(h, tp);
        } else {
            h.HashByte(base::NilFlag);
        }
        self.collation_info.Hash64(h);
        if let Some(expr) = &self.DeferredExpr {
            expr.Hash64(h);
            return;
        }
        if let Some(marker) = &self.ParamMarker {
            h.HashByte(parameterFlag);
            h.HashInt64(marker.order as i64);
            return;
        }
        h.HashByte(constantFlag);
        self.Value.Hash64(h);
    }
    pub fn Equals(&self, other: &Constant) -> bool {
        self.RetType == other.RetType
            && self.collation_info.Equals(&other.collation_info)
            && option_expr_equals(&self.DeferredExpr, &other.DeferredExpr)
            && self.ParamMarker.as_ref().map(|m| m.order)
                == other.ParamMarker.as_ref().map(|m| m.order)
            && self.Value.Equals(&other.Value)
    }
    fn getHashCode(&mut self, canonical: bool) -> Vec<u8> {
        if !self.hashcode.is_empty() {
            return self.hashcode.clone();
        }
        if let Some(expr) = &mut self.DeferredExpr {
            self.hashcode = if canonical {
                expr.CanonicalHashCode()
            } else {
                expr.HashCode()
            };
        } else if let Some(marker) = &self.ParamMarker {
            self.hashcode.push(parameterFlag);
            self.hashcode =
                codec::EncodeInt(std::mem::take(&mut self.hashcode), marker.order as i64);
        } else {
            self.hashcode.push(constantFlag);
            self.hashcode = codec::HashCode(std::mem::take(&mut self.hashcode), self.Value.clone());
        }
        self.hashcode.clone()
    }

    pub fn ResolveIndices(&self, _schema: &Schema) -> Result<Constant, Error> {
        Ok(self.Clone())
    }
    fn resolveIndices(&mut self, _schema: &Schema) -> Result<(), Error> {
        Ok(())
    }
    pub fn ResolveIndicesByVirtualExpr(
        &self,
        _ctx: &dyn EvalContext,
        _schema: &Schema,
    ) -> (Constant, bool) {
        (self.Clone(), true)
    }
    fn resolveIndicesByVirtualExpr(&mut self, _ctx: &dyn EvalContext, _schema: &Schema) -> bool {
        true
    }
    pub fn RemapColumn(
        &self,
        _mapping: &std::collections::HashMap<i64, Column>,
    ) -> Result<Constant, Error> {
        Ok(self.Clone())
    }
    pub fn Vectorized(&self) -> bool {
        self.DeferredExpr
            .as_ref()
            .map_or(true, |expr| expr.Vectorized())
    }
    pub fn Coercibility(&self) -> Coercibility {
        if !self.collation_info.HasCoercibility() {
            self.collation_info
                .SetCoercibility(deriveCoercibilityForConstant(self));
        }
        self.collation_info.Coercibility()
    }
    pub fn Repertoire(&self) -> Repertoire {
        let repertoire = self.collation_info.Repertoire();
        if repertoire != 0 {
            return repertoire;
        }
        let Some(field_type) = &self.RetType else {
            return ASCII;
        };
        if field_type.EvalType() != types::ETString {
            return ASCII;
        }
        if field_type.GetCharset() == charset::CharsetASCII {
            ASCII
        } else {
            UNICODE
        }
    }
    pub fn MemoryUsage(&self) -> i64 {
        EMPTY_CONSTANT_SIZE
            + self.Value.MemUsage()
            + self.hashcode.capacity() as i64
            + self
                .RetType
                .as_ref()
                .map_or(0, types::FieldType::MemoryUsage)
    }

    pub fn null(mysql_type: u8) -> Constant {
        NewNullWithFieldType(*types::NewFieldType(mysql_type))
    }
}

impl Clone for Constant {
    fn clone(&self) -> Self {
        self.Clone()
    }
}

/// 空 Constant 结构体静态大小，MemoryUsage 以此为基数。
const EMPTY_CONSTANT_SIZE: i64 = std::mem::size_of::<Constant>() as i64;
