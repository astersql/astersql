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

// SQL 表达式核心：构建选项、标量/向量求值、CNF/DNF 折叠与表元数据转 Schema。
//
// 对应 Go `expression.go`。表达式树节点（常量、列、标量函数等）通过 `Expression`
// trait 统一求值；本文件还提供布尔过滤、空拒绝折叠、赋值结构及测试辅助转换。

// Go 的 interface、类型断言和 nil 在 Rust 中分别以 trait、downcast 辅助和 Option 表达；关键分支保留原控制流。

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::*;

/// HashCode 中常量节点标志。
pub(crate) const CONSTANT_FLAG: u8 = 0;
/// HashCode 中列节点标志。
pub(crate) const COLUMN_FLAG: u8 = 1;
/// HashCode 中标量函数节点标志。
pub(crate) const SCALAR_FUNCTION_FLAG: u8 = 3;
/// HashCode 中预处理参数节点标志。
pub(crate) const PARAMETER_FLAG: u8 = 4;
/// HashCode 中标量子查询节点标志。
pub const SCALAR_SUB_Q_FLAG: u8 = 5;
/// HashCode 中关联列节点标志。
pub(crate) const CORRELATED_COLUMN_FLAG: u8 = 6;

pub(crate) use COLUMN_FLAG as columnFlag;
pub(crate) use CONSTANT_FLAG as constantFlag;
pub(crate) use CORRELATED_COLUMN_FLAG as correlatedColumn;
pub(crate) use PARAMETER_FLAG as parameterFlag;
pub(crate) use SCALAR_FUNCTION_FLAG as scalarFunctionFlag;

/// 简化 AST 求值回调类型；由外部在启动时注入。
pub type EvalSimpleAstFn =
    fn(&dyn BuildContext, &ast::ExprNode) -> Result<types::Datum, errors::Error>;
/// 全局简化 AST 求值入口；`None` 表示尚未安装。
pub static mut EvalSimpleAst: Option<EvalSimpleAstFn> = None;

/// 构建简单表达式时的可选设置，字段顺序与 Go BuildOptions 一致。
pub struct BuildOptions<'a> {
    pub InputSchema: Option<&'a Schema>,
    pub InputNames: types::NameSlice,
    pub SourceTableDB: ast::CIStr,
    pub SourceTable: Option<&'a model::TableInfo>,
    pub AllowCastArray: bool,
    pub TargetFieldType: Option<&'a types::FieldType>,
    pub UseNewCollate: bool,
}

impl Default for BuildOptions<'_> {
    fn default() -> Self {
        Self {
            InputSchema: None,
            InputNames: types::NameSlice(Vec::new()),
            SourceTableDB: ast::CIStr::default(),
            SourceTable: None,
            AllowCastArray: false,
            TargetFieldType: None,
            UseNewCollate: false,
        }
    }
}

/// 构建选项闭包；按顺序应用到 `BuildOptions`。
pub type BuildOption<'a> = Box<dyn Fn(&mut BuildOptions<'a>) + 'a>;

/// 指定源表库名与表元数据。
pub fn WithTableInfo<'a>(db: &'a str, table: &'a model::TableInfo) -> BuildOption<'a> {
    Box::new(move |o| {
        o.SourceTableDB = ast::NewCIStr(db);
        o.SourceTable = Some(table);
    })
}

/// 注入输入 Schema、列名与可选源表。
pub fn WithInputSchemaAndNames<'a>(
    schema: &'a Schema,
    names: types::NameSlice,
    table: Option<&'a model::TableInfo>,
) -> BuildOption<'a> {
    Box::new(move |o| {
        o.InputSchema = Some(schema);
        o.InputNames = names.Shallow();
        o.SourceTable = table;
    })
}

/// 是否允许把数组 CAST 为目标类型。
pub fn WithAllowCastArray<'a>(allow: bool) -> BuildOption<'a> {
    Box::new(move |o| o.AllowCastArray = allow)
}

/// 指定构建结果要强制 CAST 到的目标 FieldType。
pub fn WithCastExprTo(target: &types::FieldType) -> BuildOption<'_> {
    Box::new(move |o| o.TargetFieldType = Some(target))
}

/// 是否启用新校对规则路径。
pub fn WithUseNewCollate<'a>(enabled: bool) -> BuildOption<'a> {
    Box::new(move |o| o.UseNewCollate = enabled)
}

/// 规划器拥有的 `buildSimpleExpr` 工厂函数签名。
pub type BuildSimpleExprFn = for<'a> fn(
    &dyn BuildContext,
    &ast::ExprNode,
    Vec<BuildOption<'a>>,
) -> Result<ExprBox, errors::Error>;

static BUILD_SIMPLE_EXPR_FACTORY: OnceLock<BuildSimpleExprFn> = OnceLock::new();

/// 一次性安装工厂；同函数幂等，不同函数报错。
pub(crate) fn installBuildSimpleExprWith(
    storage: &OnceLock<BuildSimpleExprFn>,
    factory: BuildSimpleExprFn,
    before_set: impl FnOnce(),
) -> Result<(), errors::Error> {
    if let Some(installed) = storage.get() {
        if std::ptr::fn_addr_eq(*installed, factory) {
            return Ok(());
        }
        return Err(errors::New(
            "a different BuildSimpleExpr factory is already installed",
        ));
    }

    // Two package initializers may both observe an empty OnceLock. If the
    // other thread wins `set`, re-read its value: installing the same function
    // is idempotent, while a genuinely different factory remains an error.
    before_set();
    if storage.set(factory).is_ok() {
        return Ok(());
    }
    let installed = storage
        .get()
        .expect("OnceLock::set can fail only after a factory was installed");
    if std::ptr::fn_addr_eq(*installed, factory) {
        Ok(())
    } else {
        Err(errors::New(
            "a different BuildSimpleExpr factory is already installed",
        ))
    }
}

