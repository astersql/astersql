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

// 标量函数（ScalarFunction）表达式节点与构造/折叠/哈希辅助。
//
// 对应 Go `scalar_function.go`：包装 builtin 实现，提供向量/标量求值入口、
// 常量折叠、规范哈希（用于语义相等）以及 `NewFunction*` 系列工厂。

use crate::*;

/// 对应 Go 的 ScalarFunction：保存函数名、返回类型、builtin 实现以及两类惰性哈希缓存。
pub struct ScalarFunction {
    pub FuncName: ast::CIStr,
    pub RetType: Option<types::FieldType>,
    pub Function: Box<dyn builtinFunc>,
    pub(crate) hashcode: Vec<u8>,
    pub(crate) canonicalhashcode: Vec<u8>,
}

impl ScalarFunction {
    /// 委托 builtin 判断实例能否跨会话共享。
    pub fn SafeToShareAcrossSession(&self) -> bool {
        self.Function.SafeToShareAcrossSession()
    }

    // 向量求值保留 Go 的断言包装：测试断言开启时先包装上下文，再交给 builtin。
    pub fn VecEvalInt(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        intest::AssertNotNil(Some(ctx), &[]);
        if intest::EnableAssert.load(std::sync::atomic::Ordering::Relaxed) {
            let asserted = wrapEvalAssert(ctx, self.Function.as_ref());
            return self.Function.vecEvalInt(&asserted, input, result);
        }
        self.Function.vecEvalInt(ctx, input, result)
    }
    pub fn VecEvalReal(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        intest::AssertNotNil(Some(ctx), &[]);
        if intest::EnableAssert.load(std::sync::atomic::Ordering::Relaxed) {
            let asserted = wrapEvalAssert(ctx, self.Function.as_ref());
            return self.Function.vecEvalReal(&asserted, input, result);
        }
        self.Function.vecEvalReal(ctx, input, result)
    }
    pub fn VecEvalString(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        intest::AssertNotNil(Some(ctx), &[]);
        if intest::EnableAssert.load(std::sync::atomic::Ordering::Relaxed) {
            let asserted = wrapEvalAssert(ctx, self.Function.as_ref());
            return self.Function.vecEvalString(&asserted, input, result);
        }
        self.Function.vecEvalString(ctx, input, result)
    }
    pub fn VecEvalDecimal(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        intest::AssertNotNil(Some(ctx), &[]);
        if intest::EnableAssert.load(std::sync::atomic::Ordering::Relaxed) {
            let asserted = wrapEvalAssert(ctx, self.Function.as_ref());
            return self.Function.vecEvalDecimal(&asserted, input, result);
        }
        self.Function.vecEvalDecimal(ctx, input, result)
    }
    pub fn VecEvalTime(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        intest::AssertNotNil(Some(ctx), &[]);
        if intest::EnableAssert.load(std::sync::atomic::Ordering::Relaxed) {
            let asserted = wrapEvalAssert(ctx, self.Function.as_ref());
            return self.Function.vecEvalTime(&asserted, input, result);
        }
        self.Function.vecEvalTime(ctx, input, result)
    }
    pub fn VecEvalDuration(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        intest::AssertNotNil(Some(ctx), &[]);
        if intest::EnableAssert.load(std::sync::atomic::Ordering::Relaxed) {
            let asserted = wrapEvalAssert(ctx, self.Function.as_ref());
            return self.Function.vecEvalDuration(&asserted, input, result);
        }
        self.Function.vecEvalDuration(ctx, input, result)
    }
    pub fn VecEvalJSON(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        intest::AssertNotNil(Some(ctx), &[]);
        if intest::EnableAssert.load(std::sync::atomic::Ordering::Relaxed) {
            let asserted = wrapEvalAssert(ctx, self.Function.as_ref());
            return self.Function.vecEvalJSON(&asserted, input, result);
        }
        self.Function.vecEvalJSON(ctx, input, result)
    }
    pub fn VecEvalVectorFloat32(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        self.Function.vecEvalVectorFloat32(ctx, input, result)
    }

    /// 只读访问 builtin 参数列表。
    pub fn GetArgs(&self) -> &[Box<dyn Expression>] {
        self.Function.getArgs()
    }
    /// 可变访问 builtin 参数列表（参数改写后通常需 ReHashCode）。
    pub fn GetArgsMut(&mut self) -> &mut [Box<dyn Expression>] {
        self.Function.getArgsMut()
    }
    /// 自身与全部子表达式均支持向量化求值时返回 true。
    pub fn Vectorized(&self) -> bool {
        self.Function.vectorized() && self.Function.isChildrenVectorized()
    }

