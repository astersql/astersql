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

// 列字段类型 FieldType：存储类型元数据、比较、串化与 CAST 格式化。
//
// 对照 Go 的 `field_type.go`。FieldType 描述列/表达式的 MySQL 类型码、长度、
// 小数位、字符集/排序规则、ENUM/SET 元素及数组标记，供解析器、类型推断与
// information_schema 输出使用。

// 本文件对照 pkg/parser/types/field_type.go，保留字段、方法与控制流顺序。

use std::any::Any;
use std::fmt;
use std::io::Write;
use std::mem::size_of;

/// UnspecifiedLength 表示长度或小数位未指定的哨兵值（-1）。
// UnspecifiedLength 对应 Go 的未指定长度和小数位哨兵值。
pub const UnspecifiedLength: isize = -1;

/// TiDBStrictIntegerDisplayWidth 控制整型显示宽度是否严格输出。
// TiDBStrictIntegerDisplayWidth 对应 Go 包级兼容开关。
// Rust 的可变静态值需要同步保护；保留读写形状，未接入真实配置系统。
pub static mut TiDBStrictIntegerDisplayWidth: bool = false;

/// PostgreSQL array element identity, separate from MySQL JSON/multi-valued indexes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum PostgreSQLArrayElement {
    Int2,
    Int4,
    Int8,
    Numeric,
    Float4,
    Float8,
    Bool,
    Text,
    Varchar,
    Bpchar,
    Bytea,
    Date,
    Time,
    Timestamp,
    Timestamptz,
    Interval,
    Uuid,
}
impl PostgreSQLArrayElement {
    pub fn oid(self) -> u32 {
        match self {
            Self::Int2 => 21,
            Self::Int4 => 23,
            Self::Int8 => 20,
            Self::Numeric => 1700,
            Self::Float4 => 700,
            Self::Float8 => 701,
            Self::Bool => 16,
            Self::Text => 25,
            Self::Varchar => 1043,
            Self::Bpchar => 1042,
            Self::Bytea => 17,
            Self::Date => 1082,
            Self::Time => 1083,
            Self::Timestamp => 1114,
            Self::Timestamptz => 1184,
            Self::Interval => 1186,
            Self::Uuid => 2950,
        }
    }
    pub fn array_oid(self) -> u32 {
        match self {
            Self::Int2 => 1005,
            Self::Int4 => 1007,
            Self::Int8 => 1016,
            Self::Numeric => 1231,
            Self::Float4 => 1021,
            Self::Float8 => 1022,
            Self::Bool => 1000,
            Self::Text => 1009,
            Self::Varchar => 1015,
            Self::Bpchar => 1014,
            Self::Bytea => 1001,
            Self::Date => 1182,
            Self::Time => 1183,
            Self::Timestamp => 1115,
            Self::Timestamptz => 1185,
            Self::Interval => 1187,
            Self::Uuid => 2951,
        }
    }
}

/// A field of an anonymous PostgreSQL record. Scalar OIDs and nested layouts
/// remain independent of the shared MySQL transport code and display strings.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PostgreSQLRecordField {
    pub oid: u32,
    pub fields: Option<Vec<PostgreSQLRecordField>>,
}

/// FieldType 是列/表达式的完整类型描述，字段布局对齐 Go 同名结构。
// FieldType 逐字段对应 Go 结构，Option<Vec<_>> 同时保留切片顺序与 nil/非 nil 状态。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FieldType {
    tp: u8,
    flag: usize,
    flen: isize,
    decimal: isize,
    charset: String,
    collate: String,
    elems: Option<Vec<String>>,
    elemsIsBinaryLit: Option<Vec<bool>>,
    array: bool,
    postgresql_text_oid: Option<u32>,
    postgresql_array_element: Option<PostgreSQLArrayElement>,
    postgresql_record_fields: Option<Vec<PostgreSQLRecordField>>,
}

impl FieldType {
    /// DeepCopy 深复制 FieldType；`None` 对应 Go 的 nil 接收者。
    // DeepCopy 对应 Go 的深复制；Option 保留 nil 接收者可返回 nil 的语义。
    pub fn DeepCopy(value: Option<&Self>) -> Option<Self> {
        value.cloned()
    }

    /// Hash64 按 Go 顺序把全部字段写入 IHasher，用于规划器等价判定。
    // Hash64 依次写入全部字段，保持 Go 哈希顺序，尤其保留两个切片的长度前缀。
    pub fn Hash64(&self, h: &mut dyn util::IHasher) {
        h.HashByte(self.tp);
        h.HashUint64(self.flag as u64);
        h.HashInt(self.flen);
        h.HashInt(self.decimal);
        h.HashString(&self.charset);
        h.HashString(&self.collate);
        h.HashInt(self.GetElems().len() as isize);
        for elem in self.GetElems() {
            h.HashString(elem);
        }
        h.HashInt(self.GetElemsIsBinaryLit().len() as isize);
        for elem in self.GetElemsIsBinaryLit() {
            h.HashBool(*elem);
        }
        h.HashBool(self.array);
        if let Some(oid) = self.postgresql_text_oid {
            h.HashUint64(oid as u64);
        }
        if let Some(element) = self.postgresql_array_element {
            h.HashUint64(element.array_oid() as u64);
        }
        fn record_hash(fields: &[PostgreSQLRecordField], h: &mut dyn util::IHasher) {
            h.HashInt(fields.len() as isize);
            for field in fields {
                h.HashUint64(field.oid as u64);
                h.HashBool(field.fields.is_some());
                if let Some(fields) = &field.fields {
                    record_hash(fields, h);
                }
            }
        }
        if let Some(fields) = &self.postgresql_record_fields {
            h.HashUint64(2249);
            record_hash(fields, h);
        }
    }

