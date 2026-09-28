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

// MySQL 字段类型分类与杂项工具函数。
//
// 提供 Blob/Char/时间/数值等类型判定、字符串种别判断、
// Kind/Type 名称映射，以及 EOF 归一化与运算溢出辅助。

// Copyright 2014 The ql Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSES/QL-LICENSE file.

/// 判断是否为 Blob 族类型（tiny/medium/long blob）。
pub fn IsTypeBlob(tp: u8) -> bool {
    ast::IsTypeBlob(tp)
}
/// 判断是否为定长/变长字符类型（CHAR/VARCHAR 等）。
pub fn IsTypeChar(tp: u8) -> bool {
    ast::IsTypeChar(tp)
}
/// 判断是否为向量类型（VECTOR）。
pub fn IsTypeVector(tp: u8) -> bool {
    ast::IsTypeVector(tp)
}

/// 判断是否为 VARCHAR/VAR_STRING。
pub fn IsTypeVarchar(tp: u8) -> bool {
    tp == mysql::TypeVarString || tp == mysql::TypeVarchar
}

/// 判断是否为未指定类型（TypeUnspecified）。
pub fn IsTypeUnspecified(tp: u8) -> bool {
    tp == mysql::TypeUnspecified
}
/// 判断是否支持前缀索引（Blob 或 Char）。
pub fn IsTypePrefixable(tp: u8) -> bool {
    IsTypeBlob(tp) || IsTypeChar(tp)
}
/// 判断是否带小数秒精度（FSP）：DATETIME/TIME/TIMESTAMP。
pub fn IsTypeFractionable(tp: u8) -> bool {
    matches!(
        tp,
        mysql::TypeDatetime | mysql::TypeDuration | mysql::TypeTimestamp
    )
}
/// 判断是否为带日期部分的时间类型（不含 TIME/Duration）。
pub fn IsTypeTime(tp: u8) -> bool {
    matches!(
        tp,
        mysql::TypeDatetime | mysql::TypeDate | mysql::TypeTimestamp
    )
}
/// 判断是否为 FLOAT。
pub fn IsTypeFloat(tp: u8) -> bool {
    tp == mysql::TypeFloat
}
/// 判断是否为整数族（含 YEAR）。
pub fn IsTypeInteger(tp: u8) -> bool {
    matches!(
        tp,
        mysql::TypeTiny
            | mysql::TypeShort
            | mysql::TypeInt24
            | mysql::TypeLong
            | mysql::TypeLonglong
            | mysql::TypeYear
    )
}
/// 判断底层是否按整数存储（整数与时间/日期/Duration）。
pub fn IsTypeStoredAsInteger(tp: u8) -> bool {
    matches!(
        tp,
        mysql::TypeTiny
            | mysql::TypeShort
            | mysql::TypeInt24
            | mysql::TypeLong
            | mysql::TypeLonglong
            | mysql::TypeYear
            | mysql::TypeDatetime
            | mysql::TypeDate
            | mysql::TypeTimestamp
            | mysql::TypeDuration
    )
}
/// 判断是否为数值族（整数、浮点、Decimal、Bit）。
pub fn IsTypeNumeric(tp: u8) -> bool {
    matches!(
        tp,
        mysql::TypeBit
            | mysql::TypeTiny
            | mysql::TypeInt24
            | mysql::TypeLong
            | mysql::TypeLonglong
            | mysql::TypeNewDecimal
            | mysql::TypeFloat
            | mysql::TypeDouble
            | mysql::TypeShort
    )
}
/// 判断 FieldType 是否为 BIT。
pub fn IsTypeBit(ft: &FieldType) -> bool {
    ft.GetType() == mysql::TypeBit
}
/// 判断是否为带日期的时间类型（同 IsTypeTime）。
pub fn IsTemporalWithDate(tp: u8) -> bool {
    IsTypeTime(tp)
}
/// 判断是否为二进制字符串：bin collation 且字符串类类型。
pub fn IsBinaryStr(ft: &FieldType) -> bool {
    ft.GetCollate() == charset::CollationBin && IsString(ft.GetType())
}
/// 判断是否为非二进制字符串：非 bin collation 且字符串类类型。
pub fn IsNonBinaryStr(ft: &FieldType) -> bool {
    ft.GetCollate() != charset::CollationBin && IsString(ft.GetType())
}
/// 新校对规则下，非二进制字符串是否需要保留原始数据以便还原。
pub fn NeedRestoredData(ft: &FieldType) -> bool {
    NeedRestoredDataWithCollate(ft, collate::NewCollationEnabled())
}
/// 按是否启用新 collation 判定是否需要 restored data。
pub fn NeedRestoredDataWithCollate(ft: &FieldType, useNewCollate: bool) -> bool {
    useNewCollate
        && IsNonBinaryStr(ft)
        && (!collate::IsBinCollation(ft.GetCollate()) || IsTypeVarchar(ft.GetType()))
        && ft.GetCollate() != "utf8mb4_0900_bin"
}
/// 判断 MySQL 类型码是否属于字符串类（Char/Blob/Varchar/Unspecified）。
pub fn IsString(tp: u8) -> bool {
    IsTypeChar(tp) || IsTypeBlob(tp) || IsTypeVarchar(tp) || IsTypeUnspecified(tp)
}
/// 判断 Datum Kind 是否为字符串或字节。
pub fn IsStringKind(kind: u8) -> bool {
    kind == KindString || kind == KindBytes
}

