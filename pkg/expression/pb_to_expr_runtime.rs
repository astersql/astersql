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

// TiPB 表达式反序列化为形式化表达式运行时（对应 Go `pb_to_expr`）。
//
// 将 tipb::Expr（下推到 TiKV/TiFlash 的协议缓冲节点）还原为 Column/Constant/
// ScalarFunction，并维护 ScalarFuncSig → AST 函数名的映射表。

use crate::{BuildContext, ExprBox, ast, collate, errors, mysql, types};

/// 按 MySQL 类型码构造默认 FieldType。
fn new_field_type(tp: u8) -> types::FieldType {
    *types::NewFieldType(tp)
}

/// 载荷解码失败时生成带十六进制转储的错误。
fn decode_error(kind: &str, value: &[u8]) -> crate::Error {
    errors::New(format!("invalid {kind} {:x?}", value))
}

/// 从 Datum 推断参数侧 FieldType（用于 ValueList 展开）。
fn field_type_from_datum(value: &types::Datum) -> types::FieldType {
    let mut field_type = types::FieldType::default();
    types::InferParamTypeFromDatum(value, &mut field_type);
    field_type
}

/// Converts a list of TiPB expressions into the formal expression runtime.
///
/// 批量将 tipb 表达式列表转为形式化表达式。
pub fn PBToExprs(
    context: &dyn BuildContext,
    expressions: &[tipb::Expr],
    field_types: &[types::FieldType],
) -> Result<Vec<ExprBox>, crate::Error> {
    expressions
        .iter()
        .map(|expression| PBToExpr(context, expression, field_types))
        .collect()
}

