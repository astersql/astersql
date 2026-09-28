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

// 表达式公共支撑：对象池桩、函数分类表、CAST 包装、常量折叠与向量化填充。
//
// 对应 Go 侧 function_traits / constant_fold 等辅助逻辑；常量折叠在优化期
// 将可确定子树求值为 Constant，向量化路径用 `genVecFromConstExpr` 把常量
// 扩展到整块 Chunk。

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::*;

/// Go sync.Pool 的占位实现：当前仅丢弃归还对象，保留调用面。
pub struct DropPool;
impl DropPool {
    pub fn Put<T>(&self, _value: T) {}
}

/// 表达式切片临时缓冲池（占位）。
pub static expressionSlices: DropPool = DropPool;
/// 选择向量临时缓冲池（占位）。
pub static selPool: DropPool = DropPool;
/// 零值列临时缓冲池（占位）。
pub static zeroPool: DropPool = DropPool;

/// 列分配器占位：get/put 与 Go ColumnAllocator 调用面对齐。
pub struct ColumnAllocator;
impl ColumnAllocator {
    pub fn get(&self) -> Result<chunk::Column, errors::Error> {
        Ok(chunk::Column::default())
    }
    pub fn put(&self, _column: chunk::Column) {}
}
/// 全局列分配器单例。
pub static globalColumnAllocator: ColumnAllocator = ColumnAllocator;

/// 逻辑/比较类算子名集合，供优化器识别可参与谓词处理的函数。
pub static logicalOps: LazyLock<HashMap<&'static str, ()>> = LazyLock::new(|| {
    HashMap::from([
        (ast::LT, ()),
        (ast::GE, ()),
        (ast::GT, ()),
        (ast::LE, ()),
        (ast::EQ, ()),
        (ast::NE, ()),
        (ast::UnaryNot, ()),
        (ast::Like, ()),
        (ast::LogicAnd, ()),
        (ast::LogicOr, ()),
        (ast::LogicXor, ()),
        (ast::In, ()),
        (ast::IsNull, ()),
        (ast::IsFalsity, ()),
        (ast::IsTruthWithoutNull, ()),
        (ast::IsTruthWithNull, ()),
        (ast::NullEQ, ()),
        (ast::Regexp, ()),
    ])
});

/// 不可折叠函数（含随机、会话变量、序列等），常量折叠须跳过。
pub static unFoldableFunctions: LazyLock<HashMap<&'static str, ()>> = LazyLock::new(|| {
    HashMap::from([
        (ast::Sysdate, ()),
        (ast::FoundRows, ()),
        (ast::Rand, ()),
        (ast::UUID, ()),
        (ast::UUIDv4, ()),
        (ast::UUIDv7, ()),
        (ast::Sleep, ()),
        (ast::RowFunc, ()),
        (ast::Values, ()),
        (ast::SetVar, ()),
        (ast::GetVar, ()),
        (ast::GetParam, ()),
        (ast::Benchmark, ()),
        (ast::DayName, ()),
        (ast::NextVal, ()),
        (ast::LastVal, ()),
        (ast::SetVal, ()),
        (ast::AnyValue, ()),
    ])
});

/// noop 模式下列出的空操作函数名（当前为空表，与 Go 初始化一致）。
pub static noopFuncs: LazyLock<HashMap<&'static str, ()>> = LazyLock::new(HashMap::new);

/// Functions whose children must not be folded while they are built. This is
/// the exact set from Go `function_traits.go`.
/// 构建期禁止折叠子表达式的函数集合（与 Go function_traits.go 一致）。
pub static DisableFoldFunctions: LazyLock<HashMap<&'static str, ()>> =
    LazyLock::new(|| HashMap::from([("benchmark", ())]));

/// Functions which optimistically fold their children and roll back on an
/// error or warning. This is the exact Go set.
/// 可乐观折叠子节点、遇错/告警再回滚的函数集合。
pub static TryFoldFunctions: LazyLock<HashMap<&'static str, ()>> = LazyLock::new(|| {
    HashMap::from([
        ("if", ()),
        ("ifnull", ()),
        ("case", ()),
        ("and", ()),
        ("or", ()),
        ("coalesce", ()),
        ("interval", ()),
    ])
});

