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

// 内置函数核心 trait：`builtinFunc`。
//
// 对应 Go 的标量函数签名接口：按求值类型（EvalType）提供行级 `eval*`
// 与列级 `vecEval*`，并携带返回类型、校对器、protobuf 编码与向量化能力元数据。
// 默认的向量化实现逐行调用标量求值，具体签名可覆盖为真正的批量路径。

use std::any::Any;

use crate::{
    BuildContext, CollationInfo, Error, EvalContext, Expression, OptionalEvalPropKeySet, chunk,
    collate, errors, types,
};

#[allow(non_camel_case_types)]
/// 内置函数签名的公共接口（对齐 Go `builtinFunc`）。
/// 各具体函数只实现其返回类型对应的 `eval*`/`vecEval*`，其余默认返回未实现错误。
pub trait builtinFunc: CollationInfo {
    /// 向下转型为具体签名类型。
    fn as_any(&self) -> &dyn Any;

    /// 求值所需的可选会话属性集合；多数内置函数为空。
    fn RequiredOptionalEvalProps(&self) -> OptionalEvalPropKeySet {
        OptionalEvalPropKeySet::default()
    }

    /// 是否为扩展（非核心）函数。
    fn isExtensionFunction(&self) -> bool {
        false
    }
    /// GROUPING 元数据是否已初始化；非 grouping 函数返回 None。
    fn groupingMetaInitialized(&self) -> Option<bool> {
        None
    }
    /// 返回 grouping 模式与标记位图；非 grouping 函数返回 None。
    fn groupingModeAndMarks(&self) -> Option<(i64, Vec<Vec<u64>>)> {
        None
    }
    /// Restores GROUPING metadata captured at the plan-cache boundary.
    fn restoreGroupingModeAndMarks(&self, _mode: i64, _marks: Vec<Vec<u64>>) -> Result<(), Error> {
        Err(errors::New("builtin does not accept GROUPING metadata"))
    }

    /// Returns the already protobuf-encoded optional builtin metadata.
    /// Most builtins do not carry metadata; specialized signatures override it.
    fn metadata(&self) -> Option<Vec<u8>> {
        None
    }

    /// 签名是否可跨会话安全共享（无会话可变状态）。
    fn SafeToShareAcrossSession(&self) -> bool;