/// Converts one TiPB expression into the formal expression runtime.
///
/// 将单个 tipb 表达式转为 Column / Constant / ScalarFunction。
pub fn PBToExpr(
    context: &dyn BuildContext,
    expression: &tipb::Expr,
    field_types: &[types::FieldType],
) -> Result<ExprBox, crate::Error> {
    let result: ExprBox = match expression.get_tp() {
        // 列引用：载荷为列偏移，类型取自外层 field_types。
        tipb::ExprType::ColumnRef => {
            let (_, offset) = crate::codec::DecodeInt(expression.get_val())?;
            let index = usize::try_from(offset)
                .map_err(|_| errors::New(format!("negative column offset {offset}")))?;
            let return_type = field_types
                .get(index)
                .ok_or_else(|| errors::New(format!("column offset {index} is out of range")))?
                .clone();
            Box::new(crate::Column {
                Index: index as isize,
                RetType: Some(return_type),
                ..Default::default()
            })
        }
        tipb::ExprType::Null => Box::new(crate::Constant::with_type(
            types::Datum::default(),
            new_field_type(mysql::TypeNull),
        )),
        tipb::ExprType::Int64 => {
            let (_, value) = crate::codec::DecodeInt(expression.get_val())
                .map_err(|_| decode_error("int", expression.get_val()))?;
            Box::new(crate::Constant::with_type(
                types::NewIntDatum(value),
                PbTypeToFieldType(expression.get_field_type()),
            ))
        }
        tipb::ExprType::Uint64 => {
            let (_, value) = crate::codec::DecodeUint(expression.get_val())
                .map_err(|_| decode_error("uint", expression.get_val()))?;
            Box::new(crate::Constant::with_type(
                types::NewUintDatum(value),
                PbTypeToFieldType(expression.get_field_type()),
            ))
        }
        tipb::ExprType::String => {
            let field_type = PbTypeToFieldType(expression.get_field_type());
            let mut value = types::Datum::default();
            // 字符串需带上排序规则与显示宽度。
            value.SetBytesAsString(
                expression.get_val().to_vec(),
                collate::ProtoToCollation(expression.get_field_type().get_collate()),
                expression.get_field_type().get_flen() as u32,
            );
            Box::new(crate::Constant::with_type(value, field_type))
        }
        tipb::ExprType::Bytes => Box::new(crate::Constant::with_type(
            types::NewBytesDatum(expression.get_val().to_vec()),
            new_field_type(mysql::TypeString),
        )),
        tipb::ExprType::MysqlBit => Box::new(crate::Constant::with_type(
            types::NewMysqlBitDatum(types::BinaryLiteral(expression.get_val().to_vec())),
            new_field_type(mysql::TypeString),
        )),
        tipb::ExprType::Float32 | tipb::ExprType::Float64 => {
            let (_, value) = crate::codec::DecodeFloat(expression.get_val())
                .map_err(|_| decode_error("float", expression.get_val()))?;
            let datum = if expression.get_tp() == tipb::ExprType::Float32 {
                types::NewFloat32Datum(value as f32)
            } else {
                types::NewFloat64Datum(value)
            };
            Box::new(crate::Constant::with_type(
                datum,
                new_field_type(mysql::TypeDouble),
            ))
        }
        tipb::ExprType::MysqlDecimal => {
            let (_, decimal, precision, fraction) =
                crate::codec::DecodeDecimal(expression.get_val())
                    .map_err(|_| decode_error("decimal", expression.get_val()))?;
            let mut datum = types::NewDecimalDatum(decimal);
            datum.SetLength(precision);
            datum.SetFrac(fraction);
            Box::new(crate::Constant::with_type(
                datum,
                PbTypeToFieldType(expression.get_field_type()),
            ))
        }
        tipb::ExprType::MysqlDuration => {
            let (_, duration) = crate::codec::DecodeInt(expression.get_val())
                .map_err(|_| decode_error("duration", expression.get_val()))?;
            Box::new(crate::Constant::with_type(
                types::NewDurationDatum(types::Duration {
                    Duration: duration,
                    Fsp: types::MaxFsp,
                }),
                new_field_type(mysql::TypeDuration),
            ))
        }
        tipb::ExprType::MysqlTime => {
            let field_type = PbTypeToFieldType(expression.get_field_type());
            let (_, packed) = crate::codec::DecodeUint(expression.get_val())?;
            let mut value = types::Time::default();
            value.SetType(field_type.GetType());
            value.SetFsp(field_type.GetDecimal() as i32);
            value
                .FromPackedUint(packed)
                .map_err(|error| errors::New(error.to_string()))?;
            let location = context.GetEvalCtx().Location();
            // TIMESTAMP 线载按 UTC 打包，非 UTC 会话需转换到当前时区。
            if field_type.GetType() == mysql::TypeTimestamp && location != chrono_tz::UTC {
                value
                    .ConvertTimeZone(chrono_tz::UTC, location)
                    .map_err(|error| errors::New(error.to_string()))?;
            }
            Box::new(crate::Constant::with_type(
                types::NewTimeDatum(value),
                field_type,
            ))
        }
        tipb::ExprType::MysqlJson => {
            let (_, datum) = crate::codec::DecodeOne(expression.get_val())
                .map_err(|_| decode_error("json", expression.get_val()))?;
            if datum.Kind() != types::KindMysqlJSON {
                return Err(errors::New(format!(
                    "invalid Datum.Kind() {}",
                    datum.Kind()
                )));
            }
            Box::new(crate::Constant::with_type(
                datum,
                new_field_type(mysql::TypeJSON),
            ))
        }
        tipb::ExprType::MysqlEnum => {
            let (_, number) = crate::codec::DecodeUint(expression.get_val())
                .map_err(|_| decode_error("enum", expression.get_val()))?;
            let value = if number == 0 {
                types::Enum::default()
            } else {
                types::ParseEnumValue(expression.get_field_type().get_elems(), number)
                    .map_err(|error| errors::New(error.to_string()))?
            };
            Box::new(crate::Constant::with_type(
                types::NewMysqlEnumDatum(value),
                FieldTypeFromPB(expression.get_field_type()),
            ))
        }
        tipb::ExprType::TiDbVectorFloat32 => {
            let (value, _) = types::ZeroCopyDeserializeVectorFloat32(expression.get_val())
                .map_err(|_| decode_error("VectorFloat32", expression.get_val()))?;
            Box::new(crate::Constant::with_type(
                types::NewVectorFloat32Datum(value),
                new_field_type(mysql::TypeTiDBVectorFloat32),
            ))
        }
        tipb::ExprType::ScalarFunc => {
            let mut arguments = Vec::with_capacity(expression.get_children().len());
            for child in expression.get_children() {
                // ValueList：IN 列表等扁平常量集合；空列表折叠为假。
                if child.get_tp() == tipb::ExprType::ValueList {
                    if child.get_val().is_empty() {
                        return Ok(Box::new(crate::Constant::with_type(
                            types::NewIntDatum(0),
                            new_field_type(mysql::TypeLonglong),
                        )));
                    }
                    let values = crate::codec::Decode(child.get_val().to_vec(), 1)?;
                    arguments.extend(values.into_iter().map(|value| {
                        let field_type = field_type_from_datum(&value);
                        Box::new(crate::Constant::with_type(value, field_type)) as ExprBox
                    }));
                } else {
                    arguments.push(PBToExpr(context, child, field_types)?);
                }
            }
            let signature = format!("{:?}", expression.get_sig());
            let name = PBSignatureFunctionName(&signature)
                .ok_or_else(|| errors::New(format!("FUNCTION {signature} does not exist")))?;
            // 反序列化路径用 NewFunctionBase，避免立刻常量折叠改变下推形状。
            crate::NewFunctionBase(
                context,
                name,
                PbTypeToFieldType(expression.get_field_type()),
                arguments,
            )?
        }
        other => panic!("should be a tipb.ExprType_ScalarFunc, got {other:?}"),
    };
    Ok(result)
}