/// 表达式侧常见错误种类，对应 Go terror 错误码生成路径。
#[derive(Clone, Copy)]
pub enum ExpressionErrorKind {
    OperandColumns,
    NotSupportedYet,
    IncorrectParameterCount,
}

impl ExpressionErrorKind {
    /// 按参数格式化生成带栈信息的错误（对齐 Go GenWithStackByArgs）。
    pub fn GenWithStackByArgs(&self, argument: impl std::fmt::Display) -> errors::Error {
        match self {
            Self::OperandColumns => {
                errors::New(format!("Operand should contain {argument} column(s)"))
            }
            Self::NotSupportedYet => errors::New(format!(
                "[expression:1235]This version of TiDB doesn't yet support '{argument}'"
            )),
            Self::IncorrectParameterCount => errors::New(format!(
                "Incorrect parameter count in the call to native function '{argument}'"
            )),
        }
    }
}

/// 操作数应包含指定列数的错误模板。
pub static ErrOperandColumns: ExpressionErrorKind = ExpressionErrorKind::OperandColumns;
/// 功能尚未支持的错误模板。
pub static ErrNotSupportedYet: ExpressionErrorKind = ExpressionErrorKind::NotSupportedYet;
/// 原生函数参数个数不正确的错误模板。
pub static ErrIncorrectParameterCount: ExpressionErrorKind =
    ExpressionErrorKind::IncorrectParameterCount;

/// Returns one for scalar values and the number of fields for a ROW function.
/// 标量返回 1；ROW 函数返回其字段个数。
pub fn GetRowLen(expression: &dyn Expression) -> usize {
    expression
        .as_scalar_function()
        .filter(|function| function.FuncName.L == ast::RowFunc)
        .map_or(1, |function| function.GetArgs().len())
}

/// Mirrors Go's `GetFuncArg`: callers have already established that the input
/// is a ROW expression, so a violated invariant is a programmer error.
/// 取出 ROW 表达式第 index 个参数的克隆；调用方须保证是标量函数。
pub fn GetFuncArg(expression: &dyn Expression, index: usize) -> ExprBox {
    expression
        .as_scalar_function()
        .expect("GetFuncArg requires a scalar function")
        .GetArgs()[index]
        .CloneExpr()
}

/// 判断参数列表或表达式是否含多列 ROW。
pub trait MultiColumnArguments {
    fn has_multi_column_row(&self) -> bool;
}

impl MultiColumnArguments for [ExprBox] {
    fn has_multi_column_row(&self) -> bool {
        self.iter()
            .any(|argument| GetRowLen(argument.as_ref()) != 1)
    }
}

impl MultiColumnArguments for Vec<ExprBox> {
    fn has_multi_column_row(&self) -> bool {
        self.as_slice().has_multi_column_row()
    }
}

impl MultiColumnArguments for dyn Expression {
    fn has_multi_column_row(&self) -> bool {
        GetRowLen(self) != 1
    }
}

/// 要求所有参数均为单列；否则返回 ErrOperandColumns(1)。
pub fn CheckArgsNotMultiColumnRow<T: MultiColumnArguments + ?Sized>(
    arguments: &T,
) -> Result<(), errors::Error> {
    if arguments.has_multi_column_row() {
        Err(ErrOperandColumns.GenWithStackByArgs(1))
    } else {
        Ok(())
    }
}

/// Recursively marks every column under an IN operand, preserving all scalar
/// function metadata and invalidating only its derived hash caches.
/// 递归标记 IN 操作数下的列 InOperand，并清理标量函数派生哈希缓存。
pub fn SetExprColumnInOperand(mut expression: ExprBox) -> ExprBox {
    if let Some(column) = expression.as_column() {
        let mut result = column.CloneColumn();
        result.InOperand = true;
        return Box::new(result);
    }
    if let Some(function) = expression.as_scalar_function() {
        let mut function = function.clone_scalar();
        for argument in function.GetArgsMut() {
            *argument = SetExprColumnInOperand(argument.CloneExpr());
        }
        function.CleanHashCode();
        return Box::new(function);
    }
    expression
}

