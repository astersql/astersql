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

// Chunk 级表达式求值与向量化过滤。
//
// 对应 Go `chunk_executor.go`：判断表达式组可否向量化执行，按列/按行把表达式结果
// 写入输出 Chunk，并对过滤器做向量化或逐行求值。向量化指一次对整列（Chunk）求值。

use crate::*;
use chunk::Iterator as _;

/// Vectorizable 判断整组表达式能否采用向量化执行。
pub fn Vectorizable(exprs: &[Box<dyn Expression>]) -> bool {
    // GetVar/SetVar 具有跨行状态，不能改变原始逐行求值次序。
    if exprs.iter().any(|expr| HasGetSetVarFunc(expr.as_ref())) {
        return false;
    }
    checkSequenceFunction(exprs)
}

/// checkSequenceFunction 检查 NEXTVAL/LASTVAL/SETVAL 的顺序依赖。
fn checkSequenceFunction(exprs: &[Box<dyn Expression>]) -> bool {
    let (mut nextval, mut lastval, mut setval) = (0, 0, 0);
    for expr in exprs {
        let Some(scalar) = expr.as_any().downcast_ref::<ScalarFunction>() else {
            continue;
        };
        match scalar.FuncName.L.as_str() {
            ast::NextVal => nextval += 1,
            ast::LastVal => lastval += 1,
            ast::SetVal => setval += 1,
            _ => {}
        }
    }
    // NEXTVAL 与其他序列函数并存，或出现多个 NEXTVAL 时必须逐行执行。
    !((nextval > 0 && (lastval > 0 || setval > 0)) || nextval > 1)
}

/// HasGetSetVarFunc 递归检查表达式树中是否含 SetVar/GetVar。
pub fn HasGetSetVarFunc(expr: &dyn Expression) -> bool {
    let Some(scalar) = expr.as_any().downcast_ref::<ScalarFunction>() else {
        return false;
    };
    if scalar.FuncName.L == ast::SetVar || scalar.FuncName.L == ast::GetVar {
        return true;
    }
    scalar
        .GetArgs()
        .iter()
        .any(|arg| HasGetSetVarFunc(arg.as_ref()))
}

/// HasAssignSetVarFunc 检查 SetVar 是否把另一个标量函数结果赋给变量。
pub fn HasAssignSetVarFunc(expr: &dyn Expression) -> bool {
    let Some(scalar) = expr.as_any().downcast_ref::<ScalarFunction>() else {
        return false;
    };
    if scalar.FuncName.L == ast::SetVar
        && scalar
            .GetArgs()
            .iter()
            .any(|arg| arg.as_any().is::<ScalarFunction>())
    {
        return true;
    }
    scalar
        .GetArgs()
        .iter()
        .any(|arg| HasAssignSetVarFunc(arg.as_ref()))
}

/// VectorizedExecute 依次计算表达式列，并把结果追加到 output 的对应列。
pub fn VectorizedExecute(
    ctx: &dyn EvalContext,
    exprs: &[Box<dyn Expression>],
    iterator: &mut chunk::Iterator4Chunk,
    output: &mut chunk::Chunk,
) -> Result<(), Error> {
    for (colID, expr) in exprs.iter().enumerate() {
        evalOneColumn(ctx, expr.as_ref(), iterator, output, colID)?;
    }
    Ok(())
}