    /// Equals 做动态类型比较；非 FieldType 或字段不一致时返回 false。
    // Equals 对应 cascades/base.Hasher 的动态类型比较；类型不匹配时直接返回 false。
    pub fn Equals(&self, other: &dyn Any) -> bool {
        let Some(other) = other.downcast_ref::<FieldType>() else {
            return false;
        };
        self.tp == other.tp
            && self.flag == other.flag
            && self.flen == other.flen
            && self.decimal == other.decimal
            && self.charset == other.charset
            && self.collate == other.collate
            && self.GetElems() == other.GetElems()
            && self.GetElemsIsBinaryLit() == other.GetElemsIsBinaryLit()
            && self.array == other.array
            && self.postgresql_text_oid == other.postgresql_text_oid
            && self.postgresql_array_element == other.postgresql_array_element
            && self.postgresql_record_fields == other.postgresql_record_fields
    }

    /// new_field_type 内部构造：长度与小数位初始化为 UnspecifiedLength。
    // new_field_type 支撑包级 NewFieldType，把长度与小数位初始化为未指定。
    fn new_field_type(tp: u8) -> Self {
        Self {
            tp,
            flen: UnspecifiedLength,
            decimal: UnspecifiedLength,
            ..Self::default()
        }
    }

    /// IsDecimalValid 检查 DECIMAL 精度、标度是否在 MySQL 合法范围内。
    // IsDecimalValid 检查 DECIMAL 的精度、标度及两者关系。
    pub fn IsDecimalValid(&self) -> bool {
        self.GetType() != mysql::TypeNewDecimal
            || (self.decimal >= 0
                && self.decimal <= mysql::MaxDecimalScale as isize
                && self.flen > 0
                && self.flen <= mysql::MaxDecimalWidth as isize
                && self.flen >= self.decimal)
    }

    /// IsVarLengthType 判断是否为 VARCHAR/BLOB/JSON/向量等可变长度类型。
    // IsVarLengthType 保留 Go switch 中全部可变长度类型。
    pub fn IsVarLengthType(&self) -> bool {
        matches!(
            self.GetType(),
            mysql::TypeVarchar
                | mysql::TypeVarString
                | mysql::TypeJSON
                | mysql::TypeBlob
                | mysql::TypeTinyBlob
                | mysql::TypeMediumBlob
                | mysql::TypeLongBlob
                | mysql::TypeTiDBVectorFloat32
        )
    }

    /// GetType 返回对外类型码；带 array 标记时报告为 JSON。
    // GetType 在 array 标记存在时按 Go 语义向外报告 JSON，底层 tp 保持不变。
    pub fn GetType(&self) -> u8 {
        if self.array { mysql::TypeJSON } else { self.tp }
    }

    /// GetFlag 返回类型标志位集合。
    pub fn GetFlag(&self) -> usize {
        self.flag
    }
    /// GetFlen 返回显示/存储长度。
    pub fn GetFlen(&self) -> isize {
        self.flen
    }
    /// GetDecimal 返回小数位数（标度）。
    pub fn GetDecimal(&self) -> isize {
        self.decimal
    }
    /// GetCharset 返回字符集名。
    pub fn GetCharset(&self) -> &str {
        &self.charset
    }
    /// GetCollate 返回排序规则名。
    pub fn GetCollate(&self) -> &str {
        &self.collate
    }
    /// GetElems 返回 ENUM/SET 元素列表。
    pub fn GetElems(&self) -> &[String] {
        self.elems.as_deref().unwrap_or_default()
    }
    /// GetElemsOption 返回元素列表及其 nil/非 nil 状态。
    pub fn GetElemsOption(&self) -> Option<&[String]> {
        self.elems.as_deref()
    }
    /// GetElemsIsBinaryLit 返回各元素是否为二进制字面量的标记切片。
    pub fn GetElemsIsBinaryLit(&self) -> &[bool] {
        self.elemsIsBinaryLit.as_deref().unwrap_or_default()
    }

    /// SetType 写入底层类型并清除 array 标志。
    // SetType 写入底层类型并清除 array，与 Go 避免遗留数组标志的行为一致。
    pub fn SetType(&mut self, tp: u8) {
        self.tp = tp;
        self.array = false;
        self.postgresql_text_oid = None;
        self.postgresql_array_element = None;
        self.postgresql_record_fields = None;
    }
    /// SetFlag 覆盖全部标志位。
    pub fn SetFlag(&mut self, flag: usize) {
        self.flag = flag;
    }
    /// AddFlag 按位或追加标志。
    pub fn AddFlag(&mut self, flag: usize) {
        self.flag |= flag;
    }
    /// AndFlag 按位与收敛标志。
    pub fn AndFlag(&mut self, flag: usize) {
        self.flag &= flag;
    }
    /// ToggleFlag 按位异或翻转标志。
    pub fn ToggleFlag(&mut self, flag: usize) {
        self.flag ^= flag;
    }
    /// DelFlag 清除指定标志位。
    pub fn DelFlag(&mut self, flag: usize) {
        self.flag &= !flag;
    }
    /// SetFlen 设置长度，不做上限截断。
    pub fn SetFlen(&mut self, flen: isize) {
        self.flen = flen;
    }

