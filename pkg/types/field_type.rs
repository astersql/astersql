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

// MySQL 字段类型（FieldType）构造、聚合与 DDL 兼容性检查。
//
// 对齐 Go `types` 包：按值推断类型、多参数类型/EvalType 合并、
// 类型合并表，以及 `ALTER COLUMN` 修改时是否需要 reorg（数据重组）的判定。

use std::any::Any;

/// 未指定长度哨兵值（-1），对齐 Go UnspecifiedLength。
// UnspecifiedLength/ErrorLength 保留 Go 中 FieldType 长度哨兵值。
pub const UnspecifiedLength: isize = -1;
/// 错误长度哨兵值（0）。
pub const ErrorLength: isize = 0;

/// 字段类型别名，实际定义来自 parser/types.FieldType。
// FieldType 对应 Go 的 type alias：实际字段和方法来自 parser/types.FieldType。
pub type FieldType = ast::FieldType;

/// 按 MySQL 类型构造 FieldType，填充默认 charset/collation 与最小 flen/decimal。
// NewFieldType 对应 Go 的构造函数：按 MySQL 类型取默认 charset/collation 和最小 flen/decimal。
pub fn NewFieldType(tp: u8) -> Box<FieldType> {
    let (charset1, collate1) = DefaultCharsetForType(tp);
    let (flen, decimal) = minFlenAndDecimalForType(tp);
    let mut field_type = FieldType::default();
    field_type.SetType(tp);
    field_type.SetCharset(charset1);
    field_type.SetCollate(collate1);
    field_type.SetFlen(flen);
    field_type.SetDecimal(decimal);
    Box::new(field_type)
}

/// 带指定 collation 与长度构造 FieldType。
// NewFieldTypeWithCollation 对应 Go 的带 collation 构造函数；GetCollationByName 的错误仍按 Go 忽略。
pub fn NewFieldTypeWithCollation(tp: u8, collation: String, length: isize) -> Box<FieldType> {
    let coll = charset::GetCollationByName(&collation)
        .expect("collation must exist when constructing FieldType");
    let mut field_type = FieldType::default();
    field_type.SetType(tp);
    field_type.SetFlen(length);
    field_type.SetCharset(coll.CharsetName);
    field_type.SetCollate(collation);
    field_type.SetDecimal(UnspecifiedLength);
    Box::new(field_type)
}

/// 聚合多个 FieldType（如 IF/IFNULL/COALESCE），含混合符号整数范围提升。
// AggFieldType 对应 Go 的多参数返回类型聚合，主要服务 IF/IFNULL/COALESCE 等表达式。
pub fn AggFieldType(tps: &[&FieldType]) -> Box<FieldType> {
    let mut currType = FieldType::default();
    let mut isMixedSign = false;
    for (i, t) in tps.iter().enumerate() {
        if i == 0 && currType.GetType() == mysql::TypeUnspecified {
            currType = (*t).clone();
            continue;
        }
        let mtp = mergeFieldType(currType.GetType(), t.GetType());
        isMixedSign = isMixedSign
            || (mysql::HasUnsignedFlag(currType.GetFlag()) != mysql::HasUnsignedFlag(t.GetFlag()));
        currType.SetType(mtp);
        currType.SetFlag(mergeTypeFlag(currType.GetFlag(), t.GetFlag()));
    }

    // Go 在有符号/无符号混合且当前是整数时，按是否需要扩大范围做逐级提升。
    if isMixedSign && IsTypeInteger(currType.GetType()) {
        let mut bumpRange = false;
        for t in tps {
            bumpRange = bumpRange
                || (mysql::HasUnsignedFlag(t.GetFlag())
                    && (t.GetType() == currType.GetType() || t.GetType() == mysql::TypeBit));
        }
        if bumpRange {
            match currType.GetType() {
                mysql::TypeTiny => currType.SetType(mysql::TypeShort),
                mysql::TypeShort => currType.SetType(mysql::TypeInt24),
                mysql::TypeInt24 => currType.SetType(mysql::TypeLong),
                mysql::TypeLong => currType.SetType(mysql::TypeLonglong),
                mysql::TypeLonglong => currType.SetType(mysql::TypeNewDecimal),
                _ => {}
            }
        }
    }

    if mysql::HasUnsignedFlag(currType.GetFlag()) && !isMixedSign {
        currType.AddFlag(mysql::UnsignedFlag);
    }

    Box::new(currType)
}

/// 按 FSP 补正 DATETIME 的显示宽度 flen。
// TryToFixFlenOfDatetime 对应 Go 对 datetime flen 的补正逻辑。
pub fn TryToFixFlenOfDatetime(resultTp: &mut FieldType) {
    if resultTp.GetType() == mysql::TypeDatetime {
        resultTp.SetFlen(mysql::MaxDatetimeWidthNoFsp as isize);
        if resultTp.GetDecimal() > 0 {
            resultTp.SetFlen(resultTp.GetFlen() + resultTp.GetDecimal() + 1);
        }
    }
}

/// 聚合多个字段的 EvalType，并同步更新 unsigned/binary flag。
// AggregateEvalType 对应 Go 的 EvalType 聚合，并同步更新 unsigned/binary flag。
pub fn AggregateEvalType(fts: &[&FieldType], flag: &mut usize) -> EvalType {
    let mut aggregatedEvalType = ETString;
    let mut unsigned = false;
    let mut gotFirst = false;
    let mut gotBinString = false;
    let mut lft = fts[0];

    for ft in fts {
        if ft.GetType() == mysql::TypeNull {
            continue;
        }
        let et = ft.EvalType();
        let rft = *ft;
        if (IsTypeBlob(ft.GetType()) || IsTypeVarchar(ft.GetType()) || IsTypeChar(ft.GetType()))
            && mysql::HasBinaryFlag(ft.GetFlag())
        {
            gotBinString = true;
        }
        if !gotFirst {
            gotFirst = true;
            aggregatedEvalType = et;
            unsigned = mysql::HasUnsignedFlag(ft.GetFlag());
        } else {
            aggregatedEvalType = mergeEvalType(
                aggregatedEvalType,
                et,
                lft,
                rft,
                unsigned,
                mysql::HasUnsignedFlag(ft.GetFlag()),
            );
            unsigned = unsigned && mysql::HasUnsignedFlag(ft.GetFlag());
        }
        lft = rft;
    }
    SetTypeFlag(flag, mysql::UnsignedFlag, unsigned);
    SetTypeFlag(
        flag,
        mysql::BinaryFlag,
        !aggregatedEvalType.IsStringKind() || gotBinString,
    );
    aggregatedEvalType
}