/// 从 tipb FieldType 还原内部 FieldType（别名入口）。
pub fn FieldTypeFromPB(field_type: &tipb::FieldType) -> types::FieldType {
    PbTypeToFieldType(field_type)
}

/// 复制 tipb 字段元数据到内部 FieldType（含排序规则枚举转换）。
pub fn PbTypeToFieldType(field_type: &tipb::FieldType) -> types::FieldType {
    let mut result = types::FieldType::default();
    result.SetType(field_type.get_tp() as u8);
    result.SetFlag(field_type.get_flag() as usize);
    result.SetFlen(field_type.get_flen() as isize);
    result.SetDecimal(field_type.get_decimal() as isize);
    result.SetCharset(field_type.get_charset().to_owned());
    result.SetCollate(collate::ProtoToCollation(field_type.get_collate()));
    result.SetElems(field_type.get_elems().to_vec());
    result
}

#[doc(hidden)]
/// 将 tipb ScalarFuncSig Debug 名映射为 AST 函数名；未知签名返回 None。
pub fn PBSignatureFunctionName(signature: &str) -> Option<&'static str> {
    let prefix = |value: &str| signature.starts_with(value);
    // 带类型后缀的比较算子（如 EqInt / LtString）。
    let typed_comparison = |name: &str| {
        signature.strip_prefix(name).is_some_and(|suffix| {
            matches!(
                suffix,
                "Int"
                    | "Real"
                    | "Decimal"
                    | "String"
                    | "Time"
                    | "Duration"
                    | "Json"
                    | "VectorFloat32"
            )
        })
    };
    Some(if prefix("Cast") {
        ast::Cast
    } else if prefix("Coalesce") {
        ast::Coalesce
    } else if prefix("NullEQ") || typed_comparison("NullEq") {
        ast::NullEQ
    } else if prefix("LT") || typed_comparison("Lt") {
        ast::LT
    } else if prefix("LE") || typed_comparison("Le") {
        ast::LE
    } else if prefix("GT") || typed_comparison("Gt") {
        ast::GT
    } else if prefix("GE") || typed_comparison("Ge") {
        ast::GE
    } else if prefix("EQ") || typed_comparison("Eq") {
        ast::EQ
    } else if prefix("NE") || typed_comparison("Ne") {
        ast::NE
    } else if prefix("Greatest") {
        ast::Greatest
    } else if prefix("Least") {
        ast::Least
    } else if prefix("Interval") {
        ast::Interval
    } else if prefix("Plus") {
        ast::Plus
    } else if prefix("Minus") {
        ast::Minus
    } else if prefix("Multiply") {
        ast::Mul
    } else if prefix("Divide") {
        ast::Div
    } else if prefix("IntDivide") {
        ast::IntDiv
    } else if prefix("Mod") {
        ast::Mod
    } else if prefix("Abs") {
        ast::Abs
    } else if prefix("Ceil") {
        ast::Ceil
    } else if prefix("Floor") {
        ast::Floor
    } else if prefix("Round") {
        ast::Round
    } else if prefix("Log10") {
        ast::Log10
    } else if prefix("Log2") {
        ast::Log2
    } else if prefix("Log") {
        ast::Log
    } else if prefix("Rand") {
        ast::Rand
    } else if prefix("Pow") {
        ast::Pow
    } else if prefix("Conv") {
        ast::Conv
    } else if prefix("CRC32") || prefix("Crc32") {
        ast::CRC32
    } else if prefix("Sign") {
        ast::Sign
    } else if prefix("Sqrt") {
        ast::Sqrt
    } else if prefix("Acos") {
        ast::Acos
    } else if prefix("Asin") {
        ast::Asin
    } else if prefix("Atan2") {
        ast::Atan2
    } else if prefix("Atan") {
        ast::Atan
    } else if prefix("Cos") {
        ast::Cos
    } else if prefix("Cot") {
        ast::Cot
    } else if prefix("Degrees") {
        ast::Degrees
    } else if prefix("Exp") {
        ast::Exp
    } else if prefix("PI") || signature == "Pi" {
        ast::PI
    } else if prefix("Radians") {
        ast::Radians
    } else if prefix("Sin") {
        ast::Sin
    } else if prefix("Tan") {
        ast::Tan
    } else if prefix("Truncate") {
        ast::Truncate
    } else if signature == "LogicalAnd" {
        ast::LogicAnd
    } else if signature == "LogicalOr" {
        ast::LogicOr
    } else if signature == "LogicalXor" {
        ast::LogicXor
    } else if prefix("UnaryNot") {
        ast::UnaryNot
    } else if prefix("UnaryMinus") {
        ast::UnaryMinus
    } else if signature.ends_with("IsNull") {
        ast::IsNull
    } else if prefix("BitAnd") {
        ast::And
    } else if prefix("BitOr") {
        ast::Or
    } else if prefix("BitXor") {
        ast::Xor
    } else if prefix("BitNeg") {
        ast::BitNeg
    } else if signature.contains("IsTrueWithNull") {
        ast::IsTruthWithNull
    } else if signature.contains("IsTrue") {
        ast::IsTruthWithoutNull
    } else if signature.contains("IsFalse") {
        ast::IsFalsity
    } else if signature == "LeftShift" {
        ast::LeftShift
    } else if signature == "RightShift" {
        ast::RightShift
    } else if signature == "BitCount" {
        ast::BitCount
    } else if prefix("GetParam") {
        ast::GetParam
    } else if signature == "GetVar" {
        ast::GetVar
    } else if signature == "SetVar" {
        ast::SetVar
    } else if typed_comparison("In") {
        ast::In
    } else if prefix("IfNull") {
        ast::Ifnull
    } else if prefix("If") {
        ast::If
    } else if prefix("CaseWhen") {
        ast::Case
    } else {
        return function_name_for_named_signature(signature);
    })
}