    /// SetFlenUnderLimit 写入 flen；DECIMAL 时截断到最大精度。
    // SetFlenUnderLimit 仅对 DECIMAL 截断到 MySQL 最大精度，其他类型原样写入。
    pub fn SetFlenUnderLimit(&mut self, flen: isize) {
        self.flen = if self.GetType() == mysql::TypeNewDecimal {
            flen.min(mysql::MaxDecimalWidth as isize)
        } else {
            flen
        };
    }

    /// SetDecimal 设置小数位，不做上限截断。
    pub fn SetDecimal(&mut self, decimal: isize) {
        self.decimal = decimal;
    }

    /// SetDecimalUnderLimit 写入 decimal；DECIMAL 时截断到最大标度。
    // SetDecimalUnderLimit 仅对 DECIMAL 截断到最大标度。
    pub fn SetDecimalUnderLimit(&mut self, decimal: isize) {
        self.decimal = if self.GetType() == mysql::TypeNewDecimal {
            decimal.min(mysql::MaxDecimalScale as isize)
        } else {
            decimal
        };
    }

    /// UpdateFlenAndDecimalUnderLimit 按旧类型与增量更新 DECIMAL 的 flen/decimal。
    // UpdateFlenAndDecimalUnderLimit 对 DECIMAL 应用增量；旧值未指定时采用 MySQL 上限。
    pub fn UpdateFlenAndDecimalUnderLimit(
        &mut self,
        old: &FieldType,
        deltaDecimal: isize,
        mut deltaFlen: isize,
    ) {
        if self.GetType() != mysql::TypeNewDecimal {
            return;
        }
        if old.decimal < 0 {
            deltaFlen += mysql::MaxDecimalScale as isize;
            self.decimal = mysql::MaxDecimalScale as isize;
        } else {
            self.SetDecimal(old.decimal + deltaDecimal);
        }
        if old.flen < 0 {
            self.flen = mysql::MaxDecimalWidth as isize;
        } else {
            self.SetFlenUnderLimit(old.flen + deltaFlen);
        }
    }

    /// SetCharset 设置字符集。
    pub fn SetCharset(&mut self, charset: String) {
        self.charset = charset;
    }
    /// SetCollate 设置排序规则。
    pub fn SetCollate(&mut self, collate: String) {
        self.collate = collate;
    }
    /// SetElems 替换 ENUM/SET 元素列表。
    pub fn SetElems(&mut self, elems: Vec<String>) {
        self.elems = Some(elems);
    }
    /// SetElem 按索引更新单个元素。
    pub fn SetElem(&mut self, idx: usize, element: String) {
        self.elems.as_mut().unwrap()[idx] = element;
    }
    /// PostgreSQL text identity is independent of MySQL storage code/charset.
    pub fn PostgreSQLTextOid(&self) -> Option<u32> {
        self.postgresql_text_oid
    }
    pub fn SetPostgreSQLTextOid(&mut self, oid: u32) {
        assert!(matches!(oid, 25 | 1042 | 1043));
        self.postgresql_text_oid = Some(oid);
    }
    pub fn PostgreSQLArrayElement(&self) -> Option<PostgreSQLArrayElement> {
        self.postgresql_array_element
    }
    pub fn SetPostgreSQLArrayElement(&mut self, element: PostgreSQLArrayElement) {
        self.postgresql_array_element = Some(element);
    }

    pub fn PostgreSQLRecordFields(&self) -> Option<&[PostgreSQLRecordField]> {
        self.postgresql_record_fields.as_deref()
    }
    pub fn SetPostgreSQLRecordFields(&mut self, fields: Vec<PostgreSQLRecordField>) {
        self.postgresql_record_fields = Some(fields);
    }

    /// SetArray 设置是否为数组类型包装。
    pub fn SetArray(&mut self, array: bool) {
        self.array = array;
    }
    /// IsArray 返回是否带数组包装标记。
    pub fn IsArray(&self) -> bool {
        self.array
    }

    /// ArrayType 返回去掉数组包装后的类型副本。
    // ArrayType 返回去掉数组包装后的类型副本；Rust 借用模型不复刻 Go 的同指针返回，仅保留值语义。
    pub fn ArrayType(&self) -> FieldType {
        let mut result = self.clone();
        result.SetArray(false);
        result
    }

    /// SetElemWithIsBinaryLit 设置元素并可选标记为二进制字面量。
    // SetElemWithIsBinaryLit 延迟分配标记数组，只在首次出现二进制字面量时扩容。
    pub fn SetElemWithIsBinaryLit(&mut self, idx: usize, element: String, isBinaryLit: bool) {
        self.elems.as_mut().unwrap()[idx] = element;
        if isBinaryLit {
            if self.elemsIsBinaryLit.is_none() {
                self.elemsIsBinaryLit = Some(vec![false; self.GetElems().len()]);
            }
            self.elemsIsBinaryLit.as_mut().unwrap()[idx] = true;
        }
    }

    /// GetElem 按索引返回元素字符串。
    pub fn GetElem(&self, idx: usize) -> &str {
        &self.GetElems()[idx]
    }

    /// GetElemIsBinaryLit 查询元素是否为二进制字面量；未分配标记时返回 false。
    // GetElemIsBinaryLit 在惰性标记数组尚未创建时统一返回 false。
    pub fn GetElemIsBinaryLit(&self, idx: usize) -> bool {
        self.GetElemsIsBinaryLit()
            .get(idx)
            .copied()
            .unwrap_or(false)
    }