/// 合并两个 EvalType：字符串优先，其次 Real/Decimal，混合符号升为 Decimal。
fn mergeEvalType(
    lhs: EvalType,
    rhs: EvalType,
    lft: &FieldType,
    rft: &FieldType,
    isLHSUnsigned: bool,
    isRHSUnsigned: bool,
) -> EvalType {
    let mut lhs = lhs;
    let mut rhs = rhs;
    if lft.GetType() == mysql::TypeUnspecified || rft.GetType() == mysql::TypeUnspecified {
        if lft.GetType() == rft.GetType() {
            return ETString;
        }
        if lft.GetType() == mysql::TypeUnspecified {
            lhs = rhs;
        } else {
            rhs = lhs;
        }
    }
    if lhs.IsStringKind() || rhs.IsStringKind() {
        ETString
    } else if lhs == ETReal || rhs == ETReal {
        ETReal
    } else if lhs == ETDecimal || rhs == ETDecimal || isLHSUnsigned != isRHSUnsigned {
        ETDecimal
    } else {
        ETInt
    }
}

/// 按开关置位或清除 flag 中的某一 flagItem。
// SetTypeFlag 对应 Go 的位开关函数，flagItem 为 true 时置位，否则清位。
pub fn SetTypeFlag(flag: &mut usize, flagItem: usize, on: bool) {
    if on {
        *flag |= flagItem;
    } else {
        *flag &= !flagItem;
    }
}

/// 从 Datum 推断预处理参数类型，并补全字符串 collation。
// InferParamTypeFromDatum 对应 Go 的 Datum 参数类型推断，并在字符串类 Datum 上补 charset/collation。
pub fn InferParamTypeFromDatum(d: &Datum, tp: &mut FieldType) {
    InferParamTypeFromUnderlyingValue(d.GetValue(), tp);
    if IsStringKind(d.k) {
        let Ok(c) = collate::GetCollationByName(&d.collation) else {
            return;
        };
        tp.SetCharset(c.CharsetName);
        tp.SetCollate(d.collation.clone());
    }
}

/// 从底层 any 值推断参数类型；None 对应 Go nil→TypeNull。
// InferParamTypeFromUnderlyingValue 对应 Go 的 any 类型分支；None 表示 Go nil 参数。
pub fn InferParamTypeFromUnderlyingValue(value: Option<&dyn Any>, tp: &mut FieldType) {
    if value.is_none() {
        // NULL 参数必须推断成 TypeNull，避免 CASE WHEN 等控制流函数复用 VarString 造成行为漂移。
        tp.SetType(mysql::TypeNull);
        tp.SetFlen(0);
        tp.SetDecimal(0);
        tp.SetCharset(mysql::DefaultCharset.to_owned());
        tp.SetCollate(mysql::DefaultCollationName.to_owned());
        return;
    }

    DefaultTypeForValue(
        value,
        tp,
        mysql::DefaultCharset,
        mysql::DefaultCollationName,
    );
    if hasVariantFieldLength(tp) {
        tp.SetFlen(UnspecifiedLength);
    }
    if tp.GetType() == mysql::TypeUnspecified {
        tp.SetType(mysql::TypeVarString);
    }
}

/// 判断该类型的 flen 是否可变（推断后常置为 UnspecifiedLength）。
fn hasVariantFieldLength(tp: &FieldType) -> bool {
    match tp.GetType() {
        mysql::TypeLonglong
        | mysql::TypeVarString
        | mysql::TypeDouble
        | mysql::TypeBlob
        | mysql::TypeBit
        | mysql::TypeDuration
        | mysql::TypeEnum
        | mysql::TypeSet => true,
        _ => false,
    }
}