/// 精确/前缀表查找：字符串、信息、JSON、向量等命名签名。
fn function_name_for_named_signature(signature: &str) -> Option<&'static str> {
    const NAMES: &[(&str, &str)] = &[
        ("RowSig", ast::RowFunc),
        ("ValuesDecimal", ast::Values),
        ("ValuesDuration", ast::Values),
        ("ValuesInt", ast::Values),
        ("ValuesJson", ast::Values),
        ("ValuesReal", ast::Values),
        ("ValuesString", ast::Values),
        ("ValuesTime", ast::Values),
        ("Compress", ast::Compress),
        ("MD5", ast::MD5),
        ("Md5", ast::MD5),
        ("Password", "password"),
        ("RandomBytes", ast::RandomBytes),
        ("SHA1", ast::SHA1),
        ("Sha1", ast::SHA1),
        ("SHA2", ast::SHA2),
        ("Sha2", ast::SHA2),
        ("AesDecrypt", ast::AesDecrypt),
        ("AesDecryptIv", ast::AesDecrypt),
        ("AesEncrypt", ast::AesEncrypt),
        ("AesEncryptIv", ast::AesEncrypt),
        ("Encode", ast::Encode),
        ("Decode", ast::Decode),
        ("Sm3", ast::SM3),
        ("Uncompress", ast::Uncompress),
        ("UncompressedLength", ast::UncompressedLength),
        ("Database", ast::Database),
        ("FoundRows", ast::FoundRows),
        ("CurrentUser", ast::CurrentUser),
        ("User", ast::User),
        ("ConnectionID", ast::ConnectionID),
        ("ConnectionId", ast::ConnectionID),
        ("LastInsertID", ast::LastInsertId),
        ("LastInsertIDWithID", ast::LastInsertId),
        ("LastInsertId", ast::LastInsertId),
        ("LastInsertIdWithId", ast::LastInsertId),
        ("Version", ast::Version),
        ("TiDBVersion", ast::TiDBVersion),
        ("TiDbVersion", ast::TiDBVersion),
        ("RowCount", ast::RowCount),
        ("Sleep", ast::Sleep),
        ("Lock", ast::GetLock),
        ("ReleaseLock", ast::ReleaseLock),
        ("DecimalAnyValue", ast::AnyValue),
        ("DurationAnyValue", ast::AnyValue),
        ("IntAnyValue", ast::AnyValue),
        ("JSONAnyValue", ast::AnyValue),
        ("JsonAnyValue", ast::AnyValue),
        ("RealAnyValue", ast::AnyValue),
        ("StringAnyValue", ast::AnyValue),
        ("TimeAnyValue", ast::AnyValue),
        ("VectorFloat32AnyValue", ast::AnyValue),
        ("InetAton", ast::InetAton),
        ("InetNtoa", ast::InetNtoa),
        ("Inet6Aton", ast::Inet6Aton),
        ("Inet6Ntoa", ast::Inet6Ntoa),
        ("IsIPv4", ast::IsIPv4),
        ("IsIPv4Compat", ast::IsIPv4Compat),
        ("IsIPv4Mapped", ast::IsIPv4Mapped),
        ("IsIPv6", ast::IsIPv6),
        ("UUID", ast::UUID),
        ("Uuid", ast::UUID),
        ("UUIDv4", ast::UUID),
        ("UuiDv4", ast::UUIDv4),
        ("UUIDv7", ast::UUID),
        ("UuiDv7", ast::UUIDv7),
        ("UUIDVersion", ast::UUIDVersion),
        ("UuidVersion", ast::UUIDVersion),
        ("UUIDTimestamp", ast::UUIDTimestamp),
        ("UuidTimestamp", ast::UUIDTimestamp),
        ("IsUuid", ast::IsUUID),
        ("VitessHash", ast::VitessHash),
        ("TiDbShard", ast::TiDBShard),
        ("GroupingSig", ast::Grouping),
        ("LikeSig", ast::Like),
        ("IlikeSig", ast::Ilike),
        ("RegexpSig", ast::Regexp),
        ("RegexpUTF8Sig", ast::Regexp),
        ("RegexpUtf8Sig", ast::Regexp),
        ("RegexpLikeSig", ast::RegexpLike),
        ("RegexpLikeUtf8Sig", ast::RegexpLike),
        ("RegexpSubstrSig", ast::RegexpSubstr),
        ("RegexpSubstrUtf8Sig", ast::RegexpSubstr),
        ("RegexpInStrSig", ast::RegexpInStr),
        ("RegexpInStrUtf8Sig", ast::RegexpInStr),
        ("RegexpReplaceSig", ast::RegexpReplace),
        ("RegexpReplaceUtf8Sig", ast::RegexpReplace),
        ("JsonExtractSig", ast::JSONExtract),
        ("JsonUnquoteSig", ast::JSONUnquote),
        ("JsonTypeSig", ast::JSONType),
        ("JsonSetSig", ast::JSONSet),
        ("JsonInsertSig", ast::JSONInsert),
        ("JsonReplaceSig", ast::JSONReplace),
        ("JsonRemoveSig", ast::JSONRemove),
        ("JsonMergeSig", ast::JSONMerge),
        ("JsonObjectSig", ast::JSONObject),
        ("JsonArraySig", ast::JSONArray),
        ("JsonValidJsonSig", ast::JSONValid),
        ("JsonValidStringSig", ast::JSONValid),
        ("JsonValidOthersSig", ast::JSONValid),
        ("JsonContainsSig", ast::JSONContains),
        ("JsonArrayAppendSig", ast::JSONArrayAppend),
        ("JsonArrayInsertSig", ast::JSONArrayInsert),
        ("JsonMergePatchSig", ast::JSONMergePatch),
        ("JsonMergePreserveSig", ast::JSONMergePreserve),
        ("JsonContainsPathSig", ast::JSONContainsPath),
        ("JsonQuoteSig", ast::JSONQuote),
        ("JsonSearchSig", ast::JSONSearch),
        ("JsonStorageSizeSig", ast::JSONStorageSize),
        ("JsonStorageFreeSig", ast::JSONStorageFree),
        ("JsonPrettySig", ast::JSONPretty),
        ("JsonDepthSig", ast::JSONDepth),
        ("JsonKeysSig", ast::JSONKeys),
        ("JsonKeys2ArgsSig", ast::JSONKeys),
        ("JsonLengthSig", ast::JSONLength),
        ("JsonMemberOfSig", ast::JSONMemberOf),
        ("VecFromTextSig", ast::VecFromText),
        ("FtsMatchWord", ast::FTSMatchWord),
        ("FtsMatchExpression", ast::FTSMysqlMatchAgainst),
        ("FtsMatchPrefix", ast::FTSMatchWord),
        ("FtsMatchRegexp", ast::FTSMatchWord),
        ("FtsMatchPhrase", ast::FTSMatchWord),
        ("DateLiteral", ast::DateLiteral),
        ("TimeLiteral", ast::TimeLiteral),
        ("TimestampLiteral", ast::TimestampLiteral),
        ("Timestamp1Arg", ast::Timestamp),
        ("Timestamp2Args", ast::Timestamp),
        ("Ascii", ast::ASCII),
        ("ConcatWs", ast::ConcatWS),
    ];
    if let Some((_, name)) = NAMES.iter().find(|(candidate, _)| *candidate == signature) {
        return Some(*name);
    }
    function_name_for_date_or_string_signature(signature)
}