/// 由 Datum 与 MySQL 类型码构造 Constant 表达式盒。
pub fn DatumToConstant(datum: types::Datum, field_type: u8, flag: u64) -> ExprBox {
    let mut return_type = types::NewFieldType(field_type);
    return_type.AddFlag(flag as usize);
    Box::new(Constant::with_type(datum, *return_type))
}

/// 在空行上把表达式求值为整数（优化期常量抽取常用）。
pub fn GetIntFromConstant(
    ctx: &dyn EvalContext,
    value: &dyn Expression,
) -> Result<(i64, bool), errors::Error> {
    let (value, null) = value.EvalString(ctx, chunk::Row::default())?;
    if null {
        return Ok((0, true));
    }
    match value.parse::<i64>() {
        Ok(value) => Ok((value, false)),
        Err(_) => Ok((0, true)),
    }
}

/// 构建 CAST 并设置是否显式字符集；`_in_union` 保留与 Go 签名对齐。
pub fn BuildCastFunctionWithCheck(
    ctx: &dyn BuildContext,
    expression: ExprBox,
    target: types::FieldType,
    _in_union: bool,
    explicit_charset: bool,
) -> Result<ExprBox, errors::Error> {
    let mut result = crate::formal_registry::BuildCastFunction(ctx, &expression, &target);
    result.SetExplicitCharset(explicit_charset);
    Ok(result)
}

/// 内部 CAST 包装入口，委托 formal_registry::BuildCastFunction。
fn wrap_with_cast(
    ctx: &dyn BuildContext,
    expression: ExprBox,
    target: types::FieldType,
) -> ExprBox {
    crate::formal_registry::BuildCastFunction(ctx, &expression, &target)
}

/// 包装为 Longlong（或调用方指定目标类型）的 CAST。
pub fn WrapWithCastAsInt(
    ctx: &dyn BuildContext,
    expression: ExprBox,
    target: Option<&types::FieldType>,
) -> ExprBox {
    let source = expression.GetType(ctx.GetEvalCtx()).clone();
    if source.EvalType() == types::ETInt {
        return expression;
    }
    let mut integer = *types::NewFieldType(mysql::TypeLonglong);
    integer.SetFlen(source.GetFlen());
    integer.SetDecimal(0);
    types_dependency::field::SetBinChsClnFlag(&mut integer);
    integer.AddFlag(source.GetFlag() & mysql::NotNullFlag);
    integer
        .AddFlag(target.map_or(source.GetFlag(), types::FieldType::GetFlag) & mysql::UnsignedFlag);
    wrap_with_cast(ctx, expression, integer)
}

/// 已是 ETReal 则原样返回，否则 CAST 为 Double 并继承无符号/非空标志。
pub fn WrapWithCastAsReal(ctx: &dyn BuildContext, expression: ExprBox) -> ExprBox {
    if expression.GetType(ctx.GetEvalCtx()).EvalType() == types::ETReal {
        return expression;
    }
    let source_flags = expression.GetType(ctx.GetEvalCtx()).GetFlag();
    let mut target = *types::NewFieldType(mysql::TypeDouble);
    target.SetFlen(mysql::MaxRealWidth as isize);
    target.SetDecimal(types::UnspecifiedLength as isize);
    target.SetCharset(charset::CharsetBin.to_owned());
    target.SetCollate(charset::CollationBin.to_owned());
    target.AddFlag(mysql::BinaryFlag);
    target.AddFlag(source_flags & (mysql::UnsignedFlag | mysql::NotNullFlag));
    wrap_with_cast(ctx, expression, target)
}