/// 按运行时值的具体类型填充默认 FieldType（对齐 Go type switch）。
// DefaultTypeForValue 对应 Go 的 type switch；用 Any downcast 保留各具体类型分支。
pub fn DefaultTypeForValue(
    value: Option<&dyn Any>,
    tp: &mut FieldType,
    char_: &str,
    collate_: &str,
) {
    if value.is_some() {
        tp.AddFlag(mysql::NotNullFlag);
    }

    let Some(value) = value else {
        tp.SetType(mysql::TypeNull);
        tp.SetFlen(0);
        tp.SetDecimal(0);
        SetBinChsClnFlag(tp);
        return;
    };

    if value.is::<bool>() {
        tp.SetType(mysql::TypeLonglong);
        tp.SetFlen(1);
        tp.SetDecimal(0);
        tp.AddFlag(mysql::IsBooleanFlag);
        SetBinChsClnFlag(tp);
    } else if let Some(x) = value.downcast_ref::<i32>() {
        tp.SetType(mysql::TypeLonglong);
        tp.SetFlen(mathutil::StrLenOfInt64Fast(*x as i64) as isize);
        tp.SetDecimal(0);
        SetBinChsClnFlag(tp);
    } else if let Some(x) = value.downcast_ref::<i64>() {
        tp.SetType(mysql::TypeLonglong);
        tp.SetFlen(mathutil::StrLenOfInt64Fast(*x) as isize);
        tp.SetDecimal(0);
        SetBinChsClnFlag(tp);
    } else if let Some(x) = value.downcast_ref::<u64>() {
        tp.SetType(mysql::TypeLonglong);
        tp.AddFlag(mysql::UnsignedFlag);
        tp.SetFlen(mathutil::StrLenOfUint64Fast(*x) as isize);
        tp.SetDecimal(0);
        SetBinChsClnFlag(tp);
    } else if let Some(x) = value.downcast_ref::<String>() {
        tp.SetType(mysql::TypeVarString);
        // Go 这里 TODO：flen 应为 len(x) * 3；保留当前 len(x) 行为。
        tp.SetFlen(x.len() as isize);
        tp.SetDecimal(UnspecifiedLength);
        tp.SetCharset(char_.to_string());
        tp.SetCollate(collate_.to_string());
    } else if let Some(x) = value.downcast_ref::<f32>() {
        tp.SetType(mysql::TypeFloat);
        let s = format!("{}", x);
        tp.SetFlen(s.len() as isize);
        tp.SetDecimal(UnspecifiedLength);
        SetBinChsClnFlag(tp);
    } else if let Some(x) = value.downcast_ref::<f64>() {
        tp.SetType(mysql::TypeDouble);
        let s = format!("{}", x);
        tp.SetFlen(s.len() as isize);
        tp.SetDecimal(UnspecifiedLength);
        SetBinChsClnFlag(tp);
    } else if let Some(x) = value.downcast_ref::<Vec<u8>>() {
        tp.SetType(mysql::TypeBlob);
        tp.SetFlen(x.len() as isize);
        tp.SetDecimal(UnspecifiedLength);
        SetBinChsClnFlag(tp);
    } else if let Some(x) = value.downcast_ref::<BitLiteral>() {
        tp.SetType(mysql::TypeVarString);
        tp.SetFlen(x.0.0.len() as isize * 3);
        tp.SetDecimal(0);
        SetBinChsClnFlag(tp);
    } else if let Some(x) = value.downcast_ref::<HexLiteral>() {
        tp.SetType(mysql::TypeVarString);
        tp.SetFlen(x.0.0.len() as isize * 3);
        tp.SetDecimal(0);
        tp.AddFlag(mysql::UnsignedFlag);
        SetBinChsClnFlag(tp);
    } else if let Some(x) = value.downcast_ref::<BinaryLiteral>() {
        tp.SetType(mysql::TypeVarString);
        tp.SetFlen(x.0.len() as isize);
        tp.SetDecimal(0);
        SetBinChsClnFlag(tp);
        tp.DelFlag(mysql::BinaryFlag);
        tp.AddFlag(mysql::UnsignedFlag);
    } else if let Some(x) = value.downcast_ref::<Time>() {
        tp.SetType(x.Type());
        match x.Type() {
            mysql::TypeDate => {
                tp.SetFlen(mysql::MaxDateWidth as isize);
                tp.SetDecimal(UnspecifiedLength);
            }
            mysql::TypeDatetime | mysql::TypeTimestamp => {
                tp.SetFlen(mysql::MaxDatetimeWidthNoFsp as isize);
                if x.Fsp() > DefaultFsp {
                    tp.SetFlen(tp.GetFlen() + x.Fsp() as isize + 1);
                }
                tp.SetDecimal(x.Fsp() as isize);
            }
            _ => {}
        }
        SetBinChsClnFlag(tp);
    } else if let Some(x) = value.downcast_ref::<Duration>() {
        tp.SetType(mysql::TypeDuration);
        tp.SetFlen(x.String().len() as isize);
        if x.Fsp > DefaultFsp {
            tp.SetFlen(x.Fsp as isize + 1);
        }
        tp.SetDecimal(x.Fsp as isize);
        SetBinChsClnFlag(tp);
    } else if let Some(x) = value.downcast_ref::<MyDecimal>() {
        tp.SetType(mysql::TypeNewDecimal);
        tp.SetFlenUnderLimit(x.ToString().len() as isize);
        let mut decimal = x.clone();
        tp.SetDecimalUnderLimit(decimal.GetDigitsFrac() as isize);
        tp.SetFlenUnderLimit(tp.GetFlen() + 1);
        SetBinChsClnFlag(tp);
    } else if let Some(x) = value.downcast_ref::<Enum>() {
        tp.SetType(mysql::TypeEnum);
        tp.SetFlen(x.Name.len() as isize);
        tp.SetDecimal(UnspecifiedLength);
        SetBinChsClnFlag(tp);
    } else if let Some(x) = value.downcast_ref::<Set>() {
        tp.SetType(mysql::TypeSet);
        tp.SetFlen(x.Name.len() as isize);
        tp.SetDecimal(UnspecifiedLength);
        SetBinChsClnFlag(tp);
    } else if value.is::<BinaryJSON>() {
        tp.SetType(mysql::TypeJSON);
        tp.SetFlen(UnspecifiedLength);
        tp.SetDecimal(0);
        tp.SetCharset(charset::CharsetUTF8MB4.to_owned());
        tp.SetCollate(charset::CollationUTF8MB4.to_owned());
    } else if value.is::<VectorFloat32>() {
        tp.SetType(mysql::TypeTiDBVectorFloat32);
        tp.SetFlen(UnspecifiedLength);
        tp.SetDecimal(0);
        SetBinChsClnFlag(tp);
    } else {
        tp.SetType(mysql::TypeUnspecified);
        tp.SetFlen(UnspecifiedLength);
        tp.SetDecimal(UnspecifiedLength);
        tp.SetCharset(charset::CharsetUTF8MB4.to_owned());
        tp.SetCollate(charset::CollationUTF8MB4.to_owned());
    }
}

/// 查询类型最小 flen/decimal；整数与 YEAR 走 mysql 默认值。
// minFlenAndDecimalForType 对应 Go 的最小 flen/decimal 查询，目前只对整数和 year 走 mysql 默认值。
fn minFlenAndDecimalForType(tp: u8) -> (isize, isize) {
    match tp {
        mysql::TypeTiny
        | mysql::TypeShort
        | mysql::TypeInt24
        | mysql::TypeLong
        | mysql::TypeLonglong
        | mysql::TypeYear => mysql::GetDefaultFieldLengthAndDecimal(tp),
        _ => (UnspecifiedLength, UnspecifiedLength),
    }
}