    /// 按 Go 的输出格式呈现函数；CAST 会把返回类型作为额外文本写在每个参数后。
    pub fn StringWithCtx(&self, ctx: &dyn ParamValues, redact: &str) -> String {
        let mut buffer =
            String::with_capacity(self.FuncName.L.len() + 8 + 16 * self.GetArgs().len());
        buffer.push_str(&self.FuncName.L);
        buffer.push('(');
        if self.FuncName.L == ast::Cast {
            for arg in self.GetArgs() {
                buffer.push_str(&arg.StringWithCtx(Some(ctx), redact));
                buffer.push_str(", ");
                buffer.push_str(&self.RetType.as_ref().unwrap().String());
            }
        } else {
            for (i, arg) in self.GetArgs().iter().enumerate() {
                buffer.push_str(&arg.StringWithCtx(Some(ctx), redact));
                if i + 1 != self.GetArgs().len() {
                    buffer.push_str(", ");
                }
            }
        }
        buffer.push(')');
        buffer
    }
    pub fn String(&self) -> String {
        self.StringWithCtx(&exprctx::EmptyParamValues, errors::RedactLogDisable)
    }

    pub fn Clone(&self) -> Box<dyn Expression> {
        let mut cloned = ScalarFunction {
            FuncName: self.FuncName.clone(),
            RetType: self.RetType.clone(),
            Function: self.Function.Clone(),
            hashcode: Vec::new(),
            canonicalhashcode: self.canonicalhashcode.clone(),
        };
        cloned.SetCharsetAndCollation(self.CharsetAndCollation());
        cloned.SetCoercibility(self.Coercibility());
        cloned.SetRepertoire(self.Repertoire());
        Box::new(cloned)
    }
    pub fn clone_scalar(&self) -> ScalarFunction {
        ScalarFunction {
            FuncName: self.FuncName.clone(),
            RetType: self.RetType.clone(),
            Function: self.Function.Clone(),
            hashcode: Vec::new(),
            canonicalhashcode: self.canonicalhashcode.clone(),
        }
    }
    pub fn GetType(&self, _ctx: &dyn EvalContext) -> &types::FieldType {
        self.GetStaticType()
    }
    pub fn GetStaticType(&self) -> &types::FieldType {
        self.RetType.as_ref().unwrap()
    }

    /// 先比较对象、函数名和返回类型；双方均已有缓存时用哈希快速判断，否则递归比较 builtin。
    pub fn Equal(&self, ctx: &dyn EvalContext, other: &dyn Expression) -> bool {
        intest::AssertNotNil(Some(ctx), &[]);
        let Some(fun) = other.as_any().downcast_ref::<ScalarFunction>() else {
            return false;
        };
        if std::ptr::eq(self, fun) {
            return true;
        }
        if self.FuncName.L != fun.FuncName.L || self.RetType != fun.RetType {
            return false;
        }
        if !self.hashcode.is_empty() && !fun.hashcode.is_empty() {
            return self.hashcode == fun.hashcode;
        }
        self.Function.equal(ctx, fun.Function.as_ref())
    }

    pub fn IsCorrelated(&self) -> bool {
        self.GetArgs().iter().any(|arg| arg.IsCorrelated())
    }

    /// 不可折叠及扩展函数保守返回 ConstNone，其余取所有子表达式中的最低常量等级。
    pub fn ConstLevel(&self) -> ConstLevel {
        if unFoldableFunctions.contains_key(self.FuncName.L.as_str())
            || self.Function.isExtensionFunction()
        {
            return ConstNone;
        }
        let mut level = ConstStrict;
        for arg in self.GetArgs() {
            let arg_level = arg.ConstLevel();
            if arg_level == ConstNone {
                return ConstNone;
            }
            if arg_level < level {
                level = arg_level;
            }
        }
        level
    }

    pub fn Decorrelate(&mut self, schema: &Schema) -> &mut Self {
        for arg in self.GetArgsMut() {
            *arg = arg.Decorrelate(schema);
        }
        self.CleanHashCode();
        self
    }
    pub fn Traverse(&self, action: &dyn TraverseAction) -> Box<dyn Expression> {
        action.Transform(self.Clone())
    }