    /// CleanElemIsBinaryLit 清空二进制字面量标记数组。
    // CleanElemIsBinaryLit 对应 Go 将切片设为 nil；Option::None 保留该状态。
    pub fn CleanElemIsBinaryLit(&mut self) {
        self.elemsIsBinaryLit = None;
    }

    /// Clone 返回字段类型的副本。
    // Clone 对应 Go 的复制方法；Rust 所有权模型要求安全复制 String/Vec 数据。
    pub fn Clone(&self) -> FieldType {
        self.clone()
    }

    /// Equal 做表达式类型等价比较，按 Go 规则忽略部分 flen/decimal。
    // Equal 是表达式类型比较，只比较 unsigned 标志，并按 Go 规则忽略部分 flen/decimal。
    pub fn Equal(&self, other: &FieldType) -> bool {
        let tpEqual = self.GetType() == other.GetType()
            || (self.GetType() == mysql::TypeVarchar && other.GetType() == mysql::TypeVarString)
            || (self.GetType() == mysql::TypeVarString && other.GetType() == mysql::TypeVarchar);
        let flenEqual = self.flen == other.flen
            || (self.EvalType() == ETReal && self.decimal == UnspecifiedLength)
            || self.EvalType() == ETJson;
        let ignoreDecimal = matches!(self.EvalType(), ETInt | ETString);
        self.postgresql_text_oid == other.postgresql_text_oid
            && self.postgresql_record_fields == other.postgresql_record_fields
            && self.postgresql_array_element == other.postgresql_array_element
            && tpEqual
            && (ignoreDecimal || self.decimal == other.decimal)
            && self.charset == other.charset
            && self.collate == other.collate
            && flenEqual
            && mysql::HasUnsignedFlag(self.flag) == mysql::HasUnsignedFlag(other.flag)
            && self.GetElems() == other.GetElems()
    }

    /// PartialEqual 在 unsafe 模式下对字符串类型放宽 flen 比较。
    // PartialEqual 的 unsafe 分支只用于字符串类型：忽略 flen，但仍检查 NotNull、字符集、排序规则和元素。
    pub fn PartialEqual(&self, other: &FieldType, unsafe_compare: bool) -> bool {
        if self.postgresql_text_oid != other.postgresql_text_oid
            || self.postgresql_array_element != other.postgresql_array_element
            || self.postgresql_record_fields != other.postgresql_record_fields
        {
            return false;
        }
        if mysql::HasNotNullFlag(self.flag) != mysql::HasNotNullFlag(other.flag) {
            return false;
        }
        if !unsafe_compare || self.EvalType() != ETString || other.EvalType() != ETString {
            return self.Equal(other);
        }
        self.charset == other.charset
            && self.collate == other.collate
            && mysql::HasUnsignedFlag(self.flag) == mysql::HasUnsignedFlag(other.flag)
            && self.GetElems() == other.GetElems()
    }

    /// EvalType 将 MySQL 存储类型映射为表达式求值类型。
    // EvalType 把 MySQL 存储类型归入表达式求值类型，ENUM/SET 可由标志强制按整数求值。
    pub fn EvalType(&self) -> EvalType {
        match self.GetType() {
            mysql::TypeTiny
            | mysql::TypeShort
            | mysql::TypeInt24
            | mysql::TypeLong
            | mysql::TypeLonglong
            | mysql::TypeBit
            | mysql::TypeYear => ETInt,
            mysql::TypeFloat | mysql::TypeDouble => ETReal,
            mysql::TypeNewDecimal => ETDecimal,
            mysql::TypeDate | mysql::TypeDatetime => ETDatetime,
            mysql::TypeTimestamp => ETTimestamp,
            mysql::TypeDuration => ETDuration,
            mysql::TypeJSON => ETJson,
            mysql::TypeTiDBVectorFloat32 => ETVectorFloat32,
            mysql::TypeEnum | mysql::TypeSet if self.flag & mysql::EnumSetAsIntFlag != 0 => ETInt,
            _ => ETString,
        }
    }

    /// Hybrid 标识 ENUM/BIT/SET 等可在多种值形态间切换的类型。
    // Hybrid 标识在不同上下文可呈现不同值形态的 ENUM、BIT 和 SET。
    pub fn Hybrid(&self) -> bool {
        matches!(
            self.GetType(),
            mysql::TypeEnum | mysql::TypeBit | mysql::TypeSet
        )
    }

    /// Init 重置类型码与长度/小数位，其它元数据保留。
    // Init 重置基础类型以及长度/小数位，其他元数据与 Go 一样保持不变。
    pub fn Init(&mut self, tp: u8) {
        self.tp = tp;
        self.flen = UnspecifiedLength;
        self.decimal = UnspecifiedLength;
    }