/// evalOneVec 使用表达式的 VecEval* 接口计算一列，并处理物理存储类型的二次转换。
#[allow(dead_code)] // 由后续 evaluator 接线调用；任务 harness 仍验证其依赖闭包。
pub(crate) fn evalOneVec(
    ctx: &dyn EvalContext,
    expr: &dyn Expression,
    input: &chunk::Chunk,
    output: &mut chunk::Chunk,
    colIdx: usize,
) -> Result<(), Error> {
    let ft = expr.GetType(ctx);
    let mut result = chunk::Column::default();
    match ft.EvalType() {
        types::ETInt => {
            expr.VecEvalInt(ctx, input, &mut result)?;
            if ft.GetType() == mysql::TypeBit {
                let i64s = result.Int64s();
                let mut buf = chunk::NewColumn(ft, input.NumRows());
                buf.ReserveBytes(input.NumRows());
                let byteSize = (ft.GetFlen() + 7) >> 3;
                for (i, value) in i64s.iter().enumerate() {
                    if result.IsNull(i) {
                        buf.AppendNull();
                    } else {
                        let literal = types::NewBinaryLiteralFromUint(*value as u64, byteSize);
                        buf.AppendBytes(literal.as_ref());
                    }
                }
                result = *buf;
            }
            // 有符号/无符号整数在 Go chunk 中共享位模式，因此无需复制。
        }
        types::ETReal => {
            expr.VecEvalReal(ctx, input, &mut result)?;
            if ft.GetType() == mysql::TypeFloat {
                let f64s = result.Float64s();
                let n = input.NumRows();
                let mut buf = chunk::NewColumn(ft, n);
                for (i, value) in f64s.iter().enumerate() {
                    if result.IsNull(i) {
                        buf.AppendNull();
                    } else {
                        buf.AppendFloat32(*value as f32);
                    }
                }
                result = *buf;
            }
        }
        types::ETDecimal => expr.VecEvalDecimal(ctx, input, &mut result)?,
        types::ETDatetime | types::ETTimestamp => expr.VecEvalTime(ctx, input, &mut result)?,
        types::ETDuration => expr.VecEvalDuration(ctx, input, &mut result)?,
        types::ETJson => expr.VecEvalJSON(ctx, input, &mut result)?,
        types::ETVectorFloat32 => expr.VecEvalVectorFloat32(ctx, input, &mut result)?,
        types::ETString => {
            expr.VecEvalString(ctx, input, &mut result)?;
            if ft.GetType() == mysql::TypeEnum {
                let n = input.NumRows();
                let mut buf = chunk::NewColumn(ft, n);
                buf.ReserveEnum(n);
                for i in 0..n {
                    if result.IsNull(i) {
                        buf.AppendNull();
                        continue;
                    }
                    // 原实现只记录非法枚举名，仍把解析结果追加到输出。
                    let parsed =
                        types::ParseEnumName(ft.GetElems(), &result.GetString(i), ft.GetCollate());
                    // 与 Go 一致：解析错误不终止整列求值，追加零值枚举。
                    buf.AppendEnum(parsed.unwrap_or_default());
                }
                result = *buf;
            } else if ft.GetType() == mysql::TypeSet {
                let n = input.NumRows();
                let mut buf = chunk::NewColumn(ft, n);
                buf.ReserveSet(n);
                for i in 0..n {
                    if result.IsNull(i) {
                        buf.AppendNull();
                        continue;
                    }
                    let parsed =
                        types::ParseSetName(ft.GetElems(), &result.GetString(i), ft.GetCollate());
                    // 与 Go 一致：解析错误不终止整列求值，追加零值集合。
                    buf.AppendSet(parsed.unwrap_or_default());
                }
                result = *buf;
            }
        }
        other => {
            return Err(errors::New(format!(
                "unsupported type {other:?} during evaluation"
            )));
        }
    }
    output.SetCol(colIdx, result);
    Ok(())
}

/// evalOneColumn 按字段求值类型逐行执行一整个输入迭代器。
fn evalOneColumn(
    ctx: &dyn EvalContext,
    expr: &dyn Expression,
    iterator: &mut chunk::Iterator4Chunk,
    output: &mut chunk::Chunk,
    colID: usize,
) -> Result<(), Error> {
    let fieldType = expr.GetType(ctx);
    let evalType = fieldType.EvalType();
    let mut row = iterator.Begin();
    while row != iterator.End() {
        evalOneCell(ctx, expr, row.clone(), output, colID)?;
        row = iterator.Next();
    }
    // evalOneCell 承担同一类型分派，避免八个循环体遗漏差异。
    if !matches!(
        evalType,
        types::ETInt
            | types::ETReal
            | types::ETDecimal
            | types::ETDatetime
            | types::ETTimestamp
            | types::ETDuration
            | types::ETJson
            | types::ETVectorFloat32
            | types::ETString
    ) {
        return Err(errors::New(format!(
            "unsupported type {evalType:?} during evaluation"
        )));
    }
    Ok(())
}