/// CAST 为 DECIMAL：按源类型推断 flen/decimal，整数源用固定显示宽度。
pub fn WrapWithCastAsDecimal(ctx: &dyn BuildContext, expression: ExprBox) -> ExprBox {
    let source = expression.GetType(ctx.GetEvalCtx()).clone();
    if source.EvalType() == types::ETDecimal {
        return expression;
    }
    let mut target = *types::NewFieldType(mysql::TypeNewDecimal);
    target.SetFlenUnderLimit(source.GetFlen());
    target.SetDecimalUnderLimit(source.GetDecimal());
    if source.EvalType() == types::ETInt {
        target.SetFlen(match source.GetType() {
            mysql::TypeTiny => 3,
            mysql::TypeShort => 5,
            mysql::TypeInt24 => 8,
            mysql::TypeLong => 10,
            mysql::TypeLonglong => 20,
            mysql::TypeYear => 4,
            _ => mysql::MaxIntWidth as isize,
        });
        target.SetDecimal(0);
    }
    if target.GetFlen() == types::UnspecifiedLength as isize
        || target.GetFlen() > mysql::MaxDecimalWidth as isize
    {
        target.SetFlen(mysql::MaxDecimalWidth as isize);
    }
    types_dependency::field::SetBinChsClnFlag(&mut target);
    target.AddFlag(source.GetFlag() & (mysql::UnsignedFlag | mysql::NotNullFlag));
    wrap_with_cast(ctx, expression, target)
}

/// 包装为 VarString 的 CAST。
pub fn WrapWithCastAsString(ctx: &dyn BuildContext, expression: ExprBox) -> ExprBox {
    let source = expression.GetType(ctx.GetEvalCtx()).clone();
    if source.EvalType() == types::ETString {
        return expression;
    }
    let mut length = source.GetFlen();
    if source.GetType() == mysql::TypeNewDecimal && length != types::UnspecifiedLength as isize {
        length += 3;
    }
    if source.EvalType() == types::ETInt {
        length = if source.GetType() == mysql::TypeBit {
            (source.GetFlen() + 7) / 8
        } else {
            mysql::MaxIntWidth as isize
        };
    }
    if matches!(source.GetType(), mysql::TypeFloat | mysql::TypeDouble) {
        length = types::UnspecifiedLength as isize;
    }
    let mut target = *types::NewFieldType(mysql::TypeVarString);
    if expression.Coercibility() == CoercibilityExplicit {
        let (charset, collation) = expression.CharsetAndCollation();
        target.SetCharset(charset);
        target.SetCollate(collation);
    } else if source.GetType() == mysql::TypeBit {
        target.SetCharset(charset::CharsetBin.to_owned());
        target.SetCollate(charset::CollationBin.to_owned());
    } else {
        let (charset, collation) = ctx.GetCharsetInfo();
        target.SetCharset(charset);
        target.SetCollate(collation);
    }
    target.SetFlen(length);
    target.SetDecimal(types::UnspecifiedLength as isize);
    wrap_with_cast(ctx, expression, target)
}

/// CAST 为日期/时间类型：推导 FSP 与显示宽度，已兼容的类型则短路。
pub fn WrapWithCastAsTime(
    ctx: &dyn BuildContext,
    expression: ExprBox,
    mut target: types::FieldType,
) -> ExprBox {
    let source = expression.GetType(ctx.GetEvalCtx());
    let source_type = source.GetType();
    if target.GetType() == source_type
        || (matches!(source_type, mysql::TypeDate | mysql::TypeTimestamp)
            && target.GetType() == mysql::TypeDatetime)
    {
        return expression;
    }
    match source.EvalType() {
        types::ETInt => target.SetDecimal(0),
        types::ETString | types::ETReal | types::ETJson => {
            target.SetDecimal(types::MaxFsp as isize)
        }
        types::ETDatetime | types::ETTimestamp | types::ETDuration | types::ETDecimal => {
            target.SetDecimal(source.GetDecimal().min(types::MaxFsp as isize))
        }
        _ => {}
    }
    match target.GetType() {
        mysql::TypeDate => target.SetFlen(mysql::MaxDateWidth as isize),
        mysql::TypeDatetime | mysql::TypeTimestamp => {
            target.SetFlen(mysql::MaxDatetimeWidthNoFsp as isize);
            if target.GetDecimal() > 0 {
                target.SetFlen(target.GetFlen() + 1 + target.GetDecimal());
            }
        }
        _ => {}
    }
    types_dependency::field::SetBinChsClnFlag(&mut target);
    wrap_with_cast(ctx, expression, target)
}