/// Installs the planner-owned implementation of Go `buildSimpleExpr`.
///
/// Expression owns the public parsing entry point, while planner owns AST
// / rewriting. The one-time factory keeps that package boundary without an
/// expression -> planner dependency cycle.
pub fn InstallBuildSimpleExpr(factory: BuildSimpleExprFn) -> Result<(), errors::Error> {
    installBuildSimpleExprWith(&BUILD_SIMPLE_EXPR_FACTORY, factory, || {})
}

/// Dispatches to the planner implementation installed during planner setup.
pub fn BuildSimpleExpr<'a>(
    ctx: &dyn BuildContext,
    node: &ast::ExprNode,
    opts: Vec<BuildOption<'a>>,
) -> Result<ExprBox, errors::Error> {
    let factory = BUILD_SIMPLE_EXPR_FACTORY
        .get()
        .ok_or_else(|| errors::New("BuildSimpleExpr factory is not installed"))?;
    factory(ctx, node, opts)
}

/// 对应 Go VecExpr，集中声明所有向量化结果类型的写入入口。
pub trait VecExpr {
    fn Vectorized(&self) -> bool;
    fn VecEvalInt(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), errors::Error>;
    fn VecEvalReal(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), errors::Error>;
    fn VecEvalString(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), errors::Error>;
    fn VecEvalDecimal(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), errors::Error>;
    fn VecEvalTime(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), errors::Error>;
    fn VecEvalDuration(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), errors::Error>;
    fn VecEvalJSON(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), errors::Error>;
    fn VecEvalVectorFloat32(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), errors::Error>;
}

/// 表达式树遍历动作：Transform 返回替换后的子树。
pub trait TraverseAction {
    fn Transform(&self, expr: ExprBox) -> ExprBox;
}

/// 常量级别：不可折叠 / 仅上下文常量 / 严格常量。
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ConstLevel {
    ConstNone,
    ConstOnlyInContext,
    ConstStrict,
}

pub use ConstLevel::{ConstNone, ConstOnlyInContext, ConstStrict};

impl ConstLevel {
    pub const None: Self = Self::ConstNone;
}

/// 表达式是否可在会话间安全共享（无会话私有可变状态）。
pub trait SafeToShareAcrossSession {
    fn SafeToShareAcrossSession(&self) -> bool;
}

/// 堆上表达式对象的类型别名。
pub type ExprBox = Box<dyn Expression>;

impl Clone for ExprBox {
    fn clone(&self) -> Self {
        self.CloneExpr()
    }
}