/// 返回类型默认 charset/collation；字符串为 utf8mb4，其余 binary。
// DefaultCharsetForType 对应 Go 的类型默认 charset/collation；字符串类型默认 utf8mb4，其余 binary。
pub fn DefaultCharsetForType(tp: u8) -> (String, String) {
    match tp {
        mysql::TypeVarString | mysql::TypeString | mysql::TypeVarchar => (
            mysql::DefaultCharset.to_string(),
            mysql::DefaultCollationName.to_string(),
        ),
        _ => (
            charset::CharsetBin.to_string(),
            charset::CollationBin.to_string(),
        ),
    }
}

/// 查表合并两个 MySQL 类型码。
// mergeFieldType 对应 Go 的二维表查找；范围提升由 AggFieldType 额外处理。
fn mergeFieldType(a: u8, b: u8) -> u8 {
    let ia = getFieldTypeIndex(a);
    let ib = getFieldTypeIndex(b);
    fieldTypeMergeRules[ia][ib]
}

/// 合并 NotNull/Unsigned 等类型 flag。
// mergeTypeFlag 对应 Go 的 NotNullFlag/UnsignedFlag 合并规则，其它 flag 暂按原位运算保留。
fn mergeTypeFlag(a: usize, b: usize) -> usize {
    a & ((b & mysql::NotNullFlag) | !mysql::NotNullFlag)
        & ((b & mysql::UnsignedFlag) | !mysql::UnsignedFlag)
}

/// 将 MySQL 类型码映射为 fieldTypeMergeRules 行/列下标。
// getFieldTypeIndex 对应 Go 的 fieldTypeIndexes map；match 形式避免在这里引入全局 HashMap 初始化。
fn getFieldTypeIndex(tp: u8) -> usize {
    match tp {
        mysql::TypeUnspecified => 0,
        mysql::TypeTiny => 1,
        mysql::TypeShort => 2,
        mysql::TypeLong => 3,
        mysql::TypeFloat => 4,
        mysql::TypeDouble => 5,
        mysql::TypeNull => 6,
        mysql::TypeTimestamp => 7,
        mysql::TypeLonglong => 8,
        mysql::TypeInt24 => 9,
        mysql::TypeDate => 10,
        mysql::TypeDuration => 11,
        mysql::TypeDatetime => 12,
        mysql::TypeYear => 13,
        mysql::TypeNewDate => 14,
        mysql::TypeVarchar => 15,
        mysql::TypeBit => 16,
        mysql::TypeJSON => 17,
        mysql::TypeNewDecimal => 18,
        mysql::TypeEnum => 19,
        mysql::TypeSet => 20,
        mysql::TypeTinyBlob => 21,
        mysql::TypeMediumBlob => 22,
        mysql::TypeLongBlob => 23,
        mysql::TypeBlob => 24,
        mysql::TypeVarString => 25,
        mysql::TypeString => 26,
        mysql::TypeGeometry => 27,
        mysql::TypeTiDBVectorFloat32 => 28,
        _ => 0,
    }
}