/// 可选表达式盒的结构相等比较。
pub fn option_expr_equals(left: &Option<ExprBox>, right: &Option<ExprBox>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => left.Equals(right.as_any()),
        _ => false,
    }
}

/// 从表达式森林抽取列，按 UniqueID 去重；filter 为真才纳入。
pub fn ExtractColumnsMapFromExpressions(
    filter: fn(&Column) -> bool,
    expressions: &[ExprBox],
) -> HashMap<i64, Column> {
    fn visit(
        result: &mut HashMap<i64, Column>,
        filter: fn(&Column) -> bool,
        expression: &dyn Expression,
    ) {
        if let Some(column) = expression.as_any().downcast_ref::<Column>() {
            if filter(column) {
                result.insert(column.UniqueID, column.clone());
            }
            return;
        }
        if let Some(function) = expression.as_any().downcast_ref::<ScalarFunction>() {
            for argument in function.GetArgs() {
                visit(result, filter, argument.as_ref());
            }
        }
    }
    let mut result = HashMap::with_capacity(expressions.len());
    for expression in expressions {
        visit(&mut result, filter, expression.as_ref());
    }
    result
}

/// DayName 等允许隐式按 Real 求值的特例识别。
pub fn CanImplicitEvalReal(expression: &dyn Expression) -> bool {
    expression
        .as_any()
        .downcast_ref::<ScalarFunction>()
        .is_some_and(|function| function.FuncName.L == ast::DayName)
}

/// Plan Cache 场景：若含 ParamMarker/DeferredExpr，折叠可能过度优化，需跳过缓存。
pub fn MaybeOverOptimized4PlanCache(ctx: &dyn BuildContext, expression: &dyn Expression) -> bool {
    if !ctx.IsUseCache() {
        return false;
    }
    if let Some(constant) = expression.as_constant() {
        return constant.ParamMarker.is_some() || constant.DeferredExpr.is_some();
    }
    expression.as_scalar_function().is_some_and(|function| {
        function
            .GetArgs()
            .iter()
            .any(|argument| MaybeOverOptimized4PlanCache(ctx, argument.as_ref()))
    })
}

/// 常量折叠入口：折叠后恢复 coercibility、字符集与排序规则元数据。
pub fn FoldConstant(ctx: &dyn BuildContext, expression: ExprBox) -> ExprBox {
    let coercibility = expression.Coercibility();
    let repertoire = expression.Repertoire();
    let charset = expression.GetType(ctx.GetEvalCtx()).GetCharset().to_owned();
    let collation = expression.GetType(ctx.GetEvalCtx()).GetCollate().to_owned();
    let (mut folded, _) = foldConstant(ctx, expression);
    folded.SetCoercibility(coercibility);
    folded.SetRepertoire(repertoire);
    folded.GetTypeMut().SetCharset(charset);
    folded.GetTypeMut().SetCollate(collation);
    folded
}

