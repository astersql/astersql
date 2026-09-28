// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 向量化求值辅助：把常量表达式的标量结果扩展为整列向量。
//
// 向量化（vectorized execution）按批处理多行，避免逐行解释；
// `genVecFromConstExpr` 在输入批大小已知时，将常量广播到 `result` 的每一行。

use crate::*;

// genVecFromConstExpr 对应 Go 的同名函数：根据目标求值类型选择标量求值入口，
// 并把得到的常量值或 NULL 扩展到 result 的每一行。
/// 按目标 `EvalType` 对常量表达式做一次标量求值，并把结果/NULL 广播到 `result` 的 n 行。
pub fn genVecFromConstExpr(
    ctx: &dyn EvalContext,
    expr: &dyn Expression,
    targetType: types::EvalType,
    input: Option<&chunk::Chunk>,
    result: &mut chunk::Column,
) -> Result<(), errors::Error> {
    // Go 在没有输入 Chunk 时仍生成一行，供常量表达式的独立向量求值使用。
    let mut n = 1usize;
    if let Some(input) = input {
        n = input.NumRows();
        if n == 0 {
            // 空输入必须按目标类型重置结果，避免上次复用 Column 时残留数据。
            result.Reset(targetType);
            return Ok(());
        }
    }

    match targetType {
        types::ETInt => {
            // 各 Eval* 调用的错误直接上抛，保持 Go 的早返回语义。
            let (v, isNull) = expr.EvalInt(ctx, chunk::Row::default())?;
            if isNull {
                result.ResizeInt64(n, true);
                return Ok(());
            }
            result.Reset(types::ETInt);
            for _ in 0..n {
                result.AppendInt64(v);
            }
        }
        types::ETReal => {
            let (v, isNull) = expr.EvalReal(ctx, chunk::Row::default())?;
            if isNull {
                result.ResizeFloat64(n, true);
                return Ok(());
            }
            result.Reset(types::ETReal);
            for _ in 0..n {
                result.AppendFloat64(v);
            }
        }
        types::ETDecimal => {
            let (v, isNull) = expr.EvalDecimal(ctx, chunk::Row::default())?;
            if isNull {
                result.ResizeDecimal(n, true);
                return Ok(());
            }
            result.Reset(types::ETDecimal);
            // Go 的 EvalDecimal 返回指针并逐行解引用复制。
            for _ in 0..n {
                result.AppendMyDecimal(&v);
            }
        }
        types::ETDatetime | types::ETTimestamp => {
            // Datetime 与 Timestamp 在 Go 中共享 EvalTime 和 Column 的时间存储布局。
            let (v, isNull) = expr.EvalTime(ctx, chunk::Row::default())?;
            if isNull {
                result.ResizeTime(n, true);
                return Ok(());
            }
            result.Reset(targetType);
            for _ in 0..n {
                result.AppendTime(v.clone());
            }
        }
        types::ETDuration => {
            let (v, isNull) = expr.EvalDuration(ctx, chunk::Row::default())?;
            if isNull {
                result.ResizeGoDuration(n, true);
                return Ok(());
            }
            result.Reset(types::ETDuration);
            // Go 只写入 types.Duration 包装值中的 Duration 字段。
            for _ in 0..n {
                result.AppendDuration(v.clone());
            }
        }
        types::ETJson => {
            // 变长类型先预留容量，再按行追加；这与定长类型的 Resize 写入路径不同。
            result.ReserveJSON(n);
            let (v, isNull) = expr.EvalJSON(ctx, chunk::Row::default())?;
            for _ in 0..n {
                if isNull {
                    result.AppendNull();
                } else {
                    result.AppendJSON(v.clone());
                }
            }
        }
        types::ETVectorFloat32 => {
            result.ReserveVectorFloat32(n);
            let (v, isNull) = expr.EvalVectorFloat32(ctx, chunk::Row::default())?;
            for _ in 0..n {
                if isNull {
                    result.AppendNull();
                } else {
                    result.AppendVectorFloat32(v.Clone());
                }
            }
        }
        types::ETString => {
            result.ReserveString(n);
            let (v, isNull) = expr.EvalString(ctx, chunk::Row::default())?;
            for _ in 0..n {
                if isNull {
                    result.AppendNull();
                } else {
                    result.AppendString(&v);
                }
            }
        }
        _ => {
            // 未覆盖类型沿用 Go 的 errors.Errorf 文案，不静默产生空结果。
            return Err(errors::Errorf(format!(
                "unsupported type {} during evaluation",
                targetType
            )));
        }
    }

    Ok(())
}