/// 29×29 MySQL 类型合并表，下标与 getFieldTypeIndex 对齐。
// fieldTypeMergeRules: 29x29 MySQL type merge table aligned with getFieldTypeIndex.
pub static fieldTypeMergeRules: [[u8; 29]; 29] = [
    /* mysql::TypeUnspecified -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeNewDecimal,
        mysql::TypeNewDecimal,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeNewDecimal,
        mysql::TypeNewDecimal,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeDouble,
        mysql::TypeDouble,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeNewDecimal,
        mysql::TypeVarchar,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeUnspecified,
        mysql::TypeUnspecified,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeVarchar,
        // mysql::TypeJSON
        mysql::TypeVarchar,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeNewDecimal,
        mysql::TypeVarchar,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeVarchar,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeVarchar,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeString,
        mysql::TypeVarchar,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeVarchar,
    ],
    /* mysql::TypeTiny -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeNewDecimal,
        mysql::TypeTiny,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeShort,
        mysql::TypeLong,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeFloat,
        mysql::TypeDouble,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeTiny,
        mysql::TypeVarchar,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeLonglong,
        mysql::TypeInt24,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeVarchar,
        mysql::TypeTiny,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeLonglong,
        // mysql::TypeJSON
        mysql::TypeVarchar,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeNewDecimal,
        mysql::TypeVarchar,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeVarchar,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeVarchar,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeString,
        mysql::TypeVarchar,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeVarchar,
    ],
    /* mysql::TypeShort -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeNewDecimal,
        mysql::TypeShort,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeShort,
        mysql::TypeLong,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeFloat,
        mysql::TypeDouble,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeShort,
        mysql::TypeVarchar,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeLonglong,
        mysql::TypeInt24,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeVarchar,
        mysql::TypeShort,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeLonglong,
        // mysql::TypeJSON
        mysql::TypeVarchar,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeNewDecimal,
        mysql::TypeVarchar,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeVarchar,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeVarchar,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeString,
        mysql::TypeVarchar,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeVarchar,
    ],
    /* mysql::TypeLong -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeNewDecimal,
        mysql::TypeLong,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeLong,
        mysql::TypeLong,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeDouble,
        mysql::TypeDouble,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeLong,
        mysql::TypeVarchar,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeLonglong,
        mysql::TypeLong,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeVarchar,
        mysql::TypeLong,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeLonglong,
        // mysql::TypeJSON
        mysql::TypeVarchar,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeNewDecimal,
        mysql::TypeVarchar,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeVarchar,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeVarchar,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeString,
        mysql::TypeVarchar,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeVarchar,
    ],
    /* mysql::TypeFloat -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeDouble,
        mysql::TypeFloat,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeFloat,
        mysql::TypeDouble,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeFloat,
        mysql::TypeDouble,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeFloat,
        mysql::TypeVarchar,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeFloat,
        mysql::TypeFloat,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeVarchar,
        mysql::TypeFloat,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeDouble,
        // mysql::TypeJSON
        mysql::TypeVarchar,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeDouble,
        mysql::TypeVarchar,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeVarchar,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeVarchar,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeString,
        mysql::TypeVarchar,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeVarchar,
    ],
    /* mysql::TypeDouble -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeDouble,
        mysql::TypeDouble,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeDouble,
        mysql::TypeDouble,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeDouble,
        mysql::TypeDouble,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeDouble,
        mysql::TypeVarchar,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeDouble,
        mysql::TypeDouble,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeVarchar,
        mysql::TypeDouble,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeDouble,
        // mysql::TypeJSON
        mysql::TypeVarchar,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeDouble,
        mysql::TypeVarchar,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeVarchar,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeVarchar,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeString,
        mysql::TypeVarchar,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeVarchar,
    ],
    /* mysql::TypeNull -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeNewDecimal,
        mysql::TypeTiny,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeShort,
        mysql::TypeLong,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeFloat,
        mysql::TypeDouble,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeNull,
        mysql::TypeTimestamp,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeLonglong,
        mysql::TypeLonglong,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeDate,
        mysql::TypeDuration,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeDatetime,
        mysql::TypeYear,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeNewDate,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeBit,
        // mysql::TypeJSON
        mysql::TypeJSON,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeNewDecimal,
        mysql::TypeEnum,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeSet,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeVarchar,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeString,
        mysql::TypeGeometry,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeTiDBVectorFloat32,
    ],
    /* mysql::TypeTimestamp -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeTimestamp,
        mysql::TypeTimestamp,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeDatetime,
        mysql::TypeDatetime,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeDatetime,
        mysql::TypeVarchar,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeNewDate,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeVarchar,
        // mysql::TypeJSON
        mysql::TypeVarchar,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeVarchar,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeVarchar,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeString,
        mysql::TypeVarchar,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeVarchar,
    ],
    /* mysql::TypeLonglong -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeNewDecimal,
        mysql::TypeLonglong,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeLonglong,
        mysql::TypeLonglong,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeDouble,
        mysql::TypeDouble,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeLonglong,
        mysql::TypeVarchar,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeLonglong,
        mysql::TypeLong,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeVarchar,
        mysql::TypeLonglong,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeNewDate,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeLonglong,
        // mysql::TypeJSON
        mysql::TypeVarchar,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeNewDecimal,
        mysql::TypeVarchar,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeVarchar,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeVarchar,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeString,
        mysql::TypeVarchar,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeVarchar,
    ],
    /* mysql::TypeInt24 -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeNewDecimal,
        mysql::TypeInt24,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeInt24,
        mysql::TypeLong,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeFloat,
        mysql::TypeDouble,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeInt24,
        mysql::TypeVarchar,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeLonglong,
        mysql::TypeInt24,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeVarchar,
        mysql::TypeInt24,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeNewDate,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeLonglong,
        // mysql::TypeJSON
        mysql::TypeVarchar,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeNewDecimal,
        mysql::TypeVarchar,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeVarchar,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeVarchar,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeString,
        mysql::TypeVarchar,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeVarchar,
    ],
    /* mysql::TypeDate -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeDate,
        mysql::TypeDatetime,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeDate,
        mysql::TypeDatetime,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeDatetime,
        mysql::TypeVarchar,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeNewDate,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeVarchar,
        // mysql::TypeJSON
        mysql::TypeVarchar,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeVarchar,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeVarchar,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeString,
        mysql::TypeVarchar,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeVarchar,
    ],
    /* mysql::TypeTime -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeDuration,
        mysql::TypeDatetime,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeDatetime,
        mysql::TypeDuration,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeDatetime,
        mysql::TypeVarchar,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeNewDate,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeVarchar,
        // mysql::TypeJSON
        mysql::TypeVarchar,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeVarchar,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeVarchar,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeString,
        mysql::TypeVarchar,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeVarchar,
    ],
    /* mysql::TypeDatetime -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeDatetime,
        mysql::TypeDatetime,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeDatetime,
        mysql::TypeDatetime,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeDatetime,
        mysql::TypeVarchar,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeNewDate,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeVarchar,
        // mysql::TypeJSON
        mysql::TypeVarchar,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeVarchar,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeVarchar,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeString,
        mysql::TypeVarchar,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeVarchar,
    ],
    /* mysql::TypeYear -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeUnspecified,
        mysql::TypeTiny,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeShort,
        mysql::TypeLong,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeFloat,
        mysql::TypeDouble,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeYear,
        mysql::TypeVarchar,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeLonglong,
        mysql::TypeInt24,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeVarchar,
        mysql::TypeYear,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeLonglong,
        // mysql::TypeJSON
        mysql::TypeVarchar,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeNewDecimal,
        mysql::TypeVarchar,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeVarchar,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeVarchar,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeString,
        mysql::TypeVarchar,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeVarchar,
    ],
    /* mysql::TypeNewDate -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeNewDate,
        mysql::TypeDatetime,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeNewDate,
        mysql::TypeDatetime,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeDatetime,
        mysql::TypeVarchar,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeNewDate,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeVarchar,
        // mysql::TypeJSON
        mysql::TypeVarchar,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeVarchar,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeVarchar,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeString,
        mysql::TypeVarchar,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeVarchar,
    ],
    /* mysql::TypeVarchar -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeVarchar,
        // mysql::TypeJSON
        mysql::TypeVarchar,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeVarchar,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeVarchar,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeVarchar,
    ],
    /* mysql::TypeBit -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeVarchar,
        mysql::TypeLonglong,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeLonglong,
        mysql::TypeLonglong,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeDouble,
        mysql::TypeDouble,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeBit,
        mysql::TypeVarchar,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeLonglong,
        mysql::TypeLonglong,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeVarchar,
        mysql::TypeLonglong,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeBit,
        // mysql::TypeJSON
        mysql::TypeVarchar,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeNewDecimal,
        mysql::TypeVarchar,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeVarchar,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeVarchar,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeString,
        mysql::TypeVarchar,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeVarchar,
    ],
    /* mysql::TypeJSON -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeNewFloat mysql::TypeDouble
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeJSON,
        mysql::TypeVarchar,
        // mysql::TypeLongLONG mysql::TypeInt24
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDate MYSQL_TYPE_TIME
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDatetime MYSQL_TYPE_YEAR
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeVarchar,
        // mysql::TypeJSON
        mysql::TypeJSON,
        // mysql::TypeNewDecimal MYSQL_TYPE_ENUM
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeVarchar,
        mysql::TypeLongBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeLongBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeLongBlob,
        mysql::TypeVarchar,
        // mysql::TypeString MYSQL_TYPE_GEOMETRY
        mysql::TypeString,
        mysql::TypeVarchar,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeVarchar,
    ],
    /* mysql::TypeNewDecimal -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeNewDecimal,
        mysql::TypeNewDecimal,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeNewDecimal,
        mysql::TypeNewDecimal,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeDouble,
        mysql::TypeDouble,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeNewDecimal,
        mysql::TypeVarchar,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeNewDecimal,
        mysql::TypeNewDecimal,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeVarchar,
        mysql::TypeNewDecimal,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeNewDecimal,
        // mysql::TypeJSON
        mysql::TypeVarchar,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeNewDecimal,
        mysql::TypeVarchar,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeVarchar,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeVarchar,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeString,
        mysql::TypeVarchar,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeVarchar,
    ],
    /* mysql::TypeEnum -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeEnum,
        mysql::TypeVarchar,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeVarchar,
        // mysql::TypeJSON
        mysql::TypeVarchar,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeVarchar,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeVarchar,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeString,
        mysql::TypeVarchar,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeVarchar,
    ],
    /* mysql::TypeSet -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeSet,
        mysql::TypeVarchar,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeVarchar,
        // mysql::TypeJSON
        mysql::TypeVarchar,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeVarchar,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeVarchar,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeString,
        mysql::TypeVarchar,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeVarchar,
    ],
    /* mysql::TypeTinyBlob -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeTinyBlob,
        mysql::TypeTinyBlob,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeTinyBlob,
        mysql::TypeTinyBlob,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeTinyBlob,
        mysql::TypeTinyBlob,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeTinyBlob,
        mysql::TypeTinyBlob,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeTinyBlob,
        mysql::TypeTinyBlob,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeTinyBlob,
        mysql::TypeTinyBlob,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeTinyBlob,
        mysql::TypeTinyBlob,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeTinyBlob,
        mysql::TypeTinyBlob,
        // mysql::TypeBit <16>-<244>
        mysql::TypeTinyBlob,
        // mysql::TypeJSON
        mysql::TypeLongBlob,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeTinyBlob,
        mysql::TypeTinyBlob,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeTinyBlob,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeTinyBlob,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeTinyBlob,
        mysql::TypeTinyBlob,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeLongBlob,
    ],
    /* mysql::TypeMediumBlob -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeMediumBlob,
        mysql::TypeMediumBlob,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeMediumBlob,
        mysql::TypeMediumBlob,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeMediumBlob,
        mysql::TypeMediumBlob,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeMediumBlob,
        mysql::TypeMediumBlob,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeMediumBlob,
        mysql::TypeMediumBlob,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeMediumBlob,
        mysql::TypeMediumBlob,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeMediumBlob,
        mysql::TypeMediumBlob,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeMediumBlob,
        mysql::TypeMediumBlob,
        // mysql::TypeBit <16>-<244>
        mysql::TypeMediumBlob,
        // mysql::TypeJSON
        mysql::TypeLongBlob,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeMediumBlob,
        mysql::TypeMediumBlob,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeMediumBlob,
        mysql::TypeMediumBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeMediumBlob,
        mysql::TypeMediumBlob,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeMediumBlob,
        mysql::TypeMediumBlob,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeLongBlob,
    ],
    /* mysql::TypeLongBlob -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeLongBlob,
        mysql::TypeLongBlob,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeLongBlob,
        mysql::TypeLongBlob,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeLongBlob,
        mysql::TypeLongBlob,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeLongBlob,
        mysql::TypeLongBlob,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeLongBlob,
        mysql::TypeLongBlob,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeLongBlob,
        mysql::TypeLongBlob,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeLongBlob,
        mysql::TypeLongBlob,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeLongBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBit <16>-<244>
        mysql::TypeLongBlob,
        // mysql::TypeJSON
        mysql::TypeLongBlob,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeLongBlob,
        mysql::TypeLongBlob,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeLongBlob,
        mysql::TypeLongBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeLongBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeLongBlob,
        mysql::TypeLongBlob,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeLongBlob,
        mysql::TypeLongBlob,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeLongBlob,
    ],
    /* mysql::TypeBlob -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeBlob,
        mysql::TypeBlob,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeBlob,
        mysql::TypeBlob,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeBlob,
        mysql::TypeBlob,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeBlob,
        mysql::TypeBlob,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeBlob,
        mysql::TypeBlob,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeBlob,
        mysql::TypeBlob,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeBlob,
        mysql::TypeBlob,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeBlob,
        mysql::TypeBlob,
        // mysql::TypeBit <16>-<244>
        mysql::TypeBlob,
        // mysql::TypeJSON
        mysql::TypeLongBlob,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeBlob,
        mysql::TypeBlob,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeBlob,
        mysql::TypeBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeBlob,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeBlob,
        mysql::TypeBlob,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeLongBlob,
    ],
    /* mysql::TypeVarString -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeVarchar,
        // mysql::TypeJSON
        mysql::TypeVarchar,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeVarchar,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeVarchar,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeVarchar,
    ],
    /* mysql::TypeString -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeString,
        mysql::TypeString,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeString,
        mysql::TypeString,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeString,
        mysql::TypeString,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeString,
        mysql::TypeString,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeString,
        mysql::TypeString,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeString,
        mysql::TypeString,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeString,
        mysql::TypeString,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeString,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeString,
        // mysql::TypeJSON
        mysql::TypeString,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeString,
        mysql::TypeString,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeString,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeVarchar,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeString,
        mysql::TypeString,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeString,
    ],
    /* mysql::TypeGeometry -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeFloat mysql::TypeDouble
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeGeometry,
        mysql::TypeVarchar,
        // mysql::TypeLonglong mysql::TypeInt24
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDate mysql::TypeTime
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDatetime mysql::TypeYear
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeVarchar,
        // mysql::TypeJSON
        mysql::TypeVarchar,
        // mysql::TypeNewDecimal mysql::TypeEnum
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeVarchar,
        mysql::TypeTinyBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeMediumBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeBlob,
        mysql::TypeVarchar,
        // mysql::TypeString mysql::TypeGeometry
        mysql::TypeString,
        mysql::TypeGeometry,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeVarchar,
    ],
    /* mysql::TypeTiDBVectorFloat32 -> */
    [
        // mysql::TypeUnspecified mysql::TypeTiny
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeShort mysql::TypeLong
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeNewFloat mysql::TypeDouble
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeNull mysql::TypeTimestamp
        mysql::TypeTiDBVectorFloat32,
        mysql::TypeVarchar,
        // mysql::TypeLongLONG mysql::TypeInt24
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDate MYSQL_TYPE_TIME
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeDatetime MYSQL_TYPE_YEAR
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeNewDate mysql::TypeVarchar
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeBit <16>-<244>
        mysql::TypeVarchar,
        // mysql::TypeJSON
        mysql::TypeVarchar,
        // mysql::TypeNewDecimal MYSQL_TYPE_ENUM
        mysql::TypeVarchar,
        mysql::TypeVarchar,
        // mysql::TypeSet mysql::TypeTinyBlob
        mysql::TypeVarchar,
        mysql::TypeLongBlob,
        // mysql::TypeMediumBlob mysql::TypeLongBlob
        mysql::TypeLongBlob,
        mysql::TypeLongBlob,
        // mysql::TypeBlob mysql::TypeVarString
        mysql::TypeLongBlob,
        mysql::TypeVarchar,
        // mysql::TypeString MYSQL_TYPE_GEOMETRY
        mysql::TypeString,
        mysql::TypeVarchar,
        // mysql::TypeTiDBVectorFloat32
        mysql::TypeTiDBVectorFloat32,
    ],
];