/// 内部折叠：特殊短路 → 全常量求值 → 空值拒绝试探 → 参数/延迟常量求值。
fn foldConstant(ctx: &dyn BuildContext, expression: ExprBox) -> (ExprBox, bool) {
    if let Some(function_ref) = expression.as_scalar_function() {
        // 不可折叠或扩展函数：保持原树。
        if unFoldableFunctions.contains_key(function_ref.FuncName.L.as_str())
            || function_ref.Function.isExtensionFunction()
        {
            return (expression, false);
        }
        let function = function_ref.clone_scalar();
        if !MaybeOverOptimized4PlanCache(ctx, &function) {
            // IF / IFNULL / CASE / ISNULL 走专用短路，避免错误求值未选分支。
            match function.FuncName.L.as_str() {
                ast::If => return foldIf(ctx, function),
                ast::Ifnull => return foldIfNull(ctx, function),
                ast::Case => return foldCase(ctx, function),
                ast::IsNull => return foldIsNull(ctx, function),
                _ => {}
            }
        }

        let mut all_constant = true;
        let mut has_null = false;
        let mut deferred = false;
        let mut constant_flags = Vec::with_capacity(function.GetArgs().len());
        for argument in function.GetArgs() {
            if let Some(constant) = argument.as_constant() {
                deferred |= constant.DeferredExpr.is_some() || constant.ParamMarker.is_some();
                has_null |= constant.Value.IsNull();
                constant_flags.push(true);
            } else {
                all_constant = false;
                constant_flags.push(false);
            }
        }
        if !all_constant {
            // 空值拒绝检查：用哑参数替换非常量，探测函数是否恒空/恒假。
            let excluded = matches!(
                function.FuncName.L.as_str(),
                ast::NullEQ | ast::ConcatWS | ast::Field
            );
            if !has_null || !ctx.IsInNullRejectCheck() || excluded {
                return (expression, deferred);
            }
            let dummy_arguments = function
                .GetArgs()
                .iter()
                .zip(&constant_flags)
                .map(|(argument, constant)| {
                    if *constant {
                        argument.CloneExpr()
                    } else {
                        Box::new(NewOne()) as ExprBox
                    }
                })
                .collect();
            let Ok(dummy) = NewFunctionBase(
                ctx,
                &function.FuncName.L,
                function.GetStaticType().clone(),
                dummy_arguments,
            ) else {
                return (expression, deferred);
            };
            let Ok(value) = dummy.Eval(ctx.GetEvalCtx(), chunk::Row::default()) else {
                return (expression, deferred);
            };
            if value.IsNull()
                || value
                    .ToBool(ctx.GetEvalCtx().TypeCtx())
                    .is_ok_and(|result| result == 0)
            {
                return (
                    Box::new(Constant::with_type(value, function.GetStaticType().clone())),
                    false,
                );
            }
            return (expression, deferred);
        }

        let value = match function.Eval(ctx.GetEvalCtx(), chunk::Row::default()) {
            Ok(value) => value,
            Err(error) => {
                logutil::BgLogger().debug(format!("fold expression to constant: {error}"));
                return (expression, deferred);
            }
        };
        let mut return_type = function.GetStaticType().clone();
        if !has_null {
            if value.Kind() == types::KindNull {
                return_type.DelFlag(mysql::NotNullFlag);
            } else {
                return_type.AddFlag(mysql::NotNullFlag);
            }
        }
        if deferred {
            return (
                Box::new(Constant::with_deferred(value, return_type, function)),
                true,
            );
        }
        let subquery_ref = function
            .GetArgs()
            .iter()
            .filter_map(|argument| argument.as_constant())
            .map(|constant| constant.SubqueryRefID)
            .find(|id| *id > 0)
            .unwrap_or(0);
        return (
            Box::new(Constant::with_subquery(value, return_type, subquery_ref)),
            false,
        );
    }

    if let Some(constant) = expression.as_constant() {
        if let Some(marker) = &constant.ParamMarker {
            return match marker.GetUserVar(ctx.GetEvalCtx()) {
                Ok(value) => (Box::new(constant.clone_with_value(value)), true),
                Err(error) => {
                    logutil::BgLogger().warn(format!("fail to get param: {error}"));
                    (expression, true)
                }
            };
        }
        if let Some(deferred_expression) = &constant.DeferredExpr {
            return match deferred_expression.Eval(ctx.GetEvalCtx(), chunk::Row::default()) {
                Ok(value) => (Box::new(constant.clone_with_value(value)), true),
                Err(error) => {
                    logutil::BgLogger().debug(format!("fold deferred expression: {error}"));
                    (expression, true)
                }
            };
        }
    }
    (expression, false)
}