/// 日期/字符串类签名的前缀匹配回退表。
fn function_name_for_date_or_string_signature(signature: &str) -> Option<&'static str> {
    let prefix = |value: &str| signature.starts_with(value);
    Some(if prefix("DateFormat") {
        ast::DateFormat
    } else if prefix("DateDiff") {
        ast::DateDiff
    } else if signature.contains("TimeDiff") {
        ast::TimeDiff
    } else if signature == "Date" {
        "date"
    } else if signature == "Hour" {
        "hour"
    } else if signature == "Minute" {
        "minute"
    } else if signature == "Second" {
        "second"
    } else if signature == "MicroSecond" {
        ast::MicroSecond
    } else if signature == "Month" {
        "month"
    } else if signature == "MonthName" {
        ast::MonthName
    } else if prefix("Now") {
        ast::Now
    } else if signature == "DayName" {
        ast::DayName
    } else if signature == "DayOfMonth" {
        ast::DayOfMonth
    } else if signature == "DayOfWeek" {
        ast::DayOfWeek
    } else if signature == "DayOfYear" {
        ast::DayOfYear
    } else if prefix("WeekWith") || prefix("WeekWithout") {
        "week"
    } else if signature == "WeekDay" {
        ast::Weekday
    } else if signature == "WeekOfYear" {
        ast::WeekOfYear
    } else if signature == "Year" {
        "year"
    } else if prefix("YearWeek") {
        ast::YearWeek
    } else if signature == "GetFormat" {
        ast::GetFormat
    } else if prefix("SysDate") {
        ast::Sysdate
    } else if signature == "CurrentDate" {
        ast::CurrentDate
    } else if prefix("CurrentTime") {
        ast::CurrentTime
    } else if signature == "Time" {
        "time"
    } else if signature == "UTCDate" || signature == "UtcDate" {
        ast::UTCDate
    } else if prefix("UTCTimestamp") || prefix("UtcTimestamp") {
        ast::UTCTimestamp
    } else if prefix("UTCTime") || prefix("UtcTime") {
        ast::UTCTime
    } else if prefix("SubstringIndex") {
        ast::SubstringIndex
    } else if prefix("Substring") {
        ast::Substring
    } else if prefix("AddDatetime") || prefix("AddDateAnd") {
        ast::AddTime
    } else if prefix("SubDatetime") || prefix("SubDateAnd") {
        ast::SubTime
    } else if prefix("AddDate") {
        ast::AddDate
    } else if prefix("SubDate") {
        ast::SubDate
    } else if prefix("Add") {
        ast::AddTime
    } else if prefix("Sub") {
        ast::SubTime
    } else if prefix("UnixTimestamp") {
        ast::UnixTimestamp
    } else if signature == "MakeDate" {
        ast::MakeDate
    } else if signature == "MakeTime" {
        ast::MakeTime
    } else if signature == "PeriodAdd" {
        ast::PeriodAdd
    } else if signature == "PeriodDiff" {
        ast::PeriodDiff
    } else if signature == "Quarter" {
        "quarter"
    } else if signature == "SecToTime" {
        ast::SecToTime
    } else if signature == "TimeToSec" {
        ast::TimeToSec
    } else if signature == "TimestampAdd" {
        ast::TimestampAdd
    } else if signature == "TimestampDiff" {
        ast::TimestampDiff
    } else if signature == "ToDays" {
        ast::ToDays
    } else if signature == "ToSeconds" {
        ast::ToSeconds
    } else if signature == "LastDay" {
        ast::LastDay
    } else if prefix("StrToDate") {
        ast::StrToDate
    } else if prefix("FromUnixTime") {
        ast::FromUnixTime
    } else if prefix("Extract") {
        ast::Extract
    } else if signature == "FromDays" {
        ast::FromDays
    } else if signature == "TimeFormat" {
        ast::TimeFormat
    } else if signature == "BitLength" {
        ast::BitLength
    } else if signature == "Bin" {
        ast::Bin
    } else if signature == "ASCII" {
        ast::ASCII
    } else if signature == "Char" {
        ast::CharFunc
    } else if prefix("CharLength") {
        ast::CharLength
    } else if signature == "Concat" {
        ast::Concat
    } else if signature == "ConcatWS" {
        ast::ConcatWS
    } else if signature == "Convert" {
        ast::Convert
    } else if signature == "Elt" {
        ast::Elt
    } else if prefix("ExportSet") {
        ast::ExportSet
    } else if prefix("Field") {
        ast::Field
    } else if signature == "FindInSet" {
        ast::FindInSet
    } else if prefix("Format") {
        ast::Format
    } else if signature == "FromBase64" {
        ast::FromBase64
    } else if prefix("Hex") {
        ast::Hex
    } else if prefix("Insert") {
        ast::InsertFunc
    } else if prefix("Instr") {
        ast::Instr
    } else if signature == "LTrim" {
        ast::LTrim
    } else if prefix("Left") {
        ast::Left
    } else if signature == "Length" {
        ast::Length
    } else if prefix("Locate") {
        ast::Locate
    } else if prefix("Lower") {
        ast::Lower
    } else if prefix("Lpad") {
        ast::Lpad
    } else if signature == "MakeSet" {
        ast::MakeSet
    } else if prefix("Oct") {
        ast::Oct
    } else if signature == "Ord" {
        ast::Ord
    } else if signature == "Quote" {
        ast::Quote
    } else if signature == "RTrim" {
        ast::RTrim
    } else if signature == "Repeat" {
        ast::Repeat
    } else if signature == "Replace" {
        ast::Replace
    } else if prefix("Reverse") {
        ast::Reverse
    } else if prefix("Right") {
        ast::Right
    } else if prefix("Rpad") {
        ast::Rpad
    } else if signature == "Space" {
        ast::Space
    } else if signature == "Strcmp" {
        ast::Strcmp
    } else if signature == "ToBase64" {
        ast::ToBase64
    } else if prefix("Trim") {
        ast::Trim
    } else if signature == "UnHex" {
        ast::Unhex
    } else if prefix("Upper") {
        ast::Upper
    } else if signature == "ToBinary" {
        crate::InternalFuncToBinary
    } else if signature == "FromBinary" {
        crate::InternalFuncFromBinary
    } else if signature == "VectorFloat32IsNull" {
        ast::IsNull
    } else if signature == "VecAsTextSig" {
        ast::VecAsText
    } else if signature == "VecDimsSig" {
        ast::VecDims
    } else if signature == "VecL1DistanceSig" {
        ast::VecL1Distance
    } else if signature == "VecL2DistanceSig" {
        ast::VecL2Distance
    } else if signature == "VecNegativeInnerProductSig" {
        ast::VecNegativeInnerProduct
    } else if signature == "VecCosineDistanceSig" {
        ast::VecCosineDistance
    } else if signature == "VecL2NormSig" {
        ast::VecL2Norm
    } else if signature == "FTSMatchWord" {
        ast::FTSMatchWord
    } else if signature == "FTSMatchExpression" {
        ast::FTSMysqlMatchAgainst
    } else {
        return None;
    })
}