/// 设置为 binary charset/collation 并加上 BinaryFlag。
// SetBinChsClnFlag 对应 Go：设置 binary charset/collation 并补 BinaryFlag。
pub fn SetBinChsClnFlag(ft: &mut FieldType) {
    ft.SetCharset(charset::CharsetBin.to_owned());
    ft.SetCollate(charset::CollationBin.to_owned());
    ft.AddFlag(mysql::BinaryFlag);
}

/// 可变长度列存储长度哨兵，转发自 parser/types。
// VarStorageLen 表示可变长度列，直接沿用 parser/types 的常量。
pub const VarStorageLen: isize = ast::VarStorageLen;

/// 检查 ALTER 修改列类型是否兼容；返回是否需 reorg 及错误。
// CheckModifyTypeCompatible 对应 Go 的列类型修改兼容性判断，返回是否需要 reorg 以及可选错误。
pub fn CheckModifyTypeCompatible(
    origin: &FieldType,
    to: &FieldType,
) -> (bool, Option<errors::SharedError>) {
    if origin.GetType() == to.GetType() {
        if origin.GetType() == mysql::TypeEnum || origin.GetType() == mysql::TypeSet {
            let typeVar = if origin.GetType() == mysql::TypeEnum {
                "enum"
            } else {
                "set"
            };
            if to.GetElems().len() < origin.GetElems().len() {
                let msg = format!(
                    "the number of {} column's elements is less than the original: {}",
                    typeVar,
                    origin.GetElems().len()
                );
                return (
                    true,
                    Some(dbterror::ErrUnsupportedModifyColumn.GenWithStackByArgs(&[msg.into()])),
                );
            }
            for (index, originElem) in origin.GetElems().iter().enumerate() {
                let toElem = &to.GetElems()[index];
                if originElem != toElem {
                    let msg = format!(
                        "cannot modify {} column value {} to {}",
                        typeVar, originElem, toElem
                    );
                    return (
                        true,
                        Some(
                            dbterror::ErrUnsupportedModifyColumn.GenWithStackByArgs(&[msg.into()]),
                        ),
                    );
                }
            }
        }

        if origin.GetType() == mysql::TypeNewDecimal {
            // Go 对 decimal 的 flen/decimal/unsigned 改动均判为需要 reorg 的不支持修改。
            if to.GetFlen() != origin.GetFlen()
                || to.GetDecimal() != origin.GetDecimal()
                || mysql::HasUnsignedFlag(to.GetFlag()) != mysql::HasUnsignedFlag(origin.GetFlag())
            {
                let msg = format!(
                    "decimal change from decimal({}, {}) to decimal({}, {})",
                    origin.GetFlen(),
                    origin.GetDecimal(),
                    to.GetFlen(),
                    to.GetDecimal()
                );
                return (
                    true,
                    Some(dbterror::ErrUnsupportedModifyColumn.GenWithStackByArgs(&[msg.into()])),
                );
            }
        }

        let (needReorg, reason) = needReorgToChange(origin, to);
        if !needReorg {
            return (false, None);
        }
        return (
            true,
            Some(dbterror::ErrUnsupportedModifyColumn.GenWithStackByArgs(&[reason.into()])),
        );
    }

    if !checkTypeChangeSupported(origin, to) {
        let unsupportedMsg = format!(
            "change from original type {:?} to {:?} is currently unsupported yet",
            origin.CompactStr(),
            to.CompactStr()
        );
        return (
            false,
            Some(dbterror::ErrUnsupportedModifyColumn.GenWithStackByArgs(&[unsupportedMsg.into()])),
        );
    }

    let stringToString = IsString(origin.GetType()) && IsString(to.GetType());
    let integerToInteger =
        mysql::IsIntegerType(origin.GetType()) && mysql::IsIntegerType(to.GetType());
    if stringToString || integerToInteger {
        let (needReorg, reason) = needReorgToChange(origin, to);
        if !needReorg {
            return (false, None);
        }
        return (
            true,
            Some(dbterror::ErrUnsupportedModifyColumn.GenWithStackByArgs(&[reason.into()])),
        );
    }

    let notCompatibleMsg = format!(
        "type {:?} not match origin {:?}",
        to.CompactStr(),
        origin.CompactStr()
    );
    (
        true,
        Some(dbterror::ErrUnsupportedModifyColumn.GenWithStackByArgs(&[notCompatibleMsg.into()])),
    )
}