    /// CompactStr 生成 information_schema 使用的紧凑类型字符串。
    // CompactStr 生成 information_schema 使用的紧凑类型串，保留各类型后缀分支。
    pub fn CompactStr(&self) -> String {
        let ts = TypeToStr(self.GetType(), &self.charset);
        let (defaultFlen, defaultDecimal) = mysql::GetDefaultFieldLengthAndDecimal(self.GetType());
        let isDecimalNotDefault = self.decimal != defaultDecimal
            && self.decimal != 0
            && self.decimal != UnspecifiedLength;
        let displayFlen = if self.flen == UnspecifiedLength {
            defaultFlen
        } else {
            self.flen
        };
        let displayDecimal = if self.decimal == UnspecifiedLength {
            defaultDecimal
        } else {
            self.decimal
        };

        let suffix = match self.GetType() {
            mysql::TypeEnum | mysql::TypeSet => {
                // 每个元素先执行 SQL 输出转义，再按 Go 的 "','" 连接。
                let elems: Vec<String> = self
                    .GetElems()
                    .iter()
                    .map(|e| format::OutputFormat(e))
                    .collect();
                format!("('{}')", elems.join("','"))
            }
            mysql::TypeTimestamp | mysql::TypeDatetime | mysql::TypeDuration
                if isDecimalNotDefault =>
            {
                format!("({displayDecimal})")
            }
            mysql::TypeDouble | mysql::TypeFloat if isDecimalNotDefault => {
                format!("({displayFlen},{displayDecimal})")
            }
            mysql::TypeNewDecimal => format!("({displayFlen},{displayDecimal})"),
            mysql::TypeBit | mysql::TypeVarchar | mysql::TypeString | mysql::TypeVarString => {
                format!("({displayFlen})")
            }
            mysql::TypeTiny
                if !strict_integer_width()
                    || mysql::HasZerofillFlag(self.flag)
                    || displayFlen == 1 =>
            {
                format!("({displayFlen})")
            }
            mysql::TypeShort | mysql::TypeInt24 | mysql::TypeLong | mysql::TypeLonglong
                if !strict_integer_width() || mysql::HasZerofillFlag(self.flag) =>
            {
                format!("({displayFlen})")
            }
            mysql::TypeYear => format!("({})", self.flen),
            mysql::TypeTiDBVectorFloat32 if self.flen != UnspecifiedLength => {
                format!("({})", self.flen)
            }
            mysql::TypeNull => "(0)".to_owned(),
            _ => String::new(),
        };
        ts + &suffix
    }

    /// InfoSchemaStr 在 CompactStr 基础上为无符号类型追加 ` unsigned`。
    // InfoSchemaStr 只为非 BIT/YEAR 的无符号类型追加 lowercase unsigned。
    pub fn InfoSchemaStr(&self) -> String {
        let suffix = if mysql::HasUnsignedFlag(self.flag)
            && self.GetType() != mysql::TypeBit
            && self.GetType() != mysql::TypeYear
        {
            " unsigned"
        } else {
            ""
        };
        self.CompactStr() + suffix
    }

    /// String 组合紧凑类型、UNSIGNED/ZEROFILL/BINARY 与字符集信息。
    // String 组合紧凑类型、数值标志以及字符集/排序规则，保持 Go 输出顺序。
    pub fn String(&self) -> String {
        if self.postgresql_record_fields.is_some() {
            return "record".to_owned();
        }
        let mut parts = vec![self.CompactStr()];
        if mysql::HasUnsignedFlag(self.flag) {
            parts.push("UNSIGNED".to_owned());
        }
        if mysql::HasZerofillFlag(self.flag) {
            parts.push("ZEROFILL".to_owned());
        }
        if mysql::HasBinaryFlag(self.flag) && self.GetType() != mysql::TypeString {
            parts.push("BINARY".to_owned());
        }
        if IsTypeChar(self.GetType()) || IsTypeBlob(self.GetType()) {
            if !self.charset.is_empty() && self.charset != charset::CharsetBin {
                parts.push(format!("CHARACTER SET {}", self.charset));
            }
            if !self.collate.is_empty() && self.collate != charset::CharsetBin {
                parts.push(format!("COLLATE {}", self.collate));
            }
        }
        parts.join(" ")
    }