    /// 依据静态 EvalType 分派到具体求值入口，并在 NULL/错误时保持 Go Datum 的空值处理。
    pub fn Eval(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<types::Datum, Error> {
        intest::AssertNotNil(Some(ctx), &[]);
        let tp = self.GetType(ctx);
        let mut result = types::Datum::default();
        let is_null = match tp.EvalType() {
            types::ETInt => {
                let (v, null) = self.EvalInt(ctx, row)?;
                if !null && mysql::HasUnsignedFlag(tp.GetFlag()) {
                    result.SetUint64(v as u64);
                } else if !null {
                    result.SetInt64(v);
                }
                null
            }
            types::ETReal => {
                let (v, n) = self.EvalReal(ctx, row)?;
                if !n {
                    result.SetFloat64(v);
                }
                n
            }
            types::ETDecimal => {
                let (v, n) = self.EvalDecimal(ctx, row)?;
                if !n {
                    result.SetMysqlDecimal(v);
                }
                n
            }
            types::ETDatetime | types::ETTimestamp => {
                let (v, n) = self.EvalTime(ctx, row)?;
                if !n {
                    result.SetMysqlTime(v);
                }
                n
            }
            types::ETDuration => {
                let (v, n) = self.EvalDuration(ctx, row)?;
                if !n {
                    result.SetMysqlDuration(v);
                }
                n
            }
            types::ETJson => {
                let (v, n) = self.EvalJSON(ctx, row)?;
                if !n {
                    result.SetMysqlJSON(v);
                }
                n
            }
            types::ETVectorFloat32 => {
                let (v, n) = self.EvalVectorFloat32(ctx, row)?;
                if !n {
                    result.SetVectorFloat32(v);
                }
                n
            }
            types::ETString => {
                let (text, null) = self.EvalString(ctx, row)?;
                if !null && tp.GetType() == mysql::TypeEnum {
                    let value = match types::ParseEnum(tp.GetElems(), &text, tp.GetCollate()) {
                        Ok(value) => value,
                        Err(error) => ctx
                            .TypeCtx()
                            .HandleTruncate(types::Enum::default(), error)?,
                    };
                    result.SetMysqlEnum(value, tp.GetCollate().to_owned());
                } else if !null {
                    result.SetString(text, tp.GetCollate().to_owned());
                }
                null
            }
            _ => {
                return Err(errors::Errorf(format!(
                    "unsupported evaluation type {:?}",
                    tp.EvalType()
                )));
            }
        };
        if is_null {
            result.SetNull();
        }
        Ok(result)
    }

    // 标量求值入口与向量入口一样，仅负责断言包装和委托。
    pub fn EvalInt(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<(i64, bool), Error> {
        intest::AssertNotNil(Some(ctx), &[]);
        if intest::EnableAssert.load(std::sync::atomic::Ordering::Relaxed) {
            let asserted = wrapEvalAssert(ctx, self.Function.as_ref());
            return self.Function.evalInt(&asserted, row);
        }
        self.Function.evalInt(ctx, row)
    }
    pub fn EvalReal(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<(f64, bool), Error> {
        intest::AssertNotNil(Some(ctx), &[]);
        if intest::EnableAssert.load(std::sync::atomic::Ordering::Relaxed) {
            let asserted = wrapEvalAssert(ctx, self.Function.as_ref());
            return self.Function.evalReal(&asserted, row);
        }
        self.Function.evalReal(ctx, row)
    }
    pub fn EvalDecimal(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::MyDecimal, bool), Error> {
        intest::AssertNotNil(Some(ctx), &[]);
        if intest::EnableAssert.load(std::sync::atomic::Ordering::Relaxed) {
            let asserted = wrapEvalAssert(ctx, self.Function.as_ref());
            return self.Function.evalDecimal(&asserted, row);
        }
        self.Function.evalDecimal(ctx, row)
    }
    pub fn EvalString(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(String, bool), Error> {
        intest::AssertNotNil(Some(ctx), &[]);
        if intest::EnableAssert.load(std::sync::atomic::Ordering::Relaxed) {
            let asserted = wrapEvalAssert(ctx, self.Function.as_ref());
            return self.Function.evalString(&asserted, row);
        }
        self.Function.evalString(ctx, row)
    }
    pub fn EvalTime(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::Time, bool), Error> {
        intest::AssertNotNil(Some(ctx), &[]);
        if intest::EnableAssert.load(std::sync::atomic::Ordering::Relaxed) {
            let asserted = wrapEvalAssert(ctx, self.Function.as_ref());
            return self.Function.evalTime(&asserted, row);
        }
        self.Function.evalTime(ctx, row)
    }
    pub fn EvalDuration(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::Duration, bool), Error> {
        intest::AssertNotNil(Some(ctx), &[]);
        if intest::EnableAssert.load(std::sync::atomic::Ordering::Relaxed) {
            let asserted = wrapEvalAssert(ctx, self.Function.as_ref());
            return self.Function.evalDuration(&asserted, row);
        }
        self.Function.evalDuration(ctx, row)
    }
    pub fn EvalJSON(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::BinaryJSON, bool), Error> {
        intest::AssertNotNil(Some(ctx), &[]);
        if intest::EnableAssert.load(std::sync::atomic::Ordering::Relaxed) {
            let asserted = wrapEvalAssert(ctx, self.Function.as_ref());
            return self.Function.evalJSON(&asserted, row);
        }
        self.Function.evalJSON(ctx, row)
    }
    pub fn EvalVectorFloat32(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::VectorFloat32, bool), Error> {
        self.Function.evalVectorFloat32(ctx, row)
    }

    pub fn HashCode(&mut self) -> &[u8] {
        if !self.hashcode.is_empty() {
            if intest::InTest.load(std::sync::atomic::Ordering::SeqCst) {
                assertCheckHashCode(self);
            }
            return &self.hashcode;
        }
        ReHashCode(self);
        &self.hashcode
    }
    pub fn CanonicalHashCode(&mut self) -> &[u8] {
        if self.canonicalhashcode.is_empty() {
            simpleCanonicalizedHashCode(self);
        }
        &self.canonicalhashcode
    }
    pub fn CleanHashCode(&mut self) {
        self.hashcode.clear();
        self.canonicalhashcode.clear();
    }

    /// HashEquals 的 Hash64 实现把函数名、可空返回类型和参数数量一并编码，避免不同参数数目碰撞。
    pub fn Hash64(&self, h: &mut dyn base::Hasher) {
        h.HashByte(scalarFunctionFlag);
        h.HashString(&self.FuncName.L);
        match &self.RetType {
            None => h.HashByte(base::NilFlag),
            Some(tp) => {
                h.HashByte(base::NotNilFlag);
                hashFieldType(h, tp);
            }
        }
        h.HashInt(self.GetArgs().len() as isize);
        for arg in self.GetArgs() {
            arg.Hash64(h);
        }
    }

    pub fn Equals(&self, other: &dyn std::any::Any) -> bool {
        let Some(sf2) = other.downcast_ref::<ScalarFunction>() else {
            return false;
        };
        if self.FuncName.L != sf2.FuncName.L
            || self.RetType != sf2.RetType
            || self.GetArgs().len() != sf2.GetArgs().len()
        {
            return false;
        }
        self.GetArgs()
            .iter()
            .zip(sf2.GetArgs())
            .all(|(left, right)| left.Equals(right.as_any()))
    }

    /// 解析索引时先克隆，避免改写共享表达式；递归失败立即返回原错误。
    pub fn ResolveIndices(&self, schema: &Schema) -> Result<Box<dyn Expression>, Error> {
        let mut cloned = self.Clone();
        cloned.resolveIndices(schema)?;
        Ok(cloned)
    }
    fn resolveIndices(&mut self, schema: &Schema) -> Result<(), Error> {
        for arg in self.GetArgsMut() {
            arg.resolveIndices(schema)?;
        }
        Ok(())
    }
    pub fn ResolveIndicesByVirtualExpr(
        &self,
        ctx: &dyn EvalContext,
        schema: &Schema,
    ) -> (Box<dyn Expression>, bool) {
        let mut cloned = self.Clone();
        let ok = cloned.resolveIndicesByVirtualExpr(ctx, schema);
        (cloned, ok)
    }
    fn resolveIndicesByVirtualExpr(&mut self, ctx: &dyn EvalContext, schema: &Schema) -> bool {
        self.GetArgsMut()
            .iter_mut()
            .all(|arg| arg.resolveIndicesByVirtualExpr(ctx, schema))
    }

    pub fn RemapColumn(
        &self,
        mapping: &std::collections::HashMap<i64, Column>,
    ) -> Result<Box<dyn Expression>, Error> {
        let mut cloned = self.clone_scalar();
        for arg in cloned.GetArgsMut() {
            *arg = arg.RemapColumn(mapping)?;
        }
        cloned.CleanHashCode();
        Ok(Box::new(cloned))
    }

    /// 识别仅由加、减、取负和常量包裹的单列排序键，并累计是否需要反转排序方向。
    pub fn GetSingleColumn(&self, reverse: bool) -> (Option<&Column>, bool) {
        let args = self.GetArgs();
        match self.FuncName.L.as_str() {
            ast::Plus => single_column_from_binary(args, reverse, false),
            ast::Minus => single_column_from_binary(args, reverse, true),
            ast::UnaryMinus => single_column_from_unary(args, !reverse),
            _ => (None, false),
        }
    }

    pub fn Coercibility(&self) -> Coercibility {
        if !self.Function.HasCoercibility() {
            self.Function
                .SetCoercibility(deriveCoercibilityForScalarFunc(self));
        }
        self.Function.Coercibility()
    }
    pub fn HasCoercibility(&self) -> bool {
        self.Function.HasCoercibility()
    }
    pub fn SetCoercibility(&self, value: Coercibility) {
        self.Function.SetCoercibility(value);
    }
    pub fn CharsetAndCollation(&self) -> (String, String) {
        self.Function.CharsetAndCollation()
    }
    pub fn SetCharsetAndCollation(&mut self, pair: (String, String)) {
        self.Function.SetCharsetAndCollation(pair.0, pair.1);
    }
    pub fn Repertoire(&self) -> Repertoire {
        self.Function.Repertoire()
    }
    pub fn SetRepertoire(&mut self, value: Repertoire) {
        self.Function.SetRepertoire(value);
    }
    pub fn IsExplicitCharset(&self) -> bool {
        self.Function.IsExplicitCharset()
    }
    pub fn SetExplicitCharset(&mut self, explicit: bool) {
        self.Function.SetExplicitCharset(explicit);
    }

    /// 估算结构体、函数名、哈希容量以及动态返回类型/builtin 的堆内存。
    pub fn MemoryUsage(&self) -> i64 {
        std::mem::size_of::<ScalarFunction>() as i64
            + (self.FuncName.L.len() + self.FuncName.O.len() + self.hashcode.capacity()) as i64
            + self.RetType.as_ref().map_or(0, |tp| tp.MemoryUsage())
            + self.Function.MemoryUsage()
    }
}

/// NULL 类型推导：只有参数中同时存在 NULL 与非 NULL 时，才用非空操作数类型克隆替换 NULL 常量，并移除 NotNull 标记。
pub fn typeInferForNull(ctx: &dyn EvalContext, args: &mut [Box<dyn Expression>]) {
    if args.len() < 2 {
        return;
    }
    let is_null = |expr: &dyn Expression| {
        expr.as_any().downcast_ref::<Constant>().is_some_and(|c| {
            c.RetType
                .as_ref()
                .is_some_and(|tp| tp.GetType() == mysql::TypeNull)
                && c.Value.IsNull()
        })
    };
    let ret_type = args
        .iter()
        .rev()
        .find(|arg| !is_null(arg.as_ref()))
        .map(|arg| arg.GetType(ctx).clone());
    if ret_type.is_none() || !args.iter().any(|arg| is_null(arg.as_ref())) {
        return;
    }
    let ret_type = ret_type.unwrap();
    for arg in args.iter_mut().filter(|arg| is_null(arg.as_ref())) {
        let mut new_arg = arg.CloneExpr();
        *new_arg.GetTypeMut() = ret_type.clone();
        new_arg.GetTypeMut().DelFlag(mysql::NotNullFlag);
        *arg = new_arg;
    }
}

/// ScalarFunction 构造时可选的检查/初始化回调类型。
pub type ScalarFunctionCallBack = fn(&mut ScalarFunction) -> Result<(), Error>;

/// 构造入口保留特殊函数短路、函数类查找、noop 模式、NULL 推导、初始化回调与三种折叠策略。
///
/// `fold`：1=强制折叠，0=不折叠，-1=尝试折叠（产生新告警则回退）。
pub fn newFunctionImpl(
    ctx: &dyn BuildContext,
    fold: i32,
    mut func_name: String,
    mut ret_type: Option<types::FieldType>,
    check_or_init: Option<ScalarFunctionCallBack>,
    args: Vec<Box<dyn Expression>>,
) -> Result<Box<dyn Expression>, Error> {
    let Some(mut return_type) = ret_type.take() else {
        return Err(errors::Errorf("RetType cannot be nil for ScalarFunction"));
    };
    match func_name.as_str() {
        ast::Cast => return Ok(BuildCastFunction(ctx, &args[0], &return_type)),
        ast::GetVar => return BuildGetVarFunction(ctx, &args[0], &return_type),
        InternalFuncFromBinary => {
            return Ok(BuildFromBinaryFunction(ctx, &args[0], &return_type, false));
        }
        InternalFuncToBinary => return Ok(BuildToBinaryFunction(ctx, &args[0])),
        ast::Sysdate if ctx.GetSysdateIsNow() => func_name = ast::Now.to_owned(),
        _ => {}
    }
    let Some(fc) = funcs
        .get(&func_name)
        .or_else(|| extensionFuncs.Load(&func_name))
    else {
        let db = ctx.GetEvalCtx().CurrentDB();
        return if db.is_empty() {
            Err(errors::New("No database selected"))
        } else {
            Err(errors::New(format!(
                "FUNCTION {}.{} does not exist",
                db, func_name
            )))
        };
    };
    let noop_mode = ctx.GetNoopFuncsMode();
    if noop_mode != variable::OnInt && noopFuncs.contains_key(func_name.as_str()) {
        let err = errors::New(format!(
            "function {func_name} has only noop implementation in tidb now, use tidb_enable_noop_functions to enable these functions"
        ));
        if noop_mode == variable::OffInt {
            return Err(err);
        }
        // Warn 模式只追加警告，仍继续构造函数。
        ctx.GetEvalCtx()
            .AppendWarning(contextutil::errors::SharedError::new(err));
    }
    let mut func_args = args;
    if !matches!(
        func_name.as_str(),
        ast::If | ast::Ifnull | ast::Nullif | ast::RowFunc
    ) {
        typeInferForNull(ctx.GetEvalCtx(), &mut func_args);
    }
    let function = fc.getFunction(ctx, func_args)?;
    if function.getRetTp().GetType() != mysql::TypeUnspecified
        || return_type.GetType() == mysql::TypeUnspecified
    {
        return_type = function.getRetTp().clone();
    }
    let mut scalar = ScalarFunction {
        FuncName: ast::NewCIStr(&func_name),
        RetType: Some(return_type),
        Function: function,
        hashcode: Vec::new(),
        canonicalhashcode: Vec::new(),
    };
    if let Some(callback) = check_or_init {
        callback(&mut scalar)?;
    }
    if fold == 1 {
        return Ok(FoldConstant(ctx, Box::new(scalar)));
    }
    if fold == -1 {
        let before = ctx.GetEvalCtx().WarningCount();
        let original = scalar.clone_scalar();
        let folded = FoldConstant(ctx, Box::new(scalar));
        if ctx.GetEvalCtx().WarningCount() > before {
            ctx.GetEvalCtx().TruncateWarnings(before as isize);
            return Ok(Box::new(original));
        }
        return Ok(folded);
    }
    Ok(Box::new(scalar))
}

/// GROUPING 未初始化元数据时拒绝进入执行计划缓存路径。
pub fn defaultScalarFunctionCheck(function: &mut ScalarFunction) -> Result<(), Error> {
    if function.FuncName.L == ast::Grouping
        && function.Function.groupingMetaInitialized() == Some(false)
    {
        return Err(errors::Errorf(
            "grouping meta data hasn't been initialized, try use function clone instead",
        ));
    }
    Ok(())
}
/// 带自定义初始化回调的构造，并强制常量折叠。
pub fn NewFunctionWithInit(
    ctx: &dyn BuildContext,
    name: &str,
    ret: types::FieldType,
    init: ScalarFunctionCallBack,
    args: Vec<Box<dyn Expression>>,
) -> Result<Box<dyn Expression>, Error> {
    newFunctionImpl(ctx, 1, name.to_owned(), Some(ret), Some(init), args)
}
/// 标准构造：默认检查 + 强制常量折叠。
pub fn NewFunction(
    ctx: &dyn BuildContext,
    name: &str,
    ret: types::FieldType,
    args: Vec<Box<dyn Expression>>,
) -> Result<Box<dyn Expression>, Error> {
    newFunctionImpl(
        ctx,
        1,
        name.to_owned(),
        Some(ret),
        Some(defaultScalarFunctionCheck),
        args,
    )
}
/// 基础构造：默认检查但不折叠（供 PB 反序列化等路径）。
pub fn NewFunctionBase(
    ctx: &dyn BuildContext,
    name: &str,
    ret: types::FieldType,
    args: Vec<Box<dyn Expression>>,
) -> Result<Box<dyn Expression>, Error> {
    newFunctionImpl(
        ctx,
        0,
        name.to_owned(),
        Some(ret),
        Some(defaultScalarFunctionCheck),
        args,
    )
}
/// 尝试折叠：若折叠产生新告警则回退到原表达式。
pub fn NewFunctionTryFold(
    ctx: &dyn BuildContext,
    name: &str,
    ret: types::FieldType,
    args: Vec<Box<dyn Expression>>,
) -> Result<Box<dyn Expression>, Error> {
    newFunctionImpl(
        ctx,
        -1,
        name.to_owned(),
        Some(ret),
        Some(defaultScalarFunctionCheck),
        args,
    )
}

/// 仅供内部旧调用使用：与 Go 一样记录构造错误，再返回表达式槽位。
pub fn NewFunctionInternal(
    ctx: &dyn BuildContext,
    name: &str,
    ret: types::FieldType,
    args: Vec<Box<dyn Expression>>,
) -> Option<Box<dyn Expression>> {
    match NewFunction(ctx, name, ret, args) {
        Ok(expr) => Some(expr),
        Err(err) => {
            terror::Log(err);
            None
        }
    }
}
/// 将标量函数列表装箱为动态 Expression 列表。
pub fn ScalarFuncs2Exprs(functions: Vec<ScalarFunction>) -> Vec<Box<dyn Expression>> {
    functions
        .into_iter()
        .map(|f| Box::new(f) as Box<dyn Expression>)
        .collect()
}
/// 通过规范哈希判断两表达式是否语义相等。
pub fn ExpressionsSemanticEqual(left: &mut dyn Expression, right: &mut dyn Expression) -> bool {
    left.CanonicalHashCode() == right.CanonicalHashCode()
}

/// 测试模式下复制旧哈希并在克隆体上重算，用于发现原地改写后忘记清缓存的问题；不影响生产路径。
pub fn assertCheckHashCode(sf: &ScalarFunction) {
    intest::Assert(
        intest::InTest.load(std::sync::atomic::Ordering::SeqCst),
        &[],
    );
    let original = sf.hashcode.clone();
    let mut cloned = sf.clone_scalar();
    ReHashCode(&mut cloned);
    intest::Assert(cloned.hashcode == original, &[]);
}

/// 规范哈希对交换律运算排序参数，对 GE/LE、GT/LT 统一方向，并把 NOT 比较转换成等价反向比较。
pub fn simpleCanonicalizedHashCode(sf: &mut ScalarFunction) {
    sf.canonicalhashcode.clear();
    sf.canonicalhashcode.push(scalarFunctionFlag);
    let mut args: Vec<Vec<u8>> = sf
        .GetArgs()
        .iter()
        .map(|arg| arg.CanonicalHashCode().to_vec())
        .collect();
    match sf.FuncName.L.as_str() {
        ast::Plus | ast::Mul | ast::EQ | ast::In | ast::LogicOr | ast::LogicAnd => {
            sf.canonicalhashcode = codec::EncodeCompactBytes(
                std::mem::take(&mut sf.canonicalhashcode),
                sf.FuncName.L.as_bytes(),
            );
            args.sort();
        }
        ast::GE | ast::LE => {
            sf.canonicalhashcode = codec::EncodeCompactBytes(
                std::mem::take(&mut sf.canonicalhashcode),
                ast::GE.as_bytes(),
            );
            if sf.FuncName.L == ast::LE {
                args.reverse();
            }
        }
        ast::GT | ast::LT => {
            sf.canonicalhashcode = codec::EncodeCompactBytes(
                std::mem::take(&mut sf.canonicalhashcode),
                ast::GT.as_bytes(),
            );
            if sf.FuncName.L == ast::LT {
                args.reverse();
            }
        }
        ast::UnaryNot => {
            let child_data = sf.GetArgs()[0]
                .as_any()
                .downcast_ref::<ScalarFunction>()
                .map(|child| {
                    (
                        child
                            .GetArgs()
                            .iter()
                            .map(|arg| arg.CanonicalHashCode().to_vec())
                            .collect::<Vec<_>>(),
                        child.FuncName.L.clone(),
                    )
                });
            if let Some((child_args, child_name)) = child_data {
                args = child_args;
                let encoded_name = match child_name.as_str() {
                    ast::GT | ast::LT => ast::GE.to_owned(),
                    ast::GE | ast::LE => ast::GT.to_owned(),
                    _ => {
                        args.clear();
                        String::new()
                    }
                };
                if !encoded_name.is_empty() {
                    sf.canonicalhashcode = codec::EncodeCompactBytes(
                        std::mem::take(&mut sf.canonicalhashcode),
                        encoded_name.as_bytes(),
                    );
                    if matches!(child_name.as_str(), ast::GT | ast::GE) {
                        args.reverse();
                    }
                }
            } else {
                let function_name = sf.FuncName.L.clone();
                sf.canonicalhashcode = codec::EncodeCompactBytes(
                    std::mem::take(&mut sf.canonicalhashcode),
                    function_name.as_bytes(),
                );
            }
        }
        _ => {
            sf.canonicalhashcode = codec::EncodeCompactBytes(
                std::mem::take(&mut sf.canonicalhashcode),
                sf.FuncName.L.as_bytes(),
            )
        }
    }
    for code in args {
        sf.canonicalhashcode.extend(code);
    }
    if sf.FuncName.L == ast::Cast {
        sf.canonicalhashcode
            .push(sf.RetType.as_ref().unwrap().EvalType().0);
    }
}

/// 参数原地变化后重算普通哈希；GROUPING 额外稳定编码模式和排序后的 mark 键数量。
pub fn ReHashCode(sf: &mut ScalarFunction) {
    sf.hashcode.clear();
    sf.canonicalhashcode.clear();
    sf.hashcode.push(scalarFunctionFlag);
    sf.hashcode =
        codec::EncodeCompactBytes(std::mem::take(&mut sf.hashcode), sf.FuncName.L.as_bytes());
    let argument_hashes = sf
        .GetArgs()
        .iter()
        .map(|arg| arg.HashCode().to_vec())
        .collect::<Vec<_>>();
    for hash in argument_hashes {
        sf.hashcode.extend(hash);
    }
    if sf.FuncName.L == ast::Cast {
        sf.hashcode.push(sf.RetType.as_ref().unwrap().EvalType().0);
    }
    if sf.FuncName.L == ast::Grouping {
        if let Some((mode, marks)) = sf.Function.groupingModeAndMarks() {
            sf.hashcode = codec::EncodeInt(std::mem::take(&mut sf.hashcode), mode);
            sf.hashcode = codec::EncodeInt(std::mem::take(&mut sf.hashcode), marks.len() as i64);
            for mut keys in marks {
                keys.sort_unstable();
                sf.hashcode = codec::EncodeInt(std::mem::take(&mut sf.hashcode), keys.len() as i64);
                for key in keys {
                    sf.hashcode = codec::EncodeInt(std::mem::take(&mut sf.hashcode), key as i64);
                }
            }
        }
    }
}

/// 对应 unsafe.Sizeof(ScalarFunction{})；Rust 用 size_of 记录空结构体的静态尺寸。
pub const emptyScalarFunctionSize: i64 = std::mem::size_of::<ScalarFunction>() as i64;

// 下列辅助函数展开 Go 的类型 switch；动态表达式下转由 Expression::as_any 提供。
fn single_column_from_unary(
    args: &[Box<dyn Expression>],
    reverse: bool,
) -> (Option<&Column>, bool) {
    if let Some(column) = args[0].as_any().downcast_ref::<Column>() {
        return (Some(column), reverse);
    }
    if let Some(scalar) = args[0].as_any().downcast_ref::<ScalarFunction>() {
        return scalar.GetSingleColumn(reverse);
    }
    (None, false)
}
fn single_column_from_binary(
    args: &[Box<dyn Expression>],
    reverse: bool,
    subtraction: bool,
) -> (Option<&Column>, bool) {
    if args[1].as_any().is::<Constant>() {
        if let Some(column) = args[0].as_any().downcast_ref::<Column>() {
            return (Some(column), reverse);
        }
        if let Some(scalar) = args[0].as_any().downcast_ref::<ScalarFunction>() {
            return scalar.GetSingleColumn(reverse);
        }
    }
    if args[0].as_any().is::<Constant>() {
        let direction = if subtraction { !reverse } else { reverse };
        if let Some(column) = args[1].as_any().downcast_ref::<Column>() {
            return (Some(column), direction);
        }
        if let Some(scalar) = args[1].as_any().downcast_ref::<ScalarFunction>() {
            return scalar.GetSingleColumn(direction);
        }
    }
    (None, false)
}
