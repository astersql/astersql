// Copyright 2016 PingCAP, Inc.
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

// 列表达式与关联列（CorrelatedColumn）。
//
// 对应 Go `column.go`：实现普通列与关联子查询外层列的行/向量求值、哈希与相等、
// 虚拟生成列表达式解析，以及列工具函数。关联列在执行期由外层行填充 Data。

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::*;

/// CorrelatedColumn 对应关联子查询中的外层列；Data 在执行期由外层行填充。
pub struct CorrelatedColumn {
    pub column: Column,
    pub data: Option<CorrelatedDatum>,
}

/// Go `*types.Datum` 的线程安全共享槽位。
pub type CorrelatedDatum = Arc<RwLock<types::Datum>>;

/// 把 Datum 包装进可跨线程共享的关联列数据槽。
pub fn NewCorrelatedDatum(value: types::Datum) -> CorrelatedDatum {
    Arc::new(RwLock::new(value))
}

impl CorrelatedColumn {
    /// Data 属于会话执行状态，因此与 Go 一样禁止跨会话共享。
    pub fn SafeToShareAcrossSession(&self) -> bool {
        false
    }

    /// 关联列可按常量广播路径做向量化求值。
    pub fn Vectorized(&self) -> bool {
        true
    }

    /// Clone 保留 Go 的浅拷贝语义，Datum 指针所代表的运行期槽位仍共享。
    pub fn Clone(&self) -> CorrelatedColumn {
        CorrelatedColumn {
            column: self.column.clone(),
            data: self.data.clone(),
        }
    }

    // 向量化求值把当前关联值扩展到输入的每一行，实际列缓冲操作委托公共辅助函数。
    pub fn VecEvalInt(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        genVecFromConstExpr(ctx, self, types::ETInt, input, result)
    }
    pub fn VecEvalReal(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        genVecFromConstExpr(ctx, self, types::ETReal, input, result)
    }
    pub fn VecEvalString(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        genVecFromConstExpr(ctx, self, types::ETString, input, result)
    }
    pub fn VecEvalDecimal(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        genVecFromConstExpr(ctx, self, types::ETDecimal, input, result)
    }
    pub fn VecEvalTime(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        genVecFromConstExpr(ctx, self, types::ETTimestamp, input, result)
    }
    pub fn VecEvalDuration(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        genVecFromConstExpr(ctx, self, types::ETDuration, input, result)
    }
    pub fn VecEvalJSON(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        genVecFromConstExpr(ctx, self, types::ETJson, input, result)
    }
    pub fn VecEvalVectorFloat32(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        genVecFromConstExpr(ctx, self, types::ETVectorFloat32, input, result)
    }

    pub fn Traverse(&self, action: &mut dyn TraverseAction) -> Box<dyn Expression> {
        action.Transform(Box::new(self.Clone()))
    }

    /// Eval 直接读取运行期 Datum；未绑定表示执行器违反了关联列填充约定。
    pub fn Eval(&self, _ctx: &dyn EvalContext, _row: chunk::Row) -> Result<types::Datum, Error> {
        Ok(self
            .data
            .as_ref()
            .expect("correlated column data is not bound")
            .read()
            .expect("correlated datum lock poisoned")
            .clone())
    }