    /// 按行求值返回 Int；默认未实现。返回值中 bool 表示是否为 SQL NULL。
    fn evalInt(&self, _ctx: &dyn EvalContext, _row: chunk::Row) -> Result<(i64, bool), Error> {
        Err(errors::New("builtin does not implement evalInt"))
    }
    /// 按行求值返回 Real（双精度浮点）。
    fn evalReal(&self, _ctx: &dyn EvalContext, _row: chunk::Row) -> Result<(f64, bool), Error> {
        Err(errors::New("builtin does not implement evalReal"))
    }
    /// 按行求值返回 String。
    fn evalString(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> Result<(String, bool), Error> {
        Err(errors::New("builtin does not implement evalString"))
    }
    /// 按行求值返回 Decimal（定点数）。
    fn evalDecimal(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> Result<(types::MyDecimal, bool), Error> {
        Err(errors::New("builtin does not implement evalDecimal"))
    }
    /// 按行求值返回 Time（日期时间）。
    fn evalTime(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> Result<(types::Time, bool), Error> {
        Err(errors::New("builtin does not implement evalTime"))
    }
    /// 按行求值返回 Duration（时间间隔）。
    fn evalDuration(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> Result<(types::Duration, bool), Error> {
        Err(errors::New("builtin does not implement evalDuration"))
    }
    /// 按行求值返回 JSON（BinaryJSON）。
    fn evalJSON(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> Result<(types::BinaryJSON, bool), Error> {
        Err(errors::New("builtin does not implement evalJSON"))
    }
    /// 按行求值返回 VectorFloat32 向量类型。
    fn evalVectorFloat32(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> Result<(types::VectorFloat32, bool), Error> {
        Err(errors::New("builtin does not implement evalVectorFloat32"))
    }

    /// 向量化求值 Int：默认逐行调用 `evalInt` 写入结果列。
    fn vecEvalInt(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        // 清空结果列后逐行填充；is_null 为真时追加 SQL NULL。
        result.ResizeInt64(0, false);
        for index in 0..input.NumRows() {
            let (value, is_null) = self.evalInt(ctx, input.GetRow(index))?;
            if is_null {
                result.AppendNull();
            } else {
                result.AppendInt64(value);
            }
        }
        Ok(())
    }
    /// 向量化求值 Real。
    fn vecEvalReal(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        result.ResizeFloat64(0, false);
        for index in 0..input.NumRows() {
            let (value, is_null) = self.evalReal(ctx, input.GetRow(index))?;
            if is_null {
                result.AppendNull();
            } else {
                result.AppendFloat64(value);
            }
        }
        Ok(())
    }
    /// 向量化求值 String。
    fn vecEvalString(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        result.ReserveString(input.NumRows());
        for index in 0..input.NumRows() {
            let (value, is_null) = self.evalString(ctx, input.GetRow(index))?;
            if is_null {
                result.AppendNull();
            } else {
                result.AppendString(&value);
            }
        }
        Ok(())
    }
    /// 向量化求值 Decimal。
    fn vecEvalDecimal(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        result.ResizeDecimal(0, false);
        for index in 0..input.NumRows() {
            let (value, is_null) = self.evalDecimal(ctx, input.GetRow(index))?;
            if is_null {
                result.AppendNull();
            } else {
                result.AppendMyDecimal(&value);
            }
        }
        Ok(())
    }
    /// 向量化求值 Time。
    fn vecEvalTime(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        result.ResizeTime(0, false);
        for index in 0..input.NumRows() {
            let (value, is_null) = self.evalTime(ctx, input.GetRow(index))?;
            if is_null {
                result.AppendNull();
            } else {
                result.AppendTime(value);
            }
        }
        Ok(())
    }
    /// 向量化求值 Duration。
    fn vecEvalDuration(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        result.ResizeGoDuration(0, false);
        for index in 0..input.NumRows() {
            let (value, is_null) = self.evalDuration(ctx, input.GetRow(index))?;
            if is_null {
                result.AppendNull();
            } else {
                result.AppendDuration(value);
            }
        }
        Ok(())
    }
    /// 向量化求值 JSON。
    fn vecEvalJSON(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        result.Reset(types::ETJson);
        for index in 0..input.NumRows() {
            let (value, is_null) = self.evalJSON(ctx, input.GetRow(index))?;
            if is_null {
                result.AppendNull();
            } else {
                result.AppendJSON(value);
            }
        }
        Ok(())
    }
    /// 向量化求值 VectorFloat32。
    fn vecEvalVectorFloat32(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        result.Reset(types::ETVectorFloat32);
        for index in 0..input.NumRows() {
            let (value, is_null) = self.evalVectorFloat32(ctx, input.GetRow(index))?;
            if is_null {
                result.AppendNull();
            } else {
                result.AppendVectorFloat32(value);
            }
        }
        Ok(())
    }

    /// 返回函数参数表达式切片。
    fn getArgs(&self) -> &[Box<dyn Expression>];
    /// 可变借用参数表达式切片。
    fn getArgsMut(&mut self) -> &mut [Box<dyn Expression>];
    /// 语义相等比较（含参数与类型）。
    fn equal(&self, ctx: &dyn EvalContext, other: &dyn builtinFunc) -> bool;
    /// 返回类型（FieldType）。
    fn getRetTp(&self) -> &types::FieldType;
    /// 设置下推到 TiKV/TiFlash 时使用的 protobuf 函数码。
    fn setPbCode(&mut self, code: i32);
    /// 读取 protobuf 函数码。
    fn PbCode(&self) -> i32;
    /// 设置字符串比较使用的校对器。
    fn setCollator(&mut self, collator: Box<dyn collate::Collator>);
    /// 当前校对器。
    fn collator(&self) -> &dyn collate::Collator;
    /// 深拷贝签名（对齐 Go Clone 命名）。
    fn Clone(&self) -> Box<dyn builtinFunc>;
    /// 估算内存占用字节数。
    fn MemoryUsage(&self) -> i64;
    /// 本签名是否声明支持向量化求值。
    fn vectorized(&self) -> bool;

    /// 所有子表达式是否均已向量化。
    fn isChildrenVectorized(&self) -> bool {
        self.getArgs().iter().all(|argument| argument.Vectorized())
    }
}

/// The snapshot whitelist is the built-in registry plus constructor-handled core functions.
pub(crate) fn is_core_cache_snapshot_builtin(name: &str) -> bool {
    crate::funcs.contains_key(name)
        || matches!(
            name,
            crate::ast::Cast
                | crate::ast::GetVar
                | crate::InternalFuncFromBinary
                | crate::InternalFuncToBinary
        )
}

/// Rebuilds a cache-snapshotted builtin through the canonical core registry.
///
/// Keeping this entry point separate from general expression construction makes the
/// cache boundary reject extension and unknown functions before invoking a factory.
pub(crate) fn rebuild_core_cache_snapshot_builtin(
    ctx: &dyn BuildContext,
    name: &str,
    ret_type: types::FieldType,
    arguments: Vec<Box<dyn Expression>>,
) -> Result<Box<dyn Expression>, Error> {
    if !is_core_cache_snapshot_builtin(name) {
        return Err(errors::New(format!(
            "builtin {name} is not registered for plan-cache snapshots"
        )));
    }
    crate::NewFunctionBase(ctx, name, ret_type, arguments)
}