/// SQL 标量表达式总接口；Result 保留 Go 的 error 返回，Option 保留可空指针结果。
pub trait Expression:
    VecExpr + CollationInfo + base::HashEquals + SafeToShareAcrossSession + StringerWithCtx
{
    fn Traverse(&self, action: &dyn TraverseAction) -> ExprBox;
    fn Eval(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<types::Datum, errors::Error>;
    fn EvalInt(&self, ctx: &dyn EvalContext, row: chunk::Row)
    -> Result<(i64, bool), errors::Error>;
    fn EvalReal(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(f64, bool), errors::Error>;
    fn EvalString(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(String, bool), errors::Error>;
    fn EvalDecimal(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::MyDecimal, bool), errors::Error>;
    fn EvalTime(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::Time, bool), errors::Error>;
    fn EvalDuration(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::Duration, bool), errors::Error>;
    fn EvalJSON(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::BinaryJSON, bool), errors::Error>;
    fn EvalVectorFloat32(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::VectorFloat32, bool), errors::Error>;
    fn GetType(&self, ctx: &dyn EvalContext) -> &types::FieldType;
    fn GetTypeMut(&mut self) -> &mut types::FieldType;
    fn CloneExpr(&self) -> ExprBox;
    fn Equal(&self, ctx: &dyn EvalContext, other: &dyn Expression) -> bool;
    fn IsCorrelated(&self) -> bool;
    fn ConstLevel(&self) -> ConstLevel;
    fn Decorrelate(&self, schema: &Schema) -> ExprBox;
    fn ResolveIndices(&self, schema: &Schema) -> Result<ExprBox, errors::Error>;
    fn resolveIndices(&mut self, schema: &Schema) -> Result<(), errors::Error>;
    fn ResolveIndicesByVirtualExpr(
        &self,
        ctx: &dyn EvalContext,
        schema: &Schema,
    ) -> (ExprBox, bool);
    fn resolveIndicesByVirtualExpr(&mut self, ctx: &dyn EvalContext, schema: &Schema) -> bool;
    fn RemapColumn(&self, mapping: &HashMap<i64, Column>) -> Result<ExprBox, errors::Error>;
    fn ExplainInfo(&self, ctx: &dyn EvalContext) -> String;
    fn ExplainNormalizedInfo(&self) -> String;
    fn ExplainNormalizedInfo4InList(&self) -> String;
    fn HashCode(&self) -> Vec<u8>;
    fn CanonicalHashCode(&self) -> Vec<u8>;
    fn MemoryUsage(&self) -> i64;
    /// 替代 Go 类型断言的动态访问入口。
    fn as_any(&self) -> &dyn std::any::Any;
    fn as_any_mut(&mut self) -> &mut dyn std::any::Any;
}

/// 对象池借还语义保留为独立入口；实际池由后续 zeropool 模块接线。
pub fn GetExpressionSlices(size: usize) -> Vec<ExprBox> {
    Vec::with_capacity(size.max(4))
}
/// 清空后归还表达式切片到对象池。
pub fn PutExpressionSlices(mut exprs: Vec<ExprBox>) {
    exprs.clear();
    expressionSlices.Put(exprs);
}

/// 合取范式（CNF）条件列表：多项 AND 连接的过滤谓词。
pub struct CNFExprs(pub Vec<ExprBox>);

impl CNFExprs {
    pub fn Clone(&self) -> Self {
        Self(self.0.iter().map(|e| e.CloneExpr()).collect())
    }
    // / 浅拷贝在 Go 中共享接口对象；这里以 CloneExpr 保持可拥有返回值，接线时可换 Arc。
    pub fn Shallow(&self) -> Self {
        Self(self.0.iter().map(|e| e.CloneExpr()).collect())
    }
}

fn isColumnInOperand(column: &Column) -> bool {
    column.InOperand
}

/// 是否为来自 IN 子查询展开的等值条件（右操作数列带 InOperand）。
pub fn IsEQCondFromIn(expr: &dyn Expression) -> bool {
    let Some(sf) = expr.as_any().downcast_ref::<ScalarFunction>() else {
        return false;
    };
    sf.FuncName.L == ast::EQ
        && !crate::core_support::ExtractColumnsMapFromExpressions(isColumnInOperand, sf.GetArgs())
            .is_empty()
}

/// 常量非 NULL，或列类型带 NOT NULL 标志时视为确定非空。
pub fn ExprNotNull(ctx: &dyn EvalContext, expr: &dyn Expression) -> bool {
    if let Some(c) = expr.as_any().downcast_ref::<Constant>() {
        return !c.Value.IsNull();
    }
    mysql::HasNotNullFlag(expr.GetType(ctx).GetFlag())
}

/// 逐行计算 CNF；来自 IN 子查询的等值条件遇到 NULL 时延迟裁决，保留 Go 的三值逻辑。
pub fn EvalBool(
    ctx: &dyn EvalContext,
    exprs: &CNFExprs,
    row: chunk::Row,
) -> Result<(bool, bool), errors::Error> {
    let mut has_null = false;
    for expr in &exprs.0 {
        let data = expr.Eval(ctx, row.clone())?;
        if data.IsNull() {
            if !IsEQCondFromIn(expr.as_ref()) {
                return Ok((false, false));
            }
            has_null = true;
            continue;
        }
        if data.ToBool(typeCtx(ctx))? == 0 {
            return Ok((false, false));
        }
    }
    if has_null {
        Ok((false, true))
    } else {
        Ok((true, false))
    }
}

const DEFAULT_CHUNK_SIZE: usize = 1024;
/// 从对象池借 selection 切片，容量至少为 DEFAULT_CHUNK_SIZE。
fn allocSelSlice(n: usize) -> Vec<usize> {
    Vec::with_capacity(n.max(DEFAULT_CHUNK_SIZE))
}
fn deallocateSelSlice(sel: Vec<usize>) {
    if sel.capacity() <= DEFAULT_CHUNK_SIZE {
        selPool.Put(sel);
    }
}
fn allocZeroSlice(n: usize) -> Vec<i8> {
    vec![0; n.max(DEFAULT_CHUNK_SIZE)]
}
fn deallocateZeroSlice(values: Vec<i8>) {
    if values.capacity() <= DEFAULT_CHUNK_SIZE {
        zeroPool.Put(values);
    }
}

/// 向量化 CNF 过滤。函数退出前恢复 Chunk 原 selection，并把临时列和切片归还对象池。
pub fn VecEvalBool(
    ctx: &dyn EvalContext,
    vec_enabled: bool,
    exprs: &CNFExprs,
    input: &mut chunk::Chunk,
    mut selected: Vec<bool>,
    mut nulls: Vec<bool>,
) -> Result<(Vec<bool>, Vec<bool>), errors::Error> {
    let original_sel = input.Sel().map(|v| v.to_vec());
    input.SetSel(None);
    let n = input.NumRows();
    selected.clear();
    selected.resize(n, false);
    nulls.clear();
    nulls.resize(n, false);
    let mut sel: Vec<usize> = (0..n).collect();
    input.SetSel(Some(sel.clone()));
    let mut is_zero = allocZeroSlice(n);

    let result = (|| {
        for expr in &exprs.0 {
            let tp = expr.GetType(ctx);
            let mut eval_type = tp.EvalType();
            if CanImplicitEvalReal(expr.as_ref()) {
                eval_type = types::ETReal;
            }
            let mut buf = globalColumnAllocator.get()?;
            if CanImplicitEvalReal(expr.as_ref()) {
                implicitEvalReal(ctx, vec_enabled, expr.as_ref(), input, &mut buf)?;
            } else {
                EvalExpr(ctx, vec_enabled, expr.as_ref(), eval_type, input, &mut buf)?;
            }
            toBool(typeCtx(ctx), tp, eval_type, &buf, &sel, &mut is_zero)?;

            let eq_from_in = IsEQCondFromIn(expr.as_ref());
            let mut next = Vec::with_capacity(sel.len());
            for (position, row_index) in sel.iter().copied().enumerate() {
                match is_zero[position] {
                    -1 if eval_type == types::ETInt && eq_from_in => {
                        nulls[row_index] = true;
                        next.push(row_index);
                    }
                    -1 | 0 => nulls[row_index] = false,
                    _ => next.push(row_index),
                }
            }
            sel = next;
            input.SetSel(Some(sel.clone()));
            globalColumnAllocator.put(buf);
        }
        for &row_index in &sel {
            if !nulls[row_index] {
                selected[row_index] = true;
            }
        }
        Ok((selected, nulls))
    })();

    input.SetSel(original_sel);
    deallocateZeroSlice(is_zero);
    deallocateSelSlice(sel);
    result
}

/// 把各物理求值类型归一为 -1(NULL)、0(false)、1(true)。字符串分支保留 ENUM/SET/BIT 特判。
fn toBool(
    tc: types::Context,
    tp: &types::FieldType,
    eval_type: types::EvalType,
    buf: &chunk::Column,
    sel: &[usize],
    out: &mut [i8],
) -> Result<(), errors::Error> {
    for (position, _) in sel.iter().enumerate() {
        if buf.IsNull(position) {
            out[position] = -1;
            continue;
        }
        let zero = match eval_type {
            types::ETInt => buf.Int64s()[position] == 0,
            types::ETReal => buf.Float64s()[position] == 0.0,
            types::ETDuration => buf.GoDurations()[position] == 0,
            types::ETDatetime | types::ETTimestamp => buf.Times()[position].IsZero(),
            types::ETDecimal => buf.Decimals()[position].IsZero(),
            types::ETJson => buf.GetJSON(position).IsZero(),
            types::ETVectorFloat32 => buf.GetVectorFloat32(position).IsZeroValue(),
            types::ETString => {
                let value = buf.GetString(position);
                if tp.Hybrid() {
                    match tp.GetType() {
                        mysql::TypeSet | mysql::TypeEnum => {
                            // 空字符串可能是合法枚举项；存在于元素表时其索引从 1 开始，因而为真。
                            if value.is_empty() {
                                !tp.GetElems().iter().any(|e| e == &value)
                            } else {
                                false
                            }
                        }
                        mysql::TypeBit => {
                            types::BinaryLiteral(buf.GetBytes(position).to_vec())
                                .ToInt(tc.clone())?
                                == 0
                        }
                        _ => true,
                    }
                } else {
                    types::StrToFloat(tc.clone(), &value, false)? == 0.0
                }
            }
            _ => {
                return Err(errors::Errorf(format!(
                    "unsupported type {:?} during evaluation",
                    eval_type
                )));
            }
        };
        out[position] = if zero { 0 } else { 1 };
    }
    Ok(())
}

/// 隐式按 Real 求值：优先向量化，否则逐行 EvalReal。
fn implicitEvalReal(
    ctx: &dyn EvalContext,
    vec_enabled: bool,
    expr: &dyn Expression,
    input: &chunk::Chunk,
    result: &mut chunk::Column,
) -> Result<(), errors::Error> {
    if expr.Vectorized() && vec_enabled {
        return expr.VecEvalReal(ctx, input, result);
    }
    result.ResizeFloat64(0, false);
    for index in 0..input.NumRows() {
        let (value, is_null) = expr.EvalReal(ctx, input.GetRow(index))?;
        if is_null {
            result.AppendNull();
        } else {
            result.AppendFloat64(value);
        }
    }
    Ok(())
}

/// 按 EvalType 和向量化开关分派求值；关闭向量化时逐行写入 Column 并逐次传播错误。
pub fn EvalExpr(
    ctx: &dyn EvalContext,
    vec_enabled: bool,
    expr: &dyn Expression,
    eval_type: types::EvalType,
    input: &chunk::Chunk,
    result: &mut chunk::Column,
) -> Result<(), errors::Error> {
    if expr.Vectorized() && vec_enabled {
        return match eval_type {
            types::ETInt => expr.VecEvalInt(ctx, input, result),
            types::ETReal => expr.VecEvalReal(ctx, input, result),
            types::ETDuration => expr.VecEvalDuration(ctx, input, result),
            types::ETDatetime | types::ETTimestamp => expr.VecEvalTime(ctx, input, result),
            types::ETString => expr.VecEvalString(ctx, input, result),
            types::ETJson => expr.VecEvalJSON(ctx, input, result),
            types::ETVectorFloat32 => expr.VecEvalVectorFloat32(ctx, input, result),
            types::ETDecimal => expr.VecEvalDecimal(ctx, input, result),
            _ => Err(errors::Errorf(format!(
                "unsupported type {:?} during evaluation",
                eval_type
            ))),
        };
    }
    match eval_type {
        types::ETInt => {
            result.ResizeInt64(0, false);
            for i in 0..input.NumRows() {
                let (v, n) = expr.EvalInt(ctx, input.GetRow(i))?;
                if n {
                    result.AppendNull()
                } else {
                    result.AppendInt64(v)
                }
            }
        }
        types::ETReal => {
            result.ResizeFloat64(0, false);
            for i in 0..input.NumRows() {
                let (v, n) = expr.EvalReal(ctx, input.GetRow(i))?;
                if n {
                    result.AppendNull()
                } else {
                    result.AppendFloat64(v)
                }
            }
        }
        types::ETDuration => {
            result.ResizeGoDuration(0, false);
            for i in 0..input.NumRows() {
                let (v, n) = expr.EvalDuration(ctx, input.GetRow(i))?;
                if n {
                    result.AppendNull()
                } else {
                    result.AppendDuration(v)
                }
            }
        }
        types::ETDatetime | types::ETTimestamp => {
            result.ResizeTime(0, false);
            for i in 0..input.NumRows() {
                let (v, n) = expr.EvalTime(ctx, input.GetRow(i))?;
                if n {
                    result.AppendNull()
                } else {
                    result.AppendTime(v)
                }
            }
        }
        types::ETString => {
            result.ReserveString(input.NumRows());
            for i in 0..input.NumRows() {
                let (v, n) = expr.EvalString(ctx, input.GetRow(i))?;
                if n {
                    result.AppendNull()
                } else {
                    result.AppendString(&v)
                }
            }
        }
        types::ETJson => {
            result.ReserveJSON(input.NumRows());
            for i in 0..input.NumRows() {
                let (v, n) = expr.EvalJSON(ctx, input.GetRow(i))?;
                if n {
                    result.AppendNull()
                } else {
                    result.AppendJSON(v)
                }
            }
        }
        types::ETVectorFloat32 => {
            result.ReserveVectorFloat32(input.NumRows());
            for i in 0..input.NumRows() {
                let (v, n) = expr.EvalVectorFloat32(ctx, input.GetRow(i))?;
                if n {
                    result.AppendNull()
                } else {
                    result.AppendVectorFloat32(v)
                }
            }
        }
        types::ETDecimal => {
            result.ResizeDecimal(0, false);
            for i in 0..input.NumRows() {
                let (v, n) = expr.EvalDecimal(ctx, input.GetRow(i))?;
                if n {
                    result.AppendNull()
                } else {
                    result.AppendMyDecimal(&v)
                }
            }
        }
        _ => {
            return Err(errors::Errorf(format!(
                "unsupported type {:?} during evaluation",
                eval_type
            )));
        }
    }
    Ok(())
}

/// 用二分递归构造平衡 AND/OR 树，减少 protobuf 编解码深度。
fn composeConditionWithBinaryOp(
    ctx: &dyn BuildContext,
    conditions: &[ExprBox],
    name: &str,
) -> Option<ExprBox> {
    match conditions.len() {
        0 => None,
        1 => Some(conditions[0].CloneExpr()),
        n => NewFunctionInternal(
            ctx,
            name,
            *types::NewFieldType(mysql::TypeTiny),
            vec![
                composeConditionWithBinaryOp(ctx, &conditions[..n / 2], name).unwrap(),
                composeConditionWithBinaryOp(ctx, &conditions[n / 2..], name).unwrap(),
            ],
        ),
    }
}

/// 用 LogicAnd 把条件列表合成为平衡 CNF 树。
pub fn ComposeCNFCondition(ctx: &dyn BuildContext, conditions: &[ExprBox]) -> Option<ExprBox> {
    composeConditionWithBinaryOp(ctx, conditions, ast::LogicAnd)
}
/// 用 LogicOr 把条件列表合成为平衡 DNF 树。
pub fn ComposeDNFCondition(ctx: &dyn BuildContext, conditions: &[ExprBox]) -> Option<ExprBox> {
    composeConditionWithBinaryOp(ctx, conditions, ast::LogicOr)
}

/// 递归展开同名二元逻辑运算的叶子项。
fn extractBinaryOpItems(condition: &ScalarFunction, name: &str) -> Vec<ExprBox> {
    let mut out = Vec::new();
    for arg in condition.GetArgs() {
        if let Some(sf) = arg.as_any().downcast_ref::<ScalarFunction>() {
            if sf.FuncName.L == name {
                out.extend(extractBinaryOpItems(sf, name));
                continue;
            }
        }
        out.push(arg.CloneExpr());
    }
    out
}
/// 把嵌套 OR 树展平为叶子表达式列表。
pub fn FlattenDNFConditions(condition: &ScalarFunction) -> Vec<ExprBox> {
    extractBinaryOpItems(condition, ast::LogicOr)
}
/// 把嵌套 AND 树展平为叶子表达式列表。
pub fn FlattenCNFConditions(condition: &ScalarFunction) -> Vec<ExprBox> {
    extractBinaryOpItems(condition, ast::LogicAnd)
}

/// UPDATE 中的一项列赋值；LazyErr 延迟到确认重复键分支后再返回。
pub struct Assignment {
    pub Col: Column,
    pub ColName: ast::CIStr,
    pub Expr: ExprBox,
    pub LazyErr: Option<errors::Error>,
}
impl Assignment {
    pub fn Clone(&self) -> Self {
        Self {
            Col: self.Col.Clone(),
            ColName: self.ColName.clone(),
            Expr: self.Expr.CloneExpr(),
            LazyErr: self.LazyErr.clone(),
        }
    }
    pub fn MemoryUsage(&self) -> i64 {
        size::SizeOfPointer
            + (self.ColName.O.len() + self.ColName.L.len()) as i64
            + size::SizeOfInterface * 2
            + self.Expr.MemoryUsage()
    }
}

/// SET 语句变量赋值，保留作用域标志和可选扩展常量。
pub struct VarAssignment {
    pub Name: String,
    pub Expr: ExprBox,
    pub IsDefault: bool,
    pub IsGlobal: bool,
    pub IsInstance: bool,
    pub IsSystem: bool,
    pub ExtendValue: Option<Constant>,
}

/// 按 LogicAnd / LogicOr 递归拆分范式项。
fn splitNormalFormItems(expr: &dyn Expression, name: &str) -> Vec<ExprBox> {
    if let Some(sf) = expr.as_any().downcast_ref::<ScalarFunction>() {
        if sf.FuncName.L == name {
            return sf
                .GetArgs()
                .iter()
                .flat_map(|a| splitNormalFormItems(a.as_ref(), name))
                .collect();
        }
    }
    vec![expr.CloneExpr()]
}
/// 拆分 AND 连接的 CNF 项。
pub fn SplitCNFItems(expr: &dyn Expression) -> Vec<ExprBox> {
    splitNormalFormItems(expr, ast::LogicAnd)
}
/// 拆分 OR 连接的 DNF 项。
pub fn SplitDNFItems(expr: &dyn Expression) -> Vec<ExprBox> {
    splitNormalFormItems(expr, ast::LogicOr)
}

/// 把 schema 中的列替换为 NULL 后折叠表达式；必要时标记计划缓存不可安全复用。
pub fn EvaluateExprWithNull(
    ctx: &mut dyn BuildContext,
    schema: &Schema,
    expr: ExprBox,
    skip_cache_check: bool,
) -> Result<ExprBox, errors::Error> {
    if skip_cache_check && crate::core_support::MaybeOverOptimized4PlanCache(ctx, expr.as_ref()) {
        ctx.SetSkipPlanCache(&format!(
            "{} affects null check",
            expr.StringWithCtx(Some(ctx.GetEvalCtx()), errors::RedactLogDisable)
        ));
    }
    if ctx.IsInNullRejectCheck() {
        return Ok(evaluateExprWithNullInNullRejectCheck(ctx, schema, expr)?.0);
    }
    evaluateExprWithNull(ctx, schema, expr, skip_cache_check)
}

fn evaluateExprWithNull(
    ctx: &mut dyn BuildContext,
    schema: &Schema,
    expr: ExprBox,
    skip_cache: bool,
) -> Result<ExprBox, errors::Error> {
    if let Some(sf) = expr.as_any().downcast_ref::<ScalarFunction>() {
        let args = sf
            .GetArgs()
            .iter()
            .map(|a| evaluateExprWithNull(ctx, schema, a.CloneExpr(), skip_cache))
            .collect::<Result<Vec<_>, _>>()?;
        return NewFunction(ctx, &sf.FuncName.L, sf.RetType.clone().unwrap(), args);
    }
    if let Some(column) = expr.as_any().downcast_ref::<Column>() {
        if schema.Contains(column) {
            return Ok(Box::new(Constant::null(mysql::TypeNull)));
        }
    }
    if let Some(constant) = expr.as_any().downcast_ref::<Constant>() {
        if constant.DeferredExpr.is_some() {
            return Ok(FoldConstant(ctx, constant.CloneExpr()));
        }
    }
    Ok(expr)
}

/// 空拒绝检查专用递归还会跟踪 NULL 是否源自被替换列，避免 AND/OR 被过早折叠。
fn evaluateExprWithNullInNullRejectCheck(
    ctx: &mut dyn BuildContext,
    schema: &Schema,
    expr: ExprBox,
) -> Result<(ExprBox, bool), errors::Error> {
    if let Some(sf) = expr.as_any().downcast_ref::<ScalarFunction>() {
        let func_name = sf.FuncName.L.clone();
        let mut args = Vec::new();
        let mut null_from_sets = Vec::new();
        for arg in sf.GetArgs() {
            let (value, from_set) =
                evaluateExprWithNullInNullRejectCheck(ctx, schema, arg.CloneExpr())?;
            args.push(value);
            null_from_sets.push(from_set);
        }
        let all_nulls_from_set = args.iter().zip(&null_from_sets).all(|(a, from)| {
            a.as_any()
                .downcast_ref::<Constant>()
                .map_or(true, |c| !c.Value.IsNull() || *from)
        });
        if func_name == ast::LogicAnd || func_name == ast::LogicOr {
            let has_non_constant = args
                .iter()
                .any(|a| a.as_any().downcast_ref::<Constant>().is_none());
            if has_non_constant {
                for (arg, from_set) in args.iter_mut().zip(&null_from_sets) {
                    let replace = arg
                        .as_any()
                        .downcast_ref::<Constant>()
                        .is_some_and(|c| c.Value.IsNull())
                        && *from_set;
                    if replace {
                        *arg = if func_name == ast::LogicAnd {
                            Box::new(NewOne())
                        } else {
                            Box::new(NewZero())
                        };
                        break;
                    }
                }
            }
        }
        let folded = NewFunction(ctx, &func_name, sf.RetType.clone().unwrap(), args)?;
        let derived = folded
            .as_any()
            .downcast_ref::<Constant>()
            .is_some_and(|c| c.Value.IsNull())
            && all_nulls_from_set;
        return Ok((folded, derived));
    }
    if let Some(column) = expr.as_any().downcast_ref::<Column>() {
        if schema.Contains(column) {
            return Ok((Box::new(Constant::null(mysql::TypeNull)), true));
        }
    }
    if let Some(constant) = expr.as_any().downcast_ref::<Constant>() {
        if constant.DeferredExpr.is_some() {
            return Ok((FoldConstant(ctx, constant.CloneExpr()), false));
        }
    }
    Ok((expr, false))
}

/// 从表元数据建立 Schema、字段名和唯一键集合。
pub fn TableInfo2SchemaAndNames(
    ctx: &dyn BuildContext,
    db: ast::CIStr,
    table: &model::TableInfo,
) -> Result<(Schema, types::NameSlice), errors::Error> {
    let (columns, names) =
        ColumnInfos2ColumnsAndNames(ctx, db, table.Name.clone(), &table.Columns, table)?;
    let mut keys = Vec::new();
    for index in &table.Indices {
        if !index.Unique || index.State != model::StatePublic {
            continue;
        }
        let mut key = Vec::new();
        let mut valid = true;
        for index_column in &index.Columns {
            if let Some((position, _)) = table.Columns.iter().enumerate().find(|(_, c)| {
                c.Name.L == index_column.Name.L && mysql::HasNotNullFlag(c.GetFlag())
            }) {
                key.push(columns[position].Clone());
            } else {
                valid = false;
                break;
            }
        }
        if valid {
            keys.push(key);
        }
    }
    if table.PKIsHandle {
        if let Some((i, _)) = table
            .Columns
            .iter()
            .enumerate()
            .find(|(_, c)| mysql::HasPriKeyFlag(c.GetFlag()))
        {
            keys.push(vec![columns[i].Clone()]);
        }
    }
    let mut schema = NewSchema(columns.clone());
    schema.SetKeys(keys);
    Ok((schema, names))
}

/// 使用全局新校对开关调用 `ColumnInfos2ColumnsAndNamesWithCollate`。
pub fn ColumnInfos2ColumnsAndNames(
    ctx: &dyn BuildContext,
    db: ast::CIStr,
    table_name: ast::CIStr,
    infos: &[model::ColumnInfo],
    table: &model::TableInfo,
) -> Result<(Vec<Column>, types::NameSlice), errors::Error> {
    ColumnInfos2ColumnsAndNamesWithCollate(
        ctx,
        db,
        table_name,
        infos,
        table,
        collate::NewCollationEnabled(),
    )
}

/// 虚拟生成列的构建会重复触发截断检查；与 Go 一致，将这类冗余告警设为忽略。
pub(crate) fn generatedColumnBuildContext(
    ctx: &dyn BuildContext,
) -> exprctx::CtxWithTruncateResult<'_> {
    exprctx::CtxWithHandleTruncateErrLevel(ctx, errctx::Level::LevelIgnore)
}

/// 创建列后解析虚拟生成列；首次遇到虚拟列时把截断错误设为 Ignore，避免冗余告警。
pub fn ColumnInfos2ColumnsAndNamesWithCollate(
    ctx: &dyn BuildContext,
    db: ast::CIStr,
    table_name: ast::CIStr,
    infos: &[model::ColumnInfo],
    table: &model::TableInfo,
    use_new_collate: bool,
) -> Result<(Vec<Column>, types::NameSlice), errors::Error> {
    let mut names = Vec::with_capacity(infos.len());
    let mut columns = Vec::with_capacity(infos.len());
    for info in infos {
        let name = std::sync::Arc::new(types::FieldName {
            DBName: db.clone(),
            TblName: table_name.clone(),
            ColName: info.Name.clone(),
            OrigTblName: table_name.clone(),
            OrigColName: info.Name.clone(),
            ..Default::default()
        });
        let original_name = name.String();
        names.push(Some(name));
        columns.push(Column {
            RetType: Some(info.FieldType.Clone()),
            ID: info.ID,
            UniqueID: ctx.AllocPlanColumnID(),
            Index: info.Offset,
            OrigName: original_name,
            IsHidden: info.Hidden,
            ..Default::default()
        });
    }
    let mock_schema = NewSchema(columns.clone());
    let generated_ctx = infos
        .iter()
        .any(model::ColumnInfo::IsVirtualGenerated)
        .then(|| generatedColumnBuildContext(ctx));
    let build_ctx: &dyn BuildContext = generated_ctx
        .as_ref()
        .map_or(ctx, |context| context as &dyn BuildContext);
    for (index, info) in infos.iter().enumerate() {
        if !info.IsVirtualGenerated() {
            continue;
        }
        let parsed = generatedexpr::ParseExpression(&info.GeneratedExprString)
            .map_err(|error| errors::New(error.to_string()))?;
        let resolved = generatedexpr::SimpleResolveName(parsed, table)
            .map_err(|error| errors::New(error.to_string()))?;
        let built = BuildSimpleExpr(
            build_ctx,
            &resolved,
            vec![
                WithInputSchemaAndNames(&mock_schema, types::NameSlice(names.clone()), Some(table)),
                WithAllowCastArray(true),
                WithUseNewCollate(use_new_collate),
            ],
        )?;
        columns[index].VirtualExpr = Some(built.CloneExpr());
        columns[index].VirtualExpr = Some(
            columns[index]
                .VirtualExpr
                .take()
                .unwrap()
                .ResolveIndices(&mock_schema)?,
        );
    }
    Ok((columns, types::NameSlice(names)))
}

/// 构造 INSERT ... VALUES() 标量函数，offset 为列在 VALUES 列表中的下标。
pub fn NewValuesFunc(
    ctx: &dyn BuildContext,
    offset: i32,
    ret_type: types::FieldType,
) -> ScalarFunction {
    let class = valuesFunctionClass::new(ast::Values, offset, ret_type.clone());
    let function = class.getFunction(ctx, vec![]).unwrap_or_else(|err| {
        terror::Log(&err);
        panic!("VALUES builtin factory must be registered: {err}")
    });
    ScalarFunction {
        FuncName: ast::NewCIStr(ast::Values),
        RetType: Some(ret_type),
        Function: function,
        hashcode: Vec::new(),
        canonicalhashcode: Vec::new(),
    }
}

/// 常量是否为二进制字面量（KindBinaryLiteral）。
pub fn IsBinaryLiteral(expr: &dyn Expression) -> bool {
    expr.as_any()
        .downcast_ref::<Constant>()
        .is_some_and(|c| c.Value.Kind() == types::KindBinaryLiteral)
}

/// 必要时用 IS TRUE 包装表达式；整型逻辑运算可直接返回，keep_null 决定 NULL 是否保留。
fn wrapWithIsTrue(
    ctx: &dyn BuildContext,
    keep_null: bool,
    arg: ExprBox,
    wrap_for_int: bool,
) -> Result<ExprBox, errors::Error> {
    if arg.GetType(ctx.GetEvalCtx()).EvalType() == types::ETInt {
        if !wrap_for_int {
            return Ok(arg);
        }
        if let Some(sf) = arg.as_any().downcast_ref::<ScalarFunction>() {
            if crate::core_support::logicalOps.contains_key(sf.FuncName.L.as_str()) {
                return Ok(arg);
            }
        }
    }
    let name = if keep_null {
        ast::IsTruthWithNull
    } else {
        ast::IsTruthWithoutNull
    };
    let class = isTrueOrFalseFunctionClass::new(name, opcode::IsTruth, keep_null);
    let function = class.getFunction(ctx, vec![arg])?;
    Ok(FoldConstant(
        ctx,
        Box::new(ScalarFunction {
            FuncName: ast::NewCIStr(name),
            RetType: Some(function.getRetTp().clone()),
            Function: function,
            hashcode: Vec::new(),
            canonicalhashcode: Vec::new(),
        }),
    ))
}

/// 把目标 double 的长度/小数位传播到首个参数，并在必要时克隆列、关联列或常量避免共享类型被修改。
pub fn PropagateType(ctx: &dyn EvalContext, eval_type: types::EvalType, args: &mut [ExprBox]) {
    if eval_type != types::ETReal || args.is_empty() {
        return;
    }
    let (old_flen, old_decimal) = (
        args[0].GetType(ctx).GetFlen(),
        args[0].GetType(ctx).GetDecimal(),
    );
    let (mut new_flen, mut new_decimal) = setDataTypeDouble(old_decimal);
    if new_flen < new_decimal {
        new_flen = old_flen - old_decimal + new_decimal;
    }
    if old_flen == new_flen && old_decimal == new_decimal {
        return;
    }
    args[0] = args[0].CloneExpr();
    if args[0].GetType(ctx).GetType() == mysql::TypeNewDecimal {
        new_decimal = new_decimal.min(mysql::MaxDecimalScale as isize);
        if old_flen - old_decimal > new_flen - new_decimal {
            if new_decimal > old_decimal {
                let increment =
                    (new_decimal - old_decimal).min(mysql::MaxDecimalWidth as isize - old_flen);
                new_flen = old_flen + increment;
                new_decimal = old_decimal + increment;
            } else {
                new_flen = old_flen;
                new_decimal = old_decimal;
            }
        }
    }
    args[0].GetTypeMut().SetFlenUnderLimit(new_flen);
    args[0].GetTypeMut().SetDecimalUnderLimit(new_decimal);
}

/// 按源小数位估算 double 显示长度；decimal=-1 表示未指定精度。
fn setDataTypeDouble(source_decimal: isize) -> (isize, isize) {
    const DBL_DIG: isize = 15;
    let decimal = -1;
    let length = if source_decimal != -1 {
        DBL_DIG + 2 + decimal
    } else {
        DBL_DIG + 8
    };
    (length, decimal)
}

/// 测试辅助：按 Datum Kind 为常见原生值推断 FieldType，未知类型保留 None。
pub fn Args2Expressions4Test(args: Vec<types::AnyValue>) -> Vec<Option<ExprBox>> {
    args.into_iter()
        .map(|value| {
            let datum = types::NewDatum(value.as_ref());
            let field_type = match datum.Kind() {
                types::KindNull => types::NewFieldType(mysql::TypeNull),
                types::KindInt64 => types::NewFieldType(mysql::TypeLong),
                types::KindUint64 => {
                    let mut t = types::NewFieldType(mysql::TypeLong);
                    t.AddFlag(mysql::UnsignedFlag);
                    t
                }
                types::KindFloat64 => types::NewFieldType(mysql::TypeDouble),
                types::KindString => types::NewFieldType(mysql::TypeVarString),
                types::KindMysqlTime => types::NewFieldType(mysql::TypeTimestamp),
                types::KindBytes => types::NewFieldType(mysql::TypeBlob),
                _ => return None,
            };
            Some(Box::new(Constant::with_type(datum, *field_type)) as ExprBox)
        })
        .collect()
}

/// 用求值上下文把表达式列表格式化为 `[e1 e2 ...]` 调试字符串。
pub fn StringifyExpressionsWithCtx(ctx: &dyn EvalContext, exprs: &[ExprBox]) -> String {
    let body = exprs
        .iter()
        .map(|e| e.StringWithCtx(Some(ctx), errors::RedactLogDisable))
        .collect::<Vec<_>>()
        .join(" ");
    format!("[{}]", body)
}