    /// Restore 把完整字段类型写入 RestoreCtx，顺序与 Go 一致。
    // Restore 按 AST Node 语义把完整字段类型写入 RestoreCtx；写入顺序与 Go 一致。
    pub fn Restore(&self, ctx: &mut format::RestoreCtx) -> std::io::Result<()> {
        if let Some(element @ (PostgreSQLArrayElement::Int8 | PostgreSQLArrayElement::Int2 | PostgreSQLArrayElement::Int4 | PostgreSQLArrayElement::Bool | PostgreSQLArrayElement::Text | PostgreSQLArrayElement::Varchar | PostgreSQLArrayElement::Bpchar | PostgreSQLArrayElement::Bytea | PostgreSQLArrayElement::Numeric | PostgreSQLArrayElement::Float4 | PostgreSQLArrayElement::Float8 | PostgreSQLArrayElement::Date | PostgreSQLArrayElement::Time | PostgreSQLArrayElement::Timestamp | PostgreSQLArrayElement::Timestamptz | PostgreSQLArrayElement::Interval | PostgreSQLArrayElement::Uuid)) = self.postgresql_array_element {
            if matches!(element, PostgreSQLArrayElement::Varchar | PostgreSQLArrayElement::Bpchar) {
                ctx.WriteKeyWord(if element == PostgreSQLArrayElement::Varchar { "VARCHAR" } else if self.flen >= 0 { "CHAR" } else { "BPCHAR" })?;
                if self.flen >= 0 { ctx.WritePlain(&format!("({})", self.flen))?; }
                return ctx.WritePlain("[]");
            }
            if element == PostgreSQLArrayElement::Interval {
                let mut scalar = self.clone();
                scalar.postgresql_array_element = None;
                scalar.tp = mysql::TypePgInterval;
                scalar.RestoreAsCastType(ctx, false)?;
                return ctx.WritePlain("[]");
            }
            if matches!(element, PostgreSQLArrayElement::Time | PostgreSQLArrayElement::Timestamp | PostgreSQLArrayElement::Timestamptz) {
                ctx.WriteKeyWord(if element == PostgreSQLArrayElement::Time { "TIME" } else if element == PostgreSQLArrayElement::Timestamp { "TIMESTAMP" } else { "TIMESTAMPTZ" })?;
                if self.decimal >= 0 { ctx.WritePlain(&format!("({})", self.decimal))?; }
                return ctx.WritePlain("[]");
            }
            if element == PostgreSQLArrayElement::Numeric {
                ctx.WriteKeyWord("NUMERIC")?;
                if self.flen >= 0 { ctx.WritePlain(&format!("({},{})",self.flen,self.decimal))?; }
                return ctx.WritePlain("[]");
            }
            return ctx.WriteKeyWord(match element { PostgreSQLArrayElement::Uuid => "UUID[]", PostgreSQLArrayElement::Date => "DATE[]", PostgreSQLArrayElement::Float4 => "FLOAT4[]", PostgreSQLArrayElement::Float8 => "FLOAT8[]", PostgreSQLArrayElement::Bytea => "BYTEA[]", PostgreSQLArrayElement::Int8 => "INT8[]", PostgreSQLArrayElement::Int2 => "INT2[]", PostgreSQLArrayElement::Bool => "BOOL[]", PostgreSQLArrayElement::Text => "TEXT[]", _ => "INT4[]" });
        }
        if let Some(oid) = self.postgresql_text_oid {
            ctx.WriteKeyWord(match oid {
                1043 => "VARCHAR",
                1042 if self.flen >= 0 => "CHAR",
                1042 => "BPCHAR",
                _ => "TEXT",
            })?;
            if oid != 25 && self.flen >= 0 {
                ctx.WritePlain(&format!("({})", self.flen))?;
            }
            return Ok(());
        }
        ctx.WriteKeyWord(&TypeToStr(self.GetType(), &self.charset))?;
        let (mut precision, mut scale) = (UnspecifiedLength, UnspecifiedLength);
        match self.GetType() {
            mysql::TypeEnum | mysql::TypeSet => {
                ctx.WritePlain("(")?;
                for (index, elem) in self.GetElems().iter().enumerate() {
                    if index != 0 {
                        ctx.WritePlain(",")?;
                    }
                    ctx.WriteString(elem)?;
                }
                ctx.WritePlain(")")?;
            }
            mysql::TypeTimestamp | mysql::TypeDatetime | mysql::TypeDuration => {
                precision = self.decimal
            }
            mysql::TypeUnspecified
            | mysql::TypeFloat
            | mysql::TypeDouble
            | mysql::TypeNewDecimal => {
                precision = self.flen;
                scale = self.decimal;
            }
            _ => precision = self.flen,
        }
        if precision != UnspecifiedLength {
            ctx.WritePlain(&format!("({precision}"))?;
            if scale != UnspecifiedLength {
                ctx.WritePlain(&format!(",{scale}"))?;
            }
            ctx.WritePlain(")")?;
        }
        if mysql::HasUnsignedFlag(self.flag) {
            ctx.WriteKeyWord(" UNSIGNED")?;
        }
        if mysql::HasZerofillFlag(self.flag) {
            ctx.WriteKeyWord(" ZEROFILL")?;
        }
        if mysql::HasBinaryFlag(self.flag) && self.charset != charset::CharsetBin {
            ctx.WriteKeyWord(" BINARY")?;
        }
        if IsTypeChar(self.GetType()) || IsTypeBlob(self.GetType()) {
            if !self.charset.is_empty() && self.charset != charset::CharsetBin {
                ctx.WriteKeyWord(&format!(" CHARACTER SET {}", self.charset))?;
            }
            if !self.collate.is_empty() && self.collate != charset::CharsetBin {
                ctx.WriteKeyWord(" COLLATE ")?;
                ctx.WritePlain(&self.collate)?;
            }
        }
        Ok(())
    }