/// 判断同族类型变更是否因长度/精度/符号变化而需要 reorg。
fn needReorgToChange(origin: &FieldType, to: &FieldType) -> (bool, String) {
    let mut toFlen = to.GetFlen();
    let mut originFlen = origin.GetFlen();
    if mysql::IsIntegerType(to.GetType()) && mysql::IsIntegerType(origin.GetType()) {
        // 整数类型忽略显示长度，改用类型默认 flen 与 decimal。
        let (origin_default, _) = mysql::GetDefaultFieldLengthAndDecimal(origin.GetType());
        let (to_default, _) = mysql::GetDefaultFieldLengthAndDecimal(to.GetType());
        originFlen = origin_default;
        toFlen = to_default;
    }

    if ConvertBetweenCharAndVarchar(origin.GetType(), to.GetType()) {
        return (
            true,
            "conversion between char and varchar string needs reorganization".to_string(),
        );
    }

    if toFlen > 0 && toFlen != originFlen {
        if toFlen < originFlen {
            return (
                true,
                format!("length {} is less than origin {}", toFlen, originFlen),
            );
        }

        // binary char 长度变化会影响 \x00 padding，Go 因此要求 reorg。
        let isBinaryType = |tp: &FieldType| tp.GetType() == mysql::TypeString && IsBinaryStr(tp);
        if isBinaryType(origin) && isBinaryType(to) {
            return (
                true,
                "can't change binary types of different length".to_string(),
            );
        }
    }
    if to.GetDecimal() > 0 && to.GetDecimal() < origin.GetDecimal() {
        return (
            true,
            format!(
                "decimal {} is less than origin {}",
                to.GetDecimal(),
                origin.GetDecimal()
            ),
        );
    }
    if mysql::HasUnsignedFlag(origin.GetFlag()) != mysql::HasUnsignedFlag(to.GetFlag()) {
        return (
            true,
            "can't change unsigned integer to signed or vice versa".to_string(),
        );
    }
    (false, String::new())
}

