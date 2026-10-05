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

// 表达式内置函数（builtin）公共框架，对应 Go `builtin.go`。
//
// 提供内置函数的基类、参数校验、返回类型/排序规则推导、CAST 包装、函数注册表，
// 以及按求值上下文缓存签名等设施。具体算术/字符串/时间等函数在各自 `builtin_*.rs` 中实现；
// `formal_registry` 子模块对接正式的 functionClass 与工厂边界。
// ScalarFunction 表示 SQL 中的标量函数调用节点；EvalContext 携带会话变量等求值环境。

use std::any::Any;
use std::fmt;
use std::sync::atomic::AtomicU32;
use std::sync::{Mutex, OnceLock, RwLock};

#[derive(Clone, Debug, Eq, PartialEq)]
/// builtin 框架使用的简单错误类型。
pub struct Error(String);

impl Error {
    /// 由消息构造错误。
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

/// 字符集与默认排序规则常量。
pub mod charset {
    /// 常量 `CharsetBin`。
    pub const CharsetBin: &str = "binary";
    /// 常量 `CollationBin`。
    pub const CollationBin: &str = "binary";
    /// 常量 `CharsetASCII`。
    pub const CharsetASCII: &str = "ascii";
    /// 常量 `DEFAULT_CHARSET`。
    pub const DEFAULT_CHARSET: &str = "utf8mb4";
    /// 常量 `DEFAULT_COLLATION`。
    pub const DEFAULT_COLLATION: &str = "utf8mb4_bin";
}

/// MySQL 类型码与列标志位常量。
pub mod mysql {
    /// 常量 `TypeLonglong`。
    pub const TypeLonglong: u8 = 1;
    /// 常量 `TypeDouble`。
    pub const TypeDouble: u8 = 2;
    /// 常量 `TypeNewDecimal`。
    pub const TypeNewDecimal: u8 = 3;
    /// 常量 `TypeVarString`。
    pub const TypeVarString: u8 = 4;
    /// 常量 `TypeVarchar`。
    pub const TypeVarchar: u8 = 5;
    /// 常量 `TypeString`。
    pub const TypeString: u8 = 6;
    /// 常量 `TypeDatetime`。
    pub const TypeDatetime: u8 = 7;
    /// 常量 `TypeTimestamp`。
    pub const TypeTimestamp: u8 = 8;
    /// 常量 `TypeDuration`。
    pub const TypeDuration: u8 = 9;
    /// 常量 `TypeJSON`。
    pub const TypeJSON: u8 = 10;
    /// 常量 `TypeTiDBVectorFloat32`。
    pub const TypeTiDBVectorFloat32: u8 = 11;
    /// 常量 `TypeBit`。
    pub const TypeBit: u8 = 12;
    /// 常量 `TypeEnum`。
    pub const TypeEnum: u8 = 13;
    /// 常量 `TypeSet`。
    pub const TypeSet: u8 = 14;
    /// 常量 `TypeFloat`。
    pub const TypeFloat: u8 = 15;
    /// 常量 `TypeLongBlob`。
    pub const TypeLongBlob: u8 = 16;
    /// 常量 `TypeMediumBlob`。
    pub const TypeMediumBlob: u8 = 17;

    /// 常量 `NotNullFlag`。
    pub const NotNullFlag: u64 = 1 << 0;
    /// 常量 `BinaryFlag`。
    pub const BinaryFlag: u64 = 1 << 1;
    /// 常量 `UnsignedFlag`。
    pub const UnsignedFlag: u64 = 1 << 2;
    /// 常量 `IsBooleanFlag`。
    pub const IsBooleanFlag: u64 = 1 << 3;

    /// 常量 `MaxIntWidth`。
    pub const MaxIntWidth: isize = 20;
    /// 常量 `MaxRealWidth`。
    pub const MaxRealWidth: isize = 23;
    /// 常量 `MaxDatetimeWidthWithFsp`。
    pub const MaxDatetimeWidthWithFsp: isize = 26;
    /// 常量 `MaxDurationWidthWithFsp`。
    pub const MaxDurationWidthWithFsp: isize = 17;
    /// 常量 `MaxBlobWidth`。
    pub const MaxBlobWidth: isize = 16_777_216;
    /// 常量 `DefaultCharset`。
    pub const DefaultCharset: &str = super::charset::DEFAULT_CHARSET;
    /// 常量 `DefaultCollationName`。
    pub const DefaultCollationName: &str = super::charset::DEFAULT_COLLATION;

    /// 函数 `HasNotNullFlag`。
    pub fn HasNotNullFlag(flag: u64) -> bool {
        flag & NotNullFlag != 0
    }
    /// 函数 `HasBinaryFlag`。
    pub fn HasBinaryFlag(flag: u64) -> bool {
        flag & BinaryFlag != 0
    }
    /// 函数 `HasUnsignedFlag`。
    pub fn HasUnsignedFlag(flag: u64) -> bool {
        flag & UnsignedFlag != 0
    }
}

/// 常用函数名/运算符名字符串常量（大小写不敏感匹配用）。
pub mod ast {
    /// 常量 `EQ`。
    pub const EQ: &str = "eq";
    /// 常量 `NullEQ`。
    pub const NullEQ: &str = "nulleq";
    /// 常量 `IsTruthWithoutNull`。
    pub const IsTruthWithoutNull: &str = "istrue";
    /// 常量 `IsTruthWithNull`。
    pub const IsTruthWithNull: &str = "istrue_with_null";
    /// 常量 `IsFalsity`。
    pub const IsFalsity: &str = "isfalse";
    /// 常量 `IsFalsityWithNull`。
    pub const IsFalsityWithNull: &str = "isfalse_with_null";
    /// 常量 `NE`。
    pub const NE: &str = "ne";
    /// 常量 `LT`。
    pub const LT: &str = "lt";
    /// 常量 `LE`。
    pub const LE: &str = "le";
    /// 常量 `GT`。
    pub const GT: &str = "gt";
    /// 常量 `GE`。
    pub const GE: &str = "ge";
    /// 常量 `Plus`。
    pub const Plus: &str = "plus";
    /// 常量 `Minus`。
    pub const Minus: &str = "minus";
    /// 常量 `Mul`。
    pub const Mul: &str = "mul";
    /// 常量 `Div`。
    pub const Div: &str = "div";
    /// 常量 `Mod`。
    pub const Mod: &str = "mod";
    /// 常量 `IntDiv`。
    pub const IntDiv: &str = "intdiv";
    /// 常量 `SetVar`。
    pub const SetVar: &str = "setvar";
    /// 常量 `GetVar`。
    pub const GetVar: &str = "getvar";
    /// 常量 `NextVal`。
    pub const NextVal: &str = "nextval";
    /// 常量 `LastVal`。
    pub const LastVal: &str = "lastval";
    /// 常量 `SetVal`。
    pub const SetVal: &str = "setval";
}

/// 字段类型、EvalType 与标志操作的本地精简实现。
pub mod types {
    use super::{Error, charset, mysql};

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    /// 枚举 `EvalType`。
    pub enum EvalType {
        Int,
        Real,
        Decimal,
        String,
        Datetime,
        Timestamp,
        Duration,
        Json,
        VectorFloat32,
        Unknown(u8),
    }

    /// 常量 `ETInt`。
    pub const ETInt: EvalType = EvalType::Int;
    /// 常量 `ETReal`。
    pub const ETReal: EvalType = EvalType::Real;
    /// 常量 `ETDecimal`。
    pub const ETDecimal: EvalType = EvalType::Decimal;
    /// 常量 `ETString`。
    pub const ETString: EvalType = EvalType::String;
    /// 常量 `ETDatetime`。
    pub const ETDatetime: EvalType = EvalType::Datetime;
    /// 常量 `ETTimestamp`。
    pub const ETTimestamp: EvalType = EvalType::Timestamp;
    /// 常量 `ETDuration`。
    pub const ETDuration: EvalType = EvalType::Duration;
    /// 常量 `ETJson`。
    pub const ETJson: EvalType = EvalType::Json;
    /// 常量 `ETVectorFloat32`。
    pub const ETVectorFloat32: EvalType = EvalType::VectorFloat32;
    /// 常量 `UnspecifiedLength`。
    pub const UnspecifiedLength: isize = -1;
    /// 常量 `MaxFsp`。
    pub const MaxFsp: isize = 6;

    #[derive(Clone, Debug, Eq, PartialEq)]
    /// 结构体 `FieldType`。
    pub struct FieldType {
        tp: u8,
        flag: u64,
        flen: isize,
        decimal: isize,
        charset: String,
        collate: String,
        elems: Vec<String>,
    }

    impl FieldType {
        /// 由消息构造错误。
        pub fn new(tp: u8) -> Self {
            Self {
                tp,
                flag: 0,
                flen: UnspecifiedLength,
                decimal: UnspecifiedLength,
                charset: String::new(),
                collate: String::new(),
                elems: Vec::new(),
            }
        }
        /// 函数 `EvalType`。
        pub fn EvalType(&self) -> EvalType {
            match self.tp {
                mysql::TypeLonglong | mysql::TypeBit | mysql::TypeEnum => ETInt,
                mysql::TypeDouble | mysql::TypeFloat => ETReal,
                mysql::TypeNewDecimal => ETDecimal,
                mysql::TypeDatetime => ETDatetime,
                mysql::TypeTimestamp => ETTimestamp,
                mysql::TypeDuration => ETDuration,
                mysql::TypeJSON => ETJson,
                mysql::TypeTiDBVectorFloat32 => ETVectorFloat32,
                mysql::TypeVarString
                | mysql::TypeVarchar
                | mysql::TypeString
                | mysql::TypeLongBlob
                | mysql::TypeMediumBlob
                | mysql::TypeSet => ETString,
                other => EvalType::Unknown(other),
            }
        }
        /// 函数 `GetType`。
        pub fn GetType(&self) -> u8 {
            self.tp
        }
        /// 函数 `SetType`。
        pub fn SetType(&mut self, tp: u8) {
            self.tp = tp;
        }
        /// 函数 `GetFlag`。
        pub fn GetFlag(&self) -> u64 {
            self.flag
        }
        /// 函数 `SetFlag`。
        pub fn SetFlag(&mut self, flag: u64) {
            self.flag = flag;
        }
        /// 函数 `AddFlag`。
        pub fn AddFlag(&mut self, flag: u64) {
            self.flag |= flag;
        }
        /// 函数 `DelFlag`。
        pub fn DelFlag(&mut self, flag: u64) {
            self.flag &= !flag;
        }
        /// 函数 `GetFlen`。
        pub fn GetFlen(&self) -> isize {
            self.flen
        }
        /// 函数 `SetFlen`。
        pub fn SetFlen(&mut self, flen: isize) {
            self.flen = flen;
        }
        /// 函数 `SetFlenUnderLimit`。
        pub fn SetFlenUnderLimit(&mut self, flen: isize) {
            self.flen = flen.min(mysql::MaxBlobWidth);
        }
        /// 函数 `GetDecimal`。
        pub fn GetDecimal(&self) -> isize {
            self.decimal
        }
        /// 函数 `SetDecimal`。
        pub fn SetDecimal(&mut self, decimal: isize) {
            self.decimal = decimal;
        }
        /// 函数 `GetCharset`。
        pub fn GetCharset(&self) -> &str {
            &self.charset
        }
        /// 函数 `SetCharset`。
        pub fn SetCharset(&mut self, value: impl Into<String>) {
            self.charset = value.into();
        }
        /// 函数 `GetCollate`。
        pub fn GetCollate(&self) -> &str {
            &self.collate
        }
        /// 函数 `SetCollate`。
        pub fn SetCollate(&mut self, value: impl Into<String>) {
            self.collate = value.into();
        }
        /// 函数 `GetElems`。
        pub fn GetElems(&self) -> &[String] {
            &self.elems
        }
        /// 函数 `SetElems`。
        pub fn SetElems(&mut self, elems: Vec<String>) {
            self.elems = elems;
        }
        /// 函数 `Equal`。
        pub fn Equal(&self, other: &Self) -> bool {
            self == other
        }
        /// 函数 `MemoryUsage`。
        pub fn MemoryUsage(&self) -> i64 {
            (std::mem::size_of::<Self>()
                + self.charset.len()
                + self.collate.len()
                + self.elems.iter().map(String::len).sum::<usize>()) as i64
        }
    }

    #[derive(Clone, Debug, Default, PartialEq)]
    /// 结构体 `MyDecimal`。
    pub struct MyDecimal(pub i128);
    #[derive(Clone, Debug, Default, PartialEq)]
    /// 结构体 `Time`。
    pub struct Time(pub i64);
    #[derive(Clone, Debug, Default, PartialEq)]
    /// 结构体 `Duration`。
    pub struct Duration(pub i64);
    #[derive(Clone, Debug, Default, PartialEq)]
    /// 结构体 `BinaryJSON`。
    pub struct BinaryJSON(pub String);
    #[derive(Clone, Debug, Default, PartialEq)]
    /// 结构体 `VectorFloat32`。
    pub struct VectorFloat32(pub Vec<f32>);
    #[derive(Clone, Debug, Default, PartialEq)]
    /// 结构体 `Enum`。
    pub struct Enum {
        pub Value: u64,
        pub Name: String,
    }
    #[derive(Clone, Debug, Default, PartialEq)]
    /// 结构体 `Set`。
    pub struct Set {
        pub Value: u64,
        pub Name: String,
    }

    /// 函数 `NewBinaryLiteralFromUint`。
    pub fn NewBinaryLiteralFromUint(value: u64, byte_size: isize) -> Vec<u8> {
        let size = byte_size.max(0) as usize;
        let bytes = value.to_be_bytes();
        if size <= bytes.len() {
            bytes[bytes.len() - size..].to_vec()
        } else {
            let mut result = vec![0; size - bytes.len()];
            result.extend_from_slice(&bytes);
            result
        }
    }

    /// 函数 `ParseEnumValue`。
    pub fn ParseEnumValue(elems: &[String], value: u64) -> Result<Enum, Error> {
        if value == 0 {
            return Ok(Enum::default());
        }
        elems
            .get(value as usize - 1)
            .cloned()
            .map(|Name| Enum { Value: value, Name })
            .ok_or_else(|| Error::new(format!("enum value {value} is out of range")))
    }

    /// 函数 `ParseEnumName`。
    pub fn ParseEnumName(elems: &[String], name: &str, _collation: &str) -> Result<Enum, Error> {
        elems
            .iter()
            .position(|item| item.eq_ignore_ascii_case(name))
            .map(|index| Enum {
                Value: index as u64 + 1,
                Name: elems[index].clone(),
            })
            .ok_or_else(|| Error::new(format!("unknown enum name {name}")))
    }

    /// 函数 `ParseSetName`。
    pub fn ParseSetName(elems: &[String], names: &str, _collation: &str) -> Result<Set, Error> {
        if names.is_empty() {
            return Ok(Set::default());
        }
        let mut value = 0u64;
        for name in names.split(',') {
            let index = elems
                .iter()
                .position(|item| item.eq_ignore_ascii_case(name))
                .ok_or_else(|| Error::new(format!("unknown set name {name}")))?;
            value |= 1 << index;
        }
        Ok(Set {
            Value: value,
            Name: names.to_string(),
        })
    }

    /// 函数 `default_string_field_type`。
    pub fn default_string_field_type() -> FieldType {
        let mut field_type = FieldType::new(mysql::TypeVarString);
        field_type.SetCharset(charset::DEFAULT_CHARSET);
        field_type.SetCollate(charset::DEFAULT_COLLATION);
        field_type
    }
}

/// 行/列 chunk 精简实现，供 builtin 求值与过滤使用。
pub mod chunk {
    use super::types;

    #[derive(Clone, Debug, PartialEq)]
    /// 枚举 `Value`。
    pub enum Value {
        Null,
        Int(i64),
        Uint(u64),
        Float32(f32),
        Float64(f64),
        Bytes(Vec<u8>),
        Decimal(types::MyDecimal),
        Time(types::Time),
        Duration(types::Duration),
        Json(types::BinaryJSON),
        Vector(types::VectorFloat32),
        String(String),
        Enum(types::Enum),
        Set(types::Set),
    }

    #[derive(Clone, Debug, Default)]
    /// 结构体 `Column`。
    pub struct Column {
        values: Vec<Value>,
    }

    impl Column {
        /// 函数 `Values`。
        pub fn Values(&self) -> &[Value] {
            &self.values
        }
        /// 函数 `Clear`。
        pub fn Clear(&mut self) {
            self.values.clear();
        }
        /// 函数 `AppendNull`。
        pub fn AppendNull(&mut self) {
            self.values.push(Value::Null);
        }
        /// 函数 `AppendInt64`。
        pub fn AppendInt64(&mut self, value: i64) {
            self.values.push(Value::Int(value));
        }
        /// 函数 `AppendUint64`。
        pub fn AppendUint64(&mut self, value: u64) {
            self.values.push(Value::Uint(value));
        }
        /// 函数 `AppendFloat32`。
        pub fn AppendFloat32(&mut self, value: f32) {
            self.values.push(Value::Float32(value));
        }
        /// 函数 `AppendFloat64`。
        pub fn AppendFloat64(&mut self, value: f64) {
            self.values.push(Value::Float64(value));
        }
        /// 函数 `AppendBytes`。
        pub fn AppendBytes(&mut self, value: Vec<u8>) {
            self.values.push(Value::Bytes(value));
        }
        /// 函数 `AppendMyDecimal`。
        pub fn AppendMyDecimal(&mut self, value: types::MyDecimal) {
            self.values.push(Value::Decimal(value));
        }
        /// 函数 `AppendTime`。
        pub fn AppendTime(&mut self, value: types::Time) {
            self.values.push(Value::Time(value));
        }
        /// 函数 `AppendDuration`。
        pub fn AppendDuration(&mut self, value: types::Duration) {
            self.values.push(Value::Duration(value));
        }
        /// 函数 `AppendJSON`。
        pub fn AppendJSON(&mut self, value: types::BinaryJSON) {
            self.values.push(Value::Json(value));
        }
        /// 函数 `AppendVectorFloat32`。
        pub fn AppendVectorFloat32(&mut self, value: types::VectorFloat32) {
            self.values.push(Value::Vector(value));
        }
        /// 函数 `AppendString`。
        pub fn AppendString(&mut self, value: String) {
            self.values.push(Value::String(value));
        }
        /// 函数 `AppendEnum`。
        pub fn AppendEnum(&mut self, value: types::Enum) {
            self.values.push(Value::Enum(value));
        }
        /// 函数 `AppendSet`。
        pub fn AppendSet(&mut self, value: types::Set) {
            self.values.push(Value::Set(value));
        }
        /// 函数 `IsNull`。
        pub fn IsNull(&self, index: usize) -> bool {
            matches!(self.values.get(index), Some(Value::Null))
        }
        /// 函数 `GetString`。
        pub fn GetString(&self, index: usize) -> &str {
            match &self.values[index] {
                Value::String(value) => value,
                _ => "",
            }
        }
        /// 函数 `Int64s`。
        pub fn Int64s(&self) -> Vec<i64> {
            self.values
                .iter()
                .map(|value| match value {
                    Value::Int(v) => *v,
                    Value::Uint(v) => *v as i64,
                    _ => 0,
                })
                .collect()
        }
        /// 函数 `Float64s`。
        pub fn Float64s(&self) -> Vec<f64> {
            self.values
                .iter()
                .map(|value| match value {
                    Value::Float64(v) => *v,
                    Value::Float32(v) => *v as f64,
                    _ => 0.0,
                })
                .collect()
        }
        /// 函数 `ReserveBytes`。
        pub fn ReserveBytes(&mut self, size: usize) {
            self.values.reserve(size);
        }
        /// 函数 `ReserveEnum`。
        pub fn ReserveEnum(&mut self, size: usize) {
            self.values.reserve(size);
        }
        /// 函数 `ReserveSet`。
        pub fn ReserveSet(&mut self, size: usize) {
            self.values.reserve(size);
        }
        /// 函数 `ResizeFloat32`。
        pub fn ResizeFloat32(&mut self, size: usize, is_null: bool) {
            self.values.resize(
                size,
                if is_null {
                    Value::Null
                } else {
                    Value::Float32(0.0)
                },
            );
        }
        /// 函数 `SetNull`。
        pub fn SetNull(&mut self, index: usize, is_null: bool) {
            if is_null {
                self.values[index] = Value::Null;
            }
        }
        /// 函数 `SetFloat32`。
        pub fn SetFloat32(&mut self, index: usize, value: f32) {
            self.values[index] = Value::Float32(value);
        }
    }

    #[derive(Clone, Debug, Default)]
    /// 结构体 `Chunk`。
    pub struct Chunk {
        num_rows: usize,
        sel: Option<Vec<usize>>,
        columns: Vec<Column>,
    }

    impl Chunk {
        /// 函数 `with_rows`。
        pub fn with_rows(num_rows: usize) -> Self {
            Self {
                num_rows,
                ..Self::default()
            }
        }
        /// 函数 `with_columns`。
        pub fn with_columns(count: usize) -> Self {
            Self {
                columns: vec![Column::default(); count],
                ..Self::default()
            }
        }
        /// 函数 `NumRows`。
        pub fn NumRows(&self) -> usize {
            self.num_rows
        }
        /// 函数 `Sel`。
        pub fn Sel(&self) -> Option<&[usize]> {
            self.sel.as_deref()
        }
        /// 函数 `SetSel`。
        pub fn SetSel(&mut self, sel: Option<Vec<usize>>) {
            self.sel = sel;
        }
        /// 函数 `Column`。
        pub fn Column(&self, index: usize) -> &Column {
            &self.columns[index]
        }
        /// 函数 `ColumnMut`。
        pub fn ColumnMut(&mut self, index: usize) -> &mut Column {
            &mut self.columns[index]
        }
        /// 函数 `SetCol`。
        pub fn SetCol(&mut self, index: usize, column: Column) {
            self.columns[index] = column;
        }
        /// 函数 `ensure_column`。
        pub fn ensure_column(&mut self, index: usize) {
            if self.columns.len() <= index {
                self.columns.resize_with(index + 1, Column::default);
            }
        }
        /// 函数 `AppendNull`。
        pub fn AppendNull(&mut self, index: usize) {
            self.ensure_column(index);
            self.columns[index].AppendNull();
        }
        /// 函数 `AppendInt64`。
        pub fn AppendInt64(&mut self, index: usize, value: i64) {
            self.ensure_column(index);
            self.columns[index].AppendInt64(value);
        }
        /// 函数 `AppendUint64`。
        pub fn AppendUint64(&mut self, index: usize, value: u64) {
            self.ensure_column(index);
            self.columns[index].AppendUint64(value);
        }
        /// 函数 `AppendFloat32`。
        pub fn AppendFloat32(&mut self, index: usize, value: f32) {
            self.ensure_column(index);
            self.columns[index].AppendFloat32(value);
        }
        /// 函数 `AppendFloat64`。
        pub fn AppendFloat64(&mut self, index: usize, value: f64) {
            self.ensure_column(index);
            self.columns[index].AppendFloat64(value);
        }
        /// 函数 `AppendBytes`。
        pub fn AppendBytes(&mut self, index: usize, value: Vec<u8>) {
            self.ensure_column(index);
            self.columns[index].AppendBytes(value);
        }
        /// 函数 `AppendMyDecimal`。
        pub fn AppendMyDecimal(&mut self, index: usize, value: types::MyDecimal) {
            self.ensure_column(index);
            self.columns[index].AppendMyDecimal(value);
        }
        /// 函数 `AppendTime`。
        pub fn AppendTime(&mut self, index: usize, value: types::Time) {
            self.ensure_column(index);
            self.columns[index].AppendTime(value);
        }
        /// 函数 `AppendDuration`。
        pub fn AppendDuration(&mut self, index: usize, value: types::Duration) {
            self.ensure_column(index);
            self.columns[index].AppendDuration(value);
        }
        /// 函数 `AppendJSON`。
        pub fn AppendJSON(&mut self, index: usize, value: types::BinaryJSON) {
            self.ensure_column(index);
            self.columns[index].AppendJSON(value);
        }
        /// 函数 `AppendVectorFloat32`。
        pub fn AppendVectorFloat32(&mut self, index: usize, value: types::VectorFloat32) {
            self.ensure_column(index);
            self.columns[index].AppendVectorFloat32(value);
        }
        /// 函数 `AppendString`。
        pub fn AppendString(&mut self, index: usize, value: String) {
            self.ensure_column(index);
            self.columns[index].AppendString(value);
        }
        /// 函数 `AppendEnum`。
        pub fn AppendEnum(&mut self, index: usize, value: types::Enum) {
            self.ensure_column(index);
            self.columns[index].AppendEnum(value);
        }
        /// 函数 `AppendSet`。
        pub fn AppendSet(&mut self, index: usize, value: types::Set) {
            self.ensure_column(index);
            self.columns[index].AppendSet(value);
        }
    }