/// evalOneCell 将单行结果分派到与字段求值类型匹配的追加函数。
pub(crate) fn evalOneCell(
    ctx: &dyn EvalContext,
    expr: &dyn Expression,
    row: chunk::Row,
    output: &mut chunk::Chunk,
    colID: usize,
) -> Result<(), Error> {
    let fieldType = expr.GetType(ctx);
    match fieldType.EvalType() {
        types::ETInt => executeToInt(ctx, expr, fieldType, row, output, colID),
        types::ETReal => executeToReal(ctx, expr, fieldType, row, output, colID),
        types::ETDecimal => executeToDecimal(ctx, expr, fieldType, row, output, colID),
        types::ETDatetime | types::ETTimestamp => {
            executeToDatetime(ctx, expr, fieldType, row, output, colID)
        }
        types::ETDuration => executeToDuration(ctx, expr, fieldType, row, output, colID),
        types::ETJson => executeToJSON(ctx, expr, fieldType, row, output, colID),
        types::ETVectorFloat32 => executeToVectorFloat32(ctx, expr, fieldType, row, output, colID),
        types::ETString => executeToString(ctx, expr, fieldType, row, output, colID),
        other => Err(errors::New(format!(
            "unsupported type {other:?} during evaluation"
        ))),
    }
}

/// executeToInt 求值整数，并按 BIT、ENUM、UNSIGNED、SIGNED 顺序选择物理追加方式。
fn executeToInt(
    ctx: &dyn EvalContext,
    expr: &dyn Expression,
    fieldType: &types::FieldType,
    row: chunk::Row,
    output: &mut chunk::Chunk,
    colID: usize,
) -> Result<(), Error> {
    let (res, isNull) = expr.EvalInt(ctx, row)?;
    if isNull {
        output.AppendNull(colID);
        return Ok(());
    }
    if fieldType.GetType() == mysql::TypeBit {
        let byteSize = (fieldType.GetFlen() + 7) >> 3;
        let literal = types::NewBinaryLiteralFromUint(res as u64, byteSize);
        output.AppendBytes(colID, literal.as_ref());
    } else if fieldType.GetType() == mysql::TypeEnum {
        output.AppendEnum(
            colID,
            types::ParseEnumValue(fieldType.GetElems(), res as u64)?,
        );
    } else if mysql::HasUnsignedFlag(fieldType.GetFlag()) {
        output.AppendUint64(colID, res as u64);
    } else {
        output.AppendInt64(colID, res);
    }
    Ok(())
}

/// executeToReal 对 MySQL FLOAT 缩窄为 f32，其余实数保留 f64。
fn executeToReal(
    ctx: &dyn EvalContext,
    expr: &dyn Expression,
    fieldType: &types::FieldType,
    row: chunk::Row,
    output: &mut chunk::Chunk,
    colID: usize,
) -> Result<(), Error> {
    let (res, isNull) = expr.EvalReal(ctx, row)?;
    if isNull {
        output.AppendNull(colID);
    } else if fieldType.GetType() == mysql::TypeFloat {
        output.AppendFloat32(colID, res as f32);
    } else {
        output.AppendFloat64(colID, res);
    }
    Ok(())
}

/// executeToDecimal 求值十进制数并处理 SQL NULL。
fn executeToDecimal(
    ctx: &dyn EvalContext,
    expr: &dyn Expression,
    _fieldType: &types::FieldType,
    row: chunk::Row,
    output: &mut chunk::Chunk,
    colID: usize,
) -> Result<(), Error> {
    let (res, isNull) = expr.EvalDecimal(ctx, row)?;
    if isNull {
        output.AppendNull(colID);
    } else {
        output.AppendMyDecimal(colID, &res);
    }
    Ok(())
}

/// executeToDatetime 求值日期时间并追加到目标列。
fn executeToDatetime(
    ctx: &dyn EvalContext,
    expr: &dyn Expression,
    _fieldType: &types::FieldType,
    row: chunk::Row,
    output: &mut chunk::Chunk,
    colID: usize,
) -> Result<(), Error> {
    let (res, isNull) = expr.EvalTime(ctx, row)?;
    if isNull {
        output.AppendNull(colID);
    } else {
        output.AppendTime(colID, res);
    }
    Ok(())
}