/// ISNULL 折叠：参数已 NotNull 则恒为 0；常量子树直接求值。
fn foldIsNull(ctx: &dyn BuildContext, function: ScalarFunction) -> (ExprBox, bool) {
    let Some(constant) = function.GetArgs()[0].as_constant() else {
        if mysql::HasNotNullFlag(function.GetArgs()[0].GetType(ctx.GetEvalCtx()).GetFlag()) {
            return (Box::new(NewZero()), false);
        }
        return (Box::new(function), false);
    };
    let deferred = constant.DeferredExpr.is_some() || constant.ParamMarker.is_some();
    match function.Eval(ctx.GetEvalCtx(), chunk::Row::default()) {
        Ok(value) if deferred => {
            let return_type = function.GetStaticType().clone();
            (
                Box::new(Constant::with_deferred(value, return_type, function)),
                true,
            )
        }
        Ok(value) => (
            Box::new(Constant::with_type(value, function.GetStaticType().clone())),
            false,
        ),
        Err(_) => (Box::new(function), deferred),
    }
}

/// IF 折叠：条件常量后只折叠被选中的真/假分支。
fn foldIf(ctx: &dyn BuildContext, function: ScalarFunction) -> (ExprBox, bool) {
    let (condition, _) = foldConstant(ctx, function.GetArgs()[0].CloneExpr());
    let Some(constant) = condition.as_constant() else {
        return (Box::new(function), false);
    };
    match constant.EvalInt(ctx.GetEvalCtx(), chunk::Row::default()) {
        Ok((value, null)) => foldConstant(
            ctx,
            function.GetArgs()[if !null && value != 0 { 1 } else { 2 }].CloneExpr(),
        ),
        Err(_) => (Box::new(function), false),
    }
}

/// IFNULL：第一参数为 NULL 则折叠第二参数，否则保留第一参数。
fn foldIfNull(ctx: &dyn BuildContext, function: ScalarFunction) -> (ExprBox, bool) {
    let (first, deferred) = foldConstant(ctx, function.GetArgs()[0].CloneExpr());
    let Some(constant) = first.as_constant() else {
        return (Box::new(function), false);
    };
    if constant.Value.IsNull() {
        foldConstant(ctx, function.GetArgs()[1].CloneExpr())
    } else {
        (first, deferred)
    }
}

/// CASE WHEN 折叠：逐对求值条件，命中则折叠对应 THEN，否则走 ELSE。
fn foldCase(ctx: &dyn BuildContext, function: ScalarFunction) -> (ExprBox, bool) {
    let count = function.GetArgs().len();
    let mut deferred = false;
    for index in (0..count.saturating_sub(1)).step_by(2) {
        let (condition, condition_deferred) =
            foldConstant(ctx, function.GetArgs()[index].CloneExpr());
        deferred |= condition_deferred;
        let Some(constant) = condition.as_constant() else {
            return (Box::new(function), false);
        };
        let Ok((value, null)) = constant.EvalInt(ctx.GetEvalCtx(), chunk::Row::default()) else {
            return (Box::new(function), false);
        };
        if value != 0 && !null {
            let (mut body, body_deferred) =
                foldConstant(ctx, function.GetArgs()[index + 1].CloneExpr());
            if body.as_constant().is_some() {
                body.GetTypeMut()
                    .SetDecimal(function.GetStaticType().GetDecimal());
            }
            return (body, deferred || body_deferred);
        }
    }
    if count % 2 == 1 {
        let (mut otherwise, otherwise_deferred) =
            foldConstant(ctx, function.GetArgs()[count - 1].CloneExpr());
        if otherwise.as_constant().is_some() {
            otherwise
                .GetTypeMut()
                .SetDecimal(function.GetStaticType().GetDecimal());
        }
        return (otherwise, deferred || otherwise_deferred);
    }
    (Box::new(function), deferred)
}