    /// 函数 `NewColumn`。
    pub fn NewColumn(_field_type: &types::FieldType, capacity: usize) -> Column {
        Column {
            values: Vec::with_capacity(capacity),
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    /// 结构体 `Row`。
    pub struct Row {
        pub(crate) index: usize,
    }
    impl Row {
        /// 函数 `Idx`。
        pub fn Idx(&self) -> usize {
            self.index
        }
    }

    /// 结构体 `Iterator4Chunk`。
    pub struct Iterator4Chunk {
        chunk: *mut Chunk,
        position: usize,
    }

    impl Iterator4Chunk {
        /// 函数 `GetChunk`。
        pub fn GetChunk(&self) -> &Chunk {
            unsafe { &*self.chunk }
        }
        /// 函数 `GetChunkMut`。
        pub fn GetChunkMut(&mut self) -> &mut Chunk {
            unsafe { &mut *self.chunk }
        }
        /// 函数 `Len`。
        pub fn Len(&self) -> usize {
            self.GetChunk().NumRows()
        }
        /// 函数 `Begin`。
        pub fn Begin(&mut self) -> Row {
            self.position = 0;
            self.current()
        }
        /// 函数 `End`。
        pub fn End(&self) -> Row {
            Row { index: usize::MAX }
        }
        /// 函数 `Next`。
        pub fn Next(&mut self) -> Row {
            self.position += 1;
            self.current()
        }
        fn current(&self) -> Row {
            let chunk = self.GetChunk();
            if let Some(sel) = chunk.Sel() {
                sel.get(self.position)
                    .copied()
                    .map_or(self.End(), |index| Row { index })
            } else if self.position < chunk.NumRows() {
                Row {
                    index: self.position,
                }
            } else {
                self.End()
            }
        }
    }

    /// 函数 `NewIterator4Chunk`。
    pub fn NewIterator4Chunk(chunk: &mut Chunk) -> Iterator4Chunk {
        Iterator4Chunk { chunk, position: 0 }
    }
}

/// 求值上下文 trait：提供会话侧依赖。
pub trait EvalContext: Send + Sync {
    fn CtxID(&self) -> u64;
}

#[derive(Clone, Debug)]
/// 仅含 ID 的简单求值上下文，用于测试与默认路径。
pub struct SimpleEvalContext {
    id: u64,
}
impl SimpleEvalContext {
    /// 由消息构造错误。
    pub fn new(id: u64) -> Self {
        Self { id }
    }
}
impl EvalContext for SimpleEvalContext {
    fn CtxID(&self) -> u64 {
        self.id
    }
}

/// 对象安全的表达式 clone 辅助 trait。
pub trait ExpressionClone {
    fn clone_box(&self) -> Box<dyn Expression>;
}
impl<T> ExpressionClone for T
where
    T: 'static + Expression + Clone,
{
    fn clone_box(&self) -> Box<dyn Expression> {
        Box::new(self.clone())
    }
}
impl Clone for Box<dyn Expression> {
    fn clone(&self) -> Self {
        self.clone_box()
    }
}

/// 表达式节点 trait：类型、向量化标记、以及按类型求值（EvalInt/EvalReal/...）。
pub trait Expression: ExpressionClone + Send + Sync {
    fn as_any(&self) -> &dyn Any;
    fn GetType(&self, ctx: &dyn EvalContext) -> &types::FieldType;
    fn SafeToShareAcrossSession(&self) -> bool {
        true
    }
    fn Vectorized(&self) -> bool {
        false
    }
    fn Equal(&self, _ctx: &dyn EvalContext, _other: &dyn Expression) -> bool {
        false
    }
    fn MemoryUsage(&self) -> i64 {
        0
    }
    fn EvalInt(&self, _ctx: &dyn EvalContext, _row: chunk::Row) -> (i64, bool, Option<Error>) {
        (0, false, Some(Error::new("EvalInt is not implemented")))
    }
    fn EvalReal(&self, _ctx: &dyn EvalContext, _row: chunk::Row) -> (f64, bool, Option<Error>) {
        (0.0, false, Some(Error::new("EvalReal is not implemented")))
    }
    fn EvalDecimal(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> (types::MyDecimal, bool, Option<Error>) {
        (
            types::MyDecimal::default(),
            false,
            Some(Error::new("EvalDecimal is not implemented")),
        )
    }
    fn EvalTime(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> (types::Time, bool, Option<Error>) {
        (
            types::Time::default(),
            false,
            Some(Error::new("EvalTime is not implemented")),
        )
    }
    fn EvalDuration(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> (types::Duration, bool, Option<Error>) {
        (
            types::Duration::default(),
            false,
            Some(Error::new("EvalDuration is not implemented")),
        )
    }
    fn EvalJSON(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> (types::BinaryJSON, bool, Option<Error>) {
        (
            types::BinaryJSON::default(),
            false,
            Some(Error::new("EvalJSON is not implemented")),
        )
    }
    fn EvalVectorFloat32(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> (types::VectorFloat32, bool, Option<Error>) {
        (
            types::VectorFloat32::default(),
            false,
            Some(Error::new("EvalVectorFloat32 is not implemented")),
        )
    }
    fn EvalString(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> (String, bool, Option<Error>) {
        (
            String::new(),
            false,
            Some(Error::new("EvalString is not implemented")),
        )
    }
    fn VecEvalInt(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        result.Clear();
        for index in 0..input.NumRows() {
            let (value, is_null, err) = self.EvalInt(ctx, chunk::Row { index });
            if let Some(err) = err {
                return Err(err);
            }
            if is_null {
                result.AppendNull();
            } else {
                result.AppendInt64(value);
            }
        }
        Ok(())
    }
    fn VecEvalReal(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        result.Clear();
        for index in 0..input.NumRows() {
            let (value, is_null, err) = self.EvalReal(ctx, chunk::Row { index });
            if let Some(err) = err {
                return Err(err);
            }
            if is_null {
                result.AppendNull();
            } else {
                result.AppendFloat64(value);
            }
        }
        Ok(())
    }
    fn VecEvalDecimal(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        result.Clear();
        for index in 0..input.NumRows() {
            let (v, n, e) = self.EvalDecimal(ctx, chunk::Row { index });
            if let Some(e) = e {
                return Err(e);
            }
            if n {
                result.AppendNull();
            } else {
                result.AppendMyDecimal(v);
            }
        }
        Ok(())
    }
    fn VecEvalTime(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        result.Clear();
        for index in 0..input.NumRows() {
            let (v, n, e) = self.EvalTime(ctx, chunk::Row { index });
            if let Some(e) = e {
                return Err(e);
            }
            if n {
                result.AppendNull();
            } else {
                result.AppendTime(v);
            }
        }
        Ok(())
    }
    fn VecEvalDuration(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        result.Clear();
        for index in 0..input.NumRows() {
            let (v, n, e) = self.EvalDuration(ctx, chunk::Row { index });
            if let Some(e) = e {
                return Err(e);
            }
            if n {
                result.AppendNull();
            } else {
                result.AppendDuration(v);
            }
        }
        Ok(())
    }
    fn VecEvalJSON(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        result.Clear();
        for index in 0..input.NumRows() {
            let (v, n, e) = self.EvalJSON(ctx, chunk::Row { index });
            if let Some(e) = e {
                return Err(e);
            }
            if n {
                result.AppendNull();
            } else {
                result.AppendJSON(v);
            }
        }
        Ok(())
    }
    fn VecEvalVectorFloat32(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        result.Clear();
        for index in 0..input.NumRows() {
            let (v, n, e) = self.EvalVectorFloat32(ctx, chunk::Row { index });
            if let Some(e) = e {
                return Err(e);
            }
            if n {
                result.AppendNull();
            } else {
                result.AppendVectorFloat32(v);
            }
        }
        Ok(())
    }
    fn VecEvalString(
        &self,
        ctx: &dyn EvalContext,
        input: &chunk::Chunk,
        result: &mut chunk::Column,
    ) -> Result<(), Error> {
        result.Clear();
        for index in 0..input.NumRows() {
            let (v, n, e) = self.EvalString(ctx, chunk::Row { index });
            if let Some(e) = e {
                return Err(e);
            }
            if n {
                result.AppendNull();
            } else {
                result.AppendString(v);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
/// 大小写不敏感字符串（CIStr），用于标识符比较。
pub struct CIStr {
    pub L: String,
}
impl CIStr {
    /// 由消息构造错误。
    pub fn new(value: &str) -> Self {
        Self {
            L: value.to_ascii_lowercase(),
        }
    }
}

#[derive(Clone)]
/// 标量函数表达式：函数名、参数列表与返回类型。
pub struct ScalarFunction {
    pub FuncName: CIStr,
    args: Vec<Box<dyn Expression>>,
    field_type: types::FieldType,
}

impl ScalarFunction {
    /// 由消息构造错误。
    pub fn new(name: &str, args: Vec<Box<dyn Expression>>, field_type: types::FieldType) -> Self {
        Self {
            FuncName: CIStr::new(name),
            args,
            field_type,
        }
    }
    /// 函数 `GetArgs`。
    pub fn GetArgs(&self) -> &[Box<dyn Expression>] {
        &self.args
    }
}

impl Expression for ScalarFunction {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn GetType(&self, _ctx: &dyn EvalContext) -> &types::FieldType {
        &self.field_type
    }
    fn SafeToShareAcrossSession(&self) -> bool {
        self.args
            .iter()
            .all(|argument| argument.SafeToShareAcrossSession())
    }
    fn Vectorized(&self) -> bool {
        self.args.iter().all(|arg| arg.Vectorized())
    }
    fn VecEvalDecimal(
        &self,
        _ctx: &dyn EvalContext,
        _input: &chunk::Chunk,
        _result: &mut chunk::Column,
    ) -> Result<(), Error> {
        Err(Error::new("VecEvalDecimal is not implemented"))
    }
    fn VecEvalTime(
        &self,
        _ctx: &dyn EvalContext,
        _input: &chunk::Chunk,
        _result: &mut chunk::Column,
    ) -> Result<(), Error> {
        Err(Error::new("VecEvalTime is not implemented"))
    }
    fn VecEvalDuration(
        &self,
        _ctx: &dyn EvalContext,
        _input: &chunk::Chunk,
        _result: &mut chunk::Column,
    ) -> Result<(), Error> {
        Err(Error::new("VecEvalDuration is not implemented"))
    }
    fn VecEvalJSON(
        &self,
        _ctx: &dyn EvalContext,
        _input: &chunk::Chunk,
        _result: &mut chunk::Column,
    ) -> Result<(), Error> {
        Err(Error::new("VecEvalJSON is not implemented"))
    }
    fn VecEvalVectorFloat32(
        &self,
        _ctx: &dyn EvalContext,
        _input: &chunk::Chunk,
        _result: &mut chunk::Column,
    ) -> Result<(), Error> {
        Err(Error::new("VecEvalVectorFloat32 is not implemented"))
    }
    fn VecEvalString(
        &self,
        _ctx: &dyn EvalContext,
        _input: &chunk::Chunk,
        _result: &mut chunk::Column,
    ) -> Result<(), Error> {
        Err(Error::new("VecEvalString is not implemented"))
    }
}

#[derive(Clone, Debug)]
/// 表达式排序规则信息（字符集、collation、可强制转换性）。
pub struct ExprCollation {
    pub Charset: String,
    pub Collation: String,
}

impl Default for ExprCollation {
    fn default() -> Self {
        Self {
            Charset: charset::DEFAULT_CHARSET.into(),
            Collation: charset::DEFAULT_COLLATION.into(),
        }
    }
}

/// 构建内置函数签名时的上下文（可取 EvalContext）。
pub trait BuildContext {
    fn GetEvalCtx(&self) -> &dyn EvalContext;
}

/// 简单构建上下文。
pub struct SimpleBuildContext {
    eval_ctx: SimpleEvalContext,
}

impl SimpleBuildContext {
    /// 由消息构造错误。
    pub fn new(ctx_id: u64) -> Self {
        Self {
            eval_ctx: SimpleEvalContext::new(ctx_id),
        }
    }
}

impl BuildContext for SimpleBuildContext {
    fn GetEvalCtx(&self) -> &dyn EvalContext {
        &self.eval_ctx
    }
}

#[derive(Clone)]
/// 在参数上包一层 CAST 的内部表达式包装器。
struct CastExpression {
    inner: Box<dyn Expression>,
    target: types::FieldType,
}

impl CastExpression {
    /// 由消息构造错误。
    fn new(inner: Box<dyn Expression>, target: types::FieldType) -> Self {
        Self { inner, target }
    }
}

impl Expression for CastExpression {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn GetType(&self, _ctx: &dyn EvalContext) -> &types::FieldType {
        &self.target
    }

    fn SafeToShareAcrossSession(&self) -> bool {
        self.inner.SafeToShareAcrossSession()
    }

    fn Vectorized(&self) -> bool {
        self.inner.Vectorized()
    }

    fn EvalInt(&self, ctx: &dyn EvalContext, row: chunk::Row) -> (i64, bool, Option<Error>) {
        match self.inner.GetType(ctx).EvalType() {
            types::ETInt => self.inner.EvalInt(ctx, row),
            types::ETReal => {
                let (value, is_null, err) = self.inner.EvalReal(ctx, row);
                (value as i64, is_null, err)
            }
            types::ETString => {
                let (value, is_null, err) = self.inner.EvalString(ctx, row);
                if err.is_some() || is_null {
                    return (0, is_null, err);
                }
                match value.trim().parse::<i64>() {
                    Ok(value) => (value, false, None),
                    Err(err) => (0, false, Some(Error::new(err.to_string()))),
                }
            }
            other => (
                0,
                false,
                Some(Error::new(format!("cannot cast {other:?} as integer"))),
            ),
        }
    }

    fn EvalReal(&self, ctx: &dyn EvalContext, row: chunk::Row) -> (f64, bool, Option<Error>) {
        match self.inner.GetType(ctx).EvalType() {
            types::ETReal => self.inner.EvalReal(ctx, row),
            types::ETInt => {
                let (value, is_null, err) = self.inner.EvalInt(ctx, row);
                (value as f64, is_null, err)
            }
            types::ETString => {
                let (value, is_null, err) = self.inner.EvalString(ctx, row);
                if err.is_some() || is_null {
                    return (0.0, is_null, err);
                }
                match value.trim().parse::<f64>() {
                    Ok(value) => (value, false, None),
                    Err(err) => (0.0, false, Some(Error::new(err.to_string()))),
                }
            }
            other => (
                0.0,
                false,
                Some(Error::new(format!("cannot cast {other:?} as real"))),
            ),
        }
    }

    fn EvalString(&self, ctx: &dyn EvalContext, row: chunk::Row) -> (String, bool, Option<Error>) {
        match self.inner.GetType(ctx).EvalType() {
            types::ETString => self.inner.EvalString(ctx, row),
            types::ETInt => {
                let (value, is_null, err) = self.inner.EvalInt(ctx, row);
                (value.to_string(), is_null, err)
            }
            types::ETReal => {
                let (value, is_null, err) = self.inner.EvalReal(ctx, row);
                (value.to_string(), is_null, err)
            }
            other => (
                String::new(),
                false,
                Some(Error::new(format!("cannot cast {other:?} as string"))),
            ),
        }
    }

    fn EvalDecimal(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> (types::MyDecimal, bool, Option<Error>) {
        self.inner.EvalDecimal(ctx, row)
    }
    fn EvalTime(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> (types::Time, bool, Option<Error>) {
        self.inner.EvalTime(ctx, row)
    }
    fn EvalDuration(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> (types::Duration, bool, Option<Error>) {
        self.inner.EvalDuration(ctx, row)
    }
    fn EvalJSON(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> (types::BinaryJSON, bool, Option<Error>) {
        self.inner.EvalJSON(ctx, row)
    }
    fn EvalVectorFloat32(
        &self,
        ctx: &dyn EvalContext,
        row: chunk::Row,
    ) -> (types::VectorFloat32, bool, Option<Error>) {
        self.inner.EvalVectorFloat32(ctx, row)
    }
}

/// 内置函数签名基类：参数、返回类型、常量折叠提示与缓冲列等共享状态。
pub struct baseBuiltinFunc {
    pub(crate) args: Vec<Box<dyn Expression>>,
    pub(crate) safeToShareAcrossSessionFlag: AtomicU32,
    pub(crate) generatedSignature: Option<&'static str>,
    tp: types::FieldType,
    pbCode: i32,
    collator: String,
    childrenVectorized: OnceLock<bool>,
    charset: String,
    collation: String,
}

impl baseBuiltinFunc {
    /// 由消息构造错误。
    pub fn new(args: Vec<Box<dyn Expression>>, tp: types::FieldType) -> Self {
        let charset = tp.GetCharset().to_string();
        let collation = tp.GetCollate().to_string();
        Self {
            args,
            safeToShareAcrossSessionFlag: AtomicU32::new(0),
            generatedSignature: None,
            tp,
            pbCode: 0,
            collator: collation.clone(),
            childrenVectorized: OnceLock::new(),
            charset,
            collation,
        }
    }
    /// 函数 `SafeToShareAcrossSession`。
    pub fn SafeToShareAcrossSession(&self) -> bool {
        self.GeneratedSafeToShareAcrossSession()
    }
    /// 函数 `SetGeneratedSignature`。
    pub fn SetGeneratedSignature(&mut self, signature: &'static str) -> Result<(), Error> {
        if crate::builtin_threadsafe_generated_kernel::GeneratedThreadSafetyPolicyForSignature(
            signature,
        )
        .is_none()
        {
            return Err(Error::new(format!(
                "unknown generated builtin signature '{signature}'"
            )));
        }
        self.generatedSignature = Some(signature);
        self.safeToShareAcrossSessionFlag
            .store(0, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
    /// 函数 `PbCode`。
    pub fn PbCode(&self) -> i32 {
        self.pbCode
    }
    /// 函数 `RequiredOptionalEvalProps`。
    pub fn RequiredOptionalEvalProps(&self) -> Vec<String> {
        Vec::new()
    }
    /// 函数 `metadata`。
    pub fn metadata(&self) -> Option<Vec<u8>> {
        None
    }
    /// 函数 `setPbCode`。
    pub fn setPbCode(&mut self, code: i32) {
        self.pbCode = code;
    }
    /// 函数 `setCollator`。
    pub fn setCollator(&mut self, collator: impl Into<String>) {
        self.collator = collator.into();
    }
    /// 函数 `collator`。
    pub fn collator(&self) -> &str {
        &self.collator
    }
    /// 函数 `getArgs`。
    pub fn getArgs(&self) -> &[Box<dyn Expression>] {
        &self.args
    }
    /// 函数 `vectorized`。
    pub fn vectorized(&self) -> bool {
        false
    }
    /// 函数 `isChildrenVectorized`。
    pub fn isChildrenVectorized(&self) -> bool {
        *self
            .childrenVectorized
            .get_or_init(|| self.args.iter().all(|arg| arg.Vectorized()))
    }
    /// 函数 `getRetTp`。
    pub fn getRetTp(&mut self) -> &types::FieldType {
        if self.tp.EvalType() == types::ETString {
            if self.tp.GetFlen() >= mysql::MaxBlobWidth {
                self.tp.SetType(mysql::TypeLongBlob);
            } else if self.tp.GetFlen() >= 65_536 {
                self.tp.SetType(mysql::TypeMediumBlob);
            }
            if self.tp.GetCharset().is_empty() {
                self.tp.SetCharset(charset::DEFAULT_CHARSET);
                self.tp.SetCollate(charset::DEFAULT_COLLATION);
            }
        }
        &self.tp
    }
    /// 函数 `equal`。
    pub fn equal(&self, ctx: &dyn EvalContext, other: &baseBuiltinFunc) -> bool {
        self.args.len() == other.args.len()
            && self
                .args
                .iter()
                .zip(&other.args)
                .all(|(left, right)| left.Equal(ctx, right.as_ref()))
    }
    /// 函数 `cloneFrom`。
    pub fn cloneFrom(&mut self, from: &baseBuiltinFunc) {
        self.args = from.args.clone();
        self.tp = from.tp.clone();
        self.pbCode = from.pbCode;
        self.collator = from.collator.clone();
        self.childrenVectorized = OnceLock::new();
        self.charset = from.charset.clone();
        self.collation = from.collation.clone();
    }
    /// 函数 `setDecimalAndFlenForDatetime`。
    pub fn setDecimalAndFlenForDatetime(&mut self, fsp: isize) {
        self.tp.SetDecimal(fsp);
        self.tp.SetFlen(
            mysql::MaxDatetimeWidthWithFsp - types::MaxFsp + if fsp > 0 { fsp + 1 } else { 0 },
        );
    }
    /// 函数 `setDecimalAndFlenForDate`。
    pub fn setDecimalAndFlenForDate(&mut self) {
        self.tp.SetDecimal(0);
        self.tp.SetFlen(10);
    }
    /// 函数 `setDecimalAndFlenForTime`。
    pub fn setDecimalAndFlenForTime(&mut self, fsp: isize) {
        self.tp.SetDecimal(fsp);
        self.tp.SetFlen(
            mysql::MaxDurationWidthWithFsp - types::MaxFsp + if fsp > 0 { fsp + 1 } else { 0 },
        );
    }
    /// 函数 `MemoryUsage`。
    pub fn MemoryUsage(&self) -> i64 {
        std::mem::size_of::<Self>() as i64
            + self.charset.len() as i64
            + self.collation.len() as i64
            + self.tp.MemoryUsage()
            + self.args.iter().map(|arg| arg.MemoryUsage()).sum::<i64>()
    }
}

impl Clone for baseBuiltinFunc {
    fn clone(&self) -> Self {
        Self {
            args: self.args.clone(),
            safeToShareAcrossSessionFlag: AtomicU32::new(
                self.safeToShareAcrossSessionFlag
                    .load(std::sync::atomic::Ordering::SeqCst),
            ),
            generatedSignature: self.generatedSignature,
            tp: self.tp.clone(),
            pbCode: self.pbCode,
            collator: self.collator.clone(),
            childrenVectorized: OnceLock::new(),
            charset: self.charset.clone(),
            collation: self.collation.clone(),
        }
    }
}

impl Expression for baseBuiltinFunc {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn GetType(&self, _ctx: &dyn EvalContext) -> &types::FieldType {
        &self.tp
    }

    fn SafeToShareAcrossSession(&self) -> bool {
        baseBuiltinFunc::SafeToShareAcrossSession(self)
    }

    fn Vectorized(&self) -> bool {
        false
    }

    fn EvalInt(&self, _ctx: &dyn EvalContext, _row: chunk::Row) -> (i64, bool, Option<Error>) {
        (0, false, Some(base_builtin_error("evalInt")))
    }
    fn EvalReal(&self, _ctx: &dyn EvalContext, _row: chunk::Row) -> (f64, bool, Option<Error>) {
        (0.0, false, Some(base_builtin_error("evalReal")))
    }
    fn EvalString(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> (String, bool, Option<Error>) {
        (String::new(), false, Some(base_builtin_error("evalString")))
    }
    fn EvalDecimal(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> (types::MyDecimal, bool, Option<Error>) {
        (
            types::MyDecimal::default(),
            false,
            Some(base_builtin_error("evalDecimal")),
        )
    }
    fn EvalTime(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> (types::Time, bool, Option<Error>) {
        (
            types::Time::default(),
            false,
            Some(base_builtin_error("evalTime")),
        )
    }
    fn EvalDuration(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> (types::Duration, bool, Option<Error>) {
        (
            types::Duration::default(),
            false,
            Some(base_builtin_error("evalDuration")),
        )
    }
    fn EvalJSON(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> (types::BinaryJSON, bool, Option<Error>) {
        (
            types::BinaryJSON::default(),
            false,
            Some(base_builtin_error("evalJSON")),
        )
    }
    fn EvalVectorFloat32(
        &self,
        _ctx: &dyn EvalContext,
        _row: chunk::Row,
    ) -> (types::VectorFloat32, bool, Option<Error>) {
        (
            types::VectorFloat32::default(),
            false,
            Some(base_builtin_error("evalVectorFloat32")),
        )
    }
}

/// 构造“基类方法未实现”类错误。
fn base_builtin_error(method: &str) -> Error {
    Error::new(format!(
        "baseBuiltinFunc.{method}() should never be called, please contact the TiDB team for help"
    ))
}

/// 由参数推导结果排序规则。
fn deriveCollation(
    ctx: &dyn EvalContext,
    args: &[Box<dyn Expression>],
    ret_type: types::EvalType,
) -> ExprCollation {
    if ret_type == types::ETString {
        for arg in args {
            let field_type = arg.GetType(ctx);
            if !field_type.GetCharset().is_empty() {
                return ExprCollation {
                    Charset: field_type.GetCharset().to_string(),
                    Collation: field_type.GetCollate().to_string(),
                };
            }
        }
    }
    ExprCollation::default()
}

/// 构造默认返回类型推导的 `baseBuiltinFunc`。
pub fn newBaseBuiltinFunc(
    ctx: Option<&dyn BuildContext>,
    func_name: &str,
    args: Vec<Box<dyn Expression>>,
    mut tp: types::FieldType,
) -> Result<baseBuiltinFunc, Error> {
    let ctx = ctx.ok_or_else(|| Error::new("unexpected nil session ctx"))?;
    let ec = deriveCollation(ctx.GetEvalCtx(), &args, tp.EvalType());
    tp.SetCharset(&ec.Charset);
    tp.SetCollate(&ec.Collation);
    let mut bf = baseBuiltinFunc::new(args, tp);
    bf.setCollator(&ec.Collation);
    let args = bf.args.clone();
    adjustNullFlagForReturnType(ctx.GetEvalCtx(), func_name, &args, &mut bf);
    Ok(bf)
}

/// 按 EvalType 与 collation 生成字段类型。
fn field_type_for_eval(eval_type: types::EvalType, ec: &ExprCollation) -> types::FieldType {
    newReturnFieldTypeForBaseBuiltinFunc("", eval_type, ec)
}

/// 按显式求值类型构造签名，并对参数做 CAST。
// 按显式 EvalType 构造签名：参数不足类型时自动包 CAST，并调整返回可空标志。
pub fn newBaseBuiltinFuncWithTp(
    ctx: Option<&dyn BuildContext>,
    func_name: &str,
    args: Vec<Box<dyn Expression>>,
    ret_type: types::EvalType,
    arg_types: &[types::EvalType],
) -> Result<baseBuiltinFunc, Error> {
    if args.len() != arg_types.len() {
        return Err(Error::new("unexpected length of args and argTps"));
    }
    let ctx = ctx.ok_or_else(|| Error::new("unexpected nil session ctx"))?;
    let ec = deriveCollation(ctx.GetEvalCtx(), &args, ret_type);
    let cast_args = args
        .into_iter()
        .zip(arg_types)
        .map(|(arg, eval_type)| {
            Box::new(CastExpression::new(
                arg,
                field_type_for_eval(*eval_type, &ec),
            )) as Box<dyn Expression>
        })
        .collect();
    let mut bf = baseBuiltinFunc::new(
        cast_args,
        newReturnFieldTypeForBaseBuiltinFunc(func_name, ret_type, &ec),
    );
    bf.setCollator(&ec.Collation);
    let args = bf.args.clone();
    adjustNullFlagForReturnType(ctx.GetEvalCtx(), func_name, &args, &mut bf);
    Ok(bf)
}

/// 按显式 FieldType 列表构造签名。
pub fn newBaseBuiltinFuncWithFieldTypes(
    ctx: Option<&dyn BuildContext>,
    func_name: &str,
    args: Vec<Box<dyn Expression>>,
    ret_type: types::EvalType,
    arg_types: &[types::FieldType],
) -> Result<baseBuiltinFunc, Error> {
    if args.len() != arg_types.len() {
        return Err(Error::new("unexpected length of args and argTps"));
    }
    let ctx = ctx.ok_or_else(|| Error::new("unexpected nil session ctx"))?;
    let ec = deriveCollation(ctx.GetEvalCtx(), &args, ret_type);
    let cast_args = args
        .into_iter()
        .zip(arg_types)
        .map(|(arg, field_type)| {
            if arg.GetType(ctx.GetEvalCtx()).Equal(field_type) {
                arg
            } else {
                Box::new(CastExpression::new(arg, field_type.clone())) as Box<dyn Expression>
            }
        })
        .collect();
    let mut bf = baseBuiltinFunc::new(
        cast_args,
        newReturnFieldTypeForBaseBuiltinFunc(func_name, ret_type, &ec),
    );
    bf.setCollator(&ec.Collation);
    let args = bf.args.clone();
    adjustNullFlagForReturnType(ctx.GetEvalCtx(), func_name, &args, &mut bf);
    Ok(bf)
}

/// 单返回 FieldType 的构造快捷方式。
pub fn newBaseBuiltinFuncWithFieldType(
    tp: types::FieldType,
    args: Vec<Box<dyn Expression>>,
) -> Result<baseBuiltinFunc, Error> {
    Ok(baseBuiltinFunc::new(args, tp))
}

/// 按函数名与参数可空性调整返回类型的 NOT NULL 标志。
// 按函数名集合与参数 NOT NULL 标志推导返回值可空性（恒非空/恒可空/随参数）。
pub fn adjustNullFlagForReturnType(
    ctx: &dyn EvalContext,
    func_name: &str,
    args: &[Box<dyn Expression>],
    bf: &mut baseBuiltinFunc,
) {
    if ALWAYS_NOT_NULL.contains(&func_name) {
        bf.tp.AddFlag(mysql::NotNullFlag);
    } else if ALWAYS_NULLABLE.contains(&func_name) {
        bf.tp.DelFlag(mysql::NotNullFlag);
    } else if NOT_NULL_ON_NOT_NULL.contains(&func_name) {
        if args
            .iter()
            .all(|arg| mysql::HasNotNullFlag(arg.GetType(ctx).GetFlag()))
        {
            bf.tp.AddFlag(mysql::NotNullFlag);
        } else {
            bf.tp.DelFlag(mysql::NotNullFlag);
        }
    }
}

/// 结果恒非 NULL 的函数名集合。
const ALWAYS_NOT_NULL: &[&str] = &["rand", "uuid", "connection_id", "row_count"];
/// 结果恒可空的函数名集合。
const ALWAYS_NULLABLE: &[&str] = &["nullif", "from_unixtime", "json_extract"];
/// 当所有参数非 NULL 时结果非 NULL 的函数名集合。
const NOT_NULL_ON_NOT_NULL: &[&str] = &[ast::Plus, ast::Minus, ast::Mul, ast::Div, "abs", "concat"];

/// 为基类函数生成默认返回 FieldType。
pub fn newReturnFieldTypeForBaseBuiltinFunc(
    func_name: &str,
    ret_type: types::EvalType,
    ec: &ExprCollation,
) -> types::FieldType {
    let mut field_type = match ret_type {
        types::ETInt => {
            let mut ft = types::FieldType::new(mysql::TypeLonglong);
            ft.SetFlag(mysql::BinaryFlag);
            ft.SetFlen(mysql::MaxIntWidth);
            ft
        }
        types::ETReal => {
            let mut ft = types::FieldType::new(mysql::TypeDouble);
            ft.SetFlag(mysql::BinaryFlag);
            ft.SetFlen(mysql::MaxRealWidth);
            ft
        }
        types::ETDecimal => {
            let mut ft = types::FieldType::new(mysql::TypeNewDecimal);
            ft.SetFlag(mysql::BinaryFlag);
            ft.SetFlen(11);
            ft
        }
        types::ETString => {
            let mut ft = types::FieldType::new(mysql::TypeVarString);
            ft.SetFlen(types::UnspecifiedLength);
            ft.SetDecimal(types::UnspecifiedLength);
            ft.SetCharset(&ec.Charset);
            ft.SetCollate(&ec.Collation);
            ft
        }
        types::ETDatetime => {
            let mut ft = types::FieldType::new(mysql::TypeDatetime);
            ft.SetFlag(mysql::BinaryFlag);
            ft.SetFlen(mysql::MaxDatetimeWidthWithFsp);
            ft.SetDecimal(types::MaxFsp);
            ft
        }
        types::ETTimestamp => {
            let mut ft = types::FieldType::new(mysql::TypeTimestamp);
            ft.SetFlag(mysql::BinaryFlag);
            ft.SetFlen(mysql::MaxDatetimeWidthWithFsp);
            ft.SetDecimal(types::MaxFsp);
            ft
        }
        types::ETDuration => {
            let mut ft = types::FieldType::new(mysql::TypeDuration);
            ft.SetFlag(mysql::BinaryFlag);
            ft.SetFlen(mysql::MaxDurationWidthWithFsp);
            ft.SetDecimal(types::MaxFsp);
            ft
        }
        types::ETJson => {
            let mut ft = types::FieldType::new(mysql::TypeJSON);
            ft.SetFlag(mysql::BinaryFlag);
            ft.SetFlen(mysql::MaxBlobWidth);
            ft.SetCharset(mysql::DefaultCharset);
            ft.SetCollate(mysql::DefaultCollationName);
            ft
        }
        types::ETVectorFloat32 => {
            let mut ft = types::FieldType::new(mysql::TypeTiDBVectorFloat32);
            ft.SetFlag(mysql::BinaryFlag);
            ft.SetFlen(types::UnspecifiedLength);
            ft
        }
        types::EvalType::Unknown(tp) => types::FieldType::new(tp),
    };
    if mysql::HasBinaryFlag(field_type.GetFlag()) && field_type.GetType() != mysql::TypeJSON {
        field_type.SetCharset(charset::CharsetBin);
        field_type.SetCollate(charset::CollationBin);
    }
    if BOOLEAN_FUNCTIONS.contains(&func_name) {
        field_type.AddFlag(mysql::IsBooleanFlag);
    }
    field_type
}

/// 结果带布尔语义标志的函数名集合。
const BOOLEAN_FUNCTIONS: &[&str] = &[
    ast::EQ,
    ast::NullEQ,
    ast::NE,
    ast::LT,
    ast::LE,
    ast::GT,
    ast::GE,
    ast::IsTruthWithoutNull,
    ast::IsTruthWithNull,
    ast::IsFalsity,
    ast::IsFalsityWithNull,
    "and",
    "or",
    "xor",
    "not",
    "in",
    "like",
    "regexp",
    "isnull",
];

/// CAST 类内置函数基类，含 `in_union` 等标志。
pub struct baseBuiltinCastFunc {
    pub baseBuiltinFunc: baseBuiltinFunc,
    pub inUnion: bool,
}
impl baseBuiltinCastFunc {
    /// 由消息构造错误。
    pub fn new(base: baseBuiltinFunc, in_union: bool) -> Self {
        Self {
            baseBuiltinFunc: base,
            inUnion: in_union,
        }
    }
    /// 函数 `metadata`。
    pub fn metadata(&self) -> bool {
        self.inUnion
    }
    /// 函数 `cloneFrom`。
    pub fn cloneFrom(&mut self, from: &baseBuiltinCastFunc) {
        self.baseBuiltinFunc.cloneFrom(&from.baseBuiltinFunc);
        self.inUnion = from.inUnion;
    }
}

/// 构造 CAST 基类签名。
pub fn newBaseBuiltinCastFunc(
    builtin_func: baseBuiltinFunc,
    in_union: bool,
) -> baseBuiltinCastFunc {
    baseBuiltinCastFunc::new(builtin_func, in_union)
}

/// 构造面向字符串的 CAST 基类签名。
pub fn newBaseBuiltinCastFunc4String(
    ctx: Option<&dyn BuildContext>,
    func_name: &str,
    args: Vec<Box<dyn Expression>>,
    tp: types::FieldType,
    is_explicit_charset: bool,
) -> Result<baseBuiltinFunc, Error> {
    if is_explicit_charset {
        let mut bf = baseBuiltinFunc::new(args, tp);
        bf.setCollator(bf.tp.GetCollate().to_string());
        Ok(bf)
    } else {
        newBaseBuiltinFunc(ctx, func_name, args, tp)
    }
}

#[derive(Clone, Debug)]
/// 函数类：名称与参数个数上下界，负责 `verifyArgs`。
pub struct baseFunctionClass {
    funcName: String,
    minArgs: usize,
    maxArgs: usize,
}
impl baseFunctionClass {
    /// 由消息构造错误。
    pub fn new(name: &str, min_args: usize, max_args: usize) -> Self {
        Self {
            funcName: name.into(),
            minArgs: min_args,
            maxArgs: max_args,
        }
    }
    /// 函数 `verifyArgs`。
    pub fn verifyArgs(&self, args: &[Box<dyn Expression>]) -> Result<(), Error> {
        self.verifyArgsByCount(args.len())
    }
    /// 函数 `verifyArgsByCount`。
    pub fn verifyArgsByCount(&self, count: usize) -> Result<(), Error> {
        if (self.minArgs..=self.maxArgs).contains(&count) {
            Ok(())
        } else {
            Err(Error::new(format!(
                "Incorrect parameter count in the call to native function '{}'",
                self.funcName
            )))
        }
    }
}

/// 内置函数名表（用于支持性检查与列表输出）。
const BUILTIN_NAMES: &[&str] = &[
    "abs",
    "acos",
    "adddate",
    "addtime",
    "aes_decrypt",
    "aes_encrypt",
    "and",
    "ascii",
    "asin",
    "atan",
    "atan2",
    "benchmark",
    "bin",
    "bit_count",
    "cast",
    "ceil",
    "ceiling",
    "char",
    "char_length",
    "coalesce",
    "concat",
    "concat_ws",
    "connection_id",
    "conv",
    "convert",
    "cos",
    "cot",
    "crc32",
    "curdate",
    "current_date",
    "current_role",
    "current_time",
    "current_timestamp",
    "current_user",
    "database",
    "date",
    "date_add",
    "date_format",
    "date_sub",
    "datediff",
    "day",
    "dayname",
    "dayofmonth",
    "dayofweek",
    "dayofyear",
    "degrees",
    "elt",
    "exp",
    "extract",
    "field",
    "find_in_set",
    "floor",
    "format",
    "found_rows",
    "from_base64",
    "from_days",
    "from_unixtime",
    "get_lock",
    "getvar",
    "greatest",
    "hex",
    "hour",
    "if",
    "ifnull",
    "in",
    "inet6_aton",
    "inet6_ntoa",
    "inet_aton",
    "inet_ntoa",
    "insert",
    "instr",
    "intdiv",
    "is_ipv4",
    "is_ipv4_compat",
    "is_ipv4_mapped",
    "is_ipv6",
    "isfalse",
    "isnull",
    "istrue",
    "json_array",
    "json_contains",
    "json_extract",
    "json_object",
    "json_quote",
    "json_set",
    "json_type",
    "json_unquote",
    "last_day",
    "last_insert_id",
    "lastval",
    "lcase",
    "least",
    "left",
    "length",
    "like",
    "ln",
    "load_file",
    "locate",
    "log",
    "log10",
    "log2",
    "lower",
    "lpad",
    "ltrim",
    "make_set",
    "makedate",
    "maketime",
    "md5",
    "microsecond",
    "mid",
    "minute",
    "mod",
    "month",
    "monthname",
    "mul",
    "ne",
    "nextval",
    "not",
    "now",
    "nulleq",
    "oct",
    "octet_length",
    "ord",
    "or",
    "password",
    "period_add",
    "period_diff",
    "pi",
    "plus",
    "pow",
    "power",
    "quarter",
    "quote",
    "radians",
    "rand",
    "regexp",
    "release_all_locks",
    "release_lock",
    "repeat",
    "replace",
    "reverse",
    "right",
    "round",
    "row_count",
    "rpad",
    "rtrim",
    "second",
    "setval",
    "setvar",
    "sha",
    "sha1",
    "sha2",
    "sign",
    "sin",
    "sleep",
    "space",
    "sqrt",
    "str_to_date",
    "strcmp",
    "subdate",
    "substr",
    "substring",
    "substring_index",
    "subtime",
    "sysdate",
    "tan",
    "time",
    "time_format",
    "timediff",
    "timestamp",
    "timestampadd",
    "timestampdiff",
    "to_base64",
    "to_days",
    "to_seconds",
    "trim",
    "truncate",
    "ucase",
    "unhex",
    "unix_timestamp",
    "upper",
    "user",
    "utc_date",
    "utc_time",
    "utc_timestamp",
    "uuid",
    "uuid_short",
    "validate_password_strength",
    "version",
    "week",
    "weekday",
    "weekofyear",
    "xor",
    "year",
    "yearweek",
    ast::EQ,
    ast::NE,
    ast::LT,
    ast::LE,
    ast::GT,
    ast::GE,
];

/// 判断函数名是否在内置表中。
pub fn IsFunctionSupported(name: &str) -> bool {
    BUILTIN_NAMES.contains(&name.to_ascii_lowercase().as_str())
}
/// 返回函数的展示名（运算符转为符号等）。
pub fn GetDisplayName(name: &str) -> &str {
    match name {
        ast::EQ => "=",
        ast::NullEQ => "<=>",
        ast::IsTruthWithoutNull => "IS TRUE",
        ast::IsTruthWithNull => "IS TRUE",
        ast::IsFalsity => "IS FALSE",
        ast::IsFalsityWithNull => "IS FALSE",
        ast::NE => "!=",
        ast::LT => "<",
        ast::LE => "<=",
        ast::GT => ">",
        ast::GE => ">=",
        ast::Plus => "+",
        ast::Minus => "-",
        ast::Mul => "*",
        ast::Div => "/",
        ast::Mod => "%",
        ast::IntDiv => "DIV",
        _ => name,
    }
}
/// 返回排序后的内置函数名列表。
pub fn GetBuiltinList() -> Vec<String> {
    let mut result: Vec<String> = BUILTIN_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    result.sort();
    result.dedup();
    result
}

/// 按函数名校验参数个数。
pub fn VerifyArgsWrapper(name: &str, count: usize) -> Result<(), Error> {
    if !IsFunctionSupported(name) {
        return Err(Error::new(format!("FUNCTION {name} does not exist")));
    }
    if count == 0
        && !matches!(
            name,
            "pi" | "rand" | "uuid" | "now" | "curdate" | "current_date"
        )
    {
        return Err(Error::new(format!(
            "Incorrect parameter count in the call to native function '{name}'"
        )));
    }
    Ok(())
}

#[derive(Debug)]
/// 按求值上下文缓存昂贵初始化结果；错误不缓存。
pub struct builtinFuncCache<T: Clone> {
    cached: RwLock<Option<(u64, T)>>,
    init: Mutex<()>,
}
impl<T: Clone> Default for builtinFuncCache<T> {
    fn default() -> Self {
        Self {
            cached: RwLock::new(None),
            init: Mutex::new(()),
        }
    }
}
impl<T: Clone> builtinFuncCache<T> {
    /// 函数 `getCache`。
    pub fn getCache(&self, ctx_id: u64) -> Option<T> {
        self.cached
            .read()
            .expect("builtin cache poisoned")
            .as_ref()
            .filter(|(cached_id, _)| *cached_id == ctx_id)
            .map(|(_, item)| item.clone())
    }
    /// 函数 `getOrInitCache`。
    pub fn getOrInitCache(
        &self,
        ctx: &dyn EvalContext,
        construct: impl FnOnce() -> Result<T, Error>,
    ) -> Result<T, Error> {
        let ctx_id = ctx.CtxID();
        if let Some(item) = self.getCache(ctx_id) {
            return Ok(item);
        }
        let _guard = self.init.lock().expect("builtin cache poisoned");
        if let Some(item) = self.getCache(ctx_id) {
            return Ok(item);
        }
        let item = construct()?;
        *self.cached.write().expect("builtin cache poisoned") = Some((ctx_id, item.clone()));
        Ok(item)
    }
}

/// 将表达式按布尔语义求值（行级）。
pub fn EvalBool(
    ctx: &dyn EvalContext,
    filters: &[&dyn Expression],
    row: chunk::Row,
) -> (bool, bool, Option<Error>) {
    let mut is_null = false;
    for filter in filters {
        let (value, null, err) = filter.EvalInt(ctx, row);
        if err.is_some() {
            return (false, null, err);
        }
        is_null |= null;
        if null || value == 0 {
            return (false, is_null, None);
        }
    }
    (true, is_null, None)
}

/// 向量化布尔求值，写出选中/NULL 位图。
pub fn VecEvalBool(
    ctx: &dyn EvalContext,
    _vec_enabled: bool,
    filters: &[Box<dyn Expression>],
    input: &chunk::Chunk,
    mut selected: Vec<bool>,
    mut is_null: Option<Vec<bool>>,
) -> Result<(Vec<bool>, Option<Vec<bool>>), Error> {
    selected.clear();
    selected.resize(input.NumRows(), true);
    if let Some(nulls) = is_null.as_mut() {
        nulls.clear();
        nulls.resize(input.NumRows(), false);
    }
    for index in 0..input.NumRows() {
        for filter in filters {
            if !selected[index] {
                break;
            }
            let (value, null, err) = filter.EvalInt(ctx, chunk::Row { index });
            if let Some(err) = err {
                return Err(err);
            }
            selected[index] = !null && value != 0;
            if let Some(nulls) = is_null.as_mut() {
                nulls[index] |= null;
            }
        }
    }
    Ok((selected, is_null))
}

/// 正式表达式闭包使用的 builtin 注册边界。
///
/// Legacy per-section implementations retained for comparison with `builtin.go`; this submodule
/// Go 的 `functionClass`、完整 `funcs` 表和动态扩展/工厂边界接到 crate 的正式类型，
/// 不以少量手写函数替代尚未接通的具体 builtin 实现。
pub mod formal_registry {
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, LazyLock, RwLock};

    use crate::{
        BuildContext, CollationInfo, Error, EvalContext, Expression, OptionalEvalPropKeySet,
        builtinFunc, chunk, collate, collationInfo, errors, mysql, types,
    };
    use types_dependency::json_binary::JsonValue;
    use types_dependency::json_path::{JSONPathExpression, ParseJSONPathExpr};

    /// 结构体 `GeneratedBuiltinFactoryOutput`。
    pub struct GeneratedBuiltinFactoryOutput {
        signature: &'static str,
        function: Box<dyn builtinFunc>,
    }

    /// 函数 `ValidateGeneratedBuiltinSignature`。
    pub fn ValidateGeneratedBuiltinSignature(signature: &str) -> Result<(), Error> {
        if crate::builtin_threadsafe_generated_kernel::GeneratedThreadSafetyPolicyForSignature(
            signature,
        )
        .is_none()
        {
            return Err(errors::New(format!(
                "unknown generated builtin signature '{signature}'"
            )));
        }
        Ok(())
    }

    impl GeneratedBuiltinFactoryOutput {
        /// 由消息构造错误。
        pub fn new(signature: &'static str, function: Box<dyn builtinFunc>) -> Result<Self, Error> {
            ValidateGeneratedBuiltinSignature(signature)?;
            Ok(Self {
                signature,
                function,
            })
        }
    }

    /// 类型别名 `BuiltinFactory`。
    pub type BuiltinFactory = fn(
        &dyn BuildContext,
        Vec<Box<dyn Expression>>,
        &FunctionClassMetadata,
    ) -> Result<GeneratedBuiltinFactoryOutput, Error>;

    #[derive(Clone, Debug)]
    /// 枚举 `FunctionClassMetadata`。
    pub enum FunctionClassMetadata {
        Standard,
        Values {
            offset: i32,
            return_type: types::FieldType,
        },
        Truth {
            op: opcode::Op,
            keep_null: bool,
        },
    }

    /// 对应 `pkg/parser/opcode` 中本文件实际使用的真值操作码。完整 opcode crate
    /// 接入后可以直接在构造处做一一转换，不需要改变 functionClass 注册协议。
    pub use crate::opcode;

    #[derive(Clone, Copy)]
    enum RuntimeThreadSafetyPolicy {
        Recursive,
        Never,
    }

    /// 结构体 `RegistryBuiltinBase`。
    pub(crate) struct RegistryBuiltinBase {
        pub(crate) args: Vec<Box<dyn Expression>>,
        safe_to_share_across_session_flag: AtomicU32,
        thread_safety_policy: RuntimeThreadSafetyPolicy,
        pub(crate) return_type: types::FieldType,
        pub(crate) pb_code: i32,
        pub(crate) collator: Box<dyn collate::Collator>,
        pub(crate) collation_info: collationInfo,
    }

    impl Clone for RegistryBuiltinBase {
        fn clone(&self) -> Self {
            Self {
                args: self.args.clone(),
                safe_to_share_across_session_flag: AtomicU32::new(
                    self.safe_to_share_across_session_flag
                        .load(Ordering::SeqCst),
                ),
                thread_safety_policy: self.thread_safety_policy,
                return_type: self.return_type.clone(),
                pb_code: self.pb_code,
                collator: self.collator.Clone(),
                collation_info: self.collation_info.clone(),
            }
        }
    }

    impl RegistryBuiltinBase {
        fn new_with_policy(
            args: Vec<Box<dyn Expression>>,
            return_type: types::FieldType,
            thread_safety_policy: RuntimeThreadSafetyPolicy,
        ) -> Self {
            let collator = collate::GetCollator(return_type.GetCollate());
            Self {
                args,
                safe_to_share_across_session_flag: AtomicU32::new(0),
                thread_safety_policy,
                return_type,
                pb_code: 0,
                collator,
                collation_info: collationInfo::default(),
            }
        }

        /// 函数 `new_recursive`。
        pub(crate) fn new_recursive(
            args: Vec<Box<dyn Expression>>,
            return_type: types::FieldType,
        ) -> Self {
            Self::new_with_policy(args, return_type, RuntimeThreadSafetyPolicy::Recursive)
        }

        /// 函数 `new_never`。
        pub(crate) fn new_never(
            args: Vec<Box<dyn Expression>>,
            return_type: types::FieldType,
        ) -> Self {
            Self::new_with_policy(args, return_type, RuntimeThreadSafetyPolicy::Never)
        }

        /// 函数 `equal`。
        pub(crate) fn equal(&self, ctx: &dyn EvalContext, other: &Self) -> bool {
            self.return_type == other.return_type
                && self.args.len() == other.args.len()
                && self
                    .args
                    .iter()
                    .zip(&other.args)
                    .all(|(left, right)| left.Equal(ctx, right.as_ref()))
        }

        /// 函数 `SafeToShareAcrossSession`。
        pub(crate) fn SafeToShareAcrossSession(&self) -> bool {
            match self.thread_safety_policy {
                RuntimeThreadSafetyPolicy::Recursive => {
                    crate::builtin_threadsafe_generated_kernel::safeToShareAcrossSession(
                        &self.safe_to_share_across_session_flag,
                        &self.args,
                        |argument| argument.SafeToShareAcrossSession(),
                    )
                }
                RuntimeThreadSafetyPolicy::Never => false,
            }
        }

        /// 函数 `memory_usage`。
        pub(crate) fn memory_usage(&self) -> i64 {
            std::mem::size_of::<Self>() as i64
                + self.return_type.MemoryUsage()
                + self
                    .args
                    .iter()
                    .map(|argument| argument.MemoryUsage())
                    .sum::<i64>()
        }
    }

    impl CollationInfo for RegistryBuiltinBase {
        fn HasCoercibility(&self) -> bool {
            self.collation_info.HasCoercibility()
        }
        fn Coercibility(&self) -> crate::Coercibility {
            self.collation_info.Coercibility()
        }
        fn SetCoercibility(&self, value: crate::Coercibility) {
            self.collation_info.SetCoercibility(value)
        }
        fn Repertoire(&self) -> crate::Repertoire {
            self.collation_info.Repertoire()
        }
        fn SetRepertoire(&mut self, value: crate::Repertoire) {
            self.collation_info.SetRepertoire(value)
        }
        fn CharsetAndCollation(&self) -> (String, String) {
            self.collation_info.CharsetAndCollation()
        }
        fn SetCharsetAndCollation(&mut self, charset: String, collation: String) {
            self.collation_info
                .SetCharsetAndCollation(charset, collation)
        }
        fn IsExplicitCharset(&self) -> bool {
            self.collation_info.IsExplicitCharset()
        }
        fn SetExplicitCharset(&mut self, explicit: bool) {
            self.collation_info.SetExplicitCharset(explicit)
        }
    }

    struct GeneratedPolicyBuiltin {
        signature: &'static str,
        function: Box<dyn builtinFunc>,
        safe_to_share_across_session_flag: AtomicU32,
    }

    impl GeneratedPolicyBuiltin {
        /// 由消息构造错误。
        fn new(output: GeneratedBuiltinFactoryOutput) -> Self {
            Self {
                signature: output.signature,
                function: output.function,
                safe_to_share_across_session_flag: AtomicU32::new(0),
            }
        }
    }

    pub(crate) fn allowedOptionalEvalPropsForSignature(
        signature: &str,
        fallback: OptionalEvalPropKeySet,
    ) -> OptionalEvalPropKeySet {
        if signature == "builtinUncompressSig" {
            crate::exprctx::OptPropSessionVars.AsPropKeySet()
        } else {
            fallback
        }
    }

    impl Clone for GeneratedPolicyBuiltin {
        fn clone(&self) -> Self {
            Self {
                signature: self.signature,
                function: self.function.Clone(),
                safe_to_share_across_session_flag: AtomicU32::new(
                    self.safe_to_share_across_session_flag
                        .load(Ordering::SeqCst),
                ),
            }
        }
    }

    impl CollationInfo for GeneratedPolicyBuiltin {
        fn HasCoercibility(&self) -> bool {
            self.function.HasCoercibility()
        }
        fn Coercibility(&self) -> crate::Coercibility {
            self.function.Coercibility()
        }
        fn SetCoercibility(&self, value: crate::Coercibility) {
            self.function.SetCoercibility(value)
        }
        fn Repertoire(&self) -> crate::Repertoire {
            self.function.Repertoire()
        }
        fn SetRepertoire(&mut self, value: crate::Repertoire) {
            self.function.SetRepertoire(value)
        }
        fn CharsetAndCollation(&self) -> (String, String) {
            self.function.CharsetAndCollation()
        }
        fn SetCharsetAndCollation(&mut self, charset: String, collation: String) {
            self.function.SetCharsetAndCollation(charset, collation)
        }
        fn IsExplicitCharset(&self) -> bool {
            self.function.IsExplicitCharset()
        }
        fn SetExplicitCharset(&mut self, explicit: bool) {
            self.function.SetExplicitCharset(explicit)
        }
    }

    impl builtinFunc for GeneratedPolicyBuiltin {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn RequiredOptionalEvalProps(&self) -> OptionalEvalPropKeySet {
            self.function.RequiredOptionalEvalProps()
        }
        fn AllowedOptionalEvalProps(&self) -> OptionalEvalPropKeySet {
            allowedOptionalEvalPropsForSignature(
                self.signature,
                self.function.AllowedOptionalEvalProps(),
            )
        }
        fn isExtensionFunction(&self) -> bool {
            self.function.isExtensionFunction()
        }
        fn groupingMetaInitialized(&self) -> Option<bool> {
            self.function.groupingMetaInitialized()
        }
        fn groupingModeAndMarks(&self) -> Option<(i64, Vec<Vec<u64>>)> {
            self.function.groupingModeAndMarks()
        }
        fn restoreGroupingModeAndMarks(
            &self,
            mode: i64,
            marks: Vec<Vec<u64>>,
        ) -> Result<(), Error> {
            self.function.restoreGroupingModeAndMarks(mode, marks)
        }
        fn SafeToShareAcrossSession(&self) -> bool {
            crate::builtin_threadsafe_generated_kernel::generatedSignatureSafeToShareAcrossSession(
                Some(self.signature),
                &self.safe_to_share_across_session_flag,
                self.function.getArgs(),
                |argument| argument.SafeToShareAcrossSession(),
            )
        }
        fn evalInt(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<(i64, bool), Error> {
            self.function.evalInt(ctx, row)
        }
        fn evalReal(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<(f64, bool), Error> {
            self.function.evalReal(ctx, row)
        }
        fn evalString(
            &self,
            ctx: &dyn EvalContext,
            row: chunk::Row,
        ) -> Result<(String, bool), Error> {
            self.function.evalString(ctx, row)
        }
        fn evalDecimal(
            &self,
            ctx: &dyn EvalContext,
            row: chunk::Row,
        ) -> Result<(types::MyDecimal, bool), Error> {
            self.function.evalDecimal(ctx, row)
        }
        fn evalTime(
            &self,
            ctx: &dyn EvalContext,
            row: chunk::Row,
        ) -> Result<(types::Time, bool), Error> {
            self.function.evalTime(ctx, row)
        }
        fn evalDuration(
            &self,
            ctx: &dyn EvalContext,
            row: chunk::Row,
        ) -> Result<(types::Duration, bool), Error> {
            self.function.evalDuration(ctx, row)
        }
        fn evalJSON(
            &self,
            ctx: &dyn EvalContext,
            row: chunk::Row,
        ) -> Result<(types::BinaryJSON, bool), Error> {
            self.function.evalJSON(ctx, row)
        }
        fn evalVectorFloat32(
            &self,
            ctx: &dyn EvalContext,
            row: chunk::Row,
        ) -> Result<(types::VectorFloat32, bool), Error> {
            self.function.evalVectorFloat32(ctx, row)
        }
        fn vecEvalInt(
            &self,
            ctx: &dyn EvalContext,
            input: &chunk::Chunk,
            result: &mut chunk::Column,
        ) -> Result<(), Error> {
            self.function.vecEvalInt(ctx, input, result)
        }
        fn vecEvalReal(
            &self,
            ctx: &dyn EvalContext,
            input: &chunk::Chunk,
            result: &mut chunk::Column,
        ) -> Result<(), Error> {
            self.function.vecEvalReal(ctx, input, result)
        }
        fn vecEvalString(
            &self,
            ctx: &dyn EvalContext,
            input: &chunk::Chunk,
            result: &mut chunk::Column,
        ) -> Result<(), Error> {
            self.function.vecEvalString(ctx, input, result)
        }
        fn vecEvalDecimal(
            &self,
            ctx: &dyn EvalContext,
            input: &chunk::Chunk,
            result: &mut chunk::Column,
        ) -> Result<(), Error> {
            self.function.vecEvalDecimal(ctx, input, result)
        }
        fn vecEvalTime(
            &self,
            ctx: &dyn EvalContext,
            input: &chunk::Chunk,
            result: &mut chunk::Column,
        ) -> Result<(), Error> {
            self.function.vecEvalTime(ctx, input, result)
        }
        fn vecEvalDuration(
            &self,
            ctx: &dyn EvalContext,
            input: &chunk::Chunk,
            result: &mut chunk::Column,
        ) -> Result<(), Error> {
            self.function.vecEvalDuration(ctx, input, result)
        }
        fn vecEvalJSON(
            &self,
            ctx: &dyn EvalContext,
            input: &chunk::Chunk,
            result: &mut chunk::Column,
        ) -> Result<(), Error> {
            self.function.vecEvalJSON(ctx, input, result)
        }
        fn vecEvalVectorFloat32(
            &self,
            ctx: &dyn EvalContext,
            input: &chunk::Chunk,
            result: &mut chunk::Column,
        ) -> Result<(), Error> {
            self.function.vecEvalVectorFloat32(ctx, input, result)
        }
        /// `getArgs`：builtin 内部辅助。
        fn getArgs(&self) -> &[Box<dyn Expression>] {
            self.function.getArgs()
        }
        /// `getArgsMut`：builtin 内部辅助。
        fn getArgsMut(&mut self) -> &mut [Box<dyn Expression>] {
            self.function.getArgsMut()
        }
        fn equal(&self, ctx: &dyn EvalContext, other: &dyn builtinFunc) -> bool {
            other.as_any().downcast_ref::<Self>().is_some_and(|rhs| {
                self.signature == rhs.signature && self.function.equal(ctx, rhs.function.as_ref())
            })
        }
        /// `getRetTp`：builtin 内部辅助。
        fn getRetTp(&self) -> &types::FieldType {
            self.function.getRetTp()
        }
        /// `setPbCode`：builtin 内部辅助。
        fn setPbCode(&mut self, code: i32) {
            self.function.setPbCode(code)
        }
        fn PbCode(&self) -> i32 {
            self.function.PbCode()
        }
        /// `setCollator`：builtin 内部辅助。
        fn setCollator(&mut self, collator: Box<dyn collate::Collator>) {
            self.function.setCollator(collator)
        }
        fn collator(&self) -> &dyn collate::Collator {
            self.function.collator()
        }
        fn Clone(&self) -> Box<dyn builtinFunc> {
            Box::new(self.clone())
        }
        fn MemoryUsage(&self) -> i64 {
            std::mem::size_of::<Self>() as i64 + self.function.MemoryUsage()
        }
        fn vectorized(&self) -> bool {
            self.function.vectorized()
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum TruthEvalType {
        Int,
        Real,
        Decimal,
        VectorFloat32,
    }

    #[derive(Clone)]
    struct TruthBuiltin {
        base: RegistryBuiltinBase,
        op: opcode::Op,
        keep_null: bool,
        argument_type: TruthEvalType,
    }

    impl TruthBuiltin {
        /// 由消息构造错误。
        fn new(
            ctx: &dyn BuildContext,
            args: Vec<Box<dyn Expression>>,
            op: opcode::Op,
            keep_null: bool,
        ) -> Result<Self, Error> {
            let argument_type = match args[0].GetType(ctx.GetEvalCtx()).EvalType() {
                types::ETInt => TruthEvalType::Int,
                types::ETDecimal => TruthEvalType::Decimal,
                types::ETVectorFloat32 => TruthEvalType::VectorFloat32,
                // Go casts string/time/duration/json inputs to REAL before truth evaluation.
                types::ETReal
                | types::ETString
                | types::ETDatetime
                | types::ETTimestamp
                | types::ETDuration
                | types::ETJson => TruthEvalType::Real,
                other => {
                    return Err(errors::New(format!("unexpected types.EvalType {other:?}")));
                }
            };
            if !matches!(op, opcode::Op::IsTruth | opcode::Op::IsFalsity) {
                return Err(errors::New(format!("unexpected truth-test opcode {op:?}")));
            }
            let mut return_type = *types::NewFieldType(mysql::TypeLonglong);
            return_type.SetFlen(1);
            return_type.AddFlag(mysql::IsBooleanFlag);
            let mut base = if argument_type == TruthEvalType::VectorFloat32 {
                RegistryBuiltinBase::new_never(args, return_type)
            } else {
                RegistryBuiltinBase::new_recursive(args, return_type)
            };
            // Truth predicates return numeric boolean values. Go's builtin
            // constructor records this before constant folding asks the
            // ScalarFunction for collation metadata.
            base.SetCoercibility(crate::CoercibilityNumeric);
            base.SetRepertoire(crate::ASCII);
            base.pb_code = truth_pb_code(op, argument_type, keep_null);
            Ok(Self {
                base,
                op,
                keep_null,
                argument_type,
            })
        }

        fn finish(&self, is_zero: bool, is_null: bool) -> (i64, bool) {
            if self.keep_null && is_null {
                return (0, true);
            }
            let result = match self.op {
                opcode::Op::IsTruth => !is_null && !is_zero,
                opcode::Op::IsFalsity => !is_null && is_zero,
                _ => unreachable!("TruthBuiltin validates its opcode at construction"),
            };
            (i64::from(result), false)
        }
    }

    fn truth_pb_code(op: opcode::Op, argument_type: TruthEvalType, keep_null: bool) -> i32 {
        match (op, argument_type, keep_null) {
            (opcode::Op::IsTruth, TruthEvalType::Int, false) => 3122,
            (opcode::Op::IsTruth, TruthEvalType::Real, false) => 3123,
            (opcode::Op::IsTruth, TruthEvalType::Decimal, false) => 3124,
            (opcode::Op::IsFalsity, TruthEvalType::Int, false) => 3125,
            (opcode::Op::IsFalsity, TruthEvalType::Real, false) => 3126,
            (opcode::Op::IsFalsity, TruthEvalType::Decimal, false) => 3127,
            (opcode::Op::IsTruth, TruthEvalType::Int, true) => 3142,
            (opcode::Op::IsTruth, TruthEvalType::Real, true) => 3143,
            (opcode::Op::IsTruth, TruthEvalType::Decimal, true) => 3144,
            (opcode::Op::IsFalsity, TruthEvalType::Int, true) => 3145,
            (opcode::Op::IsFalsity, TruthEvalType::Real, true) => 3146,
            (opcode::Op::IsFalsity, TruthEvalType::Decimal, true) => 3147,
            // TiDB intentionally has no protobuf truth signature for vectors yet.
            (_, TruthEvalType::VectorFloat32, _) => 0,
            _ => 0,
        }
    }

    impl CollationInfo for TruthBuiltin {
        fn HasCoercibility(&self) -> bool {
            self.base.HasCoercibility()
        }
        fn Coercibility(&self) -> crate::Coercibility {
            self.base.Coercibility()
        }
        fn SetCoercibility(&self, value: crate::Coercibility) {
            self.base.SetCoercibility(value)
        }
        fn Repertoire(&self) -> crate::Repertoire {
            self.base.Repertoire()
        }
        fn SetRepertoire(&mut self, value: crate::Repertoire) {
            self.base.SetRepertoire(value)
        }
        fn CharsetAndCollation(&self) -> (String, String) {
            self.base.CharsetAndCollation()
        }
        fn SetCharsetAndCollation(&mut self, charset: String, collation: String) {
            self.base.SetCharsetAndCollation(charset, collation)
        }
        fn IsExplicitCharset(&self) -> bool {
            self.base.IsExplicitCharset()
        }
        fn SetExplicitCharset(&mut self, explicit: bool) {
            self.base.SetExplicitCharset(explicit)
        }
    }

    impl builtinFunc for TruthBuiltin {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn SafeToShareAcrossSession(&self) -> bool {
            self.base.SafeToShareAcrossSession()
        }
        fn evalInt(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<(i64, bool), Error> {
            match self.argument_type {
                TruthEvalType::Int => {
                    let (value, null) = self.base.args[0].EvalInt(ctx, row)?;
                    Ok(self.finish(value == 0, null))
                }
                TruthEvalType::Real => {
                    let (value, null) = self.base.args[0].EvalReal(ctx, row)?;
                    Ok(self.finish(value == 0.0, null))
                }
                TruthEvalType::Decimal => {
                    let (value, null) = self.base.args[0].EvalDecimal(ctx, row)?;
                    Ok(self.finish(value.IsZero(), null))
                }
                TruthEvalType::VectorFloat32 => {
                    let (value, null) = self.base.args[0].EvalVectorFloat32(ctx, row)?;
                    Ok(self.finish(value.IsZeroValue(), null))
                }
            }
        }
        /// `getArgs`：builtin 内部辅助。
        fn getArgs(&self) -> &[Box<dyn Expression>] {
            &self.base.args
        }
        /// `getArgsMut`：builtin 内部辅助。
        fn getArgsMut(&mut self) -> &mut [Box<dyn Expression>] {
            &mut self.base.args
        }
        fn equal(&self, ctx: &dyn EvalContext, other: &dyn builtinFunc) -> bool {
            other.as_any().downcast_ref::<Self>().is_some_and(|rhs| {
                self.op == rhs.op
                    && self.keep_null == rhs.keep_null
                    && self.argument_type == rhs.argument_type
                    && self.base.equal(ctx, &rhs.base)
            })
        }
        /// `getRetTp`：builtin 内部辅助。
        fn getRetTp(&self) -> &types::FieldType {
            &self.base.return_type
        }
        /// `setPbCode`：builtin 内部辅助。
        fn setPbCode(&mut self, code: i32) {
            self.base.pb_code = code
        }
        fn PbCode(&self) -> i32 {
            self.base.pb_code
        }
        /// `setCollator`：builtin 内部辅助。
        fn setCollator(&mut self, collator: Box<dyn collate::Collator>) {
            self.base.collator = collator
        }
        fn collator(&self) -> &dyn collate::Collator {
            self.base.collator.as_ref()
        }
        fn Clone(&self) -> Box<dyn builtinFunc> {
            Box::new(self.clone())
        }
        fn MemoryUsage(&self) -> i64 {
            self.base.memory_usage()
        }
        fn vectorized(&self) -> bool {
            true
        }
    }

    /// Session 层通过这个读取器提供当前 INSERT 行；builtin 本身保持对 sessionctx 的
    /// 反向依赖隔离，同时执行语义仍是 Go 的逐次读取，而不是构建期快照。
    pub type CurrentInsertValuesReader = fn(&dyn EvalContext) -> Result<chunk::Row, Error>;
    static CURRENT_INSERT_VALUES_READER: LazyLock<RwLock<Option<CurrentInsertValuesReader>>> =
        LazyLock::new(|| RwLock::new(None));

    /// 函数 `registerCurrentInsertValuesReader`。
    pub fn registerCurrentInsertValuesReader(reader: CurrentInsertValuesReader) {
        if let Ok(mut slot) = CURRENT_INSERT_VALUES_READER.write() {
            *slot = Some(reader);
        }
    }

    fn current_insert_values(ctx: &dyn EvalContext) -> Result<chunk::Row, Error> {
        let reader = CURRENT_INSERT_VALUES_READER
            .read()
            .map_err(|_| errors::New("current insert values reader poisoned"))?
            .ok_or_else(|| errors::New("current insert values provider is not linked"))?;
        reader(ctx)
    }

    #[derive(Clone)]
    struct ValuesBuiltin {
        base: RegistryBuiltinBase,
        offset: usize,
    }

    impl CollationInfo for ValuesBuiltin {
        fn HasCoercibility(&self) -> bool {
            self.base.HasCoercibility()
        }
        fn Coercibility(&self) -> crate::Coercibility {
            self.base.Coercibility()
        }
        fn SetCoercibility(&self, value: crate::Coercibility) {
            self.base.SetCoercibility(value)
        }
        fn Repertoire(&self) -> crate::Repertoire {
            self.base.Repertoire()
        }
        fn SetRepertoire(&mut self, value: crate::Repertoire) {
            self.base.SetRepertoire(value)
        }
        fn CharsetAndCollation(&self) -> (String, String) {
            self.base.CharsetAndCollation()
        }
        fn SetCharsetAndCollation(&mut self, charset: String, collation: String) {
            self.base.SetCharsetAndCollation(charset, collation)
        }
        fn IsExplicitCharset(&self) -> bool {
            self.base.IsExplicitCharset()
        }
        fn SetExplicitCharset(&mut self, explicit: bool) {
            self.base.SetExplicitCharset(explicit)
        }
    }

    impl ValuesBuiltin {
        fn source(&self, ctx: &dyn EvalContext) -> Result<Option<chunk::Row>, Error> {
            let row = current_insert_values(ctx)?;
            if row.IsEmpty() {
                return Ok(None);
            }
            if self.offset >= row.Len() {
                return Err(errors::New(format!(
                    "Session current insert values len {} and column's offset {} don't match",
                    row.Len(),
                    self.offset
                )));
            }
            Ok(Some(row))
        }
    }

    impl builtinFunc for ValuesBuiltin {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn RequiredOptionalEvalProps(&self) -> OptionalEvalPropKeySet {
            crate::exprctx::OptPropSessionVars.AsPropKeySet()
        }
        fn SafeToShareAcrossSession(&self) -> bool {
            false
        }
        fn evalInt(&self, ctx: &dyn EvalContext, _row: chunk::Row) -> Result<(i64, bool), Error> {
            let Some(row) = self.source(ctx)? else {
                return Ok((0, true));
            };
            if row.IsNull(self.offset) {
                return Ok((0, true));
            }
            let raw = row.GetRaw(self.offset);
            if raw.len() > 8 {
                return Err(errors::New("Session current insert values is too long"));
            }
            if raw.len() < 8 {
                return Ok((
                    types::BinaryLiteral(raw).ToInt(ctx.TypeCtx())? as i64,
                    false,
                ));
            }
            Ok((row.GetInt64(self.offset), false))
        }
        fn evalReal(&self, ctx: &dyn EvalContext, _row: chunk::Row) -> Result<(f64, bool), Error> {
            let Some(row) = self.source(ctx)? else {
                return Ok((0.0, true));
            };
            if row.IsNull(self.offset) {
                return Ok((0.0, true));
            }
            if self.base.return_type.GetType() == mysql::TypeFloat {
                Ok((row.GetFloat32(self.offset) as f64, false))
            } else {
                Ok((row.GetFloat64(self.offset), false))
            }
        }
        fn evalString(
            &self,
            ctx: &dyn EvalContext,
            _row: chunk::Row,
        ) -> Result<(String, bool), Error> {
            let Some(row) = self.source(ctx)? else {
                return Ok((String::new(), true));
            };
            if row.IsNull(self.offset) {
                return Ok((String::new(), true));
            }
            if self.base.return_type.Hybrid() {
                return Ok((
                    row.GetDatum(self.offset, &self.base.return_type)
                        .ToString()?,
                    false,
                ));
            }
            Ok((row.GetString(self.offset), false))
        }
        fn evalDecimal(
            &self,
            ctx: &dyn EvalContext,
            _row: chunk::Row,
        ) -> Result<(types::MyDecimal, bool), Error> {
            let Some(row) = self.source(ctx)? else {
                return Ok((types::MyDecimal::default(), true));
            };
            if row.IsNull(self.offset) {
                return Ok((types::MyDecimal::default(), true));
            }
            Ok((row.GetMyDecimal(self.offset), false))
        }
        fn evalTime(
            &self,
            ctx: &dyn EvalContext,
            _row: chunk::Row,
        ) -> Result<(types::Time, bool), Error> {
            let Some(row) = self.source(ctx)? else {
                return Ok((types::ZeroTime, true));
            };
            if row.IsNull(self.offset) {
                return Ok((types::ZeroTime, true));
            }
            Ok((row.GetTime(self.offset), false))
        }
        fn evalDuration(
            &self,
            ctx: &dyn EvalContext,
            _row: chunk::Row,
        ) -> Result<(types::Duration, bool), Error> {
            let Some(row) = self.source(ctx)? else {
                return Ok((types::Duration::default(), true));
            };
            if row.IsNull(self.offset) {
                return Ok((types::Duration::default(), true));
            }
            Ok((
                row.GetDuration(self.offset, self.base.return_type.GetDecimal() as i32),
                false,
            ))
        }
        fn evalJSON(
            &self,
            ctx: &dyn EvalContext,
            _row: chunk::Row,
        ) -> Result<(types::BinaryJSON, bool), Error> {
            let Some(row) = self.source(ctx)? else {
                return Ok((types::BinaryJSON::default(), true));
            };
            if row.IsNull(self.offset) {
                return Ok((types::BinaryJSON::default(), true));
            }
            Ok((row.GetJSON(self.offset), false))
        }
        fn evalVectorFloat32(
            &self,
            ctx: &dyn EvalContext,
            _row: chunk::Row,
        ) -> Result<(types::VectorFloat32, bool), Error> {
            let Some(row) = self.source(ctx)? else {
                return Ok((types::ZeroVectorFloat32(), true));
            };
            if row.IsNull(self.offset) {
                return Ok((types::ZeroVectorFloat32(), true));
            }
            Ok((row.GetVectorFloat32(self.offset), false))
        }
        /// `getArgs`：builtin 内部辅助。
        fn getArgs(&self) -> &[Box<dyn Expression>] {
            &self.base.args
        }
        /// `getArgsMut`：builtin 内部辅助。
        fn getArgsMut(&mut self) -> &mut [Box<dyn Expression>] {
            &mut self.base.args
        }
        fn equal(&self, ctx: &dyn EvalContext, other: &dyn builtinFunc) -> bool {
            other
                .as_any()
                .downcast_ref::<Self>()
                .is_some_and(|rhs| self.offset == rhs.offset && self.base.equal(ctx, &rhs.base))
        }
        /// `getRetTp`：builtin 内部辅助。
        fn getRetTp(&self) -> &types::FieldType {
            &self.base.return_type
        }
        /// `setPbCode`：builtin 内部辅助。
        fn setPbCode(&mut self, code: i32) {
            self.base.pb_code = code
        }
        fn PbCode(&self) -> i32 {
            self.base.pb_code
        }
        /// `setCollator`：builtin 内部辅助。
        fn setCollator(&mut self, collator: Box<dyn collate::Collator>) {
            self.base.collator = collator
        }
        fn collator(&self) -> &dyn collate::Collator {
            self.base.collator.as_ref()
        }
        fn Clone(&self) -> Box<dyn builtinFunc> {
            Box::new(self.clone())
        }
        fn MemoryUsage(&self) -> i64 {
            self.base.memory_usage()
        }
        fn vectorized(&self) -> bool {
            false
        }
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    /// 函数类：名称与参数个数上下界，负责 `verifyArgs`。
    pub struct baseFunctionClass {
        pub funcName: String,
        pub minArgs: usize,
        /// Go 用 -1 表示参数数量无上限。
        pub maxArgs: isize,
    }

    impl baseFunctionClass {
        /// 由消息构造错误。
        pub fn new(name: impl Into<String>, min_args: usize, max_args: isize) -> Self {
            Self {
                funcName: name.into(),
                minArgs: min_args,
                maxArgs: max_args,
            }
        }

        /// 函数 `verifyArgs`。
        pub fn verifyArgs(&self, args: &[Box<dyn Expression>]) -> Result<(), Error> {
            self.verifyArgsByCount(args.len())
        }

        /// 函数 `verifyArgsByCount`。
        pub fn verifyArgsByCount(&self, count: usize) -> Result<(), Error> {
            if count < self.minArgs || (self.maxArgs >= 0 && count > self.maxArgs as usize) {
                return Err(errors::New(format!(
                    "Incorrect parameter count in the call to native function '{}'",
                    self.funcName
                )));
            }
            Ok(())
        }
    }

    #[allow(non_camel_case_types)]
    /// trait `functionClass`。
    pub trait functionClass: Send + Sync {
        /// `getFunction`：builtin 内部辅助。
        fn getFunction(
            &self,
            ctx: &dyn BuildContext,
            args: Vec<Box<dyn Expression>>,
        ) -> Result<Box<dyn builtinFunc>, Error>;
        /// `verifyArgsByCount`：builtin 内部辅助。
        fn verifyArgsByCount(&self, count: usize) -> Result<(), Error>;
        /// `getDisplayName`：builtin 内部辅助。
        fn getDisplayName(&self) -> &str;
        fn metadata(&self) -> FunctionClassMetadata {
            FunctionClassMetadata::Standard
        }
    }

    #[derive(Clone, Debug)]
    struct registeredFunctionClass {
        base: baseFunctionClass,
    }

    impl registeredFunctionClass {
        /// 由消息构造错误。
        fn new(name: &str, min_args: usize, max_args: isize) -> Self {
            Self {
                base: baseFunctionClass::new(name, min_args, max_args),
            }
        }
    }

    impl functionClass for registeredFunctionClass {
        /// `getFunction`：builtin 内部辅助。
        fn getFunction(
            &self,
            ctx: &dyn BuildContext,
            args: Vec<Box<dyn Expression>>,
        ) -> Result<Box<dyn builtinFunc>, Error> {
            self.base.verifyArgs(&args)?;
            invoke_factory(
                &self.base.funcName,
                ctx,
                args,
                &FunctionClassMetadata::Standard,
            )
        }

        /// `verifyArgsByCount`：builtin 内部辅助。
        fn verifyArgsByCount(&self, count: usize) -> Result<(), Error> {
            self.base.verifyArgsByCount(count)
        }

        /// `getDisplayName`：builtin 内部辅助。
        fn getDisplayName(&self) -> &str {
            display_name(&self.base.funcName)
        }
    }

    /// VALUES 函数类保留 Go 的列偏移和返回类型，不把它退化为普通零参函数。
    #[allow(non_camel_case_types)]
    #[derive(Clone, Debug)]
    pub struct valuesFunctionClass {
        pub baseFunctionClass: baseFunctionClass,
        pub offset: i32,
        pub tp: types::FieldType,
    }

    impl valuesFunctionClass {
        /// 由消息构造错误。
        pub fn new(name: &str, offset: i32, tp: types::FieldType) -> Self {
            Self {
                baseFunctionClass: baseFunctionClass::new(name, 0, 0),
                offset,
                tp,
            }
        }
    }

    impl functionClass for valuesFunctionClass {
        /// `getFunction`：builtin 内部辅助。
        fn getFunction(
            &self,
            _ctx: &dyn BuildContext,
            args: Vec<Box<dyn Expression>>,
        ) -> Result<Box<dyn builtinFunc>, Error> {
            self.baseFunctionClass.verifyArgs(&args)?;
            if self.offset < 0 {
                return Err(errors::New(format!(
                    "VALUES() column offset {} is invalid",
                    self.offset
                )));
            }
            match self.tp.EvalType() {
                types::ETInt
                | types::ETReal
                | types::ETDecimal
                | types::ETString
                | types::ETDatetime
                | types::ETTimestamp
                | types::ETDuration
                | types::ETJson
                | types::ETVectorFloat32 => Ok(Box::new(ValuesBuiltin {
                    base: RegistryBuiltinBase::new_never(args, self.tp.clone()),
                    offset: self.offset as usize,
                })),
                other => Err(errors::New(format!(
                    "{other:?} is not supported for VALUES()"
                ))),
            }
        }

        /// `verifyArgsByCount`：builtin 内部辅助。
        fn verifyArgsByCount(&self, count: usize) -> Result<(), Error> {
            self.baseFunctionClass.verifyArgsByCount(count)
        }

        /// `getDisplayName`：builtin 内部辅助。
        fn getDisplayName(&self) -> &str {
            &self.baseFunctionClass.funcName
        }

        fn metadata(&self) -> FunctionClassMetadata {
            FunctionClassMetadata::Values {
                offset: self.offset,
                return_type: self.tp.clone(),
            }
        }
    }

    /// IS TRUE / IS FALSE 共用函数类，显式保留 opcode 与 NULL 处理模式。
    #[allow(non_camel_case_types)]
    #[derive(Clone, Debug)]
    pub struct isTrueOrFalseFunctionClass {
        pub baseFunctionClass: baseFunctionClass,
        pub op: opcode::Op,
        pub keepNull: bool,
    }

    impl isTrueOrFalseFunctionClass {
        /// 由消息构造错误。
        pub fn new(name: &str, op: opcode::Op, keep_null: bool) -> Self {
            Self {
                baseFunctionClass: baseFunctionClass::new(name, 1, 1),
                op,
                keepNull: keep_null,
            }
        }
    }

    impl functionClass for isTrueOrFalseFunctionClass {
        /// `getFunction`：builtin 内部辅助。
        fn getFunction(
            &self,
            ctx: &dyn BuildContext,
            args: Vec<Box<dyn Expression>>,
        ) -> Result<Box<dyn builtinFunc>, Error> {
            self.baseFunctionClass.verifyArgs(&args)?;
            Ok(Box::new(TruthBuiltin::new(
                ctx,
                args,
                self.op,
                self.keepNull,
            )?))
        }

        /// `verifyArgsByCount`：builtin 内部辅助。
        fn verifyArgsByCount(&self, count: usize) -> Result<(), Error> {
            self.baseFunctionClass.verifyArgsByCount(count)
        }

        /// `getDisplayName`：builtin 内部辅助。
        fn getDisplayName(&self) -> &str {
            display_name(&self.baseFunctionClass.funcName)
        }

        fn metadata(&self) -> FunctionClassMetadata {
            FunctionClassMetadata::Truth {
                op: self.op,
                keep_null: self.keepNull,
            }
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum JsonArrayBound {
        Start(usize),
        Last(usize),
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum JsonArraySelection {
        All,
        Index(JsonArrayBound),
        Range(JsonArrayBound, JsonArrayBound),
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    enum JsonExtractLeg {
        Key(Option<String>),
        Array(JsonArraySelection),
        Descend,
    }

    fn parse_json_array_bound(value: &str) -> Result<JsonArrayBound, Error> {
        let value = value
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect::<String>();
        if value == "last" {
            return Ok(JsonArrayBound::Last(0));
        }
        if let Some(offset) = value.strip_prefix("last-") {
            return offset
                .parse()
                .map(JsonArrayBound::Last)
                .map_err(|error| errors::New(format!("invalid JSON path array bound: {error}")));
        }
        value
            .parse()
            .map(JsonArrayBound::Start)
            .map_err(|error| errors::New(format!("invalid JSON path array bound: {error}")))
    }

    fn parse_json_extract_legs(path: &JSONPathExpression) -> Result<Vec<JsonExtractLeg>, Error> {
        let path = path.String();
        let bytes = path.as_bytes();
        let mut index = 1;
        let mut legs = Vec::new();
        while index < bytes.len() {
            if bytes[index..].starts_with(b"**") {
                legs.push(JsonExtractLeg::Descend);
                index += 2;
                continue;
            }
            if bytes[index] == b'.' {
                index += 1;
                if bytes.get(index) == Some(&b'*') {
                    legs.push(JsonExtractLeg::Key(None));
                    index += 1;
                    continue;
                }
                let start = index;
                if bytes.get(index) == Some(&b'\"') {
                    index += 1;
                    let mut escaped = false;
                    while index < bytes.len() {
                        let byte = bytes[index];
                        index += 1;
                        if escaped {
                            escaped = false;
                        } else if byte == b'\\' {
                            escaped = true;
                        } else if byte == b'\"' {
                            break;
                        }
                    }
                    let key = serde_json::from_str(&path[start..index])
                        .map_err(|error| errors::New(error.to_string()))?;
                    legs.push(JsonExtractLeg::Key(Some(key)));
                    continue;
                }
                while index < bytes.len()
                    && bytes[index] != b'.'
                    && bytes[index] != b'['
                    && !bytes[index..].starts_with(b"**")
                {
                    index += 1;
                }
                legs.push(JsonExtractLeg::Key(Some(path[start..index].to_owned())));
                continue;
            }
            if bytes[index] == b'[' {
                let start = index + 1;
                let end = path[start..]
                    .find(']')
                    .map(|offset| start + offset)
                    .ok_or_else(|| errors::New("unterminated JSON path array selection"))?;
                let selection = &path[start..end];
                let selection = if selection == "*" {
                    JsonArraySelection::All
                } else if let Some((start, end)) = selection.split_once(" to ") {
                    JsonArraySelection::Range(
                        parse_json_array_bound(start)?,
                        parse_json_array_bound(end)?,
                    )
                } else {
                    JsonArraySelection::Index(parse_json_array_bound(selection)?)
                };
                legs.push(JsonExtractLeg::Array(selection));
                index = end + 1;
                continue;
            }
            return Err(errors::New("unsupported canonical JSON path leg"));
        }
        Ok(legs)
    }

    fn json_array_index(bound: JsonArrayBound, count: usize) -> Option<usize> {
        match bound {
            JsonArrayBound::Start(index) => (index < count).then_some(index),
            JsonArrayBound::Last(offset) => count.checked_sub(offset + 1),
        }
    }

    fn extract_json_values<'a>(
        value: &'a JsonValue,
        legs: &[JsonExtractLeg],
        seen: &mut HashSet<usize>,
        output: &mut Vec<JsonValue>,
    ) {
        let Some((leg, rest)) = legs.split_first() else {
            if seen.insert(value as *const JsonValue as usize) {
                output.push(value.clone());
            }
            return;
        };
        match (leg, value) {
            (JsonExtractLeg::Key(key), JsonValue::Object(values)) => match key {
                Some(key) => {
                    if let Some(child) = values.get(key) {
                        extract_json_values(child, rest, seen, output);
                    }
                }
                None => {
                    for child in values.values() {
                        extract_json_values(child, rest, seen, output);
                    }
                }
            },
            (JsonExtractLeg::Array(selection), JsonValue::Array(values)) => {
                let bounds = match selection {
                    JsonArraySelection::All => values.len().checked_sub(1).map(|last| (0, last)),
                    JsonArraySelection::Index(bound) => {
                        json_array_index(*bound, values.len()).map(|index| (index, index))
                    }
                    JsonArraySelection::Range(start, end) => {
                        let start = json_array_index(*start, values.len());
                        let end = match end {
                            JsonArrayBound::Start(index) => {
                                values.len().checked_sub(1).map(|last| (*index).min(last))
                            }
                            bound => json_array_index(*bound, values.len()),
                        };
                        start.zip(end)
                    }
                };
                if let Some((start, end)) = bounds {
                    if start <= end {
                        for child in &values[start..=end] {
                            extract_json_values(child, rest, seen, output);
                        }
                    }
                }
            }
            (JsonExtractLeg::Array(selection), _) => {
                let matches_self = match selection {
                    JsonArraySelection::All => false,
                    JsonArraySelection::Index(
                        JsonArrayBound::Start(0) | JsonArrayBound::Last(0),
                    ) => true,
                    JsonArraySelection::Range(JsonArrayBound::Start(0), end) => {
                        matches!(end, JsonArrayBound::Start(_) | JsonArrayBound::Last(0))
                    }
                    _ => false,
                };
                if matches_self {
                    extract_json_values(value, rest, seen, output);
                }
            }
            (JsonExtractLeg::Descend, _) => {
                extract_json_values(value, rest, seen, output);
                match value {
                    JsonValue::Array(values) => {
                        for child in values {
                            extract_json_values(child, legs, seen, output);
                        }
                    }
                    JsonValue::Object(values) => {
                        for child in values.values() {
                            extract_json_values(child, legs, seen, output);
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    trait CanonicalBinaryJSONExtract {
        fn Extract(&self, paths: &[JSONPathExpression])
        -> Result<Option<types::BinaryJSON>, Error>;
    }

    impl CanonicalBinaryJSONExtract for types::BinaryJSON {
        fn Extract(
            &self,
            paths: &[JSONPathExpression],
        ) -> Result<Option<types::BinaryJSON>, Error> {
            let root = self.GetValue();
            let mut seen = HashSet::new();
            let mut values = Vec::new();
            for path in paths {
                extract_json_values(
                    &root,
                    &parse_json_extract_legs(path)?,
                    &mut seen,
                    &mut values,
                );
            }
            Ok(match values.len() {
                0 => None,
                1 if paths.len() == 1 && !paths[0].CouldMatchMultipleValues() => {
                    Some(types::CreateBinaryJSON(values.pop().unwrap()))
                }
                _ => Some(types::CreateBinaryJSON(values)),
            })
        }
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum ExtractMode {
        Datetime,
        Duration,
        DatetimeFromString,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum CoreBuiltinKind {
        Compare(&'static str, types::EvalType),
        In(types::EvalType),
        IsNull,
        Logic(&'static str),
        UnaryNot,
        UnaryMinus(types::EvalType),
        Arithmetic(&'static str, types::EvalType),
        Case,
        IfNull(types::EvalType),
        Row,
        JsonExtract,
        Uuid,
        Rand,
        Md5,
        SetVar(types::EvalType),
        TimeToSec,
        Extract(ExtractMode),
        Weekday,
        Atan,
        Pow,
        DateAddSub(bool),
        Now,
        Sleep,
        Substring(bool),
        Trim,
        Ascii,
        Length,
        CharLength(bool),
        Hex(bool),
        FindInSet,
        Space,
        Rpad,
        Insert,
        Like,
        Ilike,
        IsIPv4,
        IsIPv6,
        RegexpInstr,
        RegexpSubstr,
        RegexpReplace,
        TiDBShard,
    }

    #[derive(Clone)]
    struct CoreBuiltin {
        base: RegistryBuiltinBase,
        kind: CoreBuiltinKind,
        ilike: Option<crate::builtin_ilike_kernel::IlikeSig>,
    }

    impl CollationInfo for CoreBuiltin {
        fn HasCoercibility(&self) -> bool {
            self.base.HasCoercibility()
        }
        fn Coercibility(&self) -> crate::Coercibility {
            self.base.Coercibility()
        }
        fn SetCoercibility(&self, value: crate::Coercibility) {
            self.base.SetCoercibility(value)
        }
        fn Repertoire(&self) -> crate::Repertoire {
            self.base.Repertoire()
        }
        fn SetRepertoire(&mut self, value: crate::Repertoire) {
            self.base.SetRepertoire(value)
        }
        fn CharsetAndCollation(&self) -> (String, String) {
            self.base.CharsetAndCollation()
        }
        fn SetCharsetAndCollation(&mut self, charset: String, collation: String) {
            self.base.SetCharsetAndCollation(charset, collation.clone());
            if matches!(self.kind, CoreBuiltinKind::Ilike) {
                self.ilike = Some(crate::builtin_ilike_kernel::IlikeSig::new(
                    collation,
                    self.base.args[1].ConstLevel() >= crate::ConstOnlyInContext,
                    self.base.args[2].ConstLevel() >= crate::ConstOnlyInContext,
                ));
            }
        }
        fn IsExplicitCharset(&self) -> bool {
            self.base.IsExplicitCharset()
        }
        fn SetExplicitCharset(&mut self, explicit: bool) {
            self.base.SetExplicitCharset(explicit)
        }
    }

    fn boolean_field_type() -> types::FieldType {
        let mut field_type = *types::NewFieldType(mysql::TypeLonglong);
        field_type.SetFlen(1);
        field_type.AddFlag(mysql::IsBooleanFlag);
        field_type
    }

    fn ordering(value: std::cmp::Ordering) -> i32 {
        match value {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        }
    }

    impl CoreBuiltin {
        /// `set_user_variable`：builtin 内部辅助。
        fn set_user_variable(
            &self,
            ctx: &dyn EvalContext,
            row: chunk::Row,
        ) -> Result<(types::Datum, bool), Error> {
            let session_vars = expropt::SessionVarsPropReader
                .get_session_vars(ctx)
                .map_err(|error| errors::New(error.to_string()))?;
            let (name, name_is_null) = self.base.args[0].EvalString(ctx, row.clone())?;
            if name_is_null {
                return Ok((types::Datum::default(), true));
            }
            let value = self.base.args[1].Eval(ctx, row)?;
            if value.IsNull() {
                return Ok((value, true));
            }
            let name = name.to_ascii_lowercase();
            session_vars
                .UserVars
                .SetUserVarVal(&name, value.ToString()?);
            session_vars.SetUserVarType(&name, self.base.args[1].GetType(ctx).clone());
            Ok((value, false))
        }

        fn compare(
            &self,
            ctx: &dyn EvalContext,
            row: chunk::Row,
            eval_type: types::EvalType,
            left: usize,
            right: usize,
        ) -> Result<(i32, bool), Error> {
            let lhs = &self.base.args[left];
            let rhs = &self.base.args[right];
            match eval_type {
                types::ETInt => {
                    let (lhs_value, lhs_null) = lhs.EvalInt(ctx, row.clone())?;
                    let (rhs_value, rhs_null) = rhs.EvalInt(ctx, row)?;
                    if lhs_null || rhs_null {
                        return Ok((0, true));
                    }
                    let lhs_unsigned = mysql::HasUnsignedFlag(lhs.GetType(ctx).GetFlag());
                    let rhs_unsigned = mysql::HasUnsignedFlag(rhs.GetType(ctx).GetFlag());
                    let comparison = match (lhs_unsigned, rhs_unsigned) {
                        (false, false) => ordering(lhs_value.cmp(&rhs_value)),
                        (true, true) => ordering((lhs_value as u64).cmp(&(rhs_value as u64))),
                        (true, false) if rhs_value < 0 => 1,
                        (true, false) => ordering((lhs_value as u64).cmp(&(rhs_value as u64))),
                        (false, true) if lhs_value < 0 => -1,
                        (false, true) => ordering((lhs_value as u64).cmp(&(rhs_value as u64))),
                    };
                    Ok((comparison, false))
                }
                types::ETReal => {
                    let (lhs_value, lhs_null) = lhs.EvalReal(ctx, row.clone())?;
                    let (rhs_value, rhs_null) = rhs.EvalReal(ctx, row)?;
                    if lhs_null || rhs_null {
                        return Ok((0, true));
                    }
                    Ok((
                        lhs_value.partial_cmp(&rhs_value).map(ordering).unwrap_or(0),
                        false,
                    ))
                }
                types::ETDecimal => {
                    let (lhs_value, lhs_null) = lhs.EvalDecimal(ctx, row.clone())?;
                    let (rhs_value, rhs_null) = rhs.EvalDecimal(ctx, row)?;
                    if lhs_null || rhs_null {
                        return Ok((0, true));
                    }
                    Ok((lhs_value.Compare(&rhs_value) as i32, false))
                }
                types::ETString => {
                    let (lhs_value, lhs_null) = lhs.EvalString(ctx, row.clone())?;
                    let (rhs_value, rhs_null) = rhs.EvalString(ctx, row)?;
                    if lhs_null || rhs_null {
                        return Ok((0, true));
                    }
                    Ok((self.base.collator.Compare(&lhs_value, &rhs_value), false))
                }
                types::ETDatetime | types::ETTimestamp => {
                    let (lhs_value, lhs_null) = lhs.EvalTime(ctx, row.clone())?;
                    let (rhs_value, rhs_null) = rhs.EvalTime(ctx, row)?;
                    if lhs_null || rhs_null {
                        return Ok((0, true));
                    }
                    Ok((lhs_value.Compare(rhs_value), false))
                }
                types::ETDuration => {
                    let (lhs_value, lhs_null) = lhs.EvalDuration(ctx, row.clone())?;
                    let (rhs_value, rhs_null) = rhs.EvalDuration(ctx, row)?;
                    if lhs_null || rhs_null {
                        return Ok((0, true));
                    }
                    Ok((lhs_value.Compare(rhs_value), false))
                }
                types::ETJson => {
                    let (lhs_value, lhs_null) = lhs.EvalJSON(ctx, row.clone())?;
                    let (rhs_value, rhs_null) = rhs.EvalJSON(ctx, row)?;
                    if lhs_null || rhs_null {
                        return Ok((0, true));
                    }
                    Ok((types::CompareBinaryJSON(&lhs_value, &rhs_value), false))
                }
                types::ETVectorFloat32 => {
                    let (lhs_value, lhs_null) = lhs.EvalVectorFloat32(ctx, row.clone())?;
                    let (rhs_value, rhs_null) = rhs.EvalVectorFloat32(ctx, row)?;
                    if lhs_null || rhs_null {
                        return Ok((0, true));
                    }
                    Ok((lhs_value.Compare(&rhs_value), false))
                }
                other => Err(errors::New(format!(
                    "cannot compare expressions with evaluation type {other:?}"
                ))),
            }
        }

        fn compare_result(operator: &str, comparison: i32) -> bool {
            match operator {
                "lt" => comparison < 0,
                "le" => comparison <= 0,
                "gt" => comparison > 0,
                "ge" => comparison >= 0,
                "eq" | "nulleq" => comparison == 0,
                "ne" => comparison != 0,
                _ => false,
            }
        }

        fn selected_case_argument(
            &self,
            ctx: &dyn EvalContext,
            row: chunk::Row,
        ) -> Result<Option<usize>, Error> {
            let pair_end = self.base.args.len() - (self.base.args.len() % 2);
            for index in (0..pair_end).step_by(2) {
                let (condition, null) = self.base.args[index].EvalInt(ctx, row.clone())?;
                if !null && condition != 0 {
                    return Ok(Some(index + 1));
                }
            }
            Ok((self.base.args.len() % 2 == 1).then_some(self.base.args.len() - 1))
        }

        fn eval_boolean(
            &self,
            ctx: &dyn EvalContext,
            row: chunk::Row,
        ) -> Result<(i64, bool), Error> {
            match self.kind {
                CoreBuiltinKind::Compare(operator, eval_type) => {
                    if operator == "nulleq" {
                        let left = self.base.args[0].Eval(ctx, row.clone())?;
                        let right = self.base.args[1].Eval(ctx, row.clone())?;
                        if left.IsNull() || right.IsNull() {
                            return Ok((i64::from(left.IsNull() && right.IsNull()), false));
                        }
                    }
                    let (comparison, null) = self.compare(ctx, row, eval_type, 0, 1)?;
                    Ok((i64::from(Self::compare_result(operator, comparison)), null))
                }
                CoreBuiltinKind::In(eval_type) => {
                    let mut has_null = false;
                    for index in 1..self.base.args.len() {
                        let (comparison, null) =
                            self.compare(ctx, row.clone(), eval_type, 0, index)?;
                        if null {
                            has_null = true;
                        } else if comparison == 0 {
                            return Ok((1, false));
                        }
                    }
                    Ok((0, has_null))
                }
                CoreBuiltinKind::IsNull => {
                    let value = self.base.args[0].Eval(ctx, row)?;
                    Ok((i64::from(value.IsNull()), false))
                }
                CoreBuiltinKind::Logic("and") => {
                    let (left, left_null) = self.base.args[0].EvalInt(ctx, row.clone())?;
                    if !left_null && left == 0 {
                        return Ok((0, false));
                    }
                    let (right, right_null) = self.base.args[1].EvalInt(ctx, row)?;
                    if !right_null && right == 0 {
                        return Ok((0, false));
                    }
                    if left_null || right_null {
                        Ok((0, true))
                    } else {
                        Ok((1, false))
                    }
                }
                CoreBuiltinKind::Logic("or") => {
                    let (left, left_null) = self.base.args[0].EvalInt(ctx, row.clone())?;
                    if !left_null && left != 0 {
                        return Ok((1, false));
                    }
                    let (right, right_null) = self.base.args[1].EvalInt(ctx, row)?;
                    if !right_null && right != 0 {
                        return Ok((1, false));
                    }
                    if left_null || right_null {
                        Ok((0, true))
                    } else {
                        Ok((0, false))
                    }
                }
                CoreBuiltinKind::Logic("xor") => {
                    let (left, left_null) = self.base.args[0].EvalInt(ctx, row.clone())?;
                    let (right, right_null) = self.base.args[1].EvalInt(ctx, row)?;
                    Ok((
                        i64::from((left != 0) ^ (right != 0)),
                        left_null || right_null,
                    ))
                }
                CoreBuiltinKind::UnaryNot => {
                    let value = self.base.args[0].Eval(ctx, row)?;
                    if value.IsNull() {
                        return Ok((0, true));
                    }
                    Ok((i64::from(value.ToBool(ctx.TypeCtx())? == 0), false))
                }
                _ => Err(errors::New("core builtin does not return a boolean")),
            }
        }
    }

    impl builtinFunc for CoreBuiltin {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn RequiredOptionalEvalProps(&self) -> OptionalEvalPropKeySet {
            if matches!(self.kind, CoreBuiltinKind::SetVar(_)) {
                expropt::RequireOptionalEvalProps::required_optional_eval_props(
                    &expropt::SessionVarsPropReader,
                )
            } else {
                OptionalEvalPropKeySet::default()
            }
        }
        fn SafeToShareAcrossSession(&self) -> bool {
            self.base.SafeToShareAcrossSession()
        }
        fn evalInt(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<(i64, bool), Error> {
            match self.kind {
                CoreBuiltinKind::Compare(..)
                | CoreBuiltinKind::In(..)
                | CoreBuiltinKind::IsNull
                | CoreBuiltinKind::Logic(..)
                | CoreBuiltinKind::UnaryNot => self.eval_boolean(ctx, row),
                CoreBuiltinKind::UnaryMinus(types::ETInt) => {
                    let (value, null) = self.base.args[0].EvalInt(ctx, row)?;
                    Ok((
                        value
                            .checked_neg()
                            .ok_or_else(|| errors::New("BIGINT value is out of range"))?,
                        null,
                    ))
                }
                CoreBuiltinKind::Arithmetic(operator, types::ETInt) => {
                    let (left, left_null) = self.base.args[0].EvalInt(ctx, row.clone())?;
                    let (right, right_null) = self.base.args[1].EvalInt(ctx, row)?;
                    if left_null || right_null {
                        return Ok((0, true));
                    }
                    let value = match operator {
                        "plus" => left.checked_add(right),
                        "minus" => left.checked_sub(right),
                        "mul" => left.checked_mul(right),
                        "intdiv" if right != 0 => left.checked_div(right),
                        "mod" if right != 0 => left.checked_rem(right),
                        "intdiv" | "mod" => return Ok((0, true)),
                        _ => None,
                    }
                    .ok_or_else(|| errors::New("BIGINT value is out of range"))?;
                    Ok((value, false))
                }
                CoreBuiltinKind::Case => match self.selected_case_argument(ctx, row.clone())? {
                    Some(index) => self.base.args[index].EvalInt(ctx, row),
                    None => Ok((0, true)),
                },
                CoreBuiltinKind::IfNull(types::ETInt) => {
                    let first = self.base.args[0].EvalInt(ctx, row.clone())?;
                    if first.1 {
                        self.base.args[1].EvalInt(ctx, row)
                    } else {
                        Ok(first)
                    }
                }
                CoreBuiltinKind::SetVar(types::ETInt) => {
                    let (value, null) = self.set_user_variable(ctx, row)?;
                    Ok((if null { 0 } else { value.GetInt64() }, null))
                }
                CoreBuiltinKind::TimeToSec => {
                    let (value, null) = self.base.args[0].EvalDuration(ctx, row)?;
                    if null {
                        return Ok((0, true));
                    }
                    crate::builtin_time_kernel::time_to_sec(&value.String())
                        .map(|seconds| (seconds, false))
                        .map_err(|error| errors::New(error.to_string()))
                }
                CoreBuiltinKind::FindInSet => {
                    let (needle, needle_null) = self.base.args[0].EvalString(ctx, row.clone())?;
                    let (list, list_null) = self.base.args[1].EvalString(ctx, row)?;
                    if needle_null || list_null {
                        return Ok((0, true));
                    }
                    if needle.contains(',') {
                        return Ok((0, false));
                    }
                    let position = list
                        .split(',')
                        .position(|candidate| self.base.collator.Compare(candidate, &needle) == 0)
                        .map_or(0, |index| index as i64 + 1);
                    Ok((position, false))
                }
                CoreBuiltinKind::Extract(mode) => {
                    let (unit, unit_null) = self.base.args[0].EvalString(ctx, row.clone())?;
                    if unit_null {
                        return Ok((0, true));
                    }
                    let value = match mode {
                        ExtractMode::Datetime => {
                            let (datetime, null) = self.base.args[1].EvalTime(ctx, row)?;
                            if null {
                                return Ok((0, true));
                            }
                            types_dependency::time::ExtractDatetimeNum(&datetime, &unit)
                        }
                        ExtractMode::Duration => {
                            let (duration, null) = self.base.args[1].EvalDuration(ctx, row)?;
                            if null {
                                return Ok((0, true));
                            }
                            types_dependency::time::ExtractDurationNum(&duration, &unit)
                        }
                        ExtractMode::DatetimeFromString => {
                            let (source, null) = self.base.args[1].EvalString(ctx, row)?;
                            if null {
                                return Ok((0, true));
                            }
                            let (duration, _) = types_dependency::time::ParseDuration(
                                &ctx.TypeCtx(),
                                &source,
                                types_dependency::time::GetFsp(&source),
                            )
                            .map_err(|error| errors::New(error.to_string()))?;
                            let duration_result =
                                types_dependency::time::ExtractDurationNum(&duration, &unit)
                                    .map_err(|error| errors::New(error.to_string()))?;
                            match types_dependency::time::ParseDatetime(&ctx.TypeCtx(), &source) {
                                Ok(datetime)
                                    if datetime.Hour() == duration.Hour()
                                        && datetime.Minute() == duration.Minute()
                                        && datetime.Second() == duration.Second()
                                        && datetime.Year() > 0 =>
                                {
                                    types_dependency::time::ExtractDatetimeNum(&datetime, &unit)
                                }
                                _ => return Ok((duration_result, false)),
                            }
                        }
                    };
                    value
                        .map(|value| (value, false))
                        .map_err(|error| errors::New(error.to_string()))
                }
                CoreBuiltinKind::Sleep => {
                    let (seconds, null) = self.base.args[0].EvalReal(ctx, row)?;
                    let values = [if null { None } else { Some(seconds) }];
                    let session = crate::builtin_miscellaneous_vec_kernel::SleepSession::default();
                    let outcome = crate::builtin_miscellaneous_vec_kernel::vec_sleep(
                        &values,
                        &session,
                        crate::builtin_miscellaneous_vec_kernel::InvalidArgumentMode::Error,
                    )
                    .map_err(|error| errors::New(error.to_string()))?;
                    Ok((outcome.values[0], false))
                }
                CoreBuiltinKind::Ascii => {
                    let (value, null) = self.base.args[0].EvalString(ctx, row)?;
                    if null {
                        return Ok((0, true));
                    }
                    Ok((value.as_bytes().first().copied().unwrap_or(0) as i64, false))
                }
                CoreBuiltinKind::Length => {
                    let (value, null) = self.base.args[0].EvalString(ctx, row)?;
                    if null {
                        return Ok((0, true));
                    }
                    Ok((value.len() as i64, false))
                }
                CoreBuiltinKind::TiDBShard => {
                    let (value, null) = self.base.args[0].EvalInt(ctx, row)?;
                    if null {
                        return Ok((0, true));
                    }
                    Ok((
                        i64::from(crate::builtin_miscellaneous_kernel::tidb_shard(value)),
                        false,
                    ))
                }
                CoreBuiltinKind::Weekday => {
                    let (date, null) = self.base.args[0].EvalTime(ctx, row)?;
                    if null {
                        return Ok((0, true));
                    }
                    if date.IsZero() || date.InvalidZero() {
                        let error = types::ErrWrongValue
                            .GenWithStackByArgs(&[types::DateTimeStr.into(), date.String().into()]);
                        return match crate::expression_errors_kernel::handleInvalidTimeError(
                            ctx,
                            Some(error),
                        ) {
                            Some(error) => Err(error.into()),
                            None => Ok((0, true)),
                        };
                    }
                    let datetime = date
                        .GoTime(ctx.Location())
                        .map_err(|error| errors::New(error.to_string()))?
                        .naive_local();
                    Ok((
                        i64::from(crate::builtin_time_kernel::week_day(datetime)),
                        false,
                    ))
                }
                CoreBuiltinKind::CharLength(binary) => {
                    let (value, null) = self.base.args[0].EvalString(ctx, row)?;
                    if null {
                        return Ok((0, true));
                    }
                    Ok((
                        if binary {
                            value.len() as i64
                        } else {
                            crate::builtin_string_kernel::charLength(&value)
                        },
                        false,
                    ))
                }
                CoreBuiltinKind::Like | CoreBuiltinKind::Ilike => {
                    let (value, value_null) = self.base.args[0].EvalString(ctx, row.clone())?;
                    if value_null {
                        return Ok((0, true));
                    }
                    let (pattern_source, pattern_null) =
                        self.base.args[1].EvalString(ctx, row.clone())?;
                    if pattern_null {
                        return Ok((0, true));
                    }
                    let (escape, escape_null) = self.base.args[2].EvalInt(ctx, row)?;
                    if escape_null {
                        return Ok((0, true));
                    }
                    if let Some(signature) = self.ilike.as_ref() {
                        return signature
                            .eval_int(Some(&value), Some(&pattern_source), Some(escape))
                            .map(|value| value.map_or((0, true), |value| (value, false)))
                            .map_err(|error| errors::New(error.to_string()));
                    }
                    let mut pattern = self.base.collator.Pattern();
                    pattern.Compile(&pattern_source, escape as u8);
                    Ok((i64::from(pattern.DoMatch(&value)), false))
                }
                CoreBuiltinKind::IsIPv4 | CoreBuiltinKind::IsIPv6 => {
                    let (value, null) = self.base.args[0].EvalString(ctx, row)?;
                    if null {
                        return Ok((0, true));
                    }
                    let result = match self.kind {
                        CoreBuiltinKind::IsIPv4 => {
                            crate::builtin_miscellaneous_kernel::is_ipv4(Some(&value))
                        }
                        CoreBuiltinKind::IsIPv6 => {
                            crate::builtin_miscellaneous_kernel::is_ipv6(Some(&value))
                        }
                        _ => unreachable!(),
                    };
                    Ok(result.map_or((0, true), |value| (i64::from(value), false)))
                }
                CoreBuiltinKind::RegexpInstr => {
                    let mut strings = Vec::with_capacity(2);
                    for index in 0..2 {
                        let (value, null) = self.base.args[index].EvalString(ctx, row.clone())?;
                        if null {
                            return Ok((0, true));
                        }
                        strings.push(value);
                    }
                    let optional_int = |index: usize, default: i64| {
                        self.base
                            .args
                            .get(index)
                            .map_or(Ok((default, false)), |argument| {
                                argument.EvalInt(ctx, row.clone())
                            })
                    };
                    let (position, position_null) = optional_int(2, 1)?;
                    let (occurrence, occurrence_null) = optional_int(3, 1)?;
                    let (return_option, return_option_null) = optional_int(4, 0)?;
                    let (match_type, match_type_null) = self.base.args.get(5).map_or_else(
                        || Ok((String::new(), false)),
                        |argument| argument.EvalString(ctx, row.clone()),
                    )?;
                    if position_null || occurrence_null || return_option_null || match_type_null {
                        return Ok((0, true));
                    }
                    crate::builtin_regexp_kernel::RegexpEngine::new(false)
                        .regexp_instr(
                            &strings[0],
                            &strings[1],
                            position,
                            occurrence,
                            return_option,
                            &match_type,
                        )
                        .map(|value| (value, false))
                        .map_err(|error| errors::New(error.to_string()))
                }
                CoreBuiltinKind::Row => Err(crate::ErrOperandColumns.GenWithStackByArgs(1)),
                _ => Err(errors::New("core builtin does not return INT")),
            }
        }
        fn evalReal(&self, ctx: &dyn EvalContext, row: chunk::Row) -> Result<(f64, bool), Error> {
            match self.kind {
                CoreBuiltinKind::UnaryMinus(types::ETReal) => {
                    let (value, null) = self.base.args[0].EvalReal(ctx, row)?;
                    Ok((-value, null))
                }
                CoreBuiltinKind::Arithmetic(operator, types::ETReal) => {
                    let (left, left_null) = self.base.args[0].EvalReal(ctx, row.clone())?;
                    let (right, right_null) = self.base.args[1].EvalReal(ctx, row)?;
                    if left_null || right_null {
                        return Ok((0.0, true));
                    }
                    let value = match operator {
                        "plus" => left + right,
                        "minus" => left - right,
                        "mul" => left * right,
                        "div" if right != 0.0 => left / right,
                        "mod" if right != 0.0 => left % right,
                        "div" | "mod" => return Ok((0.0, true)),
                        _ => return Err(errors::New("unsupported REAL arithmetic operator")),
                    };
                    Ok((value, false))
                }
                CoreBuiltinKind::Case => match self.selected_case_argument(ctx, row.clone())? {
                    Some(index) => self.base.args[index].EvalReal(ctx, row),
                    None => Ok((0.0, true)),
                },
                CoreBuiltinKind::IfNull(types::ETReal) => {
                    let first = self.base.args[0].EvalReal(ctx, row.clone())?;
                    if first.1 {
                        self.base.args[1].EvalReal(ctx, row)
                    } else {
                        Ok(first)
                    }
                }
                CoreBuiltinKind::SetVar(types::ETReal) => {
                    let (value, null) = self.set_user_variable(ctx, row)?;
                    Ok((if null { 0.0 } else { value.GetFloat64() }, null))
                }
                CoreBuiltinKind::Rand => Ok((rand::random::<f64>(), false)),
                CoreBuiltinKind::Atan => {
                    let (left, left_null) = self.base.args[0].EvalReal(ctx, row.clone())?;
                    if left_null {
                        return Ok((0.0, true));
                    }
                    if self.base.args.len() == 1 {
                        return Ok((left.atan(), false));
                    }
                    let (right, right_null) = self.base.args[1].EvalReal(ctx, row)?;
                    Ok((left.atan2(right), right_null))
                }
                CoreBuiltinKind::Pow => {
                    let (left, left_null) = self.base.args[0].EvalReal(ctx, row.clone())?;
                    let (right, right_null) = self.base.args[1].EvalReal(ctx, row)?;
                    if left_null || right_null {
                        return Ok((0.0, true));
                    }
                    crate::builtin_math_kernel::pow(left, right)
                        .map(|value| (value, false))
                        .map_err(|error| errors::New(error.to_string()))
                }
                _ => Err(errors::New("core builtin does not return REAL")),
            }
        }
        fn evalDecimal(
            &self,
            ctx: &dyn EvalContext,
            row: chunk::Row,
        ) -> Result<(types::MyDecimal, bool), Error> {
            match self.kind {
                CoreBuiltinKind::UnaryMinus(types::ETDecimal) => {
                    let (value, null) = self.base.args[0].EvalDecimal(ctx, row)?;
                    if null {
                        return Ok((types::MyDecimal::default(), true));
                    }
                    let mut result = types::MyDecimal::default();
                    types_decimal::mydecimal::DecimalSub(
                        &types::MyDecimal::default(),
                        &value,
                        &mut result,
                    )
                    .map_err(|error| errors::New(error.to_string()))?;
                    Ok((result, false))
                }
                CoreBuiltinKind::Arithmetic(operator, types::ETDecimal) => {
                    let (left, left_null) = self.base.args[0].EvalDecimal(ctx, row.clone())?;
                    let (right, right_null) = self.base.args[1].EvalDecimal(ctx, row)?;
                    if left_null || right_null {
                        return Ok((types::MyDecimal::default(), true));
                    }
                    let mut result = types::MyDecimal::default();
                    let operation = match operator {
                        "plus" => types_decimal::mydecimal::DecimalAdd(&left, &right, &mut result),
                        "minus" => types_decimal::mydecimal::DecimalSub(&left, &right, &mut result),
                        "mul" => types_decimal::mydecimal::DecimalMul(&left, &right, &mut result),
                        "div" => {
                            types_decimal::mydecimal::DecimalDiv(&left, &right, &mut result, 4)
                        }
                        "mod" => types_decimal::mydecimal::DecimalMod(&left, &right, &mut result),
                        _ => return Err(errors::New("unsupported DECIMAL arithmetic operator")),
                    };
                    match operation {
                        Ok(()) => Ok((result, false)),
                        Err(error) if error.to_string().contains("division by zero") => {
                            Ok((types::MyDecimal::default(), true))
                        }
                        Err(error) => Err(errors::New(error.to_string())),
                    }
                }
                CoreBuiltinKind::Case => match self.selected_case_argument(ctx, row.clone())? {
                    Some(index) => self.base.args[index].EvalDecimal(ctx, row),
                    None => Ok((types::MyDecimal::default(), true)),
                },
                CoreBuiltinKind::IfNull(types::ETDecimal) => {
                    let first = self.base.args[0].EvalDecimal(ctx, row.clone())?;
                    if first.1 {
                        self.base.args[1].EvalDecimal(ctx, row)
                    } else {
                        Ok(first)
                    }
                }
                CoreBuiltinKind::SetVar(types::ETDecimal) => {
                    let (value, null) = self.set_user_variable(ctx, row)?;
                    Ok((
                        if null {
                            types::MyDecimal::default()
                        } else {
                            value.GetMysqlDecimal()
                        },
                        null,
                    ))
                }
                _ => Err(errors::New("core builtin does not return DECIMAL")),
            }
        }
        fn evalString(
            &self,
            ctx: &dyn EvalContext,
            row: chunk::Row,
        ) -> Result<(String, bool), Error> {
            match self.kind {
                CoreBuiltinKind::Uuid => {
                    Ok((crate::builtin_miscellaneous_kernel::uuid_v1(), false))
                }
                CoreBuiltinKind::Md5 => {
                    let (value, null) = self.base.args[0].EvalString(ctx, row)?;
                    if null {
                        return Ok((String::new(), true));
                    }
                    Ok((
                        crate::builtin_encryption_kernel::md5_hash(value.as_bytes()),
                        false,
                    ))
                }
                CoreBuiltinKind::SetVar(types::ETString) => {
                    let (value, null) = self.set_user_variable(ctx, row)?;
                    Ok((
                        if null {
                            String::new()
                        } else {
                            value.ToString()?
                        },
                        null,
                    ))
                }
                CoreBuiltinKind::Case => match self.selected_case_argument(ctx, row.clone())? {
                    Some(index) => self.base.args[index].EvalString(ctx, row),
                    None => Ok((String::new(), true)),
                },
                CoreBuiltinKind::IfNull(types::ETString) => {
                    let first = self.base.args[0].EvalString(ctx, row.clone())?;
                    if first.1 {
                        self.base.args[1].EvalString(ctx, row)
                    } else {
                        Ok(first)
                    }
                }
                CoreBuiltinKind::Hex(integer) => {
                    if integer {
                        let (value, null) = self.base.args[0].EvalInt(ctx, row)?;
                        Ok((
                            if null {
                                String::new()
                            } else {
                                crate::builtin_string_kernel::hexInt(value)
                            },
                            null,
                        ))
                    } else {
                        let (value, null) = self.base.args[0].EvalString(ctx, row)?;
                        Ok((
                            if null {
                                String::new()
                            } else {
                                crate::builtin_string_kernel::hexString(value.as_bytes())
                            },
                            null,
                        ))
                    }
                }
                CoreBuiltinKind::Space => {
                    let (count, null) = self.base.args[0].EvalInt(ctx, row)?;
                    if null {
                        return Ok((String::new(), true));
                    }
                    Ok((" ".repeat(count.max(0) as usize), false))
                }
                CoreBuiltinKind::Rpad => {
                    let (value, value_null) = self.base.args[0].EvalString(ctx, row.clone())?;
                    let (length, length_null) = self.base.args[1].EvalInt(ctx, row.clone())?;
                    let (pad, pad_null) = self.base.args[2].EvalString(ctx, row)?;
                    if value_null || length_null || pad_null {
                        return Ok((String::new(), true));
                    }
                    Ok(crate::builtin_string_kernel::rpadUtf8(&value, length, &pad)
                        .map_or((String::new(), true), |value| (value, false)))
                }
                CoreBuiltinKind::Insert => {
                    let (value, value_null) = self.base.args[0].EvalString(ctx, row.clone())?;
                    let (position, position_null) = self.base.args[1].EvalInt(ctx, row.clone())?;
                    let (length, length_null) = self.base.args[2].EvalInt(ctx, row.clone())?;
                    let (replacement, replacement_null) = self.base.args[3].EvalString(ctx, row)?;
                    if value_null || position_null || length_null || replacement_null {
                        return Ok((String::new(), true));
                    }
                    Ok(crate::builtin_string_kernel::insertUtf8(
                        &value,
                        position,
                        length,
                        &replacement,
                        usize::MAX,
                    )
                    .map_or((String::new(), true), |value| (value, false)))
                }
                CoreBuiltinKind::Substring(binary) => {
                    let (value, value_null) = self.base.args[0].EvalString(ctx, row.clone())?;
                    let (position, position_null) = self.base.args[1].EvalInt(ctx, row.clone())?;
                    if value_null || position_null {
                        return Ok((String::new(), true));
                    }
                    let length = if self.base.args.len() == 3 {
                        let (length, null) = self.base.args[2].EvalInt(ctx, row)?;
                        if null {
                            return Ok((String::new(), true));
                        }
                        Some(length)
                    } else {
                        None
                    };
                    let result = if binary {
                        String::from_utf8_lossy(&crate::builtin_string_kernel::substringBytes(
                            value.as_bytes(),
                            position,
                            length,
                        ))
                        .into_owned()
                    } else {
                        crate::builtin_string_kernel::substringUtf8(&value, position, length)
                    };
                    Ok((result, false))
                }
                CoreBuiltinKind::Trim => {
                    let (value, value_null) = self.base.args[0].EvalString(ctx, row.clone())?;
                    if value_null {
                        return Ok((String::new(), true));
                    }
                    if self.base.args.len() == 1 {
                        return Ok((value.trim_matches(' ').to_owned(), false));
                    }
                    let (remove, remove_null) = self.base.args[1].EvalString(ctx, row.clone())?;
                    if remove_null {
                        return Ok((String::new(), true));
                    }
                    let direction = if self.base.args.len() == 3 {
                        let (direction, direction_null) = self.base.args[2].EvalInt(ctx, row)?;
                        if direction_null {
                            return Ok((String::new(), true));
                        }
                        direction
                    } else {
                        1
                    };
                    let result = match direction {
                        2 => crate::builtin_string_kernel::trimLeft(&value, &remove),
                        3 => crate::builtin_string_kernel::trimRight(&value, &remove),
                        _ => crate::builtin_string_kernel::trimBoth(&value, &remove),
                    };
                    Ok((result.to_owned(), false))
                }
                CoreBuiltinKind::RegexpSubstr => {
                    let (expression, expression_null) =
                        self.base.args[0].EvalString(ctx, row.clone())?;
                    let (pattern, pattern_null) = self.base.args[1].EvalString(ctx, row.clone())?;
                    let (position, position_null) =
                        self.base.args.get(2).map_or(Ok((1, false)), |argument| {
                            argument.EvalInt(ctx, row.clone())
                        })?;
                    let (occurrence, occurrence_null) =
                        self.base.args.get(3).map_or(Ok((1, false)), |argument| {
                            argument.EvalInt(ctx, row.clone())
                        })?;
                    let (match_type, match_type_null) = self.base.args.get(4).map_or_else(
                        || Ok((String::new(), false)),
                        |argument| argument.EvalString(ctx, row),
                    )?;
                    if expression_null
                        || pattern_null
                        || position_null
                        || occurrence_null
                        || match_type_null
                    {
                        return Ok((String::new(), true));
                    }
                    crate::builtin_regexp_kernel::RegexpEngine::new(false)
                        .regexp_substr(&expression, &pattern, position, occurrence, &match_type)
                        .map(|value| value.map_or((String::new(), true), |value| (value, false)))
                        .map_err(|error| errors::New(error.to_string()))
                }
                CoreBuiltinKind::RegexpReplace => {
                    let mut strings = Vec::with_capacity(3);
                    for index in 0..3 {
                        let (value, null) = self.base.args[index].EvalString(ctx, row.clone())?;
                        if null {
                            return Ok((String::new(), true));
                        }
                        strings.push(value);
                    }
                    let (position, position_null) =
                        self.base.args.get(3).map_or(Ok((1, false)), |argument| {
                            argument.EvalInt(ctx, row.clone())
                        })?;
                    let (occurrence, occurrence_null) =
                        self.base.args.get(4).map_or(Ok((0, false)), |argument| {
                            argument.EvalInt(ctx, row.clone())
                        })?;
                    let (match_type, match_type_null) = self.base.args.get(5).map_or_else(
                        || Ok((String::new(), false)),
                        |argument| argument.EvalString(ctx, row),
                    )?;
                    if position_null || occurrence_null || match_type_null {
                        return Ok((String::new(), true));
                    }
                    crate::builtin_regexp_kernel::RegexpEngine::new(false)
                        .regexp_replace(
                            &strings[0],
                            &strings[1],
                            &strings[2],
                            position,
                            occurrence,
                            &match_type,
                        )
                        .map(|value| (value, false))
                        .map_err(|error| errors::New(error.to_string()))
                }
                _ => Err(errors::New("core builtin does not return STRING")),
            }
        }
        fn evalTime(
            &self,
            ctx: &dyn EvalContext,
            row: chunk::Row,
        ) -> Result<(types::Time, bool), Error> {
            match self.kind {
                CoreBuiltinKind::SetVar(types::ETDatetime) => {
                    let (value, null) = self.set_user_variable(ctx, row)?;
                    Ok((
                        if null {
                            types::ZeroTime
                        } else {
                            value.GetMysqlTime()
                        },
                        null,
                    ))
                }
                CoreBuiltinKind::Case => match self.selected_case_argument(ctx, row.clone())? {
                    Some(index) => self.base.args[index].EvalTime(ctx, row),
                    None => Ok((types::ZeroTime, true)),
                },
                CoreBuiltinKind::IfNull(types::ETDatetime | types::ETTimestamp) => {
                    let first = self.base.args[0].EvalTime(ctx, row.clone())?;
                    if first.1 {
                        self.base.args[1].EvalTime(ctx, row)
                    } else {
                        Ok(first)
                    }
                }
                CoreBuiltinKind::Now => {
                    let fsp = if let Some(argument) = self.base.args.first() {
                        let (fsp, null) = argument.EvalInt(ctx, row)?;
                        if null {
                            0
                        } else {
                            validate_now_fsp(fsp, "now")?
                        }
                    } else {
                        0
                    };
                    // Go evalNowWithFsp uses the statement timestamp, never a
                    // per-row clock. MySQL truncates fractional seconds here.
                    let now = ctx.CurrentTime()?.with_timezone(&ctx.Location());
                    use chrono::Timelike;
                    let quantum = 10_u32.pow(9 - fsp as u32);
                    let truncated = now
                        .with_nanosecond(now.nanosecond() / quantum * quantum)
                        .ok_or_else(|| errors::New("invalid NOW fractional precision"))?;
                    Ok((
                        types_dependency::time::NewTime(
                            types_dependency::time::FromGoTime(truncated),
                            mysql::TypeDatetime,
                            fsp,
                        ),
                        false,
                    ))
                }
                CoreBuiltinKind::DateAddSub(add) => {
                    let (time, time_null) = self.base.args[0].EvalTime(ctx, row.clone())?;
                    let (interval, interval_null) = self.base.args[1].EvalInt(ctx, row.clone())?;
                    let (unit, unit_null) = self.base.args[2].EvalString(ctx, row)?;
                    if time_null || interval_null || unit_null {
                        return Ok((types::ZeroTime, true));
                    }
                    let signed_interval = if add { interval } else { -interval };
                    let unit = unit.to_ascii_uppercase();
                    if matches!(unit.as_str(), "MONTH" | "QUARTER" | "YEAR") {
                        let month_multiplier = match unit.as_str() {
                            "MONTH" => 1_i64,
                            "QUARTER" => 3,
                            "YEAR" => 12,
                            _ => unreachable!(),
                        };
                        let month_delta = signed_interval
                            .checked_mul(month_multiplier)
                            .ok_or_else(|| errors::New("date arithmetic interval overflow"))?;
                        let month_index = i64::from(time.Year())
                            .checked_mul(12)
                            .and_then(|value| value.checked_add(i64::from(time.Month()) - 1))
                            .and_then(|value| value.checked_add(month_delta))
                            .ok_or_else(|| errors::New("date arithmetic interval overflow"))?;
                        let year = month_index.div_euclid(12);
                        let month = month_index.rem_euclid(12) + 1;
                        if !(0..=9999).contains(&year) {
                            return Err(errors::New("datetime function overflow"));
                        }
                        let last_day =
                            types_dependency::core_time::GetLastDay(year as i32, month as i32);
                        let core = types_dependency::time::FromDate(
                            year as i32,
                            month as i32,
                            time.Day().min(last_day),
                            time.Hour(),
                            time.Minute(),
                            time.Second(),
                            time.Microsecond(),
                        );
                        return Ok((
                            types_dependency::time::NewTime(core, time.Type(), time.Fsp()),
                            false,
                        ));
                    }
                    let multiplier = match unit.as_str() {
                        "MICROSECOND" => 1_000_i64,
                        "SECOND" => 1_000_000_000,
                        "MINUTE" => 60 * 1_000_000_000,
                        "HOUR" => 60 * 60 * 1_000_000_000,
                        "DAY" => 24 * 60 * 60 * 1_000_000_000,
                        "WEEK" => 7 * 24 * 60 * 60 * 1_000_000_000,
                        other => {
                            return Err(errors::New(format!(
                                "date arithmetic unit '{other}' requires calendar evaluation"
                            )));
                        }
                    };
                    let delta = signed_interval
                        .checked_mul(multiplier)
                        .ok_or_else(|| errors::New("date arithmetic interval overflow"))?;
                    let duration = types::Duration {
                        Duration: delta,
                        Fsp: 0,
                    };
                    time.Add(&ctx.TypeCtx(), duration)
                        .map(|value| (value, false))
                        .map_err(|error| errors::New(error.to_string()))
                }
                _ => Err(errors::New("core builtin does not return TIME")),
            }
        }
        fn evalDuration(
            &self,
            ctx: &dyn EvalContext,
            row: chunk::Row,
        ) -> Result<(types::Duration, bool), Error> {
            match self.kind {
                CoreBuiltinKind::Case => match self.selected_case_argument(ctx, row.clone())? {
                    Some(index) => self.base.args[index].EvalDuration(ctx, row),
                    None => Ok((types::Duration::default(), true)),
                },
                CoreBuiltinKind::IfNull(types::ETDuration) => {
                    let first = self.base.args[0].EvalDuration(ctx, row.clone())?;
                    if first.1 {
                        self.base.args[1].EvalDuration(ctx, row)
                    } else {
                        Ok(first)
                    }
                }
                _ => Err(errors::New("core builtin does not return DURATION")),
            }
        }
        fn evalJSON(
            &self,
            ctx: &dyn EvalContext,
            row: chunk::Row,
        ) -> Result<(types::BinaryJSON, bool), Error> {
            match self.kind {
                CoreBuiltinKind::JsonExtract => {
                    let (document, null) = self.base.args[0].EvalJSON(ctx, row.clone())?;
                    if null {
                        return Ok((types::BinaryJSON::default(), true));
                    }
                    let mut paths = Vec::with_capacity(self.base.args.len() - 1);
                    for argument in &self.base.args[1..] {
                        let (path, null) = argument.EvalString(ctx, row.clone())?;
                        if null {
                            return Ok((types::BinaryJSON::default(), true));
                        }
                        paths.push(
                            ParseJSONPathExpr(&path)
                                .map_err(|error| errors::New(error.to_string()))?,
                        );
                    }
                    Ok(document
                        .Extract(&paths)?
                        .map_or((types::BinaryJSON::default(), true), |value| (value, false)))
                }
                CoreBuiltinKind::Case => match self.selected_case_argument(ctx, row.clone())? {
                    Some(index) => self.base.args[index].EvalJSON(ctx, row),
                    None => Ok((types::BinaryJSON::default(), true)),
                },
                CoreBuiltinKind::IfNull(types::ETJson) => {
                    let first = self.base.args[0].EvalJSON(ctx, row.clone())?;
                    if first.1 {
                        self.base.args[1].EvalJSON(ctx, row)
                    } else {
                        Ok(first)
                    }
                }
                _ => Err(errors::New("core builtin does not return JSON")),
            }
        }
        fn evalVectorFloat32(
            &self,
            ctx: &dyn EvalContext,
            row: chunk::Row,
        ) -> Result<(types::VectorFloat32, bool), Error> {
            match self.kind {
                CoreBuiltinKind::Case => match self.selected_case_argument(ctx, row.clone())? {
                    Some(index) => self.base.args[index].EvalVectorFloat32(ctx, row),
                    None => Ok((types::ZeroVectorFloat32(), true)),
                },
                CoreBuiltinKind::IfNull(types::ETVectorFloat32) => {
                    let first = self.base.args[0].EvalVectorFloat32(ctx, row.clone())?;
                    if first.1 {
                        self.base.args[1].EvalVectorFloat32(ctx, row)
                    } else {
                        Ok(first)
                    }
                }
                _ => Err(errors::New("core builtin does not return VECTOR")),
            }
        }
        /// `getArgs`：builtin 内部辅助。
        fn getArgs(&self) -> &[Box<dyn Expression>] {
            &self.base.args
        }
        /// `getArgsMut`：builtin 内部辅助。
        fn getArgsMut(&mut self) -> &mut [Box<dyn Expression>] {
            &mut self.base.args
        }
        fn equal(&self, ctx: &dyn EvalContext, other: &dyn builtinFunc) -> bool {
            other
                .as_any()
                .downcast_ref::<Self>()
                .is_some_and(|rhs| self.kind == rhs.kind && self.base.equal(ctx, &rhs.base))
        }
        /// `getRetTp`：builtin 内部辅助。
        fn getRetTp(&self) -> &types::FieldType {
            &self.base.return_type
        }
        /// `setPbCode`：builtin 内部辅助。
        fn setPbCode(&mut self, code: i32) {
            self.base.pb_code = code
        }
        fn PbCode(&self) -> i32 {
            self.base.pb_code
        }
        /// `setCollator`：builtin 内部辅助。
        fn setCollator(&mut self, collator: Box<dyn collate::Collator>) {
            self.base.collator = collator
        }
        fn collator(&self) -> &dyn collate::Collator {
            self.base.collator.as_ref()
        }
        fn Clone(&self) -> Box<dyn builtinFunc> {
            Box::new(self.clone())
        }
        fn MemoryUsage(&self) -> i64 {
            self.base.memory_usage()
        }
        fn vectorized(&self) -> bool {
            false
        }
    }

    fn comparison_signature(operator: &str, eval_type: types::EvalType) -> &'static str {
        match (operator, eval_type) {
            ("eq", types::ETInt) => "builtinEQIntSig",
            ("eq", types::ETReal) => "builtinEQRealSig",
            ("eq", types::ETDecimal) => "builtinEQDecimalSig",
            ("eq", types::ETString) => "builtinEQStringSig",
            ("eq", types::ETDuration) => "builtinEQDurationSig",
            ("eq", types::ETDatetime | types::ETTimestamp) => "builtinEQTimeSig",
            ("eq", types::ETJson) => "builtinEQJSONSig",
            ("eq", types::ETVectorFloat32) => "builtinEQVectorFloat32Sig",
            ("ne", types::ETInt) => "builtinNEIntSig",
            ("ne", types::ETReal) => "builtinNERealSig",
            ("ne", types::ETDecimal) => "builtinNEDecimalSig",
            ("ne", types::ETString) => "builtinNEStringSig",
            ("ne", types::ETDuration) => "builtinNEDurationSig",
            ("ne", types::ETDatetime | types::ETTimestamp) => "builtinNETimeSig",
            ("ne", types::ETJson) => "builtinNEJSONSig",
            ("ne", types::ETVectorFloat32) => "builtinNEVectorFloat32Sig",
            ("lt", types::ETInt) => "builtinLTIntSig",
            ("lt", types::ETReal) => "builtinLTRealSig",
            ("lt", types::ETDecimal) => "builtinLTDecimalSig",
            ("lt", types::ETString) => "builtinLTStringSig",
            ("lt", types::ETDuration) => "builtinLTDurationSig",
            ("lt", types::ETDatetime | types::ETTimestamp) => "builtinLTTimeSig",
            ("lt", types::ETJson) => "builtinLTJSONSig",
            ("lt", types::ETVectorFloat32) => "builtinLTVectorFloat32Sig",
            ("le", types::ETInt) => "builtinLEIntSig",
            ("le", types::ETReal) => "builtinLERealSig",
            ("le", types::ETDecimal) => "builtinLEDecimalSig",
            ("le", types::ETString) => "builtinLEStringSig",
            ("le", types::ETDuration) => "builtinLEDurationSig",
            ("le", types::ETDatetime | types::ETTimestamp) => "builtinLETimeSig",
            ("le", types::ETJson) => "builtinLEJSONSig",
            ("le", types::ETVectorFloat32) => "builtinLEVectorFloat32Sig",
            ("gt", types::ETInt) => "builtinGTIntSig",
            ("gt", types::ETReal) => "builtinGTRealSig",
            ("gt", types::ETDecimal) => "builtinGTDecimalSig",
            ("gt", types::ETString) => "builtinGTStringSig",
            ("gt", types::ETDuration) => "builtinGTDurationSig",
            ("gt", types::ETDatetime | types::ETTimestamp) => "builtinGTTimeSig",
            ("gt", types::ETJson) => "builtinGTJSONSig",
            ("gt", types::ETVectorFloat32) => "builtinGTVectorFloat32Sig",
            ("ge", types::ETInt) => "builtinGEIntSig",
            ("ge", types::ETReal) => "builtinGERealSig",
            ("ge", types::ETDecimal) => "builtinGEDecimalSig",
            ("ge", types::ETString) => "builtinGEStringSig",
            ("ge", types::ETDuration) => "builtinGEDurationSig",
            ("ge", types::ETDatetime | types::ETTimestamp) => "builtinGETimeSig",
            ("ge", types::ETJson) => "builtinGEJSONSig",
            ("ge", types::ETVectorFloat32) => "builtinGEVectorFloat32Sig",
            ("nulleq", types::ETInt) => "builtinNullEQIntSig",
            ("nulleq", types::ETReal) => "builtinNullEQRealSig",
            ("nulleq", types::ETDecimal) => "builtinNullEQDecimalSig",
            ("nulleq", types::ETString) => "builtinNullEQStringSig",
            ("nulleq", types::ETDuration) => "builtinNullEQDurationSig",
            ("nulleq", types::ETDatetime | types::ETTimestamp) => "builtinNullEQTimeSig",
            ("nulleq", types::ETJson) => "builtinNullEQJSONSig",
            ("nulleq", types::ETVectorFloat32) => "builtinNullEQVectorFloat32Sig",
            _ => "builtinEQIntSig",
        }
    }

    fn comparison_pb_code(operator: &str, eval_type: types::EvalType) -> i32 {
        use tipb::ScalarFuncSig as Sig;
        (match (operator, eval_type) {
            ("lt", types::ETInt) => Sig::LtInt,
            ("lt", types::ETReal) => Sig::LtReal,
            ("lt", types::ETDecimal) => Sig::LtDecimal,
            ("lt", types::ETString) => Sig::LtString,
            ("lt", types::ETDatetime | types::ETTimestamp) => Sig::LtTime,
            ("lt", types::ETDuration) => Sig::LtDuration,
            ("lt", types::ETJson) => Sig::LtJson,
            ("lt", types::ETVectorFloat32) => Sig::LtVectorFloat32,
            ("le", types::ETInt) => Sig::LeInt,
            ("le", types::ETReal) => Sig::LeReal,
            ("le", types::ETDecimal) => Sig::LeDecimal,
            ("le", types::ETString) => Sig::LeString,
            ("le", types::ETDatetime | types::ETTimestamp) => Sig::LeTime,
            ("le", types::ETDuration) => Sig::LeDuration,
            ("le", types::ETJson) => Sig::LeJson,
            ("le", types::ETVectorFloat32) => Sig::LeVectorFloat32,
            ("gt", types::ETInt) => Sig::GtInt,
            ("gt", types::ETReal) => Sig::GtReal,
            ("gt", types::ETDecimal) => Sig::GtDecimal,
            ("gt", types::ETString) => Sig::GtString,
            ("gt", types::ETDatetime | types::ETTimestamp) => Sig::GtTime,
            ("gt", types::ETDuration) => Sig::GtDuration,
            ("gt", types::ETJson) => Sig::GtJson,
            ("gt", types::ETVectorFloat32) => Sig::GtVectorFloat32,
            ("ge", types::ETInt) => Sig::GeInt,
            ("ge", types::ETReal) => Sig::GeReal,
            ("ge", types::ETDecimal) => Sig::GeDecimal,
            ("ge", types::ETString) => Sig::GeString,
            ("ge", types::ETDatetime | types::ETTimestamp) => Sig::GeTime,
            ("ge", types::ETDuration) => Sig::GeDuration,
            ("ge", types::ETJson) => Sig::GeJson,
            ("ge", types::ETVectorFloat32) => Sig::GeVectorFloat32,
            ("eq", types::ETInt) => Sig::EqInt,
            ("eq", types::ETReal) => Sig::EqReal,
            ("eq", types::ETDecimal) => Sig::EqDecimal,
            ("eq", types::ETString) => Sig::EqString,
            ("eq", types::ETDatetime | types::ETTimestamp) => Sig::EqTime,
            ("eq", types::ETDuration) => Sig::EqDuration,
            ("eq", types::ETJson) => Sig::EqJson,
            ("eq", types::ETVectorFloat32) => Sig::EqVectorFloat32,
            ("ne", types::ETInt) => Sig::NeInt,
            ("ne", types::ETReal) => Sig::NeReal,
            ("ne", types::ETDecimal) => Sig::NeDecimal,
            ("ne", types::ETString) => Sig::NeString,
            ("ne", types::ETDatetime | types::ETTimestamp) => Sig::NeTime,
            ("ne", types::ETDuration) => Sig::NeDuration,
            ("ne", types::ETJson) => Sig::NeJson,
            ("ne", types::ETVectorFloat32) => Sig::NeVectorFloat32,
            ("nulleq", types::ETInt) => Sig::NullEqInt,
            ("nulleq", types::ETReal) => Sig::NullEqReal,
            ("nulleq", types::ETDecimal) => Sig::NullEqDecimal,
            ("nulleq", types::ETString) => Sig::NullEqString,
            ("nulleq", types::ETDatetime | types::ETTimestamp) => Sig::NullEqTime,
            ("nulleq", types::ETDuration) => Sig::NullEqDuration,
            ("nulleq", types::ETJson) => Sig::NullEqJson,
            ("nulleq", types::ETVectorFloat32) => Sig::NullEqVectorFloat32,
            _ => Sig::Unspecified,
        }) as i32
    }

    fn in_signature(eval_type: types::EvalType) -> &'static str {
        match eval_type {
            types::ETInt => "builtinInIntSig",
            types::ETReal => "builtinInRealSig",
            types::ETDecimal => "builtinInDecimalSig",
            types::ETString => "builtinInStringSig",
            types::ETDuration => "builtinInDurationSig",
            types::ETDatetime | types::ETTimestamp => "builtinInTimeSig",
            types::ETJson => "builtinInJSONSig",
            types::ETVectorFloat32 => "builtinInVectorFloat32Sig",
            _ => "builtinInIntSig",
        }
    }

    fn field_type_for_eval_type(eval_type: types::EvalType) -> types::FieldType {
        match eval_type {
            types::ETInt => *types::NewFieldType(mysql::TypeLonglong),
            types::ETReal => *types::NewFieldType(mysql::TypeDouble),
            types::ETDecimal => *types::NewFieldType(mysql::TypeNewDecimal),
            types::ETString => *types::NewFieldType(mysql::TypeVarString),
            types::ETDatetime => *types::NewFieldType(mysql::TypeDatetime),
            types::ETTimestamp => *types::NewFieldType(mysql::TypeTimestamp),
            types::ETDuration => *types::NewFieldType(mysql::TypeDuration),
            types::ETJson => *types::NewFieldType(mysql::TypeJSON),
            types::ETVectorFloat32 => *types::NewFieldType(mysql::TypeTiDBVectorFloat32),
            _ => *types::NewFieldType(mysql::TypeUnspecified),
        }
    }

    /// Derive decimal/real display width and scale for binary arithmetic.
    /// This mirrors Go's `setFlenDecimal4RealOrDecimal`; keeping only the
    /// evaluation kind loses the scale later needed by CASE coercion and SUM.
    fn arithmetic_return_type(
        name: &str,
        eval_type: types::EvalType,
        left: &types::FieldType,
        right: &types::FieldType,
    ) -> types::FieldType {
        let mut result = field_type_for_eval_type(eval_type);
        if !matches!(eval_type, types::ETDecimal | types::ETReal) {
            return result;
        }
        let left_decimal = left.GetDecimal();
        let right_decimal = right.GetDecimal();
        if left_decimal == types::UnspecifiedLength as isize
            || right_decimal == types::UnspecifiedLength as isize
        {
            if eval_type == types::ETReal {
                result.SetFlen(types::UnspecifiedLength as isize);
                result.SetDecimal(types::UnspecifiedLength as isize);
            } else {
                result.SetFlen(mysql::MaxDecimalWidth as isize);
                result.SetDecimal(mysql::MaxDecimalScale as isize);
            }
            return result;
        }
        let multiply = name == "mul";
        let mut decimal = if multiply {
            left_decimal + right_decimal
        } else {
            left_decimal.max(right_decimal)
        };
        if eval_type == types::ETDecimal {
            decimal = decimal.min(mysql::MaxDecimalScale as isize);
        }
        result.SetDecimalUnderLimit(decimal);
        if left.GetFlen() == types::UnspecifiedLength as isize
            || right.GetFlen() == types::UnspecifiedLength as isize
        {
            result.SetFlen(types::UnspecifiedLength as isize);
            return result;
        }
        let integer_digits = if multiply {
            left.GetFlen() - left_decimal + right.GetFlen() - right_decimal
        } else {
            (left.GetFlen() - left_decimal).max(right.GetFlen() - right_decimal) + 1
        };
        let maximum = if eval_type == types::ETReal {
            mysql::MaxRealWidth as isize
        } else {
            mysql::MaxDecimalWidth as isize
        };
        result.SetFlenUnderLimit((integer_digits + result.GetDecimal()).min(maximum));
        result
    }

    fn validate_now_fsp(fsp: i64, name: &str) -> Result<i32, Error> {
        if fsp < 0 || fsp > i64::from(i32::MAX) {
            return Err(errors::New(
                types_dependency::errors::ErrSyntax
                    .GenWithStack("You have an error in your SQL syntax", &[])
                    .to_string(),
            ));
        }
        if fsp > 6 {
            return Err(errors::New(
                types_dependency::errors::ErrTooBigPrecision
                    .GenWithStackByArgs(&[fsp.into(), name.into(), 6_i64.into()])
                    .to_string(),
            ));
        }
        Ok(fsp as i32)
    }

    fn core_builtin_factory(
        name: &'static str,
        ctx: &dyn BuildContext,
        mut args: Vec<Box<dyn Expression>>,
        _metadata: &FunctionClassMetadata,
    ) -> Result<GeneratedBuiltinFactoryOutput, Error> {
        // Row comparison and some parser rewrites preserve SQL operator
        // spellings rather than AST function identifiers. Go routes both
        // spellings to the same function classes.
        let name = match name {
            "=" => "eq",
            "!=" | "<>" => "ne",
            "<" => "lt",
            "<=" => "le",
            ">" => "gt",
            ">=" => "ge",
            "+" => "plus",
            "-" if args.len() == 1 => "unaryminus",
            "-" => "minus",
            "*" => "mul",
            "/" => "div",
            "%" => "mod",
            other => other,
        };
        if name == "json_extract" {
            let json_type = *types::NewFieldType(mysql::TypeJSON);
            args[0] = BuildCastFunction(ctx, &args[0], &json_type);
            for index in 1..args.len() {
                let string_type = *types::NewFieldType(mysql::TypeVarString);
                args[index] = BuildCastFunction(ctx, &args[index], &string_type);
            }
        }
        if name == "weekday" {
            let datetime_type = *types::NewFieldType(mysql::TypeDatetime);
            args[0] = BuildCastFunction(ctx, &args[0], &datetime_type);
        }
        let (kind, return_type, signature) = match name {
            "eq" | "ne" | "lt" | "le" | "gt" | "ge" | "nulleq" => {
                let operation = match name {
                    "eq" => opcode::Op::EQ,
                    "ne" => opcode::Op::NE,
                    "lt" => opcode::Op::LT,
                    "le" => opcode::Op::LE,
                    "gt" => opcode::Op::GT,
                    "ge" => opcode::Op::GE,
                    "nulleq" => opcode::Op::NullEQ,
                    _ => unreachable!("comparison operator matched above"),
                };
                let left_type = args[0].GetType(ctx.GetEvalCtx()).clone();
                let right_type = args[1].GetType(ctx.GetEvalCtx()).clone();
                if left_type.EvalType() == types::ETInt
                    && args[0].as_constant().is_none()
                    && right_type.EvalType() != types::ETInt
                    && let Some(constant) = args[1].as_constant().cloned()
                {
                    let (refined, exceptional) =
                        crate::RefineComparedConstant(ctx, left_type.clone(), &constant, operation);
                    if !exceptional || mysql::HasNotNullFlag(left_type.GetFlag()) {
                        args[1] = refined;
                    }
                }
                let eval_type =
                    crate::GetAccurateCmpType(ctx.GetEvalCtx(), args[0].as_ref(), args[1].as_ref());
                // Go's comparison factory uses the canonical temporal
                // argument type for mixed DATE/string comparisons. Reusing
                // the DATE column's field type here casts the literal to DATE
                // and loses the `00:00:00.000000` datetime representation
                // visible in ranges and EXPLAIN.
                let mut comparison_type =
                    if matches!(eval_type, types::ETDatetime | types::ETTimestamp) {
                        field_type_for_eval_type(eval_type)
                    } else {
                        args.iter()
                            .map(|argument| argument.GetType(ctx.GetEvalCtx()))
                            .find(|field_type| field_type.EvalType() == eval_type)
                            .cloned()
                            .unwrap_or_else(|| field_type_for_eval_type(eval_type))
                    };
                if matches!(eval_type, types::ETDatetime | types::ETTimestamp) {
                    comparison_type.SetDecimal(types::MaxFsp as isize);
                }
                for argument in &mut args {
                    let source_type = argument.GetType(ctx.GetEvalCtx());
                    if source_type.EvalType() != eval_type {
                        if eval_type == types::ETReal {
                            *argument = crate::WrapWithCastAsReal(ctx, argument.CloneExpr());
                            continue;
                        }
                        let mut cast_type = if eval_type == types::ETDecimal
                            && source_type.EvalType() == types::ETInt
                        {
                            let mut decimal = *types::NewFieldType(mysql::TypeNewDecimal);
                            decimal.SetFlen(match source_type.GetType() {
                                mysql::TypeTiny => 3,
                                mysql::TypeShort => 5,
                                mysql::TypeInt24 => 8,
                                mysql::TypeLong => 10,
                                mysql::TypeLonglong => 20,
                                mysql::TypeYear => 4,
                                _ => mysql::MaxIntWidth as isize,
                            });
                            decimal.SetDecimal(0);
                            decimal.AddFlag(mysql::BinaryFlag);
                            decimal.SetCharset(crate::charset::CharsetBin.to_owned());
                            decimal.SetCollate(crate::charset::CollationBin.to_owned());
                            decimal
                        } else {
                            comparison_type.clone()
                        };
                        if matches!(eval_type, types::ETDatetime | types::ETTimestamp)
                            && source_type.EvalType() == types::ETString
                            && let Some(constant) = argument.as_constant()
                            && constant.Value.Kind() == types::KindString
                        {
                            let literal = constant.Value.GetString();
                            if let Some((_, fraction)) = literal.rsplit_once('.') {
                                let fsp = fraction
                                    .chars()
                                    .take_while(char::is_ascii_digit)
                                    .count()
                                    .min(types::MaxFsp as usize);
                                if fsp > 0 {
                                    cast_type.SetDecimal(fsp as isize);
                                }
                            }
                        }
                        *argument = BuildCastFunction(ctx, argument, &cast_type);
                    }
                }
                (
                    CoreBuiltinKind::Compare(name, eval_type),
                    boolean_field_type(),
                    comparison_signature(name, eval_type),
                )
            }
            "in" => {
                let eval_type =
                    crate::GetAccurateCmpType(ctx.GetEvalCtx(), args[0].as_ref(), args[1].as_ref());
                (
                    CoreBuiltinKind::In(eval_type),
                    boolean_field_type(),
                    in_signature(eval_type),
                )
            }
            "isnull" => {
                let signature = match args[0].GetType(ctx.GetEvalCtx()).EvalType() {
                    types::ETInt => "builtinIntIsNullSig",
                    types::ETReal => "builtinRealIsNullSig",
                    types::ETDecimal => "builtinDecimalIsNullSig",
                    types::ETString | types::ETJson => "builtinStringIsNullSig",
                    types::ETDatetime | types::ETTimestamp => "builtinTimeIsNullSig",
                    types::ETDuration => "builtinDurationIsNullSig",
                    types::ETVectorFloat32 => "builtinVectorFloat32IsNullSig",
                    _ => "builtinIntIsNullSig",
                };
                (CoreBuiltinKind::IsNull, boolean_field_type(), signature)
            }
            "and" | "or" | "xor" => (
                CoreBuiltinKind::Logic(name),
                boolean_field_type(),
                match name {
                    "and" => "builtinLogicAndSig",
                    "or" => "builtinLogicOrSig",
                    _ => "builtinLogicXorSig",
                },
            ),
            "not" => {
                let signature = match args[0].GetType(ctx.GetEvalCtx()).EvalType() {
                    types::ETInt => "builtinUnaryNotIntSig",
                    types::ETDecimal => "builtinUnaryNotDecimalSig",
                    types::ETJson => "builtinUnaryNotJSONSig",
                    _ => "builtinUnaryNotRealSig",
                };
                (CoreBuiltinKind::UnaryNot, boolean_field_type(), signature)
            }
            "unaryminus" => {
                let eval_type = match args[0].GetType(ctx.GetEvalCtx()).EvalType() {
                    types::ETInt => types::ETInt,
                    types::ETDecimal => types::ETDecimal,
                    _ => types::ETReal,
                };
                let signature = match eval_type {
                    types::ETInt => "builtinUnaryMinusIntSig",
                    types::ETDecimal => "builtinUnaryMinusDecimalSig",
                    _ => "builtinUnaryMinusRealSig",
                };
                (
                    CoreBuiltinKind::UnaryMinus(eval_type),
                    field_type_for_eval_type(eval_type),
                    signature,
                )
            }
            "plus" | "minus" | "mul" | "div" | "intdiv" | "mod" => {
                let left = args[0].GetType(ctx.GetEvalCtx()).EvalType();
                let right = args[1].GetType(ctx.GetEvalCtx()).EvalType();
                let eval_type = if name == "div" {
                    if left == types::ETReal || right == types::ETReal {
                        types::ETReal
                    } else {
                        types::ETDecimal
                    }
                } else if name == "intdiv" {
                    types::ETInt
                } else if left == types::ETReal || right == types::ETReal {
                    types::ETReal
                } else if left == types::ETDecimal || right == types::ETDecimal {
                    types::ETDecimal
                } else {
                    types::ETInt
                };
                let signature = match (name, eval_type) {
                    ("plus", types::ETInt) => "builtinArithmeticPlusIntSig",
                    ("plus", types::ETReal) => "builtinArithmeticPlusRealSig",
                    ("plus", types::ETDecimal) => "builtinArithmeticPlusDecimalSig",
                    ("minus", types::ETInt) => "builtinArithmeticMinusIntSig",
                    ("minus", types::ETReal) => "builtinArithmeticMinusRealSig",
                    ("minus", types::ETDecimal) => "builtinArithmeticMinusDecimalSig",
                    ("mul", types::ETInt) => "builtinArithmeticMultiplyIntSig",
                    ("mul", types::ETDecimal) => "builtinArithmeticMultiplyDecimalSig",
                    ("div", types::ETReal) => "builtinArithmeticDivideRealSig",
                    ("div", types::ETDecimal) => "builtinArithmeticDivideDecimalSig",
                    ("intdiv", _) => "builtinArithmeticIntDivideIntSig",
                    ("mod", types::ETInt) => "builtinArithmeticModIntSignedSignedSig",
                    ("mod", types::ETReal) => "builtinArithmeticModRealSig",
                    ("mod", types::ETDecimal) => "builtinArithmeticModDecimalSig",
                    _ => "builtinArithmeticMultiplyDecimalSig",
                };
                let return_type = arithmetic_return_type(
                    name,
                    eval_type,
                    args[0].GetType(ctx.GetEvalCtx()),
                    args[1].GetType(ctx.GetEvalCtx()),
                );
                (
                    CoreBuiltinKind::Arithmetic(name, eval_type),
                    return_type,
                    signature,
                )
            }
            "case" => {
                let result_indices = (1..args.len())
                    .step_by(2)
                    .chain((args.len() % 2 == 1).then_some(args.len() - 1).into_iter())
                    .collect::<Vec<_>>();
                let result_expressions = result_indices
                    .iter()
                    .map(|index| args[*index].as_ref() as &dyn Expression)
                    .collect::<Vec<_>>();
                let mut return_type =
                    crate::InferType4ControlFuncsVariadic(ctx, name, &result_expressions)?;
                // CASE without a matching WHEN returns NULL, even when every
                // explicit result arm is declared NOT NULL.
                return_type.DelFlag(mysql::NotNullFlag);
                let eval_type = return_type.EvalType();
                for index in result_indices {
                    let source_type = args[index].GetType(ctx.GetEvalCtx());
                    let needs_cast = match eval_type {
                        types::ETDecimal
                        | types::ETDatetime
                        | types::ETTimestamp
                        | types::ETDuration => !source_type.Equal(&return_type),
                        _ => source_type.EvalType() != eval_type,
                    };
                    if needs_cast {
                        args[index] = BuildCastFunction(ctx, &args[index], &return_type);
                    }
                }
                let signature = match eval_type {
                    types::ETInt => "builtinCaseWhenIntSig",
                    types::ETReal => "builtinCaseWhenRealSig",
                    types::ETDecimal => "builtinCaseWhenDecimalSig",
                    types::ETString => "builtinCaseWhenStringSig",
                    types::ETDatetime | types::ETTimestamp => "builtinCaseWhenTimeSig",
                    types::ETDuration => "builtinCaseWhenDurationSig",
                    types::ETJson => "builtinCaseWhenJSONSig",
                    types::ETVectorFloat32 => "builtinCaseWhenVectorFloat32Sig",
                    _ => "builtinCaseWhenIntSig",
                };
                (CoreBuiltinKind::Case, return_type, signature)
            }
            "ifnull" => {
                let left_type = args[0].GetType(ctx.GetEvalCtx()).clone();
                let right_type = args[1].GetType(ctx.GetEvalCtx()).clone();
                let mut return_type =
                    crate::InferType4ControlFuncs(ctx, name, args[0].as_ref(), args[1].as_ref())?;
                return_type.AddFlag(
                    (left_type.GetFlag() & mysql::NotNullFlag)
                        | (right_type.GetFlag() & mysql::NotNullFlag),
                );
                let eval_type = return_type.EvalType();
                // Go's newBaseBuiltinFuncWithFieldTypes routes integer, real,
                // string, JSON and vector arguments through WrapWithCastAs*,
                // which keeps an expression whose evaluation type already
                // matches. Decimal and temporal arguments still require the
                // complete FieldType to match (issue #44196).
                for argument in &mut args {
                    let source_type = argument.GetType(ctx.GetEvalCtx());
                    let needs_cast = match eval_type {
                        types::ETDecimal
                        | types::ETDatetime
                        | types::ETTimestamp
                        | types::ETDuration => !source_type.Equal(&return_type),
                        _ => source_type.EvalType() != eval_type,
                    };
                    if needs_cast {
                        *argument = BuildCastFunction(ctx, argument, &return_type);
                    }
                }
                let signature = match eval_type {
                    types::ETInt => "builtinIfNullIntSig",
                    types::ETReal => "builtinIfNullRealSig",
                    types::ETDecimal => "builtinIfNullDecimalSig",
                    types::ETString => "builtinIfNullStringSig",
                    types::ETDatetime | types::ETTimestamp => "builtinIfNullTimeSig",
                    types::ETDuration => "builtinIfNullDurationSig",
                    types::ETJson => "builtinIfNullJSONSig",
                    types::ETVectorFloat32 => "builtinIfNullVectorFloat32Sig",
                    other => {
                        return Err(errors::New(format!(
                            "{other:?} is not supported for IFNULL()"
                        )));
                    }
                };
                (CoreBuiltinKind::IfNull(eval_type), return_type, signature)
            }
            "row" => (
                CoreBuiltinKind::Row,
                args[0].GetType(ctx.GetEvalCtx()).clone(),
                "builtinRowSig",
            ),
            "json_extract" => (
                CoreBuiltinKind::JsonExtract,
                field_type_for_eval_type(types::ETJson),
                "builtinJSONExtractSig",
            ),
            "uuid" => {
                let mut return_type = field_type_for_eval_type(types::ETString);
                let (charset, collation) = ctx.GetCharsetInfo();
                return_type.SetCharset(charset);
                return_type.SetCollate(collation);
                return_type.SetFlen(36);
                (CoreBuiltinKind::Uuid, return_type, "builtinUUIDSig")
            }
            "rand" => (
                CoreBuiltinKind::Rand,
                field_type_for_eval_type(types::ETReal),
                "builtinRandWithSeedFirstGenSig",
            ),
            "md5" => {
                args[0] = crate::WrapWithCastAsString(ctx, args[0].CloneExpr());
                let mut return_type = field_type_for_eval_type(types::ETString);
                let (charset, collation) = ctx.GetCharsetInfo();
                return_type.SetCharset(charset);
                return_type.SetCollate(collation);
                return_type.SetFlen(32);
                (CoreBuiltinKind::Md5, return_type, "builtinMD5Sig")
            }
            "setvar" => {
                let eval_type = match args[1].GetType(ctx.GetEvalCtx()).EvalType() {
                    types::ETTimestamp | types::ETDuration | types::ETJson => types::ETString,
                    eval_type => eval_type,
                };
                let mut return_type = field_type_for_eval_type(eval_type);
                return_type.SetFlenUnderLimit(args[1].GetType(ctx.GetEvalCtx()).GetFlen());
                let signature = match eval_type {
                    types::ETString => "builtinSetStringVarSig",
                    types::ETReal => "builtinSetRealVarSig",
                    types::ETDecimal => "builtinSetDecimalVarSig",
                    types::ETInt => "builtinSetIntVarSig",
                    types::ETDatetime => "builtinSetTimeVarSig",
                    other => {
                        return Err(errors::New(format!(
                            "unexpected SETVAR evaluation type {other:?}"
                        )));
                    }
                };
                (CoreBuiltinKind::SetVar(eval_type), return_type, signature)
            }
            "time_to_sec" => (
                CoreBuiltinKind::TimeToSec,
                field_type_for_eval_type(types::ETInt),
                "builtinTimeToSecSig",
            ),
            "extract" => {
                args[0] = crate::WrapWithCastAsString(ctx, args[0].CloneExpr());
                let (unit, _) = args[0].EvalString(ctx.GetEvalCtx(), chunk::Row::default())?;
                let clock_unit = types_dependency::time::IsClockUnit(&unit);
                let date_unit = types_dependency::time::IsDateUnit(&unit);
                let second_eval_type = args[1].GetType(ctx.GetEvalCtx()).EvalType();
                let mode = if clock_unit && date_unit {
                    match second_eval_type {
                        types::ETDatetime | types::ETTimestamp => ExtractMode::Datetime,
                        types::ETDuration => ExtractMode::Duration,
                        _ => ExtractMode::DatetimeFromString,
                    }
                } else if clock_unit {
                    ExtractMode::Duration
                } else {
                    ExtractMode::Datetime
                };
                args[1] = match mode {
                    ExtractMode::Datetime => crate::WrapWithCastAsTime(
                        ctx,
                        args[1].CloneExpr(),
                        *types::NewFieldType(mysql::TypeDatetime),
                    ),
                    ExtractMode::Duration => {
                        BuildCastFunction(ctx, &args[1], &types::NewFieldType(mysql::TypeDuration))
                    }
                    ExtractMode::DatetimeFromString => {
                        crate::WrapWithCastAsString(ctx, args[1].CloneExpr())
                    }
                };
                let signature = match mode {
                    ExtractMode::Datetime => "builtinExtractDatetimeSig",
                    ExtractMode::Duration => "builtinExtractDurationSig",
                    ExtractMode::DatetimeFromString => "builtinExtractDatetimeFromStringSig",
                };
                (
                    CoreBuiltinKind::Extract(mode),
                    field_type_for_eval_type(types::ETInt),
                    signature,
                )
            }
            "sleep" => {
                args[0] = crate::WrapWithCastAsReal(ctx, args[0].CloneExpr());
                let mut return_type = field_type_for_eval_type(types::ETInt);
                return_type.SetFlen(21);
                (CoreBuiltinKind::Sleep, return_type, "builtinSleepSig")
            }
            "substr" | "substring" | "mid" => {
                args[0] = crate::WrapWithCastAsString(ctx, args[0].CloneExpr());
                for argument in args.iter_mut().skip(1) {
                    *argument = crate::WrapWithCastAsInt(ctx, argument.CloneExpr(), None);
                }
                let argument_type = args[0].GetType(ctx.GetEvalCtx()).clone();
                let binary = argument_type.GetCollate() == crate::charset::CollationBin;
                let mut return_type = field_type_for_eval_type(types::ETString);
                return_type.SetFlen(argument_type.GetFlen());
                if binary {
                    return_type.SetCharset(crate::charset::CharsetBin.to_owned());
                    return_type.SetCollate(crate::charset::CollationBin.to_owned());
                    return_type.AddFlag(mysql::BinaryFlag);
                } else if mysql::HasBinaryFlag(argument_type.GetFlag()) {
                    return_type.AddFlag(mysql::BinaryFlag);
                }
                (
                    CoreBuiltinKind::Substring(binary),
                    return_type,
                    match (args.len(), binary) {
                        (2, true) => "builtinSubstring2ArgsSig",
                        (2, false) => "builtinSubstring2ArgsUTF8Sig",
                        (3, true) => "builtinSubstring3ArgsSig",
                        _ => "builtinSubstring3ArgsUTF8Sig",
                    },
                )
            }
            "trim" => {
                for argument in args.iter_mut().take(2) {
                    *argument = crate::WrapWithCastAsString(ctx, argument.CloneExpr());
                }
                if let Some(direction) = args.get_mut(2) {
                    *direction = crate::WrapWithCastAsInt(ctx, direction.CloneExpr(), None);
                }
                let source_type = args[0].GetType(ctx.GetEvalCtx()).clone();
                let mut return_type = *types::NewFieldType(mysql::TypeVarString);
                return_type.SetFlen(source_type.GetFlen());
                let non_enum_or_set =
                    !matches!(source_type.GetType(), mysql::TypeEnum | mysql::TypeSet);
                if types::IsBinaryStr(&source_type) {
                    types_dependency::field::SetBinChsClnFlag(&mut return_type);
                } else if mysql::HasBinaryFlag(source_type.GetFlag())
                    || (!types_dependency::metadata::IsNonBinaryStr(&source_type)
                        && non_enum_or_set)
                {
                    return_type.AddFlag(mysql::BinaryFlag);
                }
                (
                    CoreBuiltinKind::Trim,
                    return_type,
                    match args.len() {
                        1 => "builtinTrim1ArgSig",
                        2 => "builtinTrim2ArgsSig",
                        _ => "builtinTrim3ArgsSig",
                    },
                )
            }
            "weekday" => {
                let mut return_type = field_type_for_eval_type(types::ETInt);
                return_type.SetFlen(1);
                (CoreBuiltinKind::Weekday, return_type, "builtinWeekDaySig")
            }
            "atan" | "atan2" => {
                let real_type = *types::NewFieldType(mysql::TypeDouble);
                for argument in &mut args {
                    if argument.GetType(ctx.GetEvalCtx()).EvalType() != types::ETReal {
                        *argument = BuildCastFunction(ctx, argument, &real_type);
                    }
                }
                let mut return_type = real_type;
                return_type.SetFlen(mysql::MaxRealWidth as isize);
                (
                    CoreBuiltinKind::Atan,
                    return_type,
                    if args.len() == 1 {
                        "builtinAtan1ArgSig"
                    } else {
                        "builtinAtan2ArgsSig"
                    },
                )
            }
            "pow" | "power" => {
                for argument in &mut args {
                    *argument = crate::WrapWithCastAsReal(ctx, argument.CloneExpr());
                }
                let real_type = *types::NewFieldType(mysql::TypeDouble);
                let mut return_type = real_type;
                return_type.SetFlen(mysql::MaxRealWidth as isize);
                (CoreBuiltinKind::Pow, return_type, "builtinPowSig")
            }
            "now" | "current_timestamp" | "localtimestamp" | "localtime" => {
                let fsp = if let Some(argument) = args.first() {
                    if argument.as_any().is::<crate::Constant>() {
                        let (fsp, null) =
                            argument.EvalInt(ctx.GetEvalCtx(), chunk::Row::default())?;
                        if null {
                            0
                        } else {
                            validate_now_fsp(fsp, name)?
                        }
                    } else {
                        0
                    }
                } else {
                    0
                };
                if let Some(argument) = args.first_mut() {
                    *argument = crate::WrapWithCastAsInt(ctx, argument.CloneExpr(), None);
                }
                let mut return_type = *types::NewFieldType(mysql::TypeDatetime);
                return_type.SetDecimal(fsp as isize);
                return_type.SetFlen(19 + if fsp > 0 { fsp as isize + 1 } else { 0 });
                return_type.AddFlag(mysql::BinaryFlag);
                return_type.SetCharset(crate::charset::CharsetBin.to_owned());
                return_type.SetCollate(crate::charset::CollationBin.to_owned());
                (
                    CoreBuiltinKind::Now,
                    return_type,
                    if args.is_empty() {
                        "builtinNowWithoutArgSig"
                    } else {
                        "builtinNowWithArgSig"
                    },
                )
            }
            "date_add" | "adddate" | "date_sub" | "subdate" => {
                let mut time_type = *types::NewFieldType(mysql::TypeDatetime);
                time_type.SetDecimal(args[0].GetType(ctx.GetEvalCtx()).GetDecimal());
                args[0] = crate::WrapWithCastAsTime(ctx, args[0].CloneExpr(), time_type.clone());
                args[1] = crate::WrapWithCastAsInt(ctx, args[1].CloneExpr(), None);
                args[2] = crate::WrapWithCastAsString(ctx, args[2].CloneExpr());
                let add = matches!(name, "date_add" | "adddate");
                (
                    CoreBuiltinKind::DateAddSub(add),
                    time_type,
                    "builtinAddSubDateDatetimeAnySig",
                )
            }
            "char_length" => {
                let binary = types::IsBinaryStr(args[0].GetType(ctx.GetEvalCtx()));
                let string_type = *types::NewFieldType(mysql::TypeVarString);
                args[0] = BuildCastFunction(ctx, &args[0], &string_type);
                (
                    CoreBuiltinKind::CharLength(binary),
                    field_type_for_eval_type(types::ETInt),
                    if binary {
                        "builtinCharLengthBinarySig"
                    } else {
                        "builtinCharLengthUTF8Sig"
                    },
                )
            }
            "length" => {
                args[0] = crate::WrapWithCastAsString(ctx, args[0].CloneExpr());
                let mut return_type = field_type_for_eval_type(types::ETInt);
                return_type.SetFlen(10);
                (CoreBuiltinKind::Length, return_type, "builtinLengthSig")
            }
            "ascii" => {
                args[0] = crate::WrapWithCastAsString(ctx, args[0].CloneExpr());
                (
                    CoreBuiltinKind::Ascii,
                    field_type_for_eval_type(types::ETInt),
                    "builtinASCIISig",
                )
            }
            "hex" => {
                let argument_type = args[0].GetType(ctx.GetEvalCtx());
                let integer = match argument_type.EvalType() {
                    types::ETInt | types::ETReal | types::ETDecimal => true,
                    types::ETString
                    | types::ETDatetime
                    | types::ETTimestamp
                    | types::ETDuration
                    | types::ETJson => false,
                    other => {
                        return Err(errors::New(format!(
                            "HEX requires an integer or string argument, got {other:?}"
                        )));
                    }
                };
                let original_flen = argument_type.GetFlen();
                let target_type = *types::NewFieldType(if integer {
                    mysql::TypeLonglong
                } else {
                    mysql::TypeVarString
                });
                args[0] = BuildCastFunction(ctx, &args[0], &target_type);
                let mut return_type = field_type_for_eval_type(types::ETString);
                return_type.SetFlen(if original_flen == types::UnspecifiedLength as isize {
                    types::UnspecifiedLength as isize
                } else if integer {
                    original_flen.saturating_mul(2)
                } else {
                    original_flen.saturating_mul(8)
                });
                (
                    CoreBuiltinKind::Hex(integer),
                    return_type,
                    if integer {
                        "builtinHexIntArgSig"
                    } else {
                        "builtinHexStrArgSig"
                    },
                )
            }
            "find_in_set" => {
                args[0] = crate::WrapWithCastAsString(ctx, args[0].CloneExpr());
                args[1] = crate::WrapWithCastAsString(ctx, args[1].CloneExpr());
                (
                    CoreBuiltinKind::FindInSet,
                    field_type_for_eval_type(types::ETInt),
                    "builtinFindInSetSig",
                )
            }
            "space" => {
                args[0] = crate::WrapWithCastAsInt(ctx, args[0].CloneExpr(), None);
                (
                    CoreBuiltinKind::Space,
                    field_type_for_eval_type(types::ETString),
                    "builtinSpaceSig",
                )
            }
            "rpad" => {
                args[0] = crate::WrapWithCastAsString(ctx, args[0].CloneExpr());
                args[1] = crate::WrapWithCastAsInt(ctx, args[1].CloneExpr(), None);
                args[2] = crate::WrapWithCastAsString(ctx, args[2].CloneExpr());
                (
                    CoreBuiltinKind::Rpad,
                    field_type_for_eval_type(types::ETString),
                    "builtinRpadUTF8Sig",
                )
            }
            "insert" => {
                args[0] = crate::WrapWithCastAsString(ctx, args[0].CloneExpr());
                args[1] = crate::WrapWithCastAsInt(ctx, args[1].CloneExpr(), None);
                args[2] = crate::WrapWithCastAsInt(ctx, args[2].CloneExpr(), None);
                args[3] = crate::WrapWithCastAsString(ctx, args[3].CloneExpr());
                (
                    CoreBuiltinKind::Insert,
                    field_type_for_eval_type(types::ETString),
                    "builtinInsertUTF8Sig",
                )
            }
            "like" | "ilike" => {
                args[0] = crate::WrapWithCastAsString(ctx, args[0].CloneExpr());
                args[1] = crate::WrapWithCastAsString(ctx, args[1].CloneExpr());
                args[2] = crate::WrapWithCastAsInt(ctx, args[2].CloneExpr(), None);
                (
                    if name == "ilike" {
                        CoreBuiltinKind::Ilike
                    } else {
                        CoreBuiltinKind::Like
                    },
                    boolean_field_type(),
                    if name == "ilike" {
                        "builtinIlikeSig"
                    } else {
                        "builtinLikeSig"
                    },
                )
            }
            "is_ipv4" => (
                CoreBuiltinKind::IsIPv4,
                boolean_field_type(),
                "builtinIsIPv4Sig",
            ),
            "is_ipv6" => (
                CoreBuiltinKind::IsIPv6,
                boolean_field_type(),
                "builtinIsIPv6Sig",
            ),
            "regexp_instr" => (
                CoreBuiltinKind::RegexpInstr,
                field_type_for_eval_type(types::ETInt),
                "builtinRegexpInStrFuncSig",
            ),
            "regexp_substr" => (
                CoreBuiltinKind::RegexpSubstr,
                field_type_for_eval_type(types::ETString),
                "builtinRegexpSubstrFuncSig",
            ),
            "regexp_replace" => (
                CoreBuiltinKind::RegexpReplace,
                field_type_for_eval_type(types::ETString),
                "builtinRegexpReplaceFuncSig",
            ),
            "tidb_shard" => {
                args[0] = crate::WrapWithCastAsInt(ctx, args[0].CloneExpr(), None);
                (
                    CoreBuiltinKind::TiDBShard,
                    field_type_for_eval_type(types::ETInt),
                    "builtinTidbShardSig",
                )
            }
            _ => {
                return Err(errors::New(format!(
                    "unsupported core builtin factory '{name}'"
                )));
            }
        };
        let mut base = if matches!(kind, CoreBuiltinKind::Sleep | CoreBuiltinKind::Rand) {
            RegistryBuiltinBase::new_never(args, return_type)
        } else {
            RegistryBuiltinBase::new_recursive(args, return_type)
        };
        // Go's newBaseBuiltinFunc always initializes the scalar function's
        // collation metadata before constant folding. Leaving the default
        // collationInfo uninitialized makes ScalarFunction::Coercibility fall
        // through to deriveCoercibilityForScalarFunc, which is deliberately an
        // unreachable guard because construction must already have derived it.
        let (default_charset, default_collation) = if base.return_type.EvalType() == types::ETString
        {
            ctx.GetCharsetInfo()
        } else {
            (
                crate::charset::CharsetBin.to_owned(),
                crate::charset::CollationBin.to_owned(),
            )
        };
        let default_coercibility = if base.return_type.EvalType() == types::ETString {
            crate::CoercibilityCoercible
        } else {
            crate::CoercibilityNumeric
        };
        let default_repertoire = if default_charset == crate::charset::CharsetASCII {
            crate::ASCII
        } else if base.return_type.EvalType() == types::ETString {
            crate::UNICODE
        } else {
            crate::ASCII
        };
        base.SetCoercibility(default_coercibility);
        base.SetRepertoire(default_repertoire);
        base.SetCharsetAndCollation(default_charset.clone(), default_collation.clone());
        base.collator =
            collate::GetCollatorWithCollate(ctx.NewCollationEnabled(), &default_collation);
        base.pb_code = match kind {
            CoreBuiltinKind::Compare(operator, eval_type) => {
                comparison_pb_code(operator, eval_type)
            }
            CoreBuiltinKind::Logic("and") => tipb::ScalarFuncSig::LogicalAnd as i32,
            CoreBuiltinKind::Logic("or") => tipb::ScalarFuncSig::LogicalOr as i32,
            CoreBuiltinKind::Logic("xor") => tipb::ScalarFuncSig::LogicalXor as i32,
            CoreBuiltinKind::IsNull => match base.args[0].GetType(ctx.GetEvalCtx()).EvalType() {
                types::ETInt => tipb::ScalarFuncSig::IntIsNull as i32,
                types::ETReal => tipb::ScalarFuncSig::RealIsNull as i32,
                types::ETDecimal => tipb::ScalarFuncSig::DecimalIsNull as i32,
                types::ETString | types::ETJson => tipb::ScalarFuncSig::StringIsNull as i32,
                types::ETDatetime | types::ETTimestamp => tipb::ScalarFuncSig::TimeIsNull as i32,
                types::ETDuration => tipb::ScalarFuncSig::DurationIsNull as i32,
                types::ETVectorFloat32 => tipb::ScalarFuncSig::VectorFloat32IsNull as i32,
                _ => tipb::ScalarFuncSig::IntIsNull as i32,
            },
            CoreBuiltinKind::UnaryNot => match base.args[0].GetType(ctx.GetEvalCtx()).EvalType() {
                types::ETInt | types::ETDatetime | types::ETTimestamp | types::ETDuration => {
                    tipb::ScalarFuncSig::UnaryNotInt as i32
                }
                types::ETDecimal => tipb::ScalarFuncSig::UnaryNotDecimal as i32,
                types::ETJson => tipb::ScalarFuncSig::UnaryNotJson as i32,
                _ => tipb::ScalarFuncSig::UnaryNotReal as i32,
            },
            CoreBuiltinKind::Arithmetic("plus", types::ETInt) => {
                tipb::ScalarFuncSig::PlusInt as i32
            }
            CoreBuiltinKind::Arithmetic("plus", types::ETReal) => {
                tipb::ScalarFuncSig::PlusReal as i32
            }
            CoreBuiltinKind::Arithmetic("plus", types::ETDecimal) => {
                tipb::ScalarFuncSig::PlusDecimal as i32
            }
            CoreBuiltinKind::Arithmetic("minus", types::ETInt) => {
                tipb::ScalarFuncSig::MinusInt as i32
            }
            CoreBuiltinKind::Arithmetic("minus", types::ETReal) => {
                tipb::ScalarFuncSig::MinusReal as i32
            }
            CoreBuiltinKind::Arithmetic("minus", types::ETDecimal) => {
                tipb::ScalarFuncSig::MinusDecimal as i32
            }
            CoreBuiltinKind::Arithmetic("mul", types::ETInt) => {
                tipb::ScalarFuncSig::MultiplyInt as i32
            }
            CoreBuiltinKind::Arithmetic("mul", types::ETReal) => {
                tipb::ScalarFuncSig::MultiplyReal as i32
            }
            CoreBuiltinKind::Arithmetic("mul", types::ETDecimal) => {
                tipb::ScalarFuncSig::MultiplyDecimal as i32
            }
            CoreBuiltinKind::Arithmetic("div", types::ETReal) => {
                tipb::ScalarFuncSig::DivideReal as i32
            }
            CoreBuiltinKind::Arithmetic("div", types::ETDecimal) => {
                tipb::ScalarFuncSig::DivideDecimal as i32
            }
            CoreBuiltinKind::Arithmetic("intdiv", types::ETInt) => {
                tipb::ScalarFuncSig::IntDivideInt as i32
            }
            CoreBuiltinKind::Arithmetic("intdiv", types::ETDecimal) => {
                tipb::ScalarFuncSig::IntDivideDecimal as i32
            }
            CoreBuiltinKind::Arithmetic("mod", types::ETInt) => {
                tipb::ScalarFuncSig::ModIntSignedSigned as i32
            }
            CoreBuiltinKind::Arithmetic("mod", types::ETReal) => {
                tipb::ScalarFuncSig::ModReal as i32
            }
            CoreBuiltinKind::Arithmetic("mod", types::ETDecimal) => {
                tipb::ScalarFuncSig::ModDecimal as i32
            }
            CoreBuiltinKind::TimeToSec => tipb::ScalarFuncSig::TimeToSec as i32,
            CoreBuiltinKind::Extract(ExtractMode::Datetime) => {
                tipb::ScalarFuncSig::ExtractDatetime as i32
            }
            CoreBuiltinKind::Extract(ExtractMode::Duration) => {
                tipb::ScalarFuncSig::ExtractDuration as i32
            }
            CoreBuiltinKind::Extract(ExtractMode::DatetimeFromString) => {
                tipb::ScalarFuncSig::ExtractDatetimeFromString as i32
            }
            CoreBuiltinKind::Weekday => tipb::ScalarFuncSig::WeekDay as i32,
            CoreBuiltinKind::Atan if base.args.len() == 1 => tipb::ScalarFuncSig::Atan1Arg as i32,
            CoreBuiltinKind::Atan => tipb::ScalarFuncSig::Atan2Args as i32,
            CoreBuiltinKind::Pow => tipb::ScalarFuncSig::Pow as i32,
            CoreBuiltinKind::Md5 => tipb::ScalarFuncSig::Md5 as i32,
            CoreBuiltinKind::Now if base.args.is_empty() => {
                tipb::ScalarFuncSig::NowWithoutArg as i32
            }
            CoreBuiltinKind::Now => tipb::ScalarFuncSig::NowWithArg as i32,
            CoreBuiltinKind::DateAddSub(true) => tipb::ScalarFuncSig::AddDateDatetimeInt as i32,
            CoreBuiltinKind::DateAddSub(false) => tipb::ScalarFuncSig::SubDateDatetimeInt as i32,
            CoreBuiltinKind::CharLength(true) => tipb::ScalarFuncSig::CharLength as i32,
            CoreBuiltinKind::CharLength(false) => tipb::ScalarFuncSig::CharLengthUtf8 as i32,
            CoreBuiltinKind::Length => tipb::ScalarFuncSig::Length as i32,
            CoreBuiltinKind::Trim => match base.args.len() {
                1 => tipb::ScalarFuncSig::Trim1Arg as i32,
                2 => tipb::ScalarFuncSig::Trim2Args as i32,
                _ => tipb::ScalarFuncSig::Trim3Args as i32,
            },
            CoreBuiltinKind::Hex(true) => tipb::ScalarFuncSig::HexIntArg as i32,
            CoreBuiltinKind::Hex(false) => tipb::ScalarFuncSig::HexStrArg as i32,
            CoreBuiltinKind::FindInSet => tipb::ScalarFuncSig::FindInSet as i32,
            CoreBuiltinKind::Space => tipb::ScalarFuncSig::Space as i32,
            CoreBuiltinKind::Rpad => tipb::ScalarFuncSig::RpadUtf8 as i32,
            CoreBuiltinKind::Insert => tipb::ScalarFuncSig::InsertUtf8 as i32,
            CoreBuiltinKind::Like => tipb::ScalarFuncSig::LikeSig as i32,
            CoreBuiltinKind::Ilike => tipb::ScalarFuncSig::IlikeSig as i32,
            CoreBuiltinKind::IfNull(types::ETInt) => tipb::ScalarFuncSig::IfNullInt as i32,
            CoreBuiltinKind::IfNull(types::ETReal) => tipb::ScalarFuncSig::IfNullReal as i32,
            CoreBuiltinKind::IfNull(types::ETDecimal) => tipb::ScalarFuncSig::IfNullDecimal as i32,
            CoreBuiltinKind::IfNull(types::ETString) => tipb::ScalarFuncSig::IfNullString as i32,
            CoreBuiltinKind::IfNull(types::ETDatetime | types::ETTimestamp) => {
                tipb::ScalarFuncSig::IfNullTime as i32
            }
            CoreBuiltinKind::IfNull(types::ETDuration) => {
                tipb::ScalarFuncSig::IfNullDuration as i32
            }
            CoreBuiltinKind::IfNull(types::ETJson) => tipb::ScalarFuncSig::IfNullJson as i32,
            CoreBuiltinKind::IsIPv4 => tipb::ScalarFuncSig::IsIPv4 as i32,
            CoreBuiltinKind::IsIPv6 => tipb::ScalarFuncSig::IsIPv6 as i32,
            CoreBuiltinKind::RegexpInstr => tipb::ScalarFuncSig::RegexpInStrUtf8Sig as i32,
            CoreBuiltinKind::RegexpSubstr => tipb::ScalarFuncSig::RegexpSubstrUtf8Sig as i32,
            CoreBuiltinKind::RegexpReplace => tipb::ScalarFuncSig::RegexpReplaceUtf8Sig as i32,
            _ => 0,
        };

        let string_collation_arguments: Option<Vec<&dyn Expression>> = match kind {
            CoreBuiltinKind::Compare(_, types::ETString) | CoreBuiltinKind::In(types::ETString) => {
                Some(base.args.iter().map(|argument| argument.as_ref()).collect())
            }
            CoreBuiltinKind::Like | CoreBuiltinKind::Ilike => Some(
                base.args[..2]
                    .iter()
                    .map(|argument| argument.as_ref())
                    .collect(),
            ),
            CoreBuiltinKind::FindInSet => {
                Some(base.args.iter().map(|argument| argument.as_ref()).collect())
            }
            CoreBuiltinKind::Trim => Some(
                base.args[..base.args.len().min(2)]
                    .iter()
                    .map(|argument| argument.as_ref())
                    .collect(),
            ),
            CoreBuiltinKind::Case if base.return_type.EvalType() == types::ETString => {
                let mut arguments: Vec<&dyn Expression> = (1..base.args.len())
                    .step_by(2)
                    .map(|index| base.args[index].as_ref())
                    .collect();
                if base.args.len() % 2 == 1 {
                    arguments.push(base.args[base.args.len() - 1].as_ref());
                }
                Some(arguments)
            }
            _ => None,
        };
        if let Some(arguments) = string_collation_arguments {
            let collation = crate::CheckAndDeriveCollationFromExprs(
                ctx,
                name,
                base.return_type.EvalType(),
                &arguments,
            )?;
            base.collator =
                collate::GetCollatorWithCollate(ctx.NewCollationEnabled(), &collation.Collation);
            base.SetCharsetAndCollation(collation.Charset, collation.Collation);
            if matches!(kind, CoreBuiltinKind::Compare(_, types::ETString)) {
                base.SetCoercibility(crate::CoercibilityNumeric);
                base.SetRepertoire(crate::ASCII);
            } else {
                base.SetCoercibility(collation.Coer);
                base.SetRepertoire(collation.Repe);
            }
        }
        let ilike = matches!(kind, CoreBuiltinKind::Ilike).then(|| {
            crate::builtin_ilike_kernel::IlikeSig::new_with_collation_mode(
                base.CharsetAndCollation().1,
                ctx.NewCollationEnabled(),
                base.args[1].ConstLevel() >= crate::ConstOnlyInContext,
                base.args[2].ConstLevel() >= crate::ConstOnlyInContext,
            )
        });
        GeneratedBuiltinFactoryOutput::new(signature, Box::new(CoreBuiltin { base, kind, ilike }))
    }

    macro_rules! core_factory {
        ($factory:ident, $name:literal) => {
            fn $factory(
                ctx: &dyn BuildContext,
                args: Vec<Box<dyn Expression>>,
                metadata: &FunctionClassMetadata,
            ) -> Result<GeneratedBuiltinFactoryOutput, Error> {
                core_builtin_factory($name, ctx, args, metadata)
            }
        };
    }

    core_factory!(eq_factory, "eq");
    core_factory!(ne_factory, "ne");
    core_factory!(lt_factory, "lt");
    core_factory!(le_factory, "le");
    core_factory!(gt_factory, "gt");
    core_factory!(ge_factory, "ge");
    core_factory!(null_eq_factory, "nulleq");
    core_factory!(in_factory, "in");
    core_factory!(is_null_factory, "isnull");
    core_factory!(and_factory, "and");
    core_factory!(or_factory, "or");
    core_factory!(xor_factory, "xor");
    core_factory!(not_factory, "not");
    core_factory!(unary_minus_factory, "unaryminus");
    core_factory!(plus_factory, "plus");
    core_factory!(minus_factory, "minus");
    core_factory!(multiply_factory, "mul");
    core_factory!(divide_factory, "div");
    core_factory!(integer_divide_factory, "intdiv");
    core_factory!(modulo_factory, "mod");
    core_factory!(case_factory, "case");
    fn if_factory(
        ctx: &dyn BuildContext,
        args: Vec<Box<dyn Expression>>,
        metadata: &FunctionClassMetadata,
    ) -> Result<GeneratedBuiltinFactoryOutput, Error> {
        // IF(cond, then, else) is the three-argument CASE execution shape.
        // Keep the registered function name (`if`) on ScalarFunction while
        // sharing CASE's fully typed evaluator and protobuf signature.
        core_builtin_factory("case", ctx, args, metadata)
    }
    core_factory!(row_factory, "row");
    core_factory!(json_extract_factory, "json_extract");
    core_factory!(uuid_factory, "uuid");
    core_factory!(rand_factory, "rand");
    core_factory!(md5_factory, "md5");
    core_factory!(set_var_factory, "setvar");
    core_factory!(time_to_sec_factory, "time_to_sec");
    core_factory!(extract_factory, "extract");
    core_factory!(sleep_factory, "sleep");
    core_factory!(substr_factory, "substr");
    core_factory!(substring_factory, "substring");
    core_factory!(mid_factory, "mid");
    core_factory!(trim_factory, "trim");
    core_factory!(ascii_factory, "ascii");
    core_factory!(weekday_factory, "weekday");
    core_factory!(atan_factory, "atan");
    core_factory!(atan2_factory, "atan2");
    core_factory!(pow_factory, "pow");
    core_factory!(power_factory, "power");
    core_factory!(now_factory, "now");
    core_factory!(current_timestamp_factory, "current_timestamp");
    core_factory!(localtimestamp_factory, "localtimestamp");
    core_factory!(localtime_factory, "localtime");
    core_factory!(date_add_factory, "date_add");
    core_factory!(adddate_factory, "adddate");
    core_factory!(date_sub_factory, "date_sub");
    core_factory!(subdate_factory, "subdate");
    core_factory!(char_length_factory, "char_length");
    core_factory!(length_factory, "length");
    core_factory!(hex_factory, "hex");
    core_factory!(find_in_set_factory, "find_in_set");
    core_factory!(space_factory, "space");
    core_factory!(rpad_factory, "rpad");
    core_factory!(insert_factory, "insert");
    core_factory!(ifnull_factory, "ifnull");
    core_factory!(like_factory, "like");
    core_factory!(ilike_factory, "ilike");
    core_factory!(is_ipv4_factory, "is_ipv4");
    core_factory!(is_ipv6_factory, "is_ipv6");
    core_factory!(regexp_instr_factory, "regexp_instr");
    core_factory!(regexp_substr_factory, "regexp_substr");
    core_factory!(regexp_replace_factory, "regexp_replace");
    core_factory!(tidb_shard_factory, "tidb_shard");
    core_factory!(symbol_eq_factory, "=");
    core_factory!(symbol_ne_factory, "!=");
    core_factory!(symbol_ne_alt_factory, "<>");
    core_factory!(symbol_lt_factory, "<");
    core_factory!(symbol_le_factory, "<=");
    core_factory!(symbol_gt_factory, ">");
    core_factory!(symbol_ge_factory, ">=");
    core_factory!(symbol_plus_factory, "+");
    core_factory!(symbol_minus_factory, "-");
    core_factory!(symbol_multiply_factory, "*");
    core_factory!(symbol_divide_factory, "/");
    core_factory!(symbol_modulo_factory, "%");

    const CORE_BUILTIN_FACTORIES: &[(&str, BuiltinFactory)] = &[
        ("eq", eq_factory),
        ("ne", ne_factory),
        ("lt", lt_factory),
        ("le", le_factory),
        ("gt", gt_factory),
        ("ge", ge_factory),
        ("nulleq", null_eq_factory),
        ("in", in_factory),
        ("isnull", is_null_factory),
        ("and", and_factory),
        ("or", or_factory),
        ("xor", xor_factory),
        ("not", not_factory),
        ("unaryminus", unary_minus_factory),
        ("plus", plus_factory),
        ("minus", minus_factory),
        ("mul", multiply_factory),
        ("div", divide_factory),
        ("intdiv", integer_divide_factory),
        ("mod", modulo_factory),
        ("case", case_factory),
        ("if", if_factory),
        ("row", row_factory),
        ("json_extract", json_extract_factory),
        ("uuid", uuid_factory),
        ("rand", rand_factory),
        ("md5", md5_factory),
        ("setvar", set_var_factory),
        ("time_to_sec", time_to_sec_factory),
        ("extract", extract_factory),
        ("sleep", sleep_factory),
        ("substr", substr_factory),
        ("substring", substring_factory),
        ("mid", mid_factory),
        ("trim", trim_factory),
        ("ascii", ascii_factory),
        ("weekday", weekday_factory),
        ("atan", atan_factory),
        ("atan2", atan2_factory),
        ("pow", pow_factory),
        ("power", power_factory),
        ("now", now_factory),
        ("current_timestamp", current_timestamp_factory),
        ("localtimestamp", localtimestamp_factory),
        ("localtime", localtime_factory),
        ("date_add", date_add_factory),
        ("adddate", adddate_factory),
        ("date_sub", date_sub_factory),
        ("subdate", subdate_factory),
        ("char_length", char_length_factory),
        ("length", length_factory),
        ("hex", hex_factory),
        ("find_in_set", find_in_set_factory),
        ("space", space_factory),
        ("rpad", rpad_factory),
        ("insert", insert_factory),
        ("ifnull", ifnull_factory),
        ("like", like_factory),
        ("ilike", ilike_factory),
        ("is_ipv4", is_ipv4_factory),
        ("is_ipv6", is_ipv6_factory),
        ("regexp_instr", regexp_instr_factory),
        ("regexp_substr", regexp_substr_factory),
        ("regexp_replace", regexp_replace_factory),
        ("tidb_shard", tidb_shard_factory),
        ("=", symbol_eq_factory),
        ("!=", symbol_ne_factory),
        ("<>", symbol_ne_alt_factory),
        ("<", symbol_lt_factory),
        ("<=", symbol_le_factory),
        (">", symbol_gt_factory),
        (">=", symbol_ge_factory),
        ("+", symbol_plus_factory),
        ("-", symbol_minus_factory),
        ("*", symbol_multiply_factory),
        ("/", symbol_divide_factory),
        ("%", symbol_modulo_factory),
    ];

    static BUILTIN_FACTORIES: LazyLock<RwLock<HashMap<String, BuiltinFactory>>> =
        LazyLock::new(|| RwLock::new(HashMap::new()));

    /// 函数 `registerBuiltinFactory`。
    pub fn registerBuiltinFactory(name: &str, factory: BuiltinFactory) -> Result<(), Error> {
        let mut factories = BUILTIN_FACTORIES
            .write()
            .map_err(|_| errors::New("builtin factory registry poisoned"))?;
        if factories.insert(name.to_owned(), factory).is_some() {
            return Err(errors::New(format!(
                "builtin factory '{name}' is already registered"
            )));
        }
        Ok(())
    }

    /// 函数 `removeBuiltinFactory`。
    pub fn removeBuiltinFactory(name: &str) {
        if let Ok(mut factories) = BUILTIN_FACTORIES.write() {
            factories.remove(name);
        }
    }

    fn invoke_factory(
        name: &str,
        ctx: &dyn BuildContext,
        args: Vec<Box<dyn Expression>>,
        metadata: &FunctionClassMetadata,
    ) -> Result<Box<dyn builtinFunc>, Error> {
        if matches!(
            name,
            crate::ast::Grouping | crate::ast::FTSMysqlMatchAgainst
        ) {
            return crate::planner_bridge_kernel::build_builtin(name, ctx, args)
                .expect("planner bridge builtin predicate and dispatcher must stay in sync");
        }
        let factory = BUILTIN_FACTORIES
            .read()
            .map_err(|_| errors::New("builtin factory registry poisoned"))?
            .get(name)
            .copied()
            .ok_or_else(|| {
                errors::New(format!(
                    "builtin function '{name}' is registered but its complete implementation is not linked"
                ))
            })?;
        factory(ctx, args, metadata)
            .map(GeneratedPolicyBuiltin::new)
            .map(|function| Box::new(function) as Box<dyn builtinFunc>)
    }

    #[derive(Default)]
    /// 结构体 `FunctionClassRegistry`。
    pub struct FunctionClassRegistry {
        entries: RwLock<HashMap<String, Arc<dyn functionClass>>>,
    }

    impl FunctionClassRegistry {
        fn from_specs(specs: &[(&str, usize, isize)]) -> Self {
            BUILTIN_FACTORIES
                .write()
                .expect("builtin factory registry poisoned")
                .extend(
                    CORE_BUILTIN_FACTORIES
                        .iter()
                        .map(|(name, factory)| ((*name).to_owned(), *factory)),
                );
            let entries = specs
                .iter()
                .map(|(name, min_args, max_args)| {
                    let class: Arc<dyn functionClass> = match *name {
                        "istrue" => Arc::new(isTrueOrFalseFunctionClass::new(
                            name,
                            opcode::IsTruth,
                            false,
                        )),
                        "istrue_with_null" => {
                            Arc::new(isTrueOrFalseFunctionClass::new(name, opcode::IsTruth, true))
                        }
                        "isfalse" => Arc::new(isTrueOrFalseFunctionClass::new(
                            name,
                            opcode::Op::IsFalsity,
                            false,
                        )),
                        "embed_text" => {
                            Arc::new(crate::builtin_inference::embedTextFunctionClass {
                                baseFunctionClass: baseFunctionClass::new(
                                    *name, *min_args, *max_args,
                                ),
                            })
                        }
                        _ => Arc::new(registeredFunctionClass::new(name, *min_args, *max_args)),
                    };
                    ((*name).to_owned(), class)
                })
                .collect();
            Self {
                entries: RwLock::new(entries),
            }
        }

        /// 函数 `get`。
        pub fn get(&self, name: &str) -> Option<Arc<dyn functionClass>> {
            self.entries.read().ok()?.get(name).cloned()
        }

        /// 函数 `Load`。
        pub fn Load(&self, name: &str) -> Option<Arc<dyn functionClass>> {
            self.get(name)
        }

        /// Atomically returns the existing class or inserts the supplied class.
        /// The boolean matches Go sync.Map.LoadOrStore: true means already loaded.
        /// 函数 `LoadOrStore`。
        pub fn LoadOrStore(
            &self,
            name: String,
            class: Arc<dyn functionClass>,
        ) -> (Arc<dyn functionClass>, bool) {
            let mut entries = self.entries.write().expect("function registry poisoned");
            if let Some(existing) = entries.get(&name) {
                return (Arc::clone(existing), true);
            }
            entries.insert(name, Arc::clone(&class));
            (class, false)
        }

        /// 函数 `contains_key`。
        pub fn contains_key(&self, name: &str) -> bool {
            self.entries
                .read()
                .is_ok_and(|entries| entries.contains_key(name))
        }

        /// 函数 `Store`。
        pub fn Store(
            &self,
            name: impl Into<String>,
            class: Arc<dyn functionClass>,
        ) -> Option<Arc<dyn functionClass>> {
            self.entries.write().ok()?.insert(name.into(), class)
        }

        /// 函数 `Delete`。
        pub fn Delete(&self, name: &str) -> Option<Arc<dyn functionClass>> {
            self.entries.write().ok()?.remove(name)
        }

        /// 函数 `names`。
        pub fn names(&self) -> Vec<String> {
            let mut names: Vec<String> = self
                .entries
                .read()
                .map(|entries| entries.keys().cloned().collect())
                .unwrap_or_default();
            names.sort_unstable();
            names
        }

        /// 函数 `len`。
        pub fn len(&self) -> usize {
            self.entries.read().map_or(0, |entries| entries.len())
        }

        /// 函数 `is_empty`。
        pub fn is_empty(&self) -> bool {
            self.len() == 0
        }
    }

    /// `builtin.go` 的完整 309 项 funcs 表；名称和 min/max 参数范围逐项来自 Go。
    const BUILTIN_SPECS: &[(&str, usize, isize)] = &[
        ("coalesce", 1, -1),
        ("isnull", 1, 1),
        ("greatest", 2, -1),
        ("least", 2, -1),
        ("interval", 2, -1),
        ("abs", 1, 1),
        ("acos", 1, 1),
        ("asin", 1, 1),
        ("atan", 1, 2),
        ("atan2", 2, 2),
        ("ceil", 1, 1),
        ("ceiling", 1, 1),
        ("conv", 3, 3),
        ("cos", 1, 1),
        ("cot", 1, 1),
        ("crc32", 1, 1),
        ("degrees", 1, 1),
        ("exp", 1, 1),
        ("floor", 1, 1),
        ("ln", 1, 1),
        ("log", 1, 2),
        ("log2", 1, 1),
        ("log10", 1, 1),
        ("pi", 0, 0),
        ("pow", 2, 2),
        ("power", 2, 2),
        ("radians", 1, 1),
        ("rand", 0, 1),
        ("round", 1, 2),
        ("sign", 1, 1),
        ("sin", 1, 1),
        ("sqrt", 1, 1),
        ("tan", 1, 1),
        ("truncate", 2, 2),
        ("adddate", 3, 3),
        ("date_add", 3, 3),
        ("subdate", 3, 3),
        ("date_sub", 3, 3),
        ("addtime", 2, 2),
        ("convert_tz", 3, 3),
        ("curdate", 0, 0),
        ("current_date", 0, 0),
        ("current_time", 0, 1),
        ("current_timestamp", 0, 1),
        ("curtime", 0, 1),
        ("date", 1, 1),
        ("'tidb`.(dateliteral", 1, 1),
        ("date_format", 2, 2),
        ("datediff", 2, 2),
        ("day", 1, 1),
        ("dayname", 1, 1),
        ("dayofmonth", 1, 1),
        ("dayofweek", 1, 1),
        ("dayofyear", 1, 1),
        ("extract", 2, 2),
        ("from_days", 1, 1),
        ("from_unixtime", 1, 2),
        ("get_format", 2, 2),
        ("hour", 1, 1),
        ("localtime", 0, 1),
        ("localtimestamp", 0, 1),
        ("makedate", 2, 2),
        ("maketime", 3, 3),
        ("microsecond", 1, 1),
        ("minute", 1, 1),
        ("month", 1, 1),
        ("monthname", 1, 1),
        ("now", 0, 1),
        ("period_add", 2, 2),
        ("period_diff", 2, 2),
        ("quarter", 1, 1),
        ("sec_to_time", 1, 1),
        ("second", 1, 1),
        ("str_to_date", 2, 2),
        ("subtime", 2, 2),
        ("sysdate", 0, 1),
        ("time", 1, 1),
        ("'tidb`.(timeliteral", 1, 1),
        ("time_format", 2, 2),
        ("time_to_sec", 1, 1),
        ("timediff", 2, 2),
        ("timestamp", 1, 2),
        ("'tidb`.(timestampliteral", 1, 2),
        ("timestampadd", 3, 3),
        ("timestampdiff", 3, 3),
        ("to_days", 1, 1),
        ("to_seconds", 1, 1),
        ("unix_timestamp", 0, 1),
        ("utc_date", 0, 0),
        ("utc_time", 0, 1),
        ("utc_timestamp", 0, 1),
        ("week", 1, 2),
        ("weekday", 1, 1),
        ("weekofyear", 1, 1),
        ("year", 1, 1),
        ("yearweek", 1, 2),
        ("last_day", 1, 1),
        ("tidb_bounded_staleness", 2, 2),
        ("tidb_parse_tso", 1, 1),
        ("tidb_parse_tso_logical", 1, 1),
        ("tidb_current_tso", 0, 0),
        ("ascii", 1, 1),
        ("bin", 1, 1),
        ("bit_length", 1, 1),
        ("char_func", 2, -1),
        ("char_length", 1, 1),
        ("character_length", 1, 1),
        ("concat", 1, -1),
        ("concat_ws", 2, -1),
        ("convert", 2, 2),
        ("elt", 2, -1),
        ("export_set", 3, 5),
        ("field", 2, -1),
        ("format", 2, 3),
        ("from_base64", 1, 1),
        ("find_in_set", 2, 2),
        ("hex", 1, 1),
        ("insert_func", 4, 4),
        ("instr", 2, 2),
        ("lcase", 1, 1),
        ("left", 2, 2),
        ("length", 1, 1),
        ("load_file", 1, 1),
        ("locate", 2, 3),
        ("lower", 1, 1),
        ("lpad", 3, 3),
        ("ltrim", 1, 1),
        ("mid", 2, 3),
        ("make_set", 2, -1),
        ("oct", 1, 1),
        ("octet_length", 1, 1),
        ("ord", 1, 1),
        ("position", 2, 2),
        ("quote", 1, 1),
        ("repeat", 2, 2),
        ("replace", 3, 3),
        ("reverse", 1, 1),
        ("right", 2, 2),
        ("rtrim", 1, 1),
        ("rpad", 3, 3),
        ("space", 1, 1),
        ("strcmp", 2, 2),
        ("substring", 2, 3),
        ("substr", 2, 3),
        ("substring_index", 3, 3),
        ("to_base64", 1, 1),
        ("trim", 1, 3),
        ("translate", 3, 3),
        ("upper", 1, 1),
        ("ucase", 1, 1),
        ("unhex", 1, 1),
        ("weight_string", 1, 3),
        ("connection_id", 0, 0),
        ("current_user", 0, 0),
        ("current_role", 0, 0),
        ("database", 0, 0),
        ("current_resource_group", 0, 0),
        ("schema", 0, 0),
        ("found_rows", 0, 0),
        ("last_insert_id", 0, 1),
        ("user", 0, 0),
        ("version", 0, 0),
        ("benchmark", 2, 2),
        ("charset", 1, 1),
        ("coercibility", 1, 1),
        ("collation", 1, 1),
        ("row_count", 0, 0),
        ("session_user", 0, 0),
        ("system_user", 0, 0),
        ("format_bytes", 1, 1),
        ("format_nano_time", 1, 1),
        ("if", 3, 3),
        ("ifnull", 2, 2),
        ("sleep", 1, 1),
        ("any_value", 1, 1),
        ("default_func", 1, 1),
        ("inet_aton", 1, 1),
        ("inet_ntoa", 1, 1),
        ("inet6_aton", 1, 1),
        ("inet6_ntoa", 1, 1),
        ("is_free_lock", 1, 1),
        ("is_ipv4", 1, 1),
        ("is_ipv4_compat", 1, 1),
        ("is_ipv4_mapped", 1, 1),
        ("is_ipv6", 1, 1),
        ("is_used_lock", 1, 1),
        ("is_uuid", 1, 1),
        ("name_const", 2, 2),
        ("release_all_locks", 0, 0),
        ("uuid", 0, 0),
        ("uuid_v4", 0, 0),
        ("uuid_v7", 0, 0),
        ("uuid_short", 0, 0),
        ("uuid_version", 1, 1),
        ("uuid_timestamp", 1, 1),
        ("vitess_hash", 1, 1),
        ("uuid_to_bin", 1, 2),
        ("bin_to_uuid", 1, 2),
        ("tidb_shard", 1, 1),
        ("tidb_row_checksum", 0, 0),
        ("grouping", 1, 1),
        ("get_lock", 2, 2),
        ("release_lock", 1, 1),
        ("and", 2, 2),
        ("or", 2, 2),
        ("xor", 2, 2),
        ("ge", 2, 2),
        ("le", 2, 2),
        ("eq", 2, 2),
        ("ne", 2, 2),
        ("lt", 2, 2),
        ("gt", 2, 2),
        ("nulleq", 2, 2),
        ("plus", 2, 2),
        ("minus", 2, 2),
        ("mod", 2, 2),
        ("div", 2, 2),
        ("mul", 2, 2),
        ("intdiv", 2, 2),
        ("bitneg", 1, 1),
        ("bitand", 2, 2),
        ("leftshift", 2, 2),
        ("rightshift", 2, 2),
        ("not", 1, 1),
        ("bitor", 2, 2),
        ("bitxor", 2, 2),
        ("unaryminus", 1, 1),
        ("=", 2, 2),
        ("!=", 2, 2),
        ("<>", 2, 2),
        ("<", 2, 2),
        ("<=", 2, 2),
        (">", 2, 2),
        (">=", 2, 2),
        ("+", 2, 2),
        ("-", 1, 2),
        ("*", 2, 2),
        ("/", 2, 2),
        ("%", 2, 2),
        ("in", 2, -1),
        ("istrue", 1, 1),
        ("istrue_with_null", 1, 1),
        ("isfalse", 1, 1),
        ("like", 3, 3),
        ("ilike", 3, 3),
        ("regexp", 2, 2),
        ("regexp_like", 2, 3),
        ("regexp_substr", 2, 5),
        ("regexp_instr", 2, 6),
        ("regexp_replace", 3, 6),
        ("case", 1, -1),
        ("row", 2, -1),
        ("setvar", 2, 2),
        ("bit_count", 1, 1),
        ("getparam", 1, 1),
        ("aes_decrypt", 2, 3),
        ("aes_encrypt", 2, 3),
        ("compress", 1, 1),
        ("decode", 2, 2),
        ("encode", 2, 2),
        ("md5", 1, 1),
        ("password", 1, 1),
        ("random_bytes", 1, 1),
        ("sha1", 1, 1),
        ("sha", 1, 1),
        ("sha2", 2, 2),
        ("sm3", 1, 1),
        ("uncompress", 1, 1),
        ("uncompressed_length", 1, 1),
        ("validate_password_strength", 1, 1),
        ("json_type", 1, 1),
        ("json_extract", 2, -1),
        ("json_unquote", 1, 1),
        ("json_set", 3, -1),
        ("json_insert", 3, -1),
        ("json_replace", 3, -1),
        ("json_remove", 2, -1),
        ("json_merge", 2, -1),
        ("json_object", 0, -1),
        ("json_array", 0, -1),
        ("json_memberof", 2, 2),
        ("json_contains", 2, 3),
        ("json_overlaps", 2, 2),
        ("json_contains_path", 3, -1),
        ("json_valid", 1, 1),
        ("json_array_append", 3, -1),
        ("json_array_insert", 3, -1),
        ("json_merge_patch", 2, -1),
        ("json_merge_preserve", 2, -1),
        ("json_pretty", 1, 1),
        ("json_quote", 1, 1),
        ("json_schema_valid", 2, 2),
        ("json_search", 3, -1),
        ("json_storage_free", 1, 1),
        ("json_storage_size", 1, 1),
        ("json_depth", 1, 1),
        ("json_keys", 1, 2),
        ("json_length", 1, 2),
        ("embed_text", 2, 3),
        ("vec_dims", 1, 1),
        ("vec_l1_distance", 2, 2),
        ("vec_l2_distance", 2, 2),
        ("vec_negative_inner_product", 2, 2),
        ("vec_cosine_distance", 2, 2),
        ("vec_l2_norm", 1, 1),
        ("vec_from_text", 1, 1),
        ("vec_as_text", 1, 1),
        ("match_against", 2, -1),
        ("tidb_decode_key", 1, 1),
        ("tidb_encode_record_key", 3, -1),
        ("tidb_encode_index_key", 4, -1),
        ("tidb_version", 0, 0),
        ("tidb_is_ddl_owner", 0, 0),
        ("tidb_decode_plan", 1, 1),
        ("tidb_decode_binary_plan", 1, 1),
        ("tidb_encode_sql_digest", 1, 1),
        ("nextval", 1, 1),
        ("lastval", 1, 1),
        ("setval", 2, 2),
        ("fts_match_word", 2, 2),
        ("tidb_mvcc_info", 1, 1),
        ("tidb_decode_sql_digests", 1, 2),
    ];

    pub static funcs: LazyLock<FunctionClassRegistry> =
        LazyLock::new(|| FunctionClassRegistry::from_specs(BUILTIN_SPECS));
    pub static extensionFuncs: LazyLock<FunctionClassRegistry> =
        LazyLock::new(FunctionClassRegistry::default);

    /// 按函数名校验参数个数。
    pub fn VerifyArgsWrapper(name: &str, count: usize) -> Result<(), Error> {
        match funcs.get(name) {
            Some(class) => class.verifyArgsByCount(count),
            // 与 Go 相同：调用者已保证函数受支持，表中不存在时不额外制造错误。
            None => Ok(()),
        }
    }

    /// 判断函数名是否在内置表中。
    pub fn IsFunctionSupported(name: &str) -> bool {
        funcs.contains_key(name)
    }

    /// 返回函数的展示名（运算符转为符号等）。
    pub fn GetDisplayName(name: &str) -> &str {
        display_name(name)
    }

    fn display_name(name: &str) -> &str {
        match name {
            "eq" => "=",
            "nulleq" => "<=>",
            "istrue" | "istrue_with_null" => "IS TRUE",
            "isfalse" => "IS FALSE",
            "ne" => "!=",
            "lt" => "<",
            "le" => "<=",
            "gt" => ">",
            "ge" => ">=",
            "plus" => "+",
            "minus" => "-",
            "mul" => "*",
            "div" => "/",
            "mod" => "%",
            "intdiv" => "DIV",
            _ => name,
        }
    }

    /// 返回排序后的内置函数名列表。
    pub fn GetBuiltinList() -> Vec<String> {
        let mut names: Vec<String> = funcs
            .names()
            .into_iter()
            .filter(|name| {
                name != "row" && name != "istrue_with_null" && !name.starts_with("'tidb`.(")
            })
            .collect();
        names.extend(extensionFuncs.names());
        names.sort_unstable();
        names.dedup();
        names
    }

    /// 常量 `InternalFuncToBinary`。
    pub const InternalFuncToBinary: &str = "to_binary";
    /// 常量 `InternalFuncFromBinary`。
    pub const InternalFuncFromBinary: &str = "from_binary";

    #[derive(Clone, PartialEq, Eq)]
    enum DynamicKind {
        Cast,
        GetVar,
        ToBinary(String),
        FromBinary(bool),
    }

    #[derive(Clone)]
    struct DynamicBuiltin {
        base: RegistryBuiltinBase,
        kind: DynamicKind,
    }

    impl DynamicBuiltin {
        fn cast(
            &self,
            ctx: &dyn EvalContext,
            row: chunk::Row,
        ) -> Result<(types::Datum, bool), Error> {
            let datum = self.base.args[0].Eval(ctx, row)?;
            if datum.IsNull() {
                return Ok((datum, true));
            }
            let datum = datum.ConvertTo(ctx.TypeCtx(), &self.base.return_type)?;
            let null = datum.IsNull();
            Ok((datum, null))
        }

        fn variable(
            &self,
            ctx: &dyn EvalContext,
            row: chunk::Row,
        ) -> Result<Option<types::Datum>, Error> {
            let (name, null) = self.base.args[0].EvalString(ctx, row)?;
            if null {
                return Ok(None);
            }
            Ok(ctx.GetUserVarsReader().GetUserVarVal(&name.to_lowercase()))
        }
    }

    /// tipb `ScalarFuncSig` values are stable wire identifiers. Keeping this table local avoids
    /// introducing a protobuf dependency into the expression crate while preserving Go's exact
    /// push-down code selection (including its deliberately unsupported vector combinations).
    fn cast_pb_code(source: &types::FieldType, target: &types::FieldType, binary: bool) -> i32 {
        use types::{
            ETDatetime, ETDecimal, ETDuration, ETInt, ETJson, ETReal, ETString, ETTimestamp,
            ETVectorFloat32,
        };

        let source_eval = source.EvalType();
        match target.EvalType() {
            ETInt if source.Hybrid() || binary => 1,
            ETInt => match source_eval {
                ETInt => 1,
                ETReal => 10,
                ETDecimal => 20,
                ETDatetime | ETTimestamp => 40,
                ETDuration => 50,
                ETJson => 60,
                ETString => 30,
                ETVectorFloat32 => 0,
                _ => 0,
            },
            ETReal if binary => 11,
            ETReal => match if source.Hybrid() { ETInt } else { source_eval } {
                ETInt => 2,
                ETReal => 11,
                ETDecimal => 21,
                ETDatetime | ETTimestamp => 41,
                ETDuration => 51,
                ETJson => 61,
                ETString => 31,
                ETVectorFloat32 => 0,
                _ => 0,
            },
            ETDecimal if binary => 23,
            ETDecimal => match if source.Hybrid() { ETInt } else { source_eval } {
                ETInt => 4,
                ETReal => 13,
                ETDecimal => 23,
                ETDatetime | ETTimestamp => 43,
                ETDuration => 53,
                ETJson => 63,
                ETString => 33,
                ETVectorFloat32 => 0,
                _ => 0,
            },
            ETString if source.Hybrid() => 32,
            ETString => match source_eval {
                ETInt => 3,
                ETReal => 12,
                ETDecimal => 22,
                ETDatetime | ETTimestamp => 42,
                ETDuration => 52,
                ETJson => 62,
                ETString => 32,
                ETVectorFloat32 => 5183,
                _ => 0,
            },
            ETDatetime | ETTimestamp => match source_eval {
                ETInt => 5,
                ETReal => 14,
                ETDecimal => 24,
                ETDatetime | ETTimestamp => 44,
                ETDuration => 54,
                ETJson => 64,
                ETString => 34,
                ETVectorFloat32 => 0,
                _ => 0,
            },
            ETDuration => match source_eval {
                ETInt => 6,
                ETReal => 15,
                ETDecimal => 25,
                ETDatetime | ETTimestamp => 45,
                ETDuration => 55,
                ETJson => 65,
                ETString => 35,
                ETVectorFloat32 => 0,
                _ => 0,
            },
            ETJson => match source_eval {
                ETInt => 7,
                ETReal => 16,
                ETDecimal => 26,
                ETDatetime | ETTimestamp => 46,
                ETDuration => 56,
                ETJson => 66,
                ETString => 36,
                ETVectorFloat32 => 0,
                _ => 0,
            },
            ETVectorFloat32 if source_eval == ETVectorFloat32 => 5188,
            ETVectorFloat32 => 0,
            _ => 0,
        }
    }

    impl CollationInfo for DynamicBuiltin {
        fn HasCoercibility(&self) -> bool {
            self.base.HasCoercibility()
        }
        fn Coercibility(&self) -> crate::Coercibility {
            self.base.Coercibility()
        }
        fn SetCoercibility(&self, v: crate::Coercibility) {
            self.base.SetCoercibility(v)
        }
        fn Repertoire(&self) -> crate::Repertoire {
            self.base.Repertoire()
        }
        fn SetRepertoire(&mut self, v: crate::Repertoire) {
            self.base.SetRepertoire(v)
        }
        fn CharsetAndCollation(&self) -> (String, String) {
            self.base.CharsetAndCollation()
        }
        fn SetCharsetAndCollation(&mut self, c: String, l: String) {
            self.base.SetCharsetAndCollation(c, l)
        }
        fn IsExplicitCharset(&self) -> bool {
            self.base.IsExplicitCharset()
        }
        fn SetExplicitCharset(&mut self, v: bool) {
            self.base.SetExplicitCharset(v)
        }
    }

    impl builtinFunc for DynamicBuiltin {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn SafeToShareAcrossSession(&self) -> bool {
            !matches!(self.kind, DynamicKind::GetVar) && self.base.SafeToShareAcrossSession()
        }
        fn evalInt(&self, c: &dyn EvalContext, r: chunk::Row) -> Result<(i64, bool), Error> {
            match self.kind {
                DynamicKind::Cast => {
                    let (d, n) = self.cast(c, r)?;
                    Ok((if n { 0 } else { d.GetInt64() }, n))
                }
                DynamicKind::GetVar => Ok(self
                    .variable(c, r)?
                    .map_or((0, true), |d| (d.GetInt64(), false))),
                _ => Err(errors::New("charset builtin does not return INT")),
            }
        }
        fn evalReal(&self, c: &dyn EvalContext, r: chunk::Row) -> Result<(f64, bool), Error> {
            match self.kind {
                DynamicKind::Cast => {
                    let (d, n) = self.cast(c, r)?;
                    Ok((if n { 0.0 } else { d.GetFloat64() }, n))
                }
                DynamicKind::GetVar => match self.variable(c, r)? {
                    None => Ok((0.0, true)),
                    Some(d) => Ok((d.ToFloat64(c.TypeCtx())?, false)),
                },
                _ => Err(errors::New("charset builtin does not return REAL")),
            }
        }
        fn evalString(&self, c: &dyn EvalContext, r: chunk::Row) -> Result<(String, bool), Error> {
            match &self.kind {
                DynamicKind::Cast => {
                    let (d, n) = self.cast(c, r)?;
                    Ok((if n { String::new() } else { d.ToString()? }, n))
                }
                DynamicKind::GetVar => match self.variable(c, r)? {
                    None => Ok((String::new(), true)),
                    Some(d) => Ok((d.ToString()?, false)),
                },
                DynamicKind::ToBinary(source) => {
                    let (v, n) = self.base.args[0].EvalString(c, r)?;
                    if n {
                        return Ok((String::new(), true));
                    }
                    let bytes = crate::charset::FindEncoding(source)
                        .Transform(&mut Vec::new(), v.as_bytes(), crate::charset::OpEncode)
                        .map_err(|error| errors::New(error.to_string()))?;
                    Ok((
                        String::from_utf8(bytes).map_err(|e| errors::New(e.to_string()))?,
                        false,
                    ))
                }
                DynamicKind::FromBinary(warn) => {
                    let (v, n) = self.base.args[0].EvalString(c, r)?;
                    if n {
                        return Ok((String::new(), true));
                    }
                    let encoding = crate::charset::FindEncoding(self.base.return_type.GetCharset());
                    let mut output = Vec::new();
                    match encoding.Transform(&mut output, v.as_bytes(), crate::charset::OpDecode) {
                        Ok(bytes) => Ok((
                            String::from_utf8(bytes).map_err(|e| errors::New(e.to_string()))?,
                            false,
                        )),
                        Err(e) if *warn => {
                            let conversion_error = format!(
                                "Cannot convert string '{}' from {} to {}: {e}",
                                v.as_bytes()
                                    .iter()
                                    .take(6)
                                    .map(|byte| format!("{byte:02X}"))
                                    .collect::<String>(),
                                crate::charset::CharsetBin,
                                self.base.return_type.GetCharset(),
                            );
                            c.AppendWarning(crate::contextutil::errors::New(conversion_error));
                            if c.SQLMode().HasStrictMode() {
                                Ok((String::new(), true))
                            } else {
                                Ok((
                                    String::from_utf8(e.output().to_vec())
                                        .map_err(|error| errors::New(error.to_string()))?,
                                    false,
                                ))
                            }
                        }
                        Err(e) => Err(errors::New(e.to_string())),
                    }
                }
            }
        }
        fn evalDecimal(
            &self,
            c: &dyn EvalContext,
            r: chunk::Row,
        ) -> Result<(types::MyDecimal, bool), Error> {
            match self.kind {
                DynamicKind::Cast => {
                    let (d, n) = self.cast(c, r)?;
                    Ok((
                        if n {
                            types::MyDecimal::default()
                        } else {
                            d.GetMysqlDecimal()
                        },
                        n,
                    ))
                }
                DynamicKind::GetVar => match self.variable(c, r)? {
                    None => Ok((types::MyDecimal::default(), true)),
                    Some(d) => Ok((d.ToDecimal(c.TypeCtx())?, false)),
                },
                _ => Err(errors::New("charset builtin does not return DECIMAL")),
            }
        }
        fn evalTime(
            &self,
            c: &dyn EvalContext,
            r: chunk::Row,
        ) -> Result<(types::Time, bool), Error> {
            match self.kind {
                DynamicKind::Cast => {
                    let (d, n) = self.cast(c, r)?;
                    Ok((if n { types::ZeroTime } else { d.GetMysqlTime() }, n))
                }
                DynamicKind::GetVar => Ok(self
                    .variable(c, r)?
                    .map_or((types::ZeroTime, true), |d| (d.GetMysqlTime(), false))),
                _ => Err(errors::New("charset builtin does not return TIME")),
            }
        }
        fn evalDuration(
            &self,
            c: &dyn EvalContext,
            r: chunk::Row,
        ) -> Result<(types::Duration, bool), Error> {
            let (d, n) = self.cast(c, r)?;
            Ok((
                if n {
                    types::Duration::default()
                } else {
                    d.GetMysqlDuration()
                },
                n,
            ))
        }
        fn evalJSON(
            &self,
            c: &dyn EvalContext,
            r: chunk::Row,
        ) -> Result<(types::BinaryJSON, bool), Error> {
            let (d, n) = self.cast(c, r)?;
            Ok((
                if n {
                    types::BinaryJSON::default()
                } else {
                    d.GetMysqlJSON()
                },
                n,
            ))
        }
        fn evalVectorFloat32(
            &self,
            c: &dyn EvalContext,
            r: chunk::Row,
        ) -> Result<(types::VectorFloat32, bool), Error> {
            let (d, n) = self.cast(c, r)?;
            Ok((
                if n {
                    types::ZeroVectorFloat32()
                } else {
                    d.GetVectorFloat32()
                },
                n,
            ))
        }
        /// `getArgs`：builtin 内部辅助。
        fn getArgs(&self) -> &[Box<dyn Expression>] {
            &self.base.args
        }
        /// `getArgsMut`：builtin 内部辅助。
        fn getArgsMut(&mut self) -> &mut [Box<dyn Expression>] {
            &mut self.base.args
        }
        fn equal(&self, c: &dyn EvalContext, o: &dyn builtinFunc) -> bool {
            o.as_any()
                .downcast_ref::<Self>()
                .is_some_and(|r| self.kind == r.kind && self.base.equal(c, &r.base))
        }
        /// `getRetTp`：builtin 内部辅助。
        fn getRetTp(&self) -> &types::FieldType {
            &self.base.return_type
        }
        /// `setPbCode`：builtin 内部辅助。
        fn setPbCode(&mut self, v: i32) {
            self.base.pb_code = v
        }
        fn PbCode(&self) -> i32 {
            self.base.pb_code
        }
        /// `setCollator`：builtin 内部辅助。
        fn setCollator(&mut self, v: Box<dyn collate::Collator>) {
            self.base.collator = v
        }
        fn collator(&self) -> &dyn collate::Collator {
            self.base.collator.as_ref()
        }
        fn Clone(&self) -> Box<dyn builtinFunc> {
            Box::new(self.clone())
        }
        fn MemoryUsage(&self) -> i64 {
            self.base.memory_usage()
        }
        fn vectorized(&self) -> bool {
            true
        }
    }

    fn scalar(
        name: &str,
        tp: types::FieldType,
        function: Box<dyn builtinFunc>,
    ) -> Box<dyn Expression> {
        Box::new(crate::ScalarFunction {
            FuncName: crate::ast::NewCIStr(name),
            RetType: Some(tp),
            Function: function,
            hashcode: Vec::new(),
            canonicalhashcode: Vec::new(),
        })
    }

    /// 函数 `BuildCastFunction`。
    pub fn BuildCastFunction(
        ctx: &dyn BuildContext,
        expr: &Box<dyn Expression>,
        target: &types::FieldType,
    ) -> Box<dyn Expression> {
        let mut tp = target.clone();
        let source_type = expr.GetType(ctx.GetEvalCtx());
        if !mysql::HasNotNullFlag(source_type.GetFlag()) {
            tp.DelFlag(mysql::NotNullFlag)
        }
        if tp.EvalType() == types::ETString && source_type.GetType() == mysql::TypeBit {
            tp.SetFlen((source_type.GetFlen() + 7) / 8)
        }
        if tp.EvalType() == types::ETJson && source_type.EvalType() == types::ETString {
            tp.AddFlag(mysql::ParseToJSONFlag);
        }
        let mut base = RegistryBuiltinBase::new_recursive(vec![expr.CloneExpr()], tp.clone());
        // Go deriveCollation(ast.Cast): CAST inherits coercibility/repertoire
        // from its argument. A string result initially uses the connection
        // charset/collation; the caller's return FieldType remains unchanged.
        let (cast_charset, cast_collation) = if tp.EvalType() == types::ETString {
            ctx.GetCharsetInfo()
        } else {
            (
                source_type.GetCharset().to_owned(),
                source_type.GetCollate().to_owned(),
            )
        };
        base.collation_info.SetCoercibility(expr.Coercibility());
        base.collation_info.SetRepertoire(expr.Repertoire());
        base.collation_info
            .SetCharsetAndCollation(cast_charset, cast_collation);
        base.pb_code = cast_pb_code(source_type, &tp, crate::IsBinaryLiteral(expr.as_ref()));
        let f = Box::new(DynamicBuiltin {
            base,
            kind: DynamicKind::Cast,
        });
        let out = scalar(crate::ast::Cast, tp.clone(), f);
        if tp.EvalType() == types::ETJson {
            out
        } else {
            crate::FoldConstant(ctx, out)
        }
    }

    /// 函数 `BuildGetVarFunction`。
    pub fn BuildGetVarFunction(
        ctx: &dyn BuildContext,
        expr: &Box<dyn Expression>,
        tp: &types::FieldType,
    ) -> Result<Box<dyn Expression>, Error> {
        let mut tp = tp.clone();
        if tp.GetType() == mysql::TypeUnspecified {
            tp = *types::NewFieldType(mysql::TypeVarString);
            let (charset, collation) = ctx.GetCharsetInfo();
            tp.SetCharset(charset);
            tp.SetCollate(collation);
        }
        let f = Box::new(DynamicBuiltin {
            base: RegistryBuiltinBase::new_never(vec![expr.CloneExpr()], tp.clone()),
            kind: DynamicKind::GetVar,
        });
        let out = scalar(crate::ast::GetVar, tp.clone(), f);
        let Some(k) = expr.as_any().downcast_ref::<crate::Constant>() else {
            return Ok(out);
        };
        if k.DeferredExpr.is_some() {
            return Ok(out);
        }
        let name = k.Value.GetString();
        if !ctx.IsReadonlyUserVar(&name) {
            return Ok(out);
        }
        let mut value = out.Eval(ctx.GetEvalCtx(), chunk::Row::default())?;
        if ctx
            .GetEvalCtx()
            .GetUserVarsReader()
            .GetUserVarVal(&name)
            .is_some_and(|datum| datum.Kind() == types::KindBinaryLiteral)
        {
            value.SetBinaryLiteral(types::BinaryLiteral(value.GetBytes()));
        }
        Ok(Box::new(crate::Constant::with_type(value, tp)))
    }

    /// 函数 `BuildToBinaryFunction`。
    pub fn BuildToBinaryFunction(
        ctx: &dyn BuildContext,
        expr: &Box<dyn Expression>,
    ) -> Box<dyn Expression> {
        if expr.GetType(ctx.GetEvalCtx()).EvalType() != types::ETString {
            return expr.CloneExpr();
        }
        let mut tp = expr.GetType(ctx.GetEvalCtx()).clone();
        let source = tp.GetCharset().to_owned();
        tp.SetType(mysql::TypeVarString);
        tp.SetCharset(crate::charset::CharsetBin.to_owned());
        tp.SetCollate(crate::charset::CollationBin.to_owned());
        let mut base = RegistryBuiltinBase::new_recursive(vec![expr.CloneExpr()], tp.clone());
        base.pb_code = 7071;
        let f = Box::new(DynamicBuiltin {
            base,
            kind: DynamicKind::ToBinary(source),
        });
        crate::FoldConstant(ctx, scalar(InternalFuncToBinary, tp, f))
    }

    /// 函数 `BuildFromBinaryFunction`。
    pub fn BuildFromBinaryFunction(
        ctx: &dyn BuildContext,
        expr: &Box<dyn Expression>,
        tp: &types::FieldType,
        warn: bool,
    ) -> Box<dyn Expression> {
        if expr.GetType(ctx.GetEvalCtx()).EvalType() != types::ETString {
            return expr.CloneExpr();
        }
        let mut base = RegistryBuiltinBase::new_never(vec![expr.CloneExpr()], tp.clone());
        base.pb_code = 7072;
        let f = Box::new(DynamicBuiltin {
            base,
            kind: DynamicKind::FromBinary(warn),
        });
        crate::FoldConstant(ctx, scalar(InternalFuncFromBinary, tp.clone(), f))
    }
}

#[cfg(test)]
#[path = "builtin_32_aster_unit_test.rs"]
/// 条件编译挂载 `builtin_32_aster_unit_test`。
mod builtin_aster_unit_test;