/// executeToDuration 求值时长并追加到目标列。
fn executeToDuration(
    ctx: &dyn EvalContext,
    expr: &dyn Expression,
    _fieldType: &types::FieldType,
    row: chunk::Row,
    output: &mut chunk::Chunk,
    colID: usize,
) -> Result<(), Error> {
    let (res, isNull) = expr.EvalDuration(ctx, row)?;
    if isNull {
        output.AppendNull(colID);
    } else {
        output.AppendDuration(colID, res);
    }
    Ok(())
}

/// executeToJSON 求值二进制 JSON 并追加到目标列。
fn executeToJSON(
    ctx: &dyn EvalContext,
    expr: &dyn Expression,
    _fieldType: &types::FieldType,
    row: chunk::Row,
    output: &mut chunk::Chunk,
    colID: usize,
) -> Result<(), Error> {
    let (res, isNull) = expr.EvalJSON(ctx, row)?;
    if isNull {
        output.AppendNull(colID);
    } else {
        output.AppendJSON(colID, res);
    }
    Ok(())
}

/// executeToVectorFloat32 求值向量并追加到目标列。
fn executeToVectorFloat32(
    ctx: &dyn EvalContext,
    expr: &dyn Expression,
    _fieldType: &types::FieldType,
    row: chunk::Row,
    output: &mut chunk::Chunk,
    colID: usize,
) -> Result<(), Error> {
    let (res, isNull) = expr.EvalVectorFloat32(ctx, row)?;
    if isNull {
        output.AppendNull(colID);
    } else {
        output.AppendVectorFloat32(colID, res);
    }
    Ok(())
}

/// executeToString 为 ENUM/SET 构造名称已知、数值暂为零的对象，其余类型直接追加字符串。
fn executeToString(
    ctx: &dyn EvalContext,
    expr: &dyn Expression,
    fieldType: &types::FieldType,
    row: chunk::Row,
    output: &mut chunk::Chunk,
    colID: usize,
) -> Result<(), Error> {
    let (res, isNull) = expr.EvalString(ctx, row)?;
    if isNull {
        output.AppendNull(colID);
    } else if fieldType.GetType() == mysql::TypeEnum {
        output.AppendEnum(
            colID,
            types::Enum {
                Value: 0,
                Name: res,
            },
        );
    } else if fieldType.GetType() == mysql::TypeSet {
        output.AppendSet(
            colID,
            types::Set {
                Value: 0,
                Name: res,
            },
        );
    } else {
        output.AppendString(colID, &res);
    }
    Ok(())
}

/// VectorizedFilter 返回每行是否通过全部过滤器。
pub fn VectorizedFilter(
    ctx: &dyn EvalContext,
    vecEnabled: bool,
    filters: &[Box<dyn Expression>],
    iterator: &mut chunk::Iterator4Chunk,
    selected: Vec<bool>,
) -> Result<Vec<bool>, Error> {
    let (selected, _, err) =
        VectorizedFilterConsiderNull(ctx, vecEnabled, filters, iterator, selected, None);
    err.map_or(Ok(selected), Err)
}

/// VectorizedFilterConsiderNull 同时返回选中位图和过滤结果 NULL 位图。
pub fn VectorizedFilterConsiderNull(
    ctx: &dyn EvalContext,
    vecEnabled: bool,
    filters: &[Box<dyn Expression>],
    iterator: &mut chunk::Iterator4Chunk,
    selected: Vec<bool>,
    isNull: Option<Vec<bool>>,
) -> (Vec<bool>, Option<Vec<bool>>, Option<Error>) {
    let canVectorized = filters.iter().all(|filter| filter.Vectorized());
    let input = iterator.GetChunk();
    let sel = input.Sel().map(|indices| indices.to_vec());
    let evaluated = if canVectorized && vecEnabled {
        vectorizedFilter(ctx, vecEnabled, filters, iterator, selected, isNull)
    } else {
        rowBasedFilter(ctx, filters, iterator, selected, isNull)
    };
    let (mut selected, isNull, err) = match evaluated {
        Ok(value) => (value.0, value.1, None),
        Err(err) => return (Vec::new(), None, Some(err)),
    };
    let Some(sel) = sel else {
        return (selected, isNull, err);
    };

    // 输入有 selection 时，过滤器只看得到其中行；最终结果必须再与原 selection 求交集。
    let mut unselected = vec![true; selected.len()];
    for index in sel {
        unselected[index] = false;
    }
    for i in 0..selected.len() {
        if selected[i] && unselected[i] {
            selected[i] = false;
        }
    }
    (selected, isNull, err)
}