    pub fn EvalInt(&self, ctx: &dyn EvalContext, _row: chunk::Row) -> Result<(i64, bool), Error> {
        let data = self
            .data
            .as_ref()
            .unwrap()
            .read()
            .expect("correlated datum lock poisoned");
        if data.IsNull() {
            return Ok((0, true));
        }
        if self.column.GetType(ctx).Hybrid() {
            return data.ToInt64(typeCtx(ctx)).map(|v| (v, false));
        }
        Ok((data.GetInt64(), false))
    }
    pub fn EvalReal(&self, _ctx: &dyn EvalContext, _row: chunk::Row) -> Result<(f64, bool), Error> {
        let data = self
            .data
            .as_ref()
            .unwrap()
            .read()
            .expect("correlated datum lock poisoned");
        if data.IsNull() {
            return Ok((0.0, true));
        }
        Ok((data.GetFloat64(), false))
    }
    pub fn EvalString(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> Result<(String, bool), Error> {
        let data = self
            .data
            .as_ref()
            .unwrap()
            .read()
            .expect("correlated datum lock poisoned");
        if data.IsNull() {
            return Ok((String::new(), true));
        }
        data.ToString().map(|v| (v, false))
    }
    pub fn EvalDecimal(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> Result<(types::MyDecimal, bool), Error> {
        let data = self
            .data
            .as_ref()
            .unwrap()
            .read()
            .expect("correlated datum lock poisoned");
        if data.IsNull() {
            return Ok((types::MyDecimal::default(), true));
        }
        Ok((data.GetMysqlDecimal().clone(), false))
    }
    pub fn EvalTime(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> Result<(types::Time, bool), Error> {
        let data = self
            .data
            .as_ref()
            .unwrap()
            .read()
            .expect("correlated datum lock poisoned");
        if data.IsNull() {
            return Ok((types::ZeroTime, true));
        }
        Ok((data.GetMysqlTime(), false))
    }
    pub fn EvalDuration(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> Result<(types::Duration, bool), Error> {
        let data = self
            .data
            .as_ref()
            .unwrap()
            .read()
            .expect("correlated datum lock poisoned");
        if data.IsNull() {
            return Ok((types::Duration::default(), true));
        }
        Ok((data.GetMysqlDuration(), false))
    }
    pub fn EvalJSON(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> Result<(types::BinaryJSON, bool), Error> {
        let data = self
            .data
            .as_ref()
            .unwrap()
            .read()
            .expect("correlated datum lock poisoned");
        if data.IsNull() {
            return Ok((types::BinaryJSON::default(), true));
        }
        Ok((data.GetMysqlJSON(), false))
    }
    pub fn EvalVectorFloat32(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> Result<(types::VectorFloat32, bool), Error> {
        let data = self
            .data
            .as_ref()
            .unwrap()
            .read()
            .expect("correlated datum lock poisoned");
        if data.IsNull() {
            return Ok((types::ZeroVectorFloat32(), true));
        }
        Ok((data.GetVectorFloat32(), false))
    }

    pub fn Equal(&self, _ctx: &dyn EvalContext, expr: &dyn Expression) -> bool {
        self.EqualColumn(expr)
    }
    pub fn EqualColumn(&self, expr: &dyn Expression) -> bool {
        expr.as_correlated_column()
            .is_some_and(|other| self.column.EqualColumn(&other.column))
    }
    pub fn IsCorrelated(&self) -> bool {
        true
    }
    pub fn ConstLevel(&self) -> ConstLevel {
        ConstLevel::None
    }

    /// schema 含有基础列时解除关联，否则保留关联列本身。
    pub fn Decorrelate<'a>(&'a self, schema: &Schema) -> &'a dyn Expression {
        if schema.Contains(&self.column) {
            &self.column
        } else {
            self
        }
    }
    pub fn ResolveIndices(&self, _schema: &Schema) -> Result<CorrelatedColumn, Error> {
        Ok(self.Clone())
    }
    fn resolveIndices(&mut self, _schema: &Schema) -> Result<(), Error> {
        Ok(())
    }
    pub fn ResolveIndicesByVirtualExpr(
        &self,
        _ctx: &dyn EvalContext,
        _schema: &Schema,
    ) -> (CorrelatedColumn, bool) {
        (self.Clone(), true)
    }
    fn resolveIndicesByVirtualExpr(&mut self, _ctx: &dyn EvalContext, _schema: &Schema) -> bool {
        true
    }

    pub fn MemoryUsage(&self) -> i64 {
        self.column.MemoryUsage()
            + size::SizeOfPointer
            + self.data.as_ref().map_or(0, |v| {
                v.read().expect("correlated datum lock poisoned").MemUsage()
            })
    }

    /// 映射只替换内嵌 Column，运行期 Data 槽位保持不变。
    pub fn RemapColumn(&self, mapping: &HashMap<i64, Column>) -> Result<CorrelatedColumn, Error> {
        let mapped = mapping.get(&self.column.UniqueID).ok_or_else(|| {
            errors::New(format!("Can't remap column for {}", self.column.String()))
        })?;
        Ok(CorrelatedColumn {
            column: mapped.clone(),
            data: self.data.clone(),
        })
    }

    pub fn Hash64(&self, h: &mut dyn base::Hasher) {
        // Data 在运行时才填充，哈希只编码关联列标志和基础列，避免缓存键随行值变化。
        h.HashByte(correlatedColumn);
        self.column.Hash64(h);
    }
    /// 仅比较内嵌 Column 身份，忽略 Data 当前值。
    pub fn Equals(&self, other: &CorrelatedColumn) -> bool {
        self.column.Equals(&other.column)
    }
}

/// Column 对应执行行中的列引用；Index 是当前 schema 下的位置，UniqueID 表示逻辑身份。
#[derive(Clone)]
pub struct Column {
    pub RetType: Option<types::FieldType>,
    pub ID: i64,
    pub UniqueID: i64,
    pub Index: isize,
    pub(crate) hashcode: Vec<u8>,
    pub VirtualExpr: Option<Box<dyn Expression>>,
    pub OrigName: String,
    pub IsHidden: bool,
    pub IsPrefix: bool,
    pub InOperand: bool,
    pub collation_info: collationInfo,
    pub CorrelatedColUniqueID: i64,
}

impl Default for Column {
    fn default() -> Self {
        Self {
            RetType: None,
            ID: 0,
            UniqueID: 0,
            Index: 0,
            hashcode: Vec::new(),
            VirtualExpr: None,
            OrigName: String::new(),
            IsHidden: false,
            IsPrefix: false,
            InOperand: false,
            collation_info: collationInfo::default(),
            CorrelatedColUniqueID: 0,
        }
    }
}

/// 展示名中列编号的前缀，形如 `Column#12`。
const COLUMN_PREFIX: &str = "Column#";

impl Column {
    /// Capture an owned representation, recursively snapshotting a virtual expression.
    pub fn ToCacheSnapshot(&self) -> Result<CachedColumn, CacheSnapshotError> {
        CachedColumn::try_from_column(self)
    }

    /// Constructs a resolved planner column while keeping hash/collation caches
    /// private to the expression crate.
    /// 构造已解析的计划列；哈希与排序规则缓存仅在本 crate 内部维护。
    pub fn new(ret_type: types::FieldType, id: i64, unique_id: i64, index: isize) -> Column {
        Column {
            RetType: Some(ret_type),
            ID: id,
            UniqueID: unique_id,
            Index: index,
            ..Default::default()
        }
    }

    /// 无虚拟表达式时可跨会话共享；虚拟列依赖会话态求值上下文。
    pub fn SafeToShareAcrossSession(&self) -> bool {
        self.VirtualExpr.is_none()
    }
    /// 按 UniqueID 判断与另一表达式是否同一列。
    pub fn Equal(&self, _ctx: &dyn EvalContext, expr: &dyn Expression) -> bool {
        self.EqualColumn(expr)
    }
    /// 若 expr 是 Column 且 UniqueID 相同则相等。
    pub fn EqualColumn(&self, expr: &dyn Expression) -> bool {
        expr.as_column()
            .is_some_and(|other| other.UniqueID == self.UniqueID)
    }
    pub fn EqualByExprAndID(&self, ctx: &dyn EvalContext, expr: &dyn Expression) -> bool {
        let Some(other) = expr.as_column() else {
            return false;
        };
        let virtual_match = self.VirtualExpr.as_ref().is_some_and(|v| {
            other
                .VirtualExpr
                .as_ref()
                .is_some_and(|rhs| v.Equal(ctx, rhs.as_ref()))
                && self.RetType == other.RetType
        });
        other.UniqueID == self.UniqueID || virtual_match
    }

    pub fn VecEvalInt(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        if self.RetType.as_ref().unwrap().Hybrid() {
            result.ResizeInt64(0, false);
            // 混合类型不能直接复制底层缓冲，逐行转换以保留 ENUM/SET/BIT 语义和错误传播。
            for index in 0..input.NumRows() {
                let row = input.GetRow(index);
                let (value, is_null) = self.EvalInt(ctx, row)?;
                if is_null {
                    result.AppendNull();
                } else {
                    result.AppendInt64(value);
                }
            }
            return Ok(());
        }
        *result = *input
            .Column(self.Index as usize)
            .CopyReconstruct(input.Sel(), None);
        Ok(())
    }

    pub fn VecEvalReal(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        if self.GetType(ctx).GetType() == mysql::TypeFloat {
            result.ResizeFloat64(input.NumRows(), false);
            // Go 对 FLOAT 做 f32→f64 扩展；选择向量存在时按逻辑行号重排，并同步 NULL 位图。
            for out in 0..input.NumRows() {
                let source = input.Sel().map_or(out, |selection| selection[out]);
                if input.Column(self.Index as usize).IsNull(source) {
                    result.SetNull(out, true);
                } else {
                    result.Float64s()[out] =
                        input.Column(self.Index as usize).GetFloat32(source) as f64;
                }
            }
            return Ok(());
        }
        *result = *input
            .Column(self.Index as usize)
            .CopyReconstruct(input.Sel(), None);
        Ok(())
    }

    pub fn VecEvalString(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        if self.RetType.as_ref().unwrap().Hybrid() {
            result.ReserveString(input.NumRows());
            for index in 0..input.NumRows() {
                let row = input.GetRow(index);
                let (value, is_null) = self.EvalString(ctx, row)?;
                if is_null {
                    result.AppendNull();
                } else {
                    result.AppendString(&value);
                }
            }
            return Ok(());
        }
        *result = *input
            .Column(self.Index as usize)
            .CopyReconstruct(input.Sel(), None);
        Ok(())
    }
    pub fn VecEvalDecimal(
        &self,
        _ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        *result = *input
            .Column(self.Index as usize)
            .CopyReconstruct(input.Sel(), None);
        Ok(())
    }
    pub fn VecEvalTime(
        &self,
        _ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        *result = *input
            .Column(self.Index as usize)
            .CopyReconstruct(input.Sel(), None);
        Ok(())
    }
    pub fn VecEvalDuration(
        &self,
        _ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        *result = *input
            .Column(self.Index as usize)
            .CopyReconstruct(input.Sel(), None);
        Ok(())
    }
    pub fn VecEvalJSON(
        &self,
        _ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        *result = *input
            .Column(self.Index as usize)
            .CopyReconstruct(input.Sel(), None);
        Ok(())
    }
    pub fn VecEvalVectorFloat32(
        &self,
        _ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        *result = *input
            .Column(self.Index as usize)
            .CopyReconstruct(input.Sel(), None);
        Ok(())
    }

    /// StringWithCtx 根据 EXPLAIN plan_tree 格式决定是否隐藏列编号。
    pub fn StringWithCtx(&self, ctx: &dyn ParamValues, redact: &str) -> String {
        self.string(redact, shouldRemoveColumnNumbers(ctx))
    }
    pub fn StringWithCtxForExplain(
        &self,
        _ctx: &dyn ParamValues,
        redact: &str,
        remove_numbers: bool,
    ) -> String {
        self.string(redact, remove_numbers)
    }
    pub fn String(&self) -> String {
        self.string(errors::RedactLogDisable, false)
    }
    fn string(&self, redact: &str, remove_numbers: bool) -> String {
        if self.IsHidden {
            if let Some(expr) = &self.VirtualExpr {
                return expr.StringWithCtx(Some(&exprctx::EmptyParamValues), redact);
            }
        }
        if !self.OrigName.is_empty() {
            return self.OrigName.clone();
        }
        if remove_numbers {
            "Column".to_owned()
        } else {
            format!("{}{}", COLUMN_PREFIX, self.UniqueID)
        }
    }

    pub fn GetType<'a>(&'a self, _ctx: &dyn EvalContext) -> &'a types::FieldType {
        self.GetStaticType()
    }
    pub fn GetStaticType(&self) -> &types::FieldType {
        self.RetType.as_ref().unwrap()
    }

    pub fn Hash64(&self, h: &mut dyn base::Hasher) {
        if let Some(tp) = &self.RetType {
            h.HashByte(base::NotNilFlag);
            hashFieldType(h, tp);
        } else {
            h.HashByte(base::NilFlag);
        }
        h.HashInt64(self.ID);
        h.HashInt64(self.UniqueID);
        h.HashInt(self.Index);
        if let Some(expr) = &self.VirtualExpr {
            h.HashByte(base::NotNilFlag);
            expr.Hash64(h);
        } else {
            h.HashByte(base::NilFlag);
        }
        h.HashString(&self.OrigName);
        h.HashBool(self.IsHidden);
        h.HashBool(self.IsPrefix);
        h.HashBool(self.InOperand);
        self.collation_info.Hash64(h);
        h.HashInt64(self.CorrelatedColUniqueID);
    }

    pub fn Equals(&self, other: &Column) -> bool {
        self.RetType == other.RetType
            && option_expr_equals(&self.VirtualExpr, &other.VirtualExpr)
            && self.ID == other.ID
            && self.UniqueID == other.UniqueID
            && self.Index == other.Index
            && self.OrigName == other.OrigName
            && self.IsHidden == other.IsHidden
            && self.IsPrefix == other.IsPrefix
            && self.InOperand == other.InOperand
            && self.collation_info.Equals(&other.collation_info)
            && self.CorrelatedColUniqueID == other.CorrelatedColUniqueID
    }

    pub fn Traverse(&self, action: &mut dyn TraverseAction) -> Box<dyn Expression> {
        action.Transform(Box::new(self.clone()))
    }
    pub fn Eval(&self, _ctx: &dyn EvalContext, row: chunk::Row) -> Result<types::Datum, Error> {
        Ok(row.GetDatum(self.Index as usize, self.GetStaticType()))
    }
    pub fn EvalInt(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<(i64, bool), Error> {
        if self.GetType(ctx).Hybrid() {
            let value = row.GetDatum(self.Index as usize, self.GetStaticType());
            if value.IsNull() {
                return Ok((0, true));
            }
            if value.Kind() == types::KindMysqlBit {
                return value
                    .GetBinaryLiteral()
                    .ToInt(typeCtx(ctx))
                    .map(|v| (v as i64, false))
                    .map_err(Into::into);
            }
            return value.ToInt64(typeCtx(ctx)).map(|v| (v, false));
        }
        if row.IsNull(self.Index as usize) {
            return Ok((0, true));
        }
        Ok((row.GetInt64(self.Index as usize), false))
    }
    pub fn EvalReal(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<(f64, bool), Error> {
        if row.IsNull(self.Index as usize) {
            return Ok((0.0, true));
        }
        if self.GetType(ctx).GetType() == mysql::TypeFloat {
            return Ok((row.GetFloat32(self.Index as usize) as f64, false));
        }
        Ok((row.GetFloat64(self.Index as usize), false))
    }
    pub fn EvalString(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(String, bool), Error> {
        if row.IsNull(self.Index as usize) {
            return Ok((String::new(), true));
        }
        if self.GetType(ctx).Hybrid() {
            return row
                .GetDatum(self.Index as usize, self.GetStaticType())
                .ToString()
                .map(|v| (v, false));
        }
        Ok((row.GetString(self.Index as usize), false))
    }
    pub fn EvalDecimal(
        &self,
        _ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::MyDecimal, bool), Error> {
        if row.IsNull(self.Index as usize) {
            return Ok((types::MyDecimal::default(), true));
        }
        Ok((row.GetMyDecimal(self.Index as usize), false))
    }
    pub fn EvalTime(
        &self,
        _ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::Time, bool), Error> {
        if row.IsNull(self.Index as usize) {
            return Ok((types::ZeroTime, true));
        }
        Ok((row.GetTime(self.Index as usize), false))
    }
    pub fn EvalDuration(
        &self,
        _ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::Duration, bool), Error> {
        if row.IsNull(self.Index as usize) {
            return Ok((types::Duration::default(), true));
        }
        Ok((
            row.GetDuration(
                self.Index as usize,
                self.GetStaticType().GetDecimal() as i32,
            ),
            false,
        ))
    }
    pub fn EvalJSON(
        &self,
        _ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::BinaryJSON, bool), Error> {
        if row.IsNull(self.Index as usize) {
            return Ok((types::BinaryJSON::default(), true));
        }
        Ok((row.GetJSON(self.Index as usize), false))
    }
    pub fn EvalVectorFloat32(
        &self,
        _ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<(types::VectorFloat32, bool), Error> {
        if row.IsNull(self.Index as usize) {
            return Ok((types::ZeroVectorFloat32(), true));
        }
        Ok((row.GetVectorFloat32(self.Index as usize), false))
    }

    pub fn Clone(&self) -> Column {
        self.clone()
    }
    pub fn CloneColumn(&self) -> Column {
        self.clone()
    }
    pub fn IsCorrelated(&self) -> bool {
        false
    }
    pub fn ConstLevel(&self) -> ConstLevel {
        ConstLevel::None
    }
    pub fn Decorrelate(&self, _schema: &Schema) -> Column {
        self.clone()
    }

    pub fn HashCode(&mut self) -> Vec<u8> {
        if self.hashcode.is_empty() {
            self.hashcode.push(columnFlag);
            self.hashcode = codec::EncodeInt(std::mem::take(&mut self.hashcode), self.UniqueID);
        }
        self.hashcode.clone()
    }
    pub fn CanonicalHashCode(&mut self) -> Vec<u8> {
        self.HashCode()
    }
    pub fn CleanHashCode(&mut self) {
        self.hashcode = Vec::with_capacity(9);
    }

    pub fn ResolveIndices(&self, schema: &Schema) -> Result<Column, Error> {
        let mut cloned = self.clone();
        cloned.resolveIndices(schema)?;
        Ok(cloned)
    }
    fn resolveIndices(&mut self, schema: &Schema) -> Result<(), Error> {
        self.Index = schema
            .ColumnIndex(self)
            .map(|index| index as isize)
            .unwrap_or(-1);
        if self.Index == -1 {
            return Err(errors::New(format!(
                "Can't find column {} in schema [{}]",
                self.String(),
                schema
                    .Columns
                    .iter()
                    .map(|column| format!("{}#{}", column.String(), column.UniqueID))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        }
        Ok(())
    }
    pub fn ResolveIndicesByVirtualExpr(
        &self,
        ctx: &dyn EvalContext,
        schema: &Schema,
    ) -> (Column, bool) {
        let mut cloned = self.clone();
        let ok = cloned.resolveIndicesByVirtualExpr(ctx, schema);
        (cloned, ok)
    }
    fn resolveIndicesByVirtualExpr(&mut self, ctx: &dyn EvalContext, schema: &Schema) -> bool {
        let mut fallback = None;
        for (index, candidate) in schema.Columns.iter().enumerate() {
            if candidate.EqualColumn(self) {
                self.Index = index as isize;
                return true;
            }
            // 多个表达式索引可能拥有相同虚拟表达式：精确 UniqueID 优先，表达式相等仅作回退。
            if fallback.is_none() && candidate.EqualByExprAndID(ctx, self) {
                fallback = Some(index);
            }
        }
        if let Some(index) = fallback {
            self.Index = index as isize;
            return true;
        }
        false
    }
    pub fn RemapColumn(&self, mapping: &HashMap<i64, Column>) -> Result<Column, Error> {
        mapping
            .get(&self.UniqueID)
            .cloned()
            .ok_or_else(|| errors::New(format!("Can't remap column for {}", self.String())))
    }
    pub fn Vectorized(&self) -> bool {
        true
    }
    pub fn ToInfo(&self) -> model::ColumnInfo {
        model::ColumnInfo {
            ID: self.ID,
            FieldType: self.GetStaticType().clone(),
            ..Default::default()
        }
    }
    pub fn EvalVirtualColumn(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> Result<types::Datum, Error> {
        self.VirtualExpr.as_ref().unwrap().Eval(ctx, row)
    }
    pub fn Coercibility(&self) -> Coercibility {
        if !self.collation_info.HasCoercibility() {
            self.collation_info
                .SetCoercibility(deriveCoercibilityForColumn(self));
        }
        self.collation_info.Coercibility()
    }
    pub fn Repertoire(&self) -> Repertoire {
        if self.collation_info.repertoire != 0 {
            return self.collation_info.repertoire;
        }
        match self.GetStaticType().EvalType() {
            types::ETJson => UNICODE,
            types::ETString if self.GetStaticType().GetCharset() == charset::CharsetASCII => ASCII,
            types::ETString => UNICODE,
            _ => ASCII,
        }
    }
    pub fn InColumnArray(&self, columns: &[Column]) -> bool {
        columns.iter().any(|c| self.EqualColumn(c))
    }
    pub fn MemoryUsage(&self) -> i64 {
        let mut sum = EMPTY_COLUMN_SIZE
            + self.hashcode.capacity() as i64
            + (self.OrigName.len()
                + self.collation_info.charset.len()
                + self.collation_info.collation.len()) as i64;
        if let Some(tp) = &self.RetType {
            sum += tp.MemoryUsage();
        }
        if let Some(expr) = &self.VirtualExpr {
            sum += expr.MemoryUsage();
        }
        sum
    }
}

/// plan_tree 仅在真实 EXPLAIN 语句中隐藏 Column#编号；其它格式保持原标识，便于定位列。
pub fn shouldRemoveColumnNumbers(_ctx: &dyn ParamValues) -> bool {
    false
}

/// 把列切片转为 Expression 对象切片。
pub fn Column2Exprs(columns: &[Column]) -> Vec<Box<dyn Expression>> {
    columns
        .iter()
        .cloned()
        .map(|c| Box::new(c) as Box<dyn Expression>)
        .collect()
}
/// 按 ColumnInfo.ID 在列列表中查找对应列。
pub fn ColInfo2Col<'a>(columns: &'a [Column], info: &model::ColumnInfo) -> Option<&'a Column> {
    columns.iter().find(|column| column.ID == info.ID)
}
/// 按 UniqueID 升序排序列副本。
pub fn SortColumns(columns: &[Column]) -> Vec<Column> {
    let mut sorted = columns.to_vec();
    sorted.sort_by_key(|column| column.UniqueID);
    sorted
}

/// 只接受名为 tidb_shard 的标量函数；nil 或其它表达式类型都不是分片生成列。
pub fn GcColumnExprIsTidbShard(virtual_expr: Option<&dyn Expression>) -> bool {
    virtual_expr
        .and_then(|expression| expression.as_scalar_function())
        .is_some_and(|function| function.FuncName.L == ast::TiDBShard)
}

/// 空 Column 结构体的静态大小，MemoryUsage 以此为基数。
const EMPTY_COLUMN_SIZE: i64 = std::mem::size_of::<Column>() as i64;