/// 判断跨类型变更当前是否支持（部分 cast 与 MySQL 不兼容则拒绝）。
fn checkTypeChangeSupported(origin: &FieldType, to: &FieldType) -> bool {
    if (IsTypeTime(origin.GetType())
        || origin.GetType() == mysql::TypeDuration
        || origin.GetType() == mysql::TypeYear
        || IsString(origin.GetType())
        || origin.GetType() == mysql::TypeJSON)
        && to.GetType() == mysql::TypeBit
    {
        // Go TODO：这些类型 cast 到 bit 当前与 MySQL 不兼容，保持不支持。
        return false;
    }

    if (IsTypeTime(origin.GetType())
        || origin.GetType() == mysql::TypeDuration
        || origin.GetType() == mysql::TypeYear
        || origin.GetType() == mysql::TypeNewDecimal
        || origin.GetType() == mysql::TypeFloat
        || origin.GetType() == mysql::TypeDouble
        || origin.GetType() == mysql::TypeJSON
        || origin.GetType() == mysql::TypeBit)
        && (to.GetType() == mysql::TypeEnum || to.GetType() == mysql::TypeSet)
    {
        return false;
    }

    if (origin.GetType() == mysql::TypeEnum
        || origin.GetType() == mysql::TypeSet
        || origin.GetType() == mysql::TypeBit
        || origin.GetType() == mysql::TypeNewDecimal
        || origin.GetType() == mysql::TypeFloat
        || origin.GetType() == mysql::TypeDouble)
        && IsTypeTime(to.GetType())
    {
        return false;
    }

    if origin.GetType() == mysql::TypeTiDBVectorFloat32
        || to.GetType() == mysql::TypeTiDBVectorFloat32
    {
        return false;
    }

    if (origin.GetType() == mysql::TypeEnum
        || origin.GetType() == mysql::TypeSet
        || origin.GetType() == mysql::TypeBit)
        && to.GetType() == mysql::TypeDuration
    {
        return false;
    }

    true
}

/// 判断 CHAR 与 VARCHAR 互转是否必须触发 reorg。
// ConvertBetweenCharAndVarchar 对应 Go 的 char/varchar 互转是否必须 reorg 判断。
pub fn ConvertBetweenCharAndVarchar(oldCol: u8, newCol: u8) -> bool {
    (IsTypeVarchar(oldCol) && newCol == mysql::TypeString)
        || (oldCol == mysql::TypeString && IsTypeVarchar(newCol) && collate::NewCollationEnabled())
}

/// 按字符集 Maxlen 折算后检查 VARCHAR flen 是否超上限。
// IsVarcharTooBigFieldLength 对应 Go 的 varchar 长度上限检查，按字符集 Maxlen 折算最大 flen。
pub fn IsVarcharTooBigFieldLength(
    colDefTpFlen: isize,
    colDefName: &str,
    setCharset: &str,
) -> Result<(), errors::SharedError> {
    let desc = charset::GetCharsetInfo(setCharset)
        .map_err(|err| errors::Errorf("%s", &[err.to_string().into()]))?;
    let mut maxFlen = mysql::MaxFieldVarCharLength as isize;
    maxFlen /= desc.Maxlen as isize;
    if colDefTpFlen != UnspecifiedLength && colDefTpFlen > maxFlen {
        return Err(ErrTooBigFieldLength.GenWithStack(
            "Column length too big for column '%s' (max = %d); use BLOB or TEXT instead",
            &[colDefName.into(), maxFlen.into()],
        ));
    }
    Ok(())
}