/// rowBasedFilter 逐个过滤器、逐行求值，并在调用期间暂时清除 Chunk selection。
fn rowBasedFilter(
    ctx: &dyn EvalContext,
    filters: &[Box<dyn Expression>],
    iterator: &mut chunk::Iterator4Chunk,
    mut selected: Vec<bool>,
    mut isNull: Option<Vec<bool>>,
) -> Result<(Vec<bool>, Option<Vec<bool>>), Error> {
    struct SelectionGuard {
        chunk: *mut chunk::Chunk,
        saved: Option<Vec<usize>>,
    }

    impl Drop for SelectionGuard {
        fn drop(&mut self) {
            // SAFETY: rowBasedFilter exclusively owns the iterator for the guard's lifetime, and
            // Iterator4Chunk keeps the referenced Chunk alive. This mirrors Go's deferred SetSel.
            unsafe { (&mut *self.chunk).SetSel(self.saved.take()) };
        }
    }

    let chunk_ptr = iterator.GetChunkMut() as *mut chunk::Chunk;
    let saved_sel = iterator.GetChunk().Sel().map(|sel| sel.to_vec());
    let _selection_guard = saved_sel.as_ref().map(|saved| SelectionGuard {
        chunk: chunk_ptr,
        saved: Some(saved.clone()),
    });
    if saved_sel.is_some() {
        iterator.GetChunkMut().SetSel(None);
        iterator.Reset();
    }
    selected.clear();
    selected.resize(iterator.Len(), true);
    if let Some(nulls) = isNull.as_mut() {
        nulls.clear();
        nulls.resize(iterator.Len(), false);
    }
    for filter in filters {
        let isIntType = filter.GetType(ctx).EvalType() == types::ETInt;
        let mut row = iterator.Begin();
        while row != iterator.End() {
            let index = row.Idx();
            if selected[index] {
                let (passed, null_result) = if isIntType {
                    let (value, is_null) = filter.EvalInt(ctx, row.clone())?;
                    (!is_null && value != 0, is_null)
                } else {
                    // Go 计划未来把非整数过滤器改写为 CAST AS SIGNED；保留 EvalBool 回退。
                    let expression = CNFExprs(vec![filter.CloneExpr()]);
                    let (value, is_null) = EvalBool(ctx, &expression, row.clone())?;
                    (value, is_null)
                };
                selected[index] = selected[index] && passed;
                if let Some(nulls) = isNull.as_mut() {
                    nulls[index] = nulls[index] || null_result;
                }
            }
            row = iterator.Next();
        }
    }
    Ok((selected, isNull))
}

/// vectorizedFilter 委托 VecEvalBool 批量计算所有过滤器。
fn vectorizedFilter(
    ctx: &dyn EvalContext,
    vecEnabled: bool,
    filters: &[Box<dyn Expression>],
    iterator: &mut chunk::Iterator4Chunk,
    selected: Vec<bool>,
    isNull: Option<Vec<bool>>,
) -> Result<(Vec<bool>, Option<Vec<bool>>), Error> {
    let wants_nulls = isNull.is_some();
    let expressions = CNFExprs(filters.iter().map(|filter| filter.CloneExpr()).collect());
    let (selected, nulls) = expression_core::VecEvalBool(
        ctx,
        vecEnabled,
        &expressions,
        iterator.GetChunkMut(),
        selected,
        isNull.unwrap_or_default(),
    )?;
    Ok((selected, wants_nulls.then_some(nulls)))
}