    /// RestoreAsCastType 写出 CAST 目标类型文本。
    // RestoreAsCastType 写出 CAST 目标类型；explicitCharset 控制 BINARY/CHARSET 附加信息。
    pub fn RestoreAsCastType(
        &self,
        ctx: &mut format::RestoreCtx,
        explicitCharset: bool,
    ) -> std::io::Result<()> {
        if self.postgresql_text_oid.is_some() || matches!(self.postgresql_array_element, Some(PostgreSQLArrayElement::Int8 | PostgreSQLArrayElement::Int2 | PostgreSQLArrayElement::Int4 | PostgreSQLArrayElement::Bool | PostgreSQLArrayElement::Text | PostgreSQLArrayElement::Varchar | PostgreSQLArrayElement::Bpchar | PostgreSQLArrayElement::Bytea | PostgreSQLArrayElement::Numeric | PostgreSQLArrayElement::Float4 | PostgreSQLArrayElement::Float8 | PostgreSQLArrayElement::Date | PostgreSQLArrayElement::Time | PostgreSQLArrayElement::Timestamp | PostgreSQLArrayElement::Timestamptz | PostgreSQLArrayElement::Interval | PostgreSQLArrayElement::Uuid)) {
            return self.Restore(ctx);
        }
        match self.tp {
            mysql::TypeVarString | mysql::TypeString => {
                let skipWriteBinary =
                    self.charset == charset::CharsetBin && self.collate == charset::CollationBin;
                ctx.WriteKeyWord(if skipWriteBinary { "BINARY" } else { "CHAR" })?;
                if self.flen != UnspecifiedLength {
                    ctx.WritePlain(&format!("({})", self.flen))?;
                }
                if explicitCharset {
                    if !skipWriteBinary && self.flag & mysql::BinaryFlag != 0 {
                        ctx.WriteKeyWord(" BINARY")?;
                    }
                    if self.charset != charset::CharsetBin && self.charset != mysql::DefaultCharset
                    {
                        ctx.WriteKeyWord(" CHARSET ")?;
                        ctx.WriteKeyWord(&self.charset)?;
                    }
                }
            }
            mysql::TypeDate => ctx.WriteKeyWord("DATE")?,
            mysql::TypePgUuid => ctx.WriteKeyWord("UUID")?,
            mysql::TypePgJson => ctx.WriteKeyWord("JSON")?,
            mysql::TypePgJsonb => ctx.WriteKeyWord("JSONB")?,
            mysql::TypePgTimestampTz => ctx.WriteKeyWord("TIMESTAMPTZ")?,
            mysql::TypePgInterval => {
                ctx.WriteKeyWord("INTERVAL")?;
                let fields = match self.flen {
                    4 => "YEAR",
                    2 => "MONTH",
                    6 => "YEAR TO MONTH",
                    8 => "DAY",
                    1024 => "HOUR",
                    2048 => "MINUTE",
                    4096 => "SECOND",
                    1032 => "DAY TO HOUR",
                    3080 => "DAY TO MINUTE",
                    7176 => "DAY TO SECOND",
                    3072 => "HOUR TO MINUTE",
                    7168 => "HOUR TO SECOND",
                    6144 => "MINUTE TO SECOND",
                    _ => "",
                };
                if !fields.is_empty() {
                    ctx.WritePlain(" ")?;
                    ctx.WriteKeyWord(fields)?;
                }
                if self.decimal >= 0 {
                    ctx.WritePlain(&format!("({})", self.decimal))?;
                }
            }
            mysql::TypeDatetime => {
                ctx.WriteKeyWord("DATETIME")?;
                write_scale(ctx, self.decimal)?;
            }
            mysql::TypeNewDecimal => {
                ctx.WriteKeyWord("DECIMAL")?;
                if self.flen > 0 && self.decimal > 0 {
                    ctx.WritePlain(&format!("({}, {})", self.flen, self.decimal))?;
                } else if self.flen > 0 {
                    ctx.WritePlain(&format!("({})", self.flen))?;
                }
            }
            mysql::TypeDuration => {
                ctx.WriteKeyWord("TIME")?;
                write_scale(ctx, self.decimal)?;
            }
            mysql::TypeLonglong => ctx.WriteKeyWord(if self.flag & mysql::UnsignedFlag != 0 {
                "UNSIGNED"
            } else {
                "SIGNED"
            })?,
            mysql::TypeJSON => ctx.WriteKeyWord("JSON")?,
            mysql::TypeDouble => ctx.WriteKeyWord("DOUBLE")?,
            mysql::TypeFloat => ctx.WriteKeyWord("FLOAT")?,
            mysql::TypeYear => ctx.WriteKeyWord("YEAR")?,
            mysql::TypeTiDBVectorFloat32 => ctx.WriteKeyWord("VECTOR")?,
            _ => {}
        }
        if self.array {
            ctx.WritePlain(" ")?;
            ctx.WriteKeyWord("ARRAY")?;
        }
        Ok(())
    }

    /// FormatAsCastType 将 CAST 类型格式化后写入任意 Writer。
    // FormatAsCastType 先复用 RestoreAsCastType 写入内存缓冲，再一次写入 io.Writer。
    pub fn FormatAsCastType(
        &self,
        writer: &mut dyn Write,
        explicitCharset: bool,
    ) -> std::io::Result<()> {
        let mut buffer = Vec::new();
        let mut ctx = format::NewRestoreCtx(format::DefaultRestoreFlags, &mut buffer);
        self.RestoreAsCastType(&mut ctx, explicitCharset)?;
        writer.write_all(&buffer)
    }

    /// StorageLength 估算编码后存储长度；可变长返回 VarStorageLen。
    // StorageLength 估算编码后固定长度；可变长类型返回 VarStorageLen。
    pub fn StorageLength(&self) -> isize {
        match self.GetType() {
            mysql::TypeTiny
            | mysql::TypeShort
            | mysql::TypeInt24
            | mysql::TypeLong
            | mysql::TypeLonglong
            | mysql::TypeDouble
            | mysql::TypeFloat
            | mysql::TypeYear
            | mysql::TypeDuration
            | mysql::TypeDate
            | mysql::TypeDatetime
            | mysql::TypeTimestamp
            | mysql::TypeEnum
            | mysql::TypeSet
            | mysql::TypeBit => 8,
            mysql::TypeNewDecimal => {
                let (precision, frac) = (self.flen - self.decimal, self.decimal);
                precision / digitsPerWord * wordSize
                    + dig2bytes[(precision % digitsPerWord) as usize]
                    + frac / digitsPerWord * wordSize
                    + dig2bytes[(frac % digitsPerWord) as usize]
            }
            _ => VarStorageLen,
        }
    }

    /// UnmarshalJSON 从 JSON 反序列化并整体覆盖本结构。
    // UnmarshalJSON 先完整解析临时结构，仅在成功后覆盖 self，避免部分写入。
    pub fn UnmarshalJSON(&mut self, data: &[u8]) -> Result<(), serde_json::Error> {
        let value: jsonFieldType = serde_json::from_slice(data)?;
        *self = value.into();
        Ok(())
    }

    /// MarshalJSON 序列化为与 Go jsonFieldType 键名一致的 JSON。
    // MarshalJSON 通过字段名与 Go jsonFieldType 一致的临时结构序列化。
    pub fn MarshalJSON(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(&jsonFieldType::from(self))
    }