/// 将 Datum Kind 映射为可读名称字符串。
pub fn KindStr(kind: u8) -> String {
    match kind {
        KindNull => "null",
        KindInt64 => "bigint",
        KindUint64 => "unsigned bigint",
        KindFloat32 => "float",
        KindFloat64 => "double",
        KindString => "char",
        KindBytes => "bytes",
        KindBinaryLiteral => "bit/hex literal",
        KindMysqlDecimal => "decimal",
        KindMysqlDuration => "time",
        KindMysqlEnum => "enum",
        KindMysqlBit => "bit",
        KindMysqlSet => "set",
        KindMysqlTime => "datetime",
        KindInterface => "interface",
        KindMinNotNull => "min_not_null",
        KindMaxValue => "max_value",
        KindRaw => "raw",
        KindMysqlJSON => "json",
        KindVectorFloat32 => "vector",
        _ => "",
    }
    .to_owned()
}

/// 将 MySQL 类型码映射为类型名。
pub fn TypeStr(tp: u8) -> &'static str {
    ast::TypeStr(tp)
}
/// 结合 charset 将类型码映射为显示名（如 blob→text）。
pub fn TypeToStr(tp: u8, charset: &str) -> String {
    ast::TypeToStr(tp, charset)
}

/// 将 UnexpectedEof 归一为 None，其它错误原样透传。
pub fn EOFAsNil(err: Option<errors::SharedError>) -> Option<errors::SharedError> {
    let is_eof = errors::Cause(err.as_ref())
        .and_then(|cause| {
            cause
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind)
        })
        .is_some_and(|kind| kind == std::io::ErrorKind::UnexpectedEof);
    if is_eof { None } else { errors::Trace(err) }
}

/// 构造二元运算类型不匹配的错误结果。
pub fn InvOp2<X, Y>(
    x: &X,
    y: &Y,
    o: opcode::Op,
) -> Result<Option<Box<dyn std::any::Any>>, errors::SharedError>
where
    X: std::fmt::Debug + 'static,
    Y: std::fmt::Debug + 'static,
{
    Err(errors::New(format!(
        "Invalid operation: {x:?} {} {y:?} (mismatched types {} and {})",
        o.String(),
        std::any::type_name::<X>(),
        std::any::type_name::<Y>()
    )))
}

/// 生成常量溢出指定类型的 ErrOverflow。
pub fn overflow(v: &dyn std::fmt::Debug, tp: u8) -> errors::SharedError {
    let args = [
        errors::ErrorArg::debug(v),
        errors::ErrorArg::from(TypeStr(tp)),
    ];
    (**ErrOverflow).GenWithStack("constant %v overflows %s", &args)
}

/// 判断是否为时间/日期类类型（含 Duration 与 NewDate）。
pub fn IsTypeTemporal(tp: u8) -> bool {
    matches!(
        tp,
        mysql::TypeDuration
            | mysql::TypeDatetime
            | mysql::TypeTimestamp
            | mysql::TypeDate
            | mysql::TypeNewDate
    )
}