/// 将常量表达式按目标求值类型重复填充到 result 列，行数与 input Chunk 一致。
pub fn genVecFromConstExpr(
    ctx: &dyn EvalContext,
    expression: &dyn Expression,
    target_type: types::EvalType,
    input: &chunk::Chunk,
    result: &mut chunk::Column,
) -> Result<(), errors::Error> {
    let count = input.NumRows();
    match target_type {
        types::ETInt => {
            let (value, null) = expression.EvalInt(ctx, chunk::Row::default())?;
            result.ResizeInt64(0, false);
            for _ in 0..count {
                if null {
                    result.AppendNull()
                } else {
                    result.AppendInt64(value)
                }
            }
        }
        types::ETReal => {
            let (value, null) = expression.EvalReal(ctx, chunk::Row::default())?;
            result.ResizeFloat64(0, false);
            for _ in 0..count {
                if null {
                    result.AppendNull()
                } else {
                    result.AppendFloat64(value)
                }
            }
        }
        types::ETDecimal => {
            let (value, null) = expression.EvalDecimal(ctx, chunk::Row::default())?;
            result.ResizeDecimal(0, false);
            for _ in 0..count {
                if null {
                    result.AppendNull()
                } else {
                    result.AppendMyDecimal(&value)
                }
            }
        }
        types::ETDatetime | types::ETTimestamp => {
            let (value, null) = expression.EvalTime(ctx, chunk::Row::default())?;
            result.ResizeTime(0, false);
            for _ in 0..count {
                if null {
                    result.AppendNull()
                } else {
                    result.AppendTime(value)
                }
            }
        }
        types::ETDuration => {
            let (value, null) = expression.EvalDuration(ctx, chunk::Row::default())?;
            result.ResizeGoDuration(0, false);
            for _ in 0..count {
                if null {
                    result.AppendNull()
                } else {
                    result.AppendDuration(value)
                }
            }
        }
        types::ETString => {
            let (value, null) = expression.EvalString(ctx, chunk::Row::default())?;
            result.ReserveString(count);
            for _ in 0..count {
                if null {
                    result.AppendNull()
                } else {
                    result.AppendString(&value)
                }
            }
        }
        types::ETJson => {
            let (value, null) = expression.EvalJSON(ctx, chunk::Row::default())?;
            result.ReserveJSON(count);
            for _ in 0..count {
                if null {
                    result.AppendNull()
                } else {
                    result.AppendJSON(value.clone())
                }
            }
        }
        types::ETVectorFloat32 => {
            let (value, null) = expression.EvalVectorFloat32(ctx, chunk::Row::default())?;
            result.ReserveVectorFloat32(count);
            for _ in 0..count {
                if null {
                    result.AppendNull()
                } else {
                    result.AppendVectorFloat32(value.Clone())
                }
            }
        }
        _ => {
            return Err(errors::New(format!(
                "unsupported evaluation type {target_type:?}"
            )));
        }
    }
    Ok(())
}

/// 将内部函数名映射为 Explain/显示用的运算符符号。
pub fn GetDisplayName(name: &str) -> &str {
    match name {
        ast::EQ => "=",
        ast::NullEQ => "<=>",
        ast::NE => "!=",
        ast::LT => "<",
        ast::LE => "<=",
        ast::GT => ">",
        ast::GE => ">=",
        ast::Plus => "+",
        ast::Minus => "-",
        ast::Mul => "*",
        ast::Div => "/",
        _ => name,
    }
}

/// Expression 动态分发辅助：向下转型为列/关联列/标量函数/常量。
impl dyn Expression + '_ {
    pub fn as_column(&self) -> Option<&Column> {
        self.as_any().downcast_ref::<Column>()
    }
    pub fn as_correlated_column(&self) -> Option<&CorrelatedColumn> {
        self.as_any().downcast_ref::<CorrelatedColumn>()
    }
    pub fn as_scalar_function(&self) -> Option<&ScalarFunction> {
        self.as_any().downcast_ref::<ScalarFunction>()
    }
    pub fn as_constant(&self) -> Option<&Constant> {
        self.as_any().downcast_ref::<Constant>()
    }
}