    /// MemoryUsage 估算本结构占用的近似内存字节数。
    // MemoryUsage 对应 Go 的容量估算：结构体、字符串、两个 Vec 容量及元素内容分别累计。
    pub fn MemoryUsage(&self) -> i64 {
        let mut sum = emptyFieldTypeSize
            + self.charset.len() as i64
            + self.collate.len() as i64
            + (self.elems.as_ref().map_or(0, Vec::capacity) * size_of::<String>()) as i64
            + (self.elemsIsBinaryLit.as_ref().map_or(0, Vec::capacity) * size_of::<bool>()) as i64;
        for elem in self.GetElems() {
            sum += elem.len() as i64;
        }
        sum
    }
}

/// NewFieldType 按类型码构造 FieldType，长度与小数位为未指定。
// NewFieldType 保留 Go 包级构造函数名称，便于未来跨文件调用接线。
pub fn NewFieldType(tp: u8) -> FieldType {
    FieldType::new_field_type(tp)
}

/// HasCharset 判断该类型在 DDL 输出中是否应附带 CHARACTER SET。
// HasCharset 决定 SHOW CREATE TABLE 等输出是否附带 CHARACTER SET 子句。
pub fn HasCharset(ft: &FieldType) -> bool {
    match ft.GetType() {
        mysql::TypeVarchar
        | mysql::TypeString
        | mysql::TypeVarString
        | mysql::TypeBlob
        | mysql::TypeTinyBlob
        | mysql::TypeMediumBlob
        | mysql::TypeLongBlob => !mysql::HasBinaryFlag(ft.flag),
        mysql::TypeEnum | mysql::TypeSet => true,
        _ => false,
    }
}

/// VarStorageLen 是可变长度列的存储长度哨兵值（-1）。
// VarStorageLen 是可变长度列的存储长度哨兵值。
pub const VarStorageLen: isize = -1;

/// jsonFieldType 是 JSON 序列化中间结构，字段名与 Go 导出形式一致。
// jsonFieldType 对应 Go 的 JSON 中间结构，字段名刻意保留导出形式以维持 JSON 键名。
#[allow(non_camel_case_types)]
#[derive(serde::Deserialize, serde::Serialize)]
struct jsonFieldType {
    Tp: u8,
    Flag: usize,
    Flen: isize,
    Decimal: isize,
    Charset: String,
    Collate: String,
    Elems: Option<Vec<String>>,
    ElemsIsBinaryLit: Option<Vec<bool>>,
    Array: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    PostgreSQLTextOid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    PostgreSQLArrayElement: Option<PostgreSQLArrayElement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    PostgreSQLRecordFields: Option<Vec<PostgreSQLRecordField>>,
}

impl From<&FieldType> for jsonFieldType {
    fn from(ft: &FieldType) -> Self {
        Self {
            Tp: ft.tp,
            Flag: ft.flag,
            Flen: ft.flen,
            Decimal: ft.decimal,
            Charset: ft.charset.clone(),
            Collate: ft.collate.clone(),
            Elems: ft.elems.clone(),
            ElemsIsBinaryLit: ft.elemsIsBinaryLit.clone(),
            Array: ft.array,
            PostgreSQLTextOid: ft.postgresql_text_oid,
            PostgreSQLArrayElement: ft.postgresql_array_element,
            PostgreSQLRecordFields: ft.postgresql_record_fields.clone(),
        }
    }
}

impl From<jsonFieldType> for FieldType {
    fn from(value: jsonFieldType) -> Self {
        Self {
            tp: value.Tp,
            flag: value.Flag,
            flen: value.Flen,
            decimal: value.Decimal,
            charset: value.Charset,
            collate: value.Collate,
            elems: value.Elems,
            elemsIsBinaryLit: value.ElemsIsBinaryLit,
            array: value.Array,
            postgresql_text_oid: value.PostgreSQLTextOid,
            postgresql_array_element: value.PostgreSQLArrayElement,
            postgresql_record_fields: value.PostgreSQLRecordFields,
        }
    }
}

// Go 的 json.Marshal/json.Unmarshal 会自动调用 FieldType 的自定义 JSON 方法；
// Rust 的 serde trait 必须显式委托给同一中间结构，避免泄露小写私有字段名。
impl serde::Serialize for FieldType {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        jsonFieldType::from(self).serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for FieldType {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        jsonFieldType::deserialize(deserializer).map(Into::into)
    }
}

/// emptyFieldTypeSize 对应空 FieldType 结构本体大小。
// emptyFieldTypeSize 对应 Go unsafe.Sizeof(FieldType{})，只统计结构本体。
pub const emptyFieldTypeSize: i64 = size_of::<FieldType>() as i64;

/// strict_integer_width 读取整型显示宽度严格模式开关。
// strict_integer_width 集中封装可变静态读取；真实接线应改为线程安全配置来源。
fn strict_integer_width() -> bool {
    unsafe { TiDBStrictIntegerDisplayWidth }
}

/// write_scale 在正标度时写出 `(n)` 后缀。
// write_scale 复用 DATETIME/TIME 的正标度输出分支。
fn write_scale(ctx: &mut format::RestoreCtx, decimal: isize) -> std::io::Result<()> {
    if decimal > 0 {
        ctx.WritePlain(&format!("({decimal})"))?;
    }
    Ok(())
}

/// Display 委托给 String，对齐 Go fmt.Stringer。
// Display 对应 Go fmt.Stringer，直接复用 String 方法生成内容。
impl fmt::Display for FieldType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.String())
    }
}
