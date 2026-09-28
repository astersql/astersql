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

// DistSQL 侧内置表达式重建：TiPB → 内存 Expression。
//
// DistSQL（分布式 SQL）把下推表达式以 tipb.Expr 传到存储/协处理器；本模块负责
// FieldType 转换、ScalarFuncSig 签名分派表、字面量 Datum 解码以及递归构建
// Column / Constant / ScalarFunction。仅做内存重建，不执行求值。

// 本文件对照 pkg/expression/distsql_builtin.go，保留 TiPB 类型转换、完整签名分派和 Datum 解码控制流。
// Reconstructs expressions in memory only.

pub use crate::{collate, types};
pub use tipb;

/// MySQL 类型常量再导出，统一大写 TYPE_* 命名供测试与解码使用。
pub mod mysql {
    pub use crate::mysql::*;

    pub const TYPE_NULL: u8 = TypeNull;
    pub const TYPE_TIMESTAMP: u8 = TypeTimestamp;
    pub const TYPE_DOUBLE: u8 = TypeDouble;
    pub const TYPE_DURATION: u8 = TypeDuration;
    pub const TYPE_JSON: u8 = TypeJSON;
    pub const TYPE_TIDB_VECTOR_FLOAT32: u8 = TypeTiDBVectorFloat32;
    pub const TYPE_LONGLONG: u8 = TypeLonglong;
    pub const TYPE_STRING: u8 = TypeString;
}

/// 时区类型别名，对齐 Go time.Location。
pub mod time {
    pub type Location = chrono_tz::Tz;
    pub const UTC: Location = chrono_tz::UTC;
}

/// DistSQL 表达式重建过程中的错误包装。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpressionError(String);

impl ExpressionError {
    /// 由任意可显示错误构造。
    pub fn external(error: impl std::fmt::Display) -> Self {
        Self(error.to_string())
    }
}

impl std::fmt::Display for ExpressionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ExpressionError {}

/// 错误构造辅助，对齐 Go errors.errorf 调用面。
pub mod errors {
    use super::ExpressionError;

    pub fn errorf(message: impl Into<String>) -> ExpressionError {
        ExpressionError(message.into())
    }
}

/// TiDB 二进制编解码封装，失败统一转为 ExpressionError。
pub mod codec {
    use super::{ExpressionError, types};

    #[allow(unused_imports)]
    pub use crate::codec::{EncodeDecimal, EncodeFloat, EncodeInt, EncodeUint};

    pub fn decode_int(data: &[u8]) -> Result<(&[u8], i64), ExpressionError> {
        crate::codec::DecodeInt(data).map_err(ExpressionError::external)
    }

    pub fn decode_uint(data: &[u8]) -> Result<(&[u8], u64), ExpressionError> {
        crate::codec::DecodeUint(data).map_err(ExpressionError::external)
    }

    pub fn decode_float(data: &[u8]) -> Result<(&[u8], f64), ExpressionError> {
        crate::codec::DecodeFloat(data).map_err(ExpressionError::external)
    }

    pub fn decode_decimal(
        data: &[u8],
    ) -> Result<(&[u8], types::MyDecimal, i32, i32), ExpressionError> {
        crate::codec::DecodeDecimal(data).map_err(ExpressionError::external)
    }

    pub fn decode_one(data: &[u8]) -> Result<(&[u8], types::Datum), ExpressionError> {
        crate::codec::DecodeOne(data).map_err(ExpressionError::external)
    }

    pub fn decode(data: &[u8], size: usize) -> Result<Vec<types::Datum>, ExpressionError> {
        crate::codec::Decode(data.to_vec(), size).map_err(ExpressionError::external)
    }
}

/// DistSQL 求值上下文：时区与 max_allowed_packet。
#[derive(Clone, Debug)]
pub struct EvalContext {
    location: time::Location,
    max_allowed_packet: u64,
}

impl EvalContext {
    pub fn location(&self) -> time::Location {
        self.location
    }

    pub fn get_max_allowed_packet(&self) -> u64 {
        self.max_allowed_packet
    }
}

/// 构建 DistSQL 表达式时的上下文，内嵌 EvalContext。
#[derive(Clone, Debug)]
pub struct BuildContext {
    eval_ctx: EvalContext,
}

impl BuildContext {
    pub fn new(location: time::Location, max_allowed_packet: u64) -> Self {
        Self {
            eval_ctx: EvalContext {
                location,
                max_allowed_packet,
            },
        }
    }

    pub fn get_eval_ctx(&self) -> &EvalContext {
        &self.eval_ctx
    }
}

/// 列引用：协议中的列下标与返回类型。
#[derive(Clone, Debug)]
pub struct Column {
    pub index: usize,
    pub ret_type: types::FieldType,
}

impl Column {
    pub fn new(index: usize, ret_type: types::FieldType) -> Self {
        Self { index, ret_type }
    }
}

/// 常量表达式：Datum 取值与字段类型。
#[derive(Clone)]
pub struct Constant {
    pub value: types::Datum,
    pub ret_type: types::FieldType,
}

impl Constant {
    pub fn with_type(value: types::Datum, ret_type: types::FieldType) -> Self {
        Self { value, ret_type }
    }

    /// 按 Datum Kind 推断默认 FieldType 后构造常量。
    pub fn from_datum(value: types::Datum) -> Self {
        let mut ret_type = types::FieldType::default();
        let tp = match value.Kind() {
            types::KindNull => mysql::TypeNull,
            types::KindInt64 => mysql::TypeLonglong,
            types::KindUint64 => {
                ret_type.AddFlag(mysql::UnsignedFlag);
                mysql::TypeLonglong
            }
            types::KindFloat32 => mysql::TypeFloat,
            types::KindFloat64 => mysql::TypeDouble,
            types::KindString => mysql::TypeVarString,
            types::KindBytes | types::KindBinaryLiteral | types::KindMysqlBit => mysql::TypeBlob,
            types::KindMysqlDecimal => mysql::TypeNewDecimal,
            types::KindMysqlDuration => mysql::TypeDuration,
            types::KindMysqlEnum => mysql::TypeEnum,
            types::KindMysqlSet => mysql::TypeSet,
            types::KindMysqlTime => value.GetMysqlTime().Type(),
            types::KindMysqlJSON => mysql::TypeJSON,
            types::KindVectorFloat32 => mysql::TypeTiDBVectorFloat32,
            _ => mysql::TypeUnspecified,
        };
        ret_type.SetType(tp);
        Self { value, ret_type }
    }

    /// NULL 常量，类型码由调用方指定。
    pub fn null(tp: u8) -> Self {
        Self::with_type(types::Datum::default(), *types::NewFieldType(tp))
    }

    pub fn bytes(value: Vec<u8>) -> Self {
        Self::with_type(
            types::NewBytesDatum(value),
            *types::NewFieldType(mysql::TypeString),
        )
    }

    pub fn mysql_bit(value: Vec<u8>) -> Self {
        Self::with_type(
            types::NewMysqlBitDatum(types::BinaryLiteral(value)),
            *types::NewFieldType(mysql::TypeString),
        )
    }

    pub fn boolean(value: bool) -> Self {
        Self::with_type(
            types::NewIntDatum(i64::from(value)),
            *types::NewFieldType(mysql::TypeLonglong),
        )
    }
}

/// DistSQL 内存表达式三分支：列、常量、标量函数。
#[derive(Clone)]
pub enum Expression {
    Column(Column),
    Constant(Constant),
    ScalarFunction(ScalarFunction),
}

impl Expression {
    pub fn get_type<'a>(&'a self, _ctx: &EvalContext) -> &'a types::FieldType {
        match self {
            Self::Column(column) => &column.ret_type,
            Self::Constant(constant) => &constant.ret_type,
            Self::ScalarFunction(function) => &function.ret_type,
        }
    }
}

/// 草稿期 builtin 基类：参数列表与返回类型。
#[derive(Clone)]
pub struct BuiltinBaseDraft {
    args: Vec<Expression>,
    return_type: types::FieldType,
}

impl BuiltinBaseDraft {
    pub fn args(&self) -> &[Expression] {
        &self.args
    }
}

/// 按 PB 签名实例化的草稿：保留构造器名字符串供对等测试核对。
#[derive(Clone)]
pub struct BuiltinFuncDraft {
    pub base: BuiltinBaseDraft,
    pub signature: tipb::ScalarFuncSig,
    pub constructor: &'static str,
    pub max_allowed_packet: u64,
}

impl BuiltinFuncDraft {
    pub fn return_type(&self) -> &types::FieldType {
        &self.base.return_type
    }
}

fn newBaseBuiltinFuncWithFieldType(
    return_type: types::FieldType,
    args: Vec<Expression>,
) -> Result<BuiltinBaseDraft, ExpressionError> {
    Ok(BuiltinBaseDraft { args, return_type })
}

/// DistSQL 标量函数：函数名、返回类型、草稿实现与 coercibility。
#[derive(Clone)]
pub struct ScalarFunction {
    pub func_name: String,
    pub ret_type: types::FieldType,
    pub function: BuiltinFuncDraft,
    pub coercibility: i32,
}

impl ScalarFunction {
    pub fn new(func_name: String, ret_type: types::FieldType, function: BuiltinFuncDraft) -> Self {
        Self {
            func_name,
            ret_type,
            function,
            coercibility: 0,
        }
    }

    pub fn set_coercibility(&mut self, coercibility: i32) {
        self.coercibility = coercibility;
    }
}

mod ast {
    pub fn new_ci_str(value: impl Into<String>) -> String {
        value.into()
    }
}

struct DerivedCollation {
    coer: i32,
}

fn deriveCollation(
    _ctx: &BuildContext,
    _function_name: &str,
    args: &[Expression],
    return_eval_type: types::EvalType,
    argument_eval_types: &[types::EvalType],
) -> Result<DerivedCollation, ExpressionError> {
    if args.len() != argument_eval_types.len() {
        return Err(errors::errorf(
            "argument collation metadata length mismatch",
        ));
    }
    let coer = if return_eval_type == types::ETString {
        4
    } else {
        5
    };
    Ok(DerivedCollation { coer })
}

struct FunctionNotExists;

impl FunctionNotExists {
    fn gen_with_stack_by_args(&self, kind: &str, name: &str) -> ExpressionError {
        errors::errorf(format!("{kind} {name} does not exist"))
    }
}

static ErrFunctionNotExists: FunctionNotExists = FunctionNotExists;

/// FieldTypeFromPB 是 PbTypeToFieldType 的别名入口。
pub fn FieldTypeFromPB(field_type: &tipb::FieldType) -> types::FieldType {
    PbTypeToFieldType(field_type)
}

/// 对应 Go 的 PbTypeToFieldType：逐字段复制 TiPB 类型，并把协议排序规则编号转成本地排序规则。
pub fn PbTypeToFieldType(tp: &tipb::FieldType) -> types::FieldType {
    let mut ft = types::FieldType::default();
    ft.SetType(tp.get_tp() as u8);
    ft.SetFlag(tp.get_flag() as usize);
    ft.SetFlen(tp.get_flen() as isize);
    ft.SetDecimal(tp.get_decimal() as isize);
    ft.SetCharset(tp.get_charset().to_owned());
    ft.SetCollate(collate::ProtoToCollation(tp.get_collate()));
    ft.SetElems(tp.get_elems().to_vec());
    ft
}

/// 每条记录对应 Go getSignatureByPB switch 的一个 case；constructor 保留原构造表达式，便于逐项核对。
pub struct SignatureRecipe {
    pub signature: &'static str,
    pub constructor: &'static str,
}

/// 完整 ScalarFuncSig → Go 构造器名映射表（约 565 项），与 tipb 枚举顺序对齐。
pub const SIGNATURE_RECIPES: &[SignatureRecipe] = &[
    SignatureRecipe {
        signature: "CastIntAsInt",
        constructor: "&builtinCastIntAsIntSig{newBaseBuiltinCastFunc(base, false)}",
    },
    SignatureRecipe {
        signature: "CastIntAsReal",
        constructor: "&builtinCastIntAsRealSig{newBaseBuiltinCastFunc(base, false)}",
    },
    SignatureRecipe {
        signature: "CastIntAsString",
        constructor: "&builtinCastIntAsStringSig{base}",
    },
    SignatureRecipe {
        signature: "CastIntAsDecimal",
        constructor: "&builtinCastIntAsDecimalSig{newBaseBuiltinCastFunc(base, false)}",
    },
    SignatureRecipe {
        signature: "CastIntAsTime",
        constructor: "&builtinCastIntAsTimeSig{base}",
    },
    SignatureRecipe {
        signature: "CastIntAsDuration",
        constructor: "&builtinCastIntAsDurationSig{base}",
    },
    SignatureRecipe {
        signature: "CastIntAsJson",
        constructor: "&builtinCastIntAsJSONSig{base}",
    },
    SignatureRecipe {
        signature: "CastRealAsInt",
        constructor: "&builtinCastRealAsIntSig{newBaseBuiltinCastFunc(base, false)}",
    },
    SignatureRecipe {
        signature: "CastRealAsReal",
        constructor: "&builtinCastRealAsRealSig{newBaseBuiltinCastFunc(base, false)}",
    },
    SignatureRecipe {
        signature: "CastRealAsString",
        constructor: "&builtinCastRealAsStringSig{base}",
    },
    SignatureRecipe {
        signature: "CastRealAsDecimal",
        constructor: "&builtinCastRealAsDecimalSig{newBaseBuiltinCastFunc(base, false)}",
    },
    SignatureRecipe {
        signature: "CastRealAsTime",
        constructor: "&builtinCastRealAsTimeSig{base}",
    },
    SignatureRecipe {
        signature: "CastRealAsDuration",
        constructor: "&builtinCastRealAsDurationSig{base}",
    },
    SignatureRecipe {
        signature: "CastRealAsJson",
        constructor: "&builtinCastRealAsJSONSig{base}",
    },
    SignatureRecipe {
        signature: "CastDecimalAsInt",
        constructor: "&builtinCastDecimalAsIntSig{newBaseBuiltinCastFunc(base, false)}",
    },
    SignatureRecipe {
        signature: "CastDecimalAsReal",
        constructor: "&builtinCastDecimalAsRealSig{newBaseBuiltinCastFunc(base, false)}",
    },
    SignatureRecipe {
        signature: "CastDecimalAsString",
        constructor: "&builtinCastDecimalAsStringSig{base}",
    },
    SignatureRecipe {
        signature: "CastDecimalAsDecimal",
        constructor: "&builtinCastDecimalAsDecimalSig{newBaseBuiltinCastFunc(base, false)}",
    },
    SignatureRecipe {
        signature: "CastDecimalAsTime",
        constructor: "&builtinCastDecimalAsTimeSig{base}",
    },
    SignatureRecipe {
        signature: "CastDecimalAsDuration",
        constructor: "&builtinCastDecimalAsDurationSig{base}",
    },
    SignatureRecipe {
        signature: "CastDecimalAsJson",
        constructor: "&builtinCastDecimalAsJSONSig{base}",
    },
    SignatureRecipe {
        signature: "CastStringAsInt",
        constructor: "&builtinCastStringAsIntSig{newBaseBuiltinCastFunc(base, false)}",
    },
    SignatureRecipe {
        signature: "CastStringAsReal",
        constructor: "&builtinCastStringAsRealSig{newBaseBuiltinCastFunc(base, false)}",
    },
    SignatureRecipe {
        signature: "CastStringAsString",
        constructor: "&builtinCastStringAsStringSig{base}",
    },
    SignatureRecipe {
        signature: "CastStringAsDecimal",
        constructor: "&builtinCastStringAsDecimalSig{newBaseBuiltinCastFunc(base, false)}",
    },
    SignatureRecipe {
        signature: "CastStringAsTime",
        constructor: "&builtinCastStringAsTimeSig{base}",
    },
    SignatureRecipe {
        signature: "CastStringAsDuration",
        constructor: "&builtinCastStringAsDurationSig{base}",
    },
    SignatureRecipe {
        signature: "CastStringAsJson",
        constructor: "&builtinCastStringAsJSONSig{base}",
    },
    SignatureRecipe {
        signature: "CastTimeAsInt",
        constructor: "&builtinCastTimeAsIntSig{newBaseBuiltinCastFunc(base, false)}",
    },
    SignatureRecipe {
        signature: "CastTimeAsReal",
        constructor: "&builtinCastTimeAsRealSig{newBaseBuiltinCastFunc(base, false)}",
    },
    SignatureRecipe {
        signature: "CastTimeAsString",
        constructor: "&builtinCastTimeAsStringSig{base}",
    },
    SignatureRecipe {
        signature: "CastTimeAsDecimal",
        constructor: "&builtinCastTimeAsDecimalSig{newBaseBuiltinCastFunc(base, false)}",
    },
    SignatureRecipe {
        signature: "CastTimeAsTime",
        constructor: "&builtinCastTimeAsTimeSig{base}",
    },
    SignatureRecipe {
        signature: "CastTimeAsDuration",
        constructor: "&builtinCastTimeAsDurationSig{base}",
    },
    SignatureRecipe {
        signature: "CastTimeAsJson",
        constructor: "&builtinCastTimeAsJSONSig{base}",
    },
    SignatureRecipe {
        signature: "CastDurationAsInt",
        constructor: "&builtinCastDurationAsIntSig{newBaseBuiltinCastFunc(base, false)}",
    },
    SignatureRecipe {
        signature: "CastDurationAsReal",
        constructor: "&builtinCastDurationAsRealSig{newBaseBuiltinCastFunc(base, false)}",
    },
    SignatureRecipe {
        signature: "CastDurationAsString",
        constructor: "&builtinCastDurationAsStringSig{base}",
    },
    SignatureRecipe {
        signature: "CastDurationAsDecimal",
        constructor: "&builtinCastDurationAsDecimalSig{newBaseBuiltinCastFunc(base, false)}",
    },
    SignatureRecipe {
        signature: "CastDurationAsTime",
        constructor: "&builtinCastDurationAsTimeSig{base}",
    },
    SignatureRecipe {
        signature: "CastDurationAsDuration",
        constructor: "&builtinCastDurationAsDurationSig{base}",
    },
    SignatureRecipe {
        signature: "CastDurationAsJson",
        constructor: "&builtinCastDurationAsJSONSig{base}",
    },
    SignatureRecipe {
        signature: "CastJsonAsInt",
        constructor: "&builtinCastJSONAsIntSig{newBaseBuiltinCastFunc(base, false)}",
    },
    SignatureRecipe {
        signature: "CastJsonAsReal",
        constructor: "&builtinCastJSONAsRealSig{newBaseBuiltinCastFunc(base, false)}",
    },
    SignatureRecipe {
        signature: "CastJsonAsString",
        constructor: "&builtinCastJSONAsStringSig{base}",
    },
    SignatureRecipe {
        signature: "CastJsonAsDecimal",
        constructor: "&builtinCastJSONAsDecimalSig{newBaseBuiltinCastFunc(base, false)}",
    },
    SignatureRecipe {
        signature: "CastJsonAsTime",
        constructor: "&builtinCastJSONAsTimeSig{base}",
    },
    SignatureRecipe {
        signature: "CastJsonAsDuration",
        constructor: "&builtinCastJSONAsDurationSig{base}",
    },
    SignatureRecipe {
        signature: "CastJsonAsJson",
        constructor: "&builtinCastJSONAsJSONSig{base}",
    },
    SignatureRecipe {
        signature: "CoalesceInt",
        constructor: "&builtinCoalesceIntSig{base}",
    },
    SignatureRecipe {
        signature: "CoalesceReal",
        constructor: "&builtinCoalesceRealSig{base}",
    },
    SignatureRecipe {
        signature: "CoalesceDecimal",
        constructor: "&builtinCoalesceDecimalSig{base}",
    },
    SignatureRecipe {
        signature: "CoalesceString",
        constructor: "&builtinCoalesceStringSig{base}",
    },
    SignatureRecipe {
        signature: "CoalesceTime",
        constructor: "&builtinCoalesceTimeSig{base}",
    },
    SignatureRecipe {
        signature: "CoalesceDuration",
        constructor: "&builtinCoalesceDurationSig{base}",
    },
    SignatureRecipe {
        signature: "CoalesceJson",
        constructor: "&builtinCoalesceJSONSig{base}",
    },
    SignatureRecipe {
        signature: "LTInt",
        constructor: "&builtinLTIntSig{base}",
    },
    SignatureRecipe {
        signature: "LTReal",
        constructor: "&builtinLTRealSig{base}",
    },
    SignatureRecipe {
        signature: "LTDecimal",
        constructor: "&builtinLTDecimalSig{base}",
    },
    SignatureRecipe {
        signature: "LTString",
        constructor: "&builtinLTStringSig{base}",
    },
    SignatureRecipe {
        signature: "LTTime",
        constructor: "&builtinLTTimeSig{base}",
    },
    SignatureRecipe {
        signature: "LTDuration",
        constructor: "&builtinLTDurationSig{base}",
    },
    SignatureRecipe {
        signature: "LTJson",
        constructor: "&builtinLTJSONSig{base}",
    },
    SignatureRecipe {
        signature: "LEInt",
        constructor: "&builtinLEIntSig{base}",
    },
    SignatureRecipe {
        signature: "LEReal",
        constructor: "&builtinLERealSig{base}",
    },
    SignatureRecipe {
        signature: "LEDecimal",
        constructor: "&builtinLEDecimalSig{base}",
    },
    SignatureRecipe {
        signature: "LEString",
        constructor: "&builtinLEStringSig{base}",
    },
    SignatureRecipe {
        signature: "LETime",
        constructor: "&builtinLETimeSig{base}",
    },
    SignatureRecipe {
        signature: "LEDuration",
        constructor: "&builtinLEDurationSig{base}",
    },
    SignatureRecipe {
        signature: "LEJson",
        constructor: "&builtinLEJSONSig{base}",
    },
    SignatureRecipe {
        signature: "GTInt",
        constructor: "&builtinGTIntSig{base}",
    },
    SignatureRecipe {
        signature: "GTReal",
        constructor: "&builtinGTRealSig{base}",
    },
    SignatureRecipe {
        signature: "GTDecimal",
        constructor: "&builtinGTDecimalSig{base}",
    },
    SignatureRecipe {
        signature: "GTString",
        constructor: "&builtinGTStringSig{base}",
    },
    SignatureRecipe {
        signature: "GTTime",
        constructor: "&builtinGTTimeSig{base}",
    },
    SignatureRecipe {
        signature: "GTDuration",
        constructor: "&builtinGTDurationSig{base}",
    },
    SignatureRecipe {
        signature: "GTJson",
        constructor: "&builtinGTJSONSig{base}",
    },
    SignatureRecipe {
        signature: "GreatestInt",
        constructor: "&builtinGreatestIntSig{base}",
    },
    SignatureRecipe {
        signature: "GreatestReal",
        constructor: "&builtinGreatestRealSig{base}",
    },
    SignatureRecipe {
        signature: "GreatestDecimal",
        constructor: "&builtinGreatestDecimalSig{base}",
    },
    SignatureRecipe {
        signature: "GreatestString",
        constructor: "&builtinGreatestStringSig{base}",
    },
    SignatureRecipe {
        signature: "GreatestTime",
        constructor: "&builtinGreatestTimeSig{base, false}",
    },
    SignatureRecipe {
        signature: "GreatestDate",
        constructor: "&builtinGreatestTimeSig{base, true}",
    },
    SignatureRecipe {
        signature: "GreatestCmpStringAsTime",
        constructor: "&builtinGreatestCmpStringAsTimeSig{base, false}",
    },
    SignatureRecipe {
        signature: "GreatestCmpStringAsDate",
        constructor: "&builtinGreatestCmpStringAsTimeSig{base, true}",
    },
    SignatureRecipe {
        signature: "GreatestDuration",
        constructor: "&builtinGreatestDurationSig{base}",
    },
    SignatureRecipe {
        signature: "LeastInt",
        constructor: "&builtinLeastIntSig{base}",
    },
    SignatureRecipe {
        signature: "LeastReal",
        constructor: "&builtinLeastRealSig{base}",
    },
    SignatureRecipe {
        signature: "LeastDecimal",
        constructor: "&builtinLeastDecimalSig{base}",
    },
    SignatureRecipe {
        signature: "LeastString",
        constructor: "&builtinLeastStringSig{base}",
    },
    SignatureRecipe {
        signature: "LeastTime",
        constructor: "&builtinLeastTimeSig{base, false}",
    },
    SignatureRecipe {
        signature: "LeastDate",
        constructor: "&builtinLeastTimeSig{base, true}",
    },
    SignatureRecipe {
        signature: "LeastCmpStringAsTime",
        constructor: "&builtinLeastCmpStringAsTimeSig{base, false}",
    },
    SignatureRecipe {
        signature: "LeastCmpStringAsDate",
        constructor: "&builtinLeastCmpStringAsTimeSig{base, true}",
    },
    SignatureRecipe {
        signature: "LeastDuration",
        constructor: "&builtinLeastDurationSig{base}",
    },
    SignatureRecipe {
        signature: "IntervalInt",
        constructor: "&builtinIntervalIntSig{base, false}",
    },
    SignatureRecipe {
        signature: "IntervalReal",
        constructor: "&builtinIntervalRealSig{base, false}",
    },
    SignatureRecipe {
        signature: "GEInt",
        constructor: "&builtinGEIntSig{base}",
    },
    SignatureRecipe {
        signature: "GEReal",
        constructor: "&builtinGERealSig{base}",
    },
    SignatureRecipe {
        signature: "GEDecimal",
        constructor: "&builtinGEDecimalSig{base}",
    },
    SignatureRecipe {
        signature: "GEString",
        constructor: "&builtinGEStringSig{base}",
    },
    SignatureRecipe {
        signature: "GETime",
        constructor: "&builtinGETimeSig{base}",
    },
    SignatureRecipe {
        signature: "GEDuration",
        constructor: "&builtinGEDurationSig{base}",
    },
    SignatureRecipe {
        signature: "GEJson",
        constructor: "&builtinGEJSONSig{base}",
    },
    SignatureRecipe {
        signature: "EQInt",
        constructor: "&builtinEQIntSig{base}",
    },
    SignatureRecipe {
        signature: "EQReal",
        constructor: "&builtinEQRealSig{base}",
    },
    SignatureRecipe {
        signature: "EQDecimal",
        constructor: "&builtinEQDecimalSig{base}",
    },
    SignatureRecipe {
        signature: "EQString",
        constructor: "&builtinEQStringSig{base}",
    },
    SignatureRecipe {
        signature: "EQTime",
        constructor: "&builtinEQTimeSig{base}",
    },
    SignatureRecipe {
        signature: "EQDuration",
        constructor: "&builtinEQDurationSig{base}",
    },
    SignatureRecipe {
        signature: "EQJson",
        constructor: "&builtinEQJSONSig{base}",
    },
    SignatureRecipe {
        signature: "NEInt",
        constructor: "&builtinNEIntSig{base}",
    },
    SignatureRecipe {
        signature: "NEReal",
        constructor: "&builtinNERealSig{base}",
    },
    SignatureRecipe {
        signature: "NEDecimal",
        constructor: "&builtinNEDecimalSig{base}",
    },
    SignatureRecipe {
        signature: "NEString",
        constructor: "&builtinNEStringSig{base}",
    },
    SignatureRecipe {
        signature: "NETime",
        constructor: "&builtinNETimeSig{base}",
    },
    SignatureRecipe {
        signature: "NEDuration",
        constructor: "&builtinNEDurationSig{base}",
    },
    SignatureRecipe {
        signature: "NEJson",
        constructor: "&builtinNEJSONSig{base}",
    },
    SignatureRecipe {
        signature: "NullEQInt",
        constructor: "&builtinNullEQIntSig{base}",
    },
    SignatureRecipe {
        signature: "NullEQReal",
        constructor: "&builtinNullEQRealSig{base}",
    },
    SignatureRecipe {
        signature: "NullEQDecimal",
        constructor: "&builtinNullEQDecimalSig{base}",
    },
    SignatureRecipe {
        signature: "NullEQString",
        constructor: "&builtinNullEQStringSig{base}",
    },
    SignatureRecipe {
        signature: "NullEQTime",
        constructor: "&builtinNullEQTimeSig{base}",
    },
    SignatureRecipe {
        signature: "NullEQDuration",
        constructor: "&builtinNullEQDurationSig{base}",
    },
    SignatureRecipe {
        signature: "NullEQJson",
        constructor: "&builtinNullEQJSONSig{base}",
    },
    SignatureRecipe {
        signature: "PlusReal",
        constructor: "&builtinArithmeticPlusRealSig{base}",
    },
    SignatureRecipe {
        signature: "PlusDecimal",
        constructor: "&builtinArithmeticPlusDecimalSig{base}",
    },
    SignatureRecipe {
        signature: "PlusInt",
        constructor: "&builtinArithmeticPlusIntSig{base}",
    },
    SignatureRecipe {
        signature: "MinusReal",
        constructor: "&builtinArithmeticMinusRealSig{base}",
    },
    SignatureRecipe {
        signature: "MinusDecimal",
        constructor: "&builtinArithmeticMinusDecimalSig{base}",
    },
    SignatureRecipe {
        signature: "MinusInt",
        constructor: "&builtinArithmeticMinusIntSig{base}",
    },
    SignatureRecipe {
        signature: "MultiplyReal",
        constructor: "&builtinArithmeticMultiplyRealSig{base, false}",
    },
    SignatureRecipe {
        signature: "MultiplyDecimal",
        constructor: "&builtinArithmeticMultiplyDecimalSig{base}",
    },
    SignatureRecipe {
        signature: "MultiplyInt",
        constructor: "&builtinArithmeticMultiplyIntSig{base}",
    },
    SignatureRecipe {
        signature: "DivideReal",
        constructor: "&builtinArithmeticDivideRealSig{base}",
    },
    SignatureRecipe {
        signature: "DivideDecimal",
        constructor: "&builtinArithmeticDivideDecimalSig{base}",
    },
    SignatureRecipe {
        signature: "IntDivideInt",
        constructor: "&builtinArithmeticIntDivideIntSig{base}",
    },
    SignatureRecipe {
        signature: "IntDivideDecimal",
        constructor: "&builtinArithmeticIntDivideDecimalSig{base}",
    },
    SignatureRecipe {
        signature: "ModReal",
        constructor: "&builtinArithmeticModRealSig{base}",
    },
    SignatureRecipe {
        signature: "ModDecimal",
        constructor: "&builtinArithmeticModDecimalSig{base}",
    },
    SignatureRecipe {
        signature: "ModIntUnsignedUnsigned",
        constructor: "&builtinArithmeticModIntUnsignedUnsignedSig{base}",
    },
    SignatureRecipe {
        signature: "ModIntUnsignedSigned",
        constructor: "&builtinArithmeticModIntUnsignedSignedSig{base}",
    },
    SignatureRecipe {
        signature: "ModIntSignedUnsigned",
        constructor: "&builtinArithmeticModIntSignedUnsignedSig{base}",
    },
    SignatureRecipe {
        signature: "ModIntSignedSigned",
        constructor: "&builtinArithmeticModIntSignedSignedSig{base}",
    },
    SignatureRecipe {
        signature: "MultiplyIntUnsigned",
        constructor: "&builtinArithmeticMultiplyIntUnsignedSig{base}",
    },
    SignatureRecipe {
        signature: "AbsInt",
        constructor: "&builtinAbsIntSig{base}",
    },
    SignatureRecipe {
        signature: "AbsUInt",
        constructor: "&builtinAbsUIntSig{base}",
    },
    SignatureRecipe {
        signature: "AbsReal",
        constructor: "&builtinAbsRealSig{base}",
    },
    SignatureRecipe {
        signature: "AbsDecimal",
        constructor: "&builtinAbsDecSig{base}",
    },
    SignatureRecipe {
        signature: "CeilIntToDec",
        constructor: "&builtinCeilIntToDecSig{base}",
    },
    SignatureRecipe {
        signature: "CeilIntToInt",
        constructor: "&builtinCeilIntToIntSig{base}",
    },
    SignatureRecipe {
        signature: "CeilDecToInt",
        constructor: "&builtinCeilDecToIntSig{base}",
    },
    SignatureRecipe {
        signature: "CeilDecToDec",
        constructor: "&builtinCeilDecToDecSig{base}",
    },
    SignatureRecipe {
        signature: "CeilReal",
        constructor: "&builtinCeilRealSig{base}",
    },
    SignatureRecipe {
        signature: "FloorIntToDec",
        constructor: "&builtinFloorIntToDecSig{base}",
    },
    SignatureRecipe {
        signature: "FloorIntToInt",
        constructor: "&builtinFloorIntToIntSig{base}",
    },
    SignatureRecipe {
        signature: "FloorDecToInt",
        constructor: "&builtinFloorDecToIntSig{base}",
    },
    SignatureRecipe {
        signature: "FloorDecToDec",
        constructor: "&builtinFloorDecToDecSig{base}",
    },
    SignatureRecipe {
        signature: "FloorReal",
        constructor: "&builtinFloorRealSig{base}",
    },
    SignatureRecipe {
        signature: "RoundReal",
        constructor: "&builtinRoundRealSig{base}",
    },
    SignatureRecipe {
        signature: "RoundInt",
        constructor: "&builtinRoundIntSig{base}",
    },
    SignatureRecipe {
        signature: "RoundDec",
        constructor: "&builtinRoundDecSig{base}",
    },
    SignatureRecipe {
        signature: "RoundWithFracReal",
        constructor: "&builtinRoundWithFracRealSig{base}",
    },
    SignatureRecipe {
        signature: "RoundWithFracInt",
        constructor: "&builtinRoundWithFracIntSig{base}",
    },
    SignatureRecipe {
        signature: "RoundWithFracDec",
        constructor: "&builtinRoundWithFracDecSig{base}",
    },
    SignatureRecipe {
        signature: "Log1Arg",
        constructor: "&builtinLog1ArgSig{base}",
    },
    SignatureRecipe {
        signature: "Log2Args",
        constructor: "&builtinLog2ArgsSig{base}",
    },
    SignatureRecipe {
        signature: "Log2",
        constructor: "&builtinLog2Sig{base}",
    },
    SignatureRecipe {
        signature: "Log10",
        constructor: "&builtinLog10Sig{base}",
    },
    SignatureRecipe {
        signature: "RandWithSeedFirstGen",
        constructor: "&builtinRandWithSeedFirstGenSig{base}",
    },
    SignatureRecipe {
        signature: "Pow",
        constructor: "&builtinPowSig{base}",
    },
    SignatureRecipe {
        signature: "Conv",
        constructor: "&builtinConvSig{base}",
    },
    SignatureRecipe {
        signature: "CRC32",
        constructor: "&builtinCRC32Sig{base}",
    },
    SignatureRecipe {
        signature: "Sign",
        constructor: "&builtinSignSig{base}",
    },
    SignatureRecipe {
        signature: "Sqrt",
        constructor: "&builtinSqrtSig{base}",
    },
    SignatureRecipe {
        signature: "Acos",
        constructor: "&builtinAcosSig{base}",
    },
    SignatureRecipe {
        signature: "Asin",
        constructor: "&builtinAsinSig{base}",
    },
    SignatureRecipe {
        signature: "Atan1Arg",
        constructor: "&builtinAtan1ArgSig{base}",
    },
    SignatureRecipe {
        signature: "Atan2Args",
        constructor: "&builtinAtan2ArgsSig{base}",
    },
    SignatureRecipe {
        signature: "Cos",
        constructor: "&builtinCosSig{base}",
    },
    SignatureRecipe {
        signature: "Cot",
        constructor: "&builtinCotSig{base}",
    },
    SignatureRecipe {
        signature: "Degrees",
        constructor: "&builtinDegreesSig{base}",
    },
    SignatureRecipe {
        signature: "Exp",
        constructor: "&builtinExpSig{base}",
    },
    SignatureRecipe {
        signature: "PI",
        constructor: "&builtinPISig{base}",
    },
    SignatureRecipe {
        signature: "Radians",
        constructor: "&builtinRadiansSig{base}",
    },
    SignatureRecipe {
        signature: "Sin",
        constructor: "&builtinSinSig{base}",
    },
    SignatureRecipe {
        signature: "Tan",
        constructor: "&builtinTanSig{base}",
    },
    SignatureRecipe {
        signature: "TruncateInt",
        constructor: "&builtinTruncateIntSig{base}",
    },
    SignatureRecipe {
        signature: "TruncateReal",
        constructor: "&builtinTruncateRealSig{base}",
    },
    SignatureRecipe {
        signature: "TruncateDecimal",
        constructor: "&builtinTruncateDecimalSig{base}",
    },
    SignatureRecipe {
        signature: "TruncateUint",
        constructor: "&builtinTruncateUintSig{base}",
    },
    SignatureRecipe {
        signature: "LogicalAnd",
        constructor: "&builtinLogicAndSig{base}",
    },
    SignatureRecipe {
        signature: "LogicalOr",
        constructor: "&builtinLogicOrSig{base}",
    },
    SignatureRecipe {
        signature: "LogicalXor",
        constructor: "&builtinLogicXorSig{base}",
    },
    SignatureRecipe {
        signature: "UnaryNotInt",
        constructor: "&builtinUnaryNotIntSig{base}",
    },
    SignatureRecipe {
        signature: "UnaryNotDecimal",
        constructor: "&builtinUnaryNotDecimalSig{base}",
    },
    SignatureRecipe {
        signature: "UnaryNotReal",
        constructor: "&builtinUnaryNotRealSig{base}",
    },
    SignatureRecipe {
        signature: "UnaryMinusInt",
        constructor: "&builtinUnaryMinusIntSig{base}",
    },
    SignatureRecipe {
        signature: "UnaryMinusReal",
        constructor: "&builtinUnaryMinusRealSig{base}",
    },
    SignatureRecipe {
        signature: "UnaryMinusDecimal",
        constructor: "&builtinUnaryMinusDecimalSig{base, false}",
    },
    SignatureRecipe {
        signature: "DecimalIsNull",
        constructor: "&builtinDecimalIsNullSig{base}",
    },
    SignatureRecipe {
        signature: "DurationIsNull",
        constructor: "&builtinDurationIsNullSig{base}",
    },
    SignatureRecipe {
        signature: "RealIsNull",
        constructor: "&builtinRealIsNullSig{base}",
    },
    SignatureRecipe {
        signature: "StringIsNull",
        constructor: "&builtinStringIsNullSig{base}",
    },
    SignatureRecipe {
        signature: "TimeIsNull",
        constructor: "&builtinTimeIsNullSig{base}",
    },
    SignatureRecipe {
        signature: "IntIsNull",
        constructor: "&builtinIntIsNullSig{base}",
    },
    SignatureRecipe {
        signature: "BitAndSig",
        constructor: "&builtinBitAndSig{base}",
    },
    SignatureRecipe {
        signature: "BitOrSig",
        constructor: "&builtinBitOrSig{base}",
    },
    SignatureRecipe {
        signature: "BitXorSig",
        constructor: "&builtinBitXorSig{base}",
    },
    SignatureRecipe {
        signature: "BitNegSig",
        constructor: "&builtinBitNegSig{base}",
    },
    SignatureRecipe {
        signature: "IntIsTrue",
        constructor: "&builtinIntIsTrueSig{base, false}",
    },
    SignatureRecipe {
        signature: "RealIsTrue",
        constructor: "&builtinRealIsTrueSig{base, false}",
    },
    SignatureRecipe {
        signature: "DecimalIsTrue",
        constructor: "&builtinDecimalIsTrueSig{base, false}",
    },
    SignatureRecipe {
        signature: "IntIsFalse",
        constructor: "&builtinIntIsFalseSig{base, false}",
    },
    SignatureRecipe {
        signature: "RealIsFalse",
        constructor: "&builtinRealIsFalseSig{base, false}",
    },
    SignatureRecipe {
        signature: "DecimalIsFalse",
        constructor: "&builtinDecimalIsFalseSig{base, false}",
    },
    SignatureRecipe {
        signature: "IntIsTrueWithNull",
        constructor: "&builtinIntIsTrueSig{base, true}",
    },
    SignatureRecipe {
        signature: "RealIsTrueWithNull",
        constructor: "&builtinRealIsTrueSig{base, true}",
    },
    SignatureRecipe {
        signature: "DecimalIsTrueWithNull",
        constructor: "&builtinDecimalIsTrueSig{base, true}",
    },
    SignatureRecipe {
        signature: "IntIsFalseWithNull",
        constructor: "&builtinIntIsFalseSig{base, true}",
    },
    SignatureRecipe {
        signature: "RealIsFalseWithNull",
        constructor: "&builtinRealIsFalseSig{base, true}",
    },
    SignatureRecipe {
        signature: "DecimalIsFalseWithNull",
        constructor: "&builtinDecimalIsFalseSig{base, true}",
    },
    SignatureRecipe {
        signature: "LeftShift",
        constructor: "&builtinLeftShiftSig{base}",
    },
    SignatureRecipe {
        signature: "RightShift",
        constructor: "&builtinRightShiftSig{base}",
    },
    SignatureRecipe {
        signature: "BitCount",
        constructor: "&builtinBitCountSig{base}",
    },
    SignatureRecipe {
        signature: "GetParamString",
        constructor: "&builtinGetParamStringSig{baseBuiltinFunc: base}",
    },
    SignatureRecipe {
        signature: "GetVar",
        constructor: "&builtinGetStringVarSig{baseBuiltinFunc: base}",
    },
    SignatureRecipe {
        signature: "SetVar",
        constructor: "&builtinSetStringVarSig{baseBuiltinFunc: base}",
    },
    SignatureRecipe {
        signature: "InInt",
        constructor: "&builtinInIntSig{baseInSig: baseInSig{baseBuiltinFunc: base}}",
    },
    SignatureRecipe {
        signature: "InReal",
        constructor: "&builtinInRealSig{baseInSig: baseInSig{baseBuiltinFunc: base}}",
    },
    SignatureRecipe {
        signature: "InDecimal",
        constructor: "&builtinInDecimalSig{baseInSig: baseInSig{baseBuiltinFunc: base}}",
    },
    SignatureRecipe {
        signature: "InString",
        constructor: "&builtinInStringSig{baseInSig: baseInSig{baseBuiltinFunc: base}}",
    },
    SignatureRecipe {
        signature: "InTime",
        constructor: "&builtinInTimeSig{baseInSig: baseInSig{baseBuiltinFunc: base}}",
    },
    SignatureRecipe {
        signature: "InDuration",
        constructor: "&builtinInDurationSig{baseInSig: baseInSig{baseBuiltinFunc: base}}",
    },
    SignatureRecipe {
        signature: "InJson",
        constructor: "&builtinInJSONSig{baseBuiltinFunc: base}",
    },
    SignatureRecipe {
        signature: "IfNullInt",
        constructor: "&builtinIfNullIntSig{base}",
    },
    SignatureRecipe {
        signature: "IfNullReal",
        constructor: "&builtinIfNullRealSig{base}",
    },
    SignatureRecipe {
        signature: "IfNullDecimal",
        constructor: "&builtinIfNullDecimalSig{base}",
    },
    SignatureRecipe {
        signature: "IfNullString",
        constructor: "&builtinIfNullStringSig{base}",
    },
    SignatureRecipe {
        signature: "IfNullTime",
        constructor: "&builtinIfNullTimeSig{base}",
    },
    SignatureRecipe {
        signature: "IfNullDuration",
        constructor: "&builtinIfNullDurationSig{base}",
    },
    SignatureRecipe {
        signature: "IfInt",
        constructor: "&builtinIfIntSig{base}",
    },
    SignatureRecipe {
        signature: "IfReal",
        constructor: "&builtinIfRealSig{base}",
    },
    SignatureRecipe {
        signature: "IfDecimal",
        constructor: "&builtinIfDecimalSig{base}",
    },
    SignatureRecipe {
        signature: "IfString",
        constructor: "&builtinIfStringSig{base}",
    },
    SignatureRecipe {
        signature: "IfTime",
        constructor: "&builtinIfTimeSig{base}",
    },
    SignatureRecipe {
        signature: "IfDuration",
        constructor: "&builtinIfDurationSig{base}",
    },
    SignatureRecipe {
        signature: "IfNullJson",
        constructor: "&builtinIfNullJSONSig{base}",
    },
    SignatureRecipe {
        signature: "IfJson",
        constructor: "&builtinIfJSONSig{base}",
    },
    SignatureRecipe {
        signature: "CaseWhenInt",
        constructor: "&builtinCaseWhenIntSig{base}",
    },
    SignatureRecipe {
        signature: "CaseWhenReal",
        constructor: "&builtinCaseWhenRealSig{base}",
    },
    SignatureRecipe {
        signature: "CaseWhenDecimal",
        constructor: "&builtinCaseWhenDecimalSig{base}",
    },
    SignatureRecipe {
        signature: "CaseWhenString",
        constructor: "&builtinCaseWhenStringSig{base}",
    },
    SignatureRecipe {
        signature: "CaseWhenTime",
        constructor: "&builtinCaseWhenTimeSig{base}",
    },
    SignatureRecipe {
        signature: "CaseWhenDuration",
        constructor: "&builtinCaseWhenDurationSig{base}",
    },
    SignatureRecipe {
        signature: "CaseWhenJson",
        constructor: "&builtinCaseWhenJSONSig{base}",
    },
    SignatureRecipe {
        signature: "Compress",
        constructor: "&builtinCompressSig{base}",
    },
    SignatureRecipe {
        signature: "MD5",
        constructor: "&builtinMD5Sig{base}",
    },
    SignatureRecipe {
        signature: "Password",
        constructor: "&builtinPasswordSig{base}",
    },
    SignatureRecipe {
        signature: "RandomBytes",
        constructor: "&builtinRandomBytesSig{base}",
    },
    SignatureRecipe {
        signature: "SHA1",
        constructor: "&builtinSHA1Sig{base}",
    },
    SignatureRecipe {
        signature: "SHA2",
        constructor: "&builtinSHA2Sig{base}",
    },
    SignatureRecipe {
        signature: "Uncompress",
        constructor: "&builtinUncompressSig{base}",
    },
    SignatureRecipe {
        signature: "UncompressedLength",
        constructor: "&builtinUncompressedLengthSig{base}",
    },
    SignatureRecipe {
        signature: "Database",
        constructor: "&builtinDatabaseSig{base}",
    },
    SignatureRecipe {
        signature: "FoundRows",
        constructor: "&builtinFoundRowsSig{baseBuiltinFunc: base}",
    },
    SignatureRecipe {
        signature: "CurrentUser",
        constructor: "&builtinCurrentUserSig{baseBuiltinFunc: base}",
    },
    SignatureRecipe {
        signature: "User",
        constructor: "&builtinUserSig{baseBuiltinFunc: base}",
    },
    SignatureRecipe {
        signature: "ConnectionID",
        constructor: "&builtinConnectionIDSig{baseBuiltinFunc: base}",
    },
    SignatureRecipe {
        signature: "LastInsertID",
        constructor: "&builtinLastInsertIDSig{baseBuiltinFunc: base}",
    },
    SignatureRecipe {
        signature: "LastInsertIDWithID",
        constructor: "&builtinLastInsertIDWithIDSig{baseBuiltinFunc: base}",
    },
    SignatureRecipe {
        signature: "Version",
        constructor: "&builtinVersionSig{base}",
    },
    SignatureRecipe {
        signature: "TiDBVersion",
        constructor: "&builtinTiDBVersionSig{base}",
    },
    SignatureRecipe {
        signature: "RowCount",
        constructor: "&builtinRowCountSig{baseBuiltinFunc: base}",
    },
    SignatureRecipe {
        signature: "Sleep",
        constructor: "&builtinSleepSig{baseBuiltinFunc: base}",
    },
    SignatureRecipe {
        signature: "Lock",
        constructor: "&builtinLockSig{baseBuiltinFunc: base}",
    },
    SignatureRecipe {
        signature: "ReleaseLock",
        constructor: "&builtinReleaseLockSig{baseBuiltinFunc: base}",
    },
    SignatureRecipe {
        signature: "DecimalAnyValue",
        constructor: "&builtinDecimalAnyValueSig{base}",
    },
    SignatureRecipe {
        signature: "DurationAnyValue",
        constructor: "&builtinDurationAnyValueSig{base}",
    },
    SignatureRecipe {
        signature: "IntAnyValue",
        constructor: "&builtinIntAnyValueSig{base}",
    },
    SignatureRecipe {
        signature: "JSONAnyValue",
        constructor: "&builtinJSONAnyValueSig{base}",
    },
    SignatureRecipe {
        signature: "RealAnyValue",
        constructor: "&builtinRealAnyValueSig{base}",
    },
    SignatureRecipe {
        signature: "StringAnyValue",
        constructor: "&builtinStringAnyValueSig{base}",
    },
    SignatureRecipe {
        signature: "TimeAnyValue",
        constructor: "&builtinTimeAnyValueSig{base}",
    },
    SignatureRecipe {
        signature: "InetAton",
        constructor: "&builtinInetAtonSig{base}",
    },
    SignatureRecipe {
        signature: "InetNtoa",
        constructor: "&builtinInetNtoaSig{base}",
    },
    SignatureRecipe {
        signature: "Inet6Aton",
        constructor: "&builtinInet6AtonSig{base}",
    },
    SignatureRecipe {
        signature: "Inet6Ntoa",
        constructor: "&builtinInet6NtoaSig{base}",
    },
    SignatureRecipe {
        signature: "IsIPv4",
        constructor: "&builtinIsIPv4Sig{base}",
    },
    SignatureRecipe {
        signature: "IsIPv4Compat",
        constructor: "&builtinIsIPv4CompatSig{base}",
    },
    SignatureRecipe {
        signature: "IsIPv4Mapped",
        constructor: "&builtinIsIPv4MappedSig{base}",
    },
    SignatureRecipe {
        signature: "IsIPv6",
        constructor: "&builtinIsIPv6Sig{base}",
    },
    SignatureRecipe {
        signature: "UUID",
        constructor: "&builtinUUIDSig{base}",
    },
    SignatureRecipe {
        signature: "UUIDv4",
        constructor: "&builtinUUIDv4Sig{base}",
    },
    SignatureRecipe {
        signature: "UUIDv7",
        constructor: "&builtinUUIDv7Sig{base}",
    },
    SignatureRecipe {
        signature: "UUIDVersion",
        constructor: "&builtinUUIDVersionSig{base}",
    },
    SignatureRecipe {
        signature: "UUIDTimestamp",
        constructor: "&builtinUUIDTimestampSig{base}",
    },
    SignatureRecipe {
        signature: "LikeSig",
        constructor: "&builtinLikeSig{baseBuiltinFunc: base}",
    },
    SignatureRecipe {
        signature: "IlikeSig",
        constructor: "&builtinIlikeSig{baseBuiltinFunc: base}",
    },
    SignatureRecipe {
        signature: "RegexpSig",
        constructor: "&builtinRegexpLikeFuncSig{regexpBaseFuncSig{baseBuiltinFunc: base}}",
    },
    SignatureRecipe {
        signature: "RegexpUTF8Sig",
        constructor: "&builtinRegexpLikeFuncSig{regexpBaseFuncSig{baseBuiltinFunc: base}}",
    },
    SignatureRecipe {
        signature: "RegexpLikeSig",
        constructor: "&builtinRegexpLikeFuncSig{regexpBaseFuncSig{baseBuiltinFunc: base}}",
    },
    SignatureRecipe {
        signature: "RegexpSubstrSig",
        constructor: "&builtinRegexpSubstrFuncSig{regexpBaseFuncSig{baseBuiltinFunc: base}}",
    },
    SignatureRecipe {
        signature: "RegexpInStrSig",
        constructor: "&builtinRegexpInStrFuncSig{regexpBaseFuncSig{baseBuiltinFunc: base}}",
    },
    SignatureRecipe {
        signature: "RegexpReplaceSig",
        constructor: "&builtinRegexpReplaceFuncSig{regexpBaseFuncSig: regexpBaseFuncSig{baseBuiltinFunc: base}}",
    },
    SignatureRecipe {
        signature: "JsonExtractSig",
        constructor: "&builtinJSONExtractSig{base}",
    },
    SignatureRecipe {
        signature: "JsonUnquoteSig",
        constructor: "&builtinJSONUnquoteSig{base}",
    },
    SignatureRecipe {
        signature: "JsonTypeSig",
        constructor: "&builtinJSONTypeSig{base}",
    },
    SignatureRecipe {
        signature: "JsonSetSig",
        constructor: "&builtinJSONSetSig{base}",
    },
    SignatureRecipe {
        signature: "JsonInsertSig",
        constructor: "&builtinJSONInsertSig{base}",
    },
    SignatureRecipe {
        signature: "JsonReplaceSig",
        constructor: "&builtinJSONReplaceSig{base}",
    },
    SignatureRecipe {
        signature: "JsonRemoveSig",
        constructor: "&builtinJSONRemoveSig{base}",
    },
    SignatureRecipe {
        signature: "JsonMergeSig",
        constructor: "&builtinJSONMergeSig{base}",
    },
    SignatureRecipe {
        signature: "JsonObjectSig",
        constructor: "&builtinJSONObjectSig{base}",
    },
    SignatureRecipe {
        signature: "JsonArraySig",
        constructor: "&builtinJSONArraySig{base}",
    },
    SignatureRecipe {
        signature: "JsonValidJsonSig",
        constructor: "&builtinJSONValidJSONSig{base}",
    },
    SignatureRecipe {
        signature: "JsonContainsSig",
        constructor: "&builtinJSONContainsSig{base}",
    },
    SignatureRecipe {
        signature: "JsonArrayAppendSig",
        constructor: "&builtinJSONArrayAppendSig{base}",
    },
    SignatureRecipe {
        signature: "JsonArrayInsertSig",
        constructor: "&builtinJSONArrayInsertSig{base}",
    },
    SignatureRecipe {
        signature: "JsonMergePatchSig",
        constructor: "&builtinJSONMergePatchSig{base}",
    },
    SignatureRecipe {
        signature: "JsonMergePreserveSig",
        constructor: "&builtinJSONMergeSig{base}",
    },
    SignatureRecipe {
        signature: "JsonContainsPathSig",
        constructor: "&builtinJSONContainsPathSig{base}",
    },
    SignatureRecipe {
        signature: "JsonQuoteSig",
        constructor: "&builtinJSONQuoteSig{base}",
    },
    SignatureRecipe {
        signature: "JsonSearchSig",
        constructor: "&builtinJSONSearchSig{base}",
    },
    SignatureRecipe {
        signature: "JsonStorageSizeSig",
        constructor: "&builtinJSONStorageSizeSig{base}",
    },
    SignatureRecipe {
        signature: "JsonDepthSig",
        constructor: "&builtinJSONDepthSig{base}",
    },
    SignatureRecipe {
        signature: "JsonKeysSig",
        constructor: "&builtinJSONKeysSig{base}",
    },
    SignatureRecipe {
        signature: "JsonLengthSig",
        constructor: "&builtinJSONLengthSig{base}",
    },
    SignatureRecipe {
        signature: "JsonKeys2ArgsSig",
        constructor: "&builtinJSONKeys2ArgsSig{base}",
    },
    SignatureRecipe {
        signature: "JsonValidStringSig",
        constructor: "&builtinJSONValidStringSig{base}",
    },
    SignatureRecipe {
        signature: "JsonValidOthersSig",
        constructor: "&builtinJSONValidOthersSig{base}",
    },
    SignatureRecipe {
        signature: "JsonMemberOfSig",
        constructor: "&builtinJSONMemberOfSig{base}",
    },
    SignatureRecipe {
        signature: "DateFormatSig",
        constructor: "&builtinDateFormatSig{base}",
    },
    SignatureRecipe {
        signature: "DateDiff",
        constructor: "&builtinDateDiffSig{base}",
    },
    SignatureRecipe {
        signature: "NullTimeDiff",
        constructor: "&builtinNullTimeDiffSig{base}",
    },
    SignatureRecipe {
        signature: "TimeStringTimeDiff",
        constructor: "&builtinTimeStringTimeDiffSig{base}",
    },
    SignatureRecipe {
        signature: "DurationStringTimeDiff",
        constructor: "&builtinDurationStringTimeDiffSig{base}",
    },
    SignatureRecipe {
        signature: "DurationDurationTimeDiff",
        constructor: "&builtinDurationDurationTimeDiffSig{base}",
    },
    SignatureRecipe {
        signature: "StringTimeTimeDiff",
        constructor: "&builtinStringTimeTimeDiffSig{base}",
    },
    SignatureRecipe {
        signature: "StringDurationTimeDiff",
        constructor: "&builtinStringDurationTimeDiffSig{base}",
    },
    SignatureRecipe {
        signature: "StringStringTimeDiff",
        constructor: "&builtinStringStringTimeDiffSig{base}",
    },
    SignatureRecipe {
        signature: "TimeTimeTimeDiff",
        constructor: "&builtinTimeTimeTimeDiffSig{base}",
    },
    SignatureRecipe {
        signature: "Date",
        constructor: "&builtinDateSig{base}",
    },
    SignatureRecipe {
        signature: "Hour",
        constructor: "&builtinHourSig{base}",
    },
    SignatureRecipe {
        signature: "Minute",
        constructor: "&builtinMinuteSig{base}",
    },
    SignatureRecipe {
        signature: "Second",
        constructor: "&builtinSecondSig{base}",
    },
    SignatureRecipe {
        signature: "MicroSecond",
        constructor: "&builtinMicroSecondSig{base}",
    },
    SignatureRecipe {
        signature: "Month",
        constructor: "&builtinMonthSig{base}",
    },
    SignatureRecipe {
        signature: "MonthName",
        constructor: "&builtinMonthNameSig{base}",
    },
    SignatureRecipe {
        signature: "NowWithArg",
        constructor: "&builtinNowWithArgSig{base}",
    },
    SignatureRecipe {
        signature: "NowWithoutArg",
        constructor: "&builtinNowWithoutArgSig{base}",
    },
    SignatureRecipe {
        signature: "DayName",
        constructor: "&builtinDayNameSig{base}",
    },
    SignatureRecipe {
        signature: "DayOfMonth",
        constructor: "&builtinDayOfMonthSig{base}",
    },
    SignatureRecipe {
        signature: "DayOfWeek",
        constructor: "&builtinDayOfWeekSig{base}",
    },
    SignatureRecipe {
        signature: "DayOfYear",
        constructor: "&builtinDayOfYearSig{base}",
    },
    SignatureRecipe {
        signature: "WeekWithMode",
        constructor: "&builtinWeekWithModeSig{base}",
    },
    SignatureRecipe {
        signature: "WeekWithoutMode",
        constructor: "&builtinWeekWithoutModeSig{base}",
    },
    SignatureRecipe {
        signature: "WeekDay",
        constructor: "&builtinWeekDaySig{base}",
    },
    SignatureRecipe {
        signature: "WeekOfYear",
        constructor: "&builtinWeekOfYearSig{base}",
    },
    SignatureRecipe {
        signature: "Year",
        constructor: "&builtinYearSig{base}",
    },
    SignatureRecipe {
        signature: "YearWeekWithMode",
        constructor: "&builtinYearWeekWithModeSig{base}",
    },
    SignatureRecipe {
        signature: "YearWeekWithoutMode",
        constructor: "&builtinYearWeekWithoutModeSig{base}",
    },
    SignatureRecipe {
        signature: "GetFormat",
        constructor: "&builtinGetFormatSig{base}",
    },
    SignatureRecipe {
        signature: "SysDateWithFsp",
        constructor: "&builtinSysDateWithFspSig{base}",
    },
    SignatureRecipe {
        signature: "SysDateWithoutFsp",
        constructor: "&builtinSysDateWithoutFspSig{base}",
    },
    SignatureRecipe {
        signature: "CurrentDate",
        constructor: "&builtinCurrentDateSig{base}",
    },
    SignatureRecipe {
        signature: "CurrentTime0Arg",
        constructor: "&builtinCurrentTime0ArgSig{base}",
    },
    SignatureRecipe {
        signature: "CurrentTime1Arg",
        constructor: "&builtinCurrentTime1ArgSig{base}",
    },
    SignatureRecipe {
        signature: "Time",
        constructor: "&builtinTimeSig{base}",
    },
    SignatureRecipe {
        signature: "UTCDate",
        constructor: "&builtinUTCDateSig{base}",
    },
    SignatureRecipe {
        signature: "UTCTimestampWithArg",
        constructor: "&builtinUTCTimestampWithArgSig{base}",
    },
    SignatureRecipe {
        signature: "UTCTimestampWithoutArg",
        constructor: "&builtinUTCTimestampWithoutArgSig{base}",
    },
    SignatureRecipe {
        signature: "AddDatetimeAndDuration",
        constructor: "&builtinAddDatetimeAndDurationSig{base}",
    },
    SignatureRecipe {
        signature: "AddDatetimeAndString",
        constructor: "&builtinAddDatetimeAndStringSig{base}",
    },
    SignatureRecipe {
        signature: "AddTimeDateTimeNull",
        constructor: "&builtinAddTimeDateTimeNullSig{base}",
    },
    SignatureRecipe {
        signature: "AddStringAndDuration",
        constructor: "&builtinAddStringAndDurationSig{base}",
    },
    SignatureRecipe {
        signature: "AddStringAndString",
        constructor: "&builtinAddStringAndStringSig{base}",
    },
    SignatureRecipe {
        signature: "AddTimeStringNull",
        constructor: "&builtinAddTimeStringNullSig{base}",
    },
    SignatureRecipe {
        signature: "AddDurationAndDuration",
        constructor: "&builtinAddDurationAndDurationSig{base}",
    },
    SignatureRecipe {
        signature: "AddDurationAndString",
        constructor: "&builtinAddDurationAndStringSig{base}",
    },
    SignatureRecipe {
        signature: "AddTimeDurationNull",
        constructor: "&builtinAddTimeDurationNullSig{base}",
    },
    SignatureRecipe {
        signature: "AddDateAndDuration",
        constructor: "&builtinAddDateAndDurationSig{base}",
    },
    SignatureRecipe {
        signature: "AddDateAndString",
        constructor: "&builtinAddDateAndStringSig{base}",
    },
    SignatureRecipe {
        signature: "SubDatetimeAndDuration",
        constructor: "&builtinSubDatetimeAndDurationSig{base}",
    },
    SignatureRecipe {
        signature: "SubDatetimeAndString",
        constructor: "&builtinSubDatetimeAndStringSig{base}",
    },
    SignatureRecipe {
        signature: "SubTimeDateTimeNull",
        constructor: "&builtinSubTimeDateTimeNullSig{base}",
    },
    SignatureRecipe {
        signature: "SubStringAndDuration",
        constructor: "&builtinSubStringAndDurationSig{base}",
    },
    SignatureRecipe {
        signature: "SubStringAndString",
        constructor: "&builtinSubStringAndStringSig{base}",
    },
    SignatureRecipe {
        signature: "SubTimeStringNull",
        constructor: "&builtinSubTimeStringNullSig{base}",
    },
    SignatureRecipe {
        signature: "SubDurationAndDuration",
        constructor: "&builtinSubDurationAndDurationSig{base}",
    },
    SignatureRecipe {
        signature: "SubDurationAndString",
        constructor: "&builtinSubDurationAndStringSig{base}",
    },
    SignatureRecipe {
        signature: "SubTimeDurationNull",
        constructor: "&builtinSubTimeDurationNullSig{base}",
    },
    SignatureRecipe {
        signature: "SubDateAndDuration",
        constructor: "&builtinSubDateAndDurationSig{base}",
    },
    SignatureRecipe {
        signature: "SubDateAndString",
        constructor: "&builtinSubDateAndStringSig{base}",
    },
    SignatureRecipe {
        signature: "UnixTimestampCurrent",
        constructor: "&builtinUnixTimestampCurrentSig{base}",
    },
    SignatureRecipe {
        signature: "UnixTimestampInt",
        constructor: "&builtinUnixTimestampIntSig{base}",
    },
    SignatureRecipe {
        signature: "UnixTimestampDec",
        constructor: "&builtinUnixTimestampDecSig{base}",
    },
    SignatureRecipe {
        signature: "MakeDate",
        constructor: "&builtinMakeDateSig{base}",
    },
    SignatureRecipe {
        signature: "MakeTime",
        constructor: "&builtinMakeTimeSig{base}",
    },
    SignatureRecipe {
        signature: "PeriodAdd",
        constructor: "&builtinPeriodAddSig{base}",
    },
    SignatureRecipe {
        signature: "PeriodDiff",
        constructor: "&builtinPeriodDiffSig{base}",
    },
    SignatureRecipe {
        signature: "Quarter",
        constructor: "&builtinQuarterSig{base}",
    },
    SignatureRecipe {
        signature: "SecToTime",
        constructor: "&builtinSecToTimeSig{base}",
    },
    SignatureRecipe {
        signature: "TimeToSec",
        constructor: "&builtinTimeToSecSig{base}",
    },
    SignatureRecipe {
        signature: "TimestampAdd",
        constructor: "&builtinTimestampAddSig{base}",
    },
    SignatureRecipe {
        signature: "ToDays",
        constructor: "&builtinToDaysSig{base}",
    },
    SignatureRecipe {
        signature: "ToSeconds",
        constructor: "&builtinToSecondsSig{base}",
    },
    SignatureRecipe {
        signature: "UTCTimeWithArg",
        constructor: "&builtinUTCTimeWithArgSig{base}",
    },
    SignatureRecipe {
        signature: "UTCTimeWithoutArg",
        constructor: "&builtinUTCTimeWithoutArgSig{base}",
    },
    SignatureRecipe {
        signature: "LastDay",
        constructor: "&builtinLastDaySig{base}",
    },
    SignatureRecipe {
        signature: "StrToDateDate",
        constructor: "&builtinStrToDateDateSig{base}",
    },
    SignatureRecipe {
        signature: "StrToDateDatetime",
        constructor: "&builtinStrToDateDatetimeSig{base}",
    },
    SignatureRecipe {
        signature: "StrToDateDuration",
        constructor: "&builtinStrToDateDurationSig{base}",
    },
    SignatureRecipe {
        signature: "FromUnixTime1Arg",
        constructor: "&builtinFromUnixTime1ArgSig{base}",
    },
    SignatureRecipe {
        signature: "FromUnixTime2Arg",
        constructor: "&builtinFromUnixTime2ArgSig{base}",
    },
    SignatureRecipe {
        signature: "ExtractDatetimeFromString",
        constructor: "&builtinExtractDatetimeFromStringSig{base}",
    },
    SignatureRecipe {
        signature: "ExtractDatetime",
        constructor: "&builtinExtractDatetimeSig{base}",
    },
    SignatureRecipe {
        signature: "ExtractDuration",
        constructor: "&builtinExtractDurationSig{base}",
    },
    SignatureRecipe {
        signature: "AddDateStringString",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateStringInt",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateStringReal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateStringDecimal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateIntString",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateIntInt",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateIntReal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateIntDecimal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateRealString",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateRealInt",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateRealReal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateRealDecimal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateDecimalString",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateDecimalInt",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateDecimalReal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateDecimalDecimal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateDatetimeString",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateDatetimeInt",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateDatetimeReal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateDatetimeDecimal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateDurationString",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateDurationInt",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateDurationReal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateDurationDecimal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateDurationStringDatetime",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateDurationIntDatetime",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateDurationRealDatetime",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "AddDateDurationDecimalDatetime",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.AddDate, 3, 3}, addTime, addDuration, setAdd}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateStringString",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateStringInt",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateStringReal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateStringDecimal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateIntString",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateIntInt",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateIntReal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateIntDecimal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateRealString",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateRealInt",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateRealReal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateRealDecimal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateDecimalString",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateDecimalInt",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateDecimalReal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateDecimalDecimal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateDatetimeString",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateDatetimeInt",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateDatetimeReal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateDatetimeDecimal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateDurationString",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateDurationInt",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateDurationReal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateDurationDecimal",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateDurationStringDatetime",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateDurationIntDatetime",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateDurationRealDatetime",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "SubDateDurationDecimalDatetime",
        constructor: "(&addSubDateFunctionClass{baseFunctionClass{ast.SubDate, 3, 3}, subTime, subDuration, setSub}).getFunction(ctx, args)",
    },
    SignatureRecipe {
        signature: "FromDays",
        constructor: "&builtinFromDaysSig{base}",
    },
    SignatureRecipe {
        signature: "TimeFormat",
        constructor: "&builtinTimeFormatSig{base}",
    },
    SignatureRecipe {
        signature: "TimestampDiff",
        constructor: "&builtinTimestampDiffSig{base}",
    },
    SignatureRecipe {
        signature: "BitLength",
        constructor: "&builtinBitLengthSig{base}",
    },
    SignatureRecipe {
        signature: "Bin",
        constructor: "&builtinBinSig{base}",
    },
    SignatureRecipe {
        signature: "ASCII",
        constructor: "&builtinASCIISig{base}",
    },
    SignatureRecipe {
        signature: "Char",
        constructor: "&builtinCharSig{base}",
    },
    SignatureRecipe {
        signature: "CharLengthUTF8",
        constructor: "&builtinCharLengthUTF8Sig{base}",
    },
    SignatureRecipe {
        signature: "CharLength",
        constructor: "&builtinCharLengthBinarySig{base}",
    },
    SignatureRecipe {
        signature: "Concat",
        constructor: "&builtinConcatSig{base, maxAllowedPacket}",
    },
    SignatureRecipe {
        signature: "ConcatWS",
        constructor: "&builtinConcatWSSig{base, maxAllowedPacket}",
    },
    SignatureRecipe {
        signature: "Convert",
        constructor: "&builtinConvertSig{base}",
    },
    SignatureRecipe {
        signature: "Elt",
        constructor: "&builtinEltSig{base}",
    },
    SignatureRecipe {
        signature: "ExportSet3Arg",
        constructor: "&builtinExportSet3ArgSig{base}",
    },
    SignatureRecipe {
        signature: "ExportSet4Arg",
        constructor: "&builtinExportSet4ArgSig{base}",
    },
    SignatureRecipe {
        signature: "ExportSet5Arg",
        constructor: "&builtinExportSet5ArgSig{base}",
    },
    SignatureRecipe {
        signature: "FieldInt",
        constructor: "&builtinFieldIntSig{base}",
    },
    SignatureRecipe {
        signature: "FieldReal",
        constructor: "&builtinFieldRealSig{base}",
    },
    SignatureRecipe {
        signature: "FieldString",
        constructor: "&builtinFieldStringSig{base}",
    },
    SignatureRecipe {
        signature: "FindInSet",
        constructor: "&builtinFindInSetSig{baseBuiltinFunc: base}",
    },
    SignatureRecipe {
        signature: "Format",
        constructor: "&builtinFormatSig{base}",
    },
    SignatureRecipe {
        signature: "FormatWithLocale",
        constructor: "&builtinFormatWithLocaleSig{base}",
    },
    SignatureRecipe {
        signature: "FromBase64",
        constructor: "&builtinFromBase64Sig{base, maxAllowedPacket}",
    },
    SignatureRecipe {
        signature: "HexIntArg",
        constructor: "&builtinHexIntArgSig{base}",
    },
    SignatureRecipe {
        signature: "HexStrArg",
        constructor: "&builtinHexStrArgSig{base}",
    },
    SignatureRecipe {
        signature: "InsertUTF8",
        constructor: "&builtinInsertUTF8Sig{base, maxAllowedPacket}",
    },
    SignatureRecipe {
        signature: "Insert",
        constructor: "&builtinInsertSig{base, maxAllowedPacket}",
    },
    SignatureRecipe {
        signature: "InstrUTF8",
        constructor: "&builtinInstrUTF8Sig{base}",
    },
    SignatureRecipe {
        signature: "Instr",
        constructor: "&builtinInstrSig{base}",
    },
    SignatureRecipe {
        signature: "LTrim",
        constructor: "&builtinLTrimSig{base}",
    },
    SignatureRecipe {
        signature: "LeftUTF8",
        constructor: "&builtinLeftUTF8Sig{base}",
    },
    SignatureRecipe {
        signature: "Left",
        constructor: "&builtinLeftSig{base}",
    },
    SignatureRecipe {
        signature: "Length",
        constructor: "&builtinLengthSig{base}",
    },
    SignatureRecipe {
        signature: "Locate2ArgsUTF8",
        constructor: "&builtinLocate2ArgsUTF8Sig{base}",
    },
    SignatureRecipe {
        signature: "Locate3ArgsUTF8",
        constructor: "&builtinLocate3ArgsUTF8Sig{base}",
    },
    SignatureRecipe {
        signature: "Locate2Args",
        constructor: "&builtinLocate2ArgsSig{base}",
    },
    SignatureRecipe {
        signature: "Locate3Args",
        constructor: "&builtinLocate3ArgsSig{base}",
    },
    SignatureRecipe {
        signature: "Lower",
        constructor: "&builtinLowerSig{base}",
    },
    SignatureRecipe {
        signature: "LowerUTF8",
        constructor: "&builtinLowerUTF8Sig{base}",
    },
    SignatureRecipe {
        signature: "LpadUTF8",
        constructor: "&builtinLpadUTF8Sig{base, maxAllowedPacket}",
    },
    SignatureRecipe {
        signature: "Lpad",
        constructor: "&builtinLpadSig{base, maxAllowedPacket}",
    },
    SignatureRecipe {
        signature: "MakeSet",
        constructor: "&builtinMakeSetSig{base}",
    },
    SignatureRecipe {
        signature: "OctInt",
        constructor: "&builtinOctIntSig{base}",
    },
    SignatureRecipe {
        signature: "OctString",
        constructor: "&builtinOctStringSig{base}",
    },
    SignatureRecipe {
        signature: "Ord",
        constructor: "&builtinOrdSig{base}",
    },
    SignatureRecipe {
        signature: "Quote",
        constructor: "&builtinQuoteSig{base}",
    },
    SignatureRecipe {
        signature: "RTrim",
        constructor: "&builtinRTrimSig{base}",
    },
    SignatureRecipe {
        signature: "Repeat",
        constructor: "&builtinRepeatSig{base, maxAllowedPacket}",
    },
    SignatureRecipe {
        signature: "Replace",
        constructor: "&builtinReplaceSig{base}",
    },
    SignatureRecipe {
        signature: "ReverseUTF8",
        constructor: "&builtinReverseUTF8Sig{base}",
    },
    SignatureRecipe {
        signature: "Reverse",
        constructor: "&builtinReverseSig{base}",
    },
    SignatureRecipe {
        signature: "RightUTF8",
        constructor: "&builtinRightUTF8Sig{base}",
    },
    SignatureRecipe {
        signature: "Right",
        constructor: "&builtinRightSig{base}",
    },
    SignatureRecipe {
        signature: "RpadUTF8",
        constructor: "&builtinRpadUTF8Sig{base, maxAllowedPacket}",
    },
    SignatureRecipe {
        signature: "Rpad",
        constructor: "&builtinRpadSig{base, maxAllowedPacket}",
    },
    SignatureRecipe {
        signature: "Space",
        constructor: "&builtinSpaceSig{base, maxAllowedPacket}",
    },
    SignatureRecipe {
        signature: "Strcmp",
        constructor: "&builtinStrcmpSig{base}",
    },
    SignatureRecipe {
        signature: "Substring2ArgsUTF8",
        constructor: "&builtinSubstring2ArgsUTF8Sig{base}",
    },
    SignatureRecipe {
        signature: "Substring3ArgsUTF8",
        constructor: "&builtinSubstring3ArgsUTF8Sig{base}",
    },
    SignatureRecipe {
        signature: "Substring2Args",
        constructor: "&builtinSubstring2ArgsSig{base}",
    },
    SignatureRecipe {
        signature: "Substring3Args",
        constructor: "&builtinSubstring3ArgsSig{base}",
    },
    SignatureRecipe {
        signature: "SubstringIndex",
        constructor: "&builtinSubstringIndexSig{base}",
    },
    SignatureRecipe {
        signature: "ToBase64",
        constructor: "&builtinToBase64Sig{base, maxAllowedPacket}",
    },
    SignatureRecipe {
        signature: "Trim1Arg",
        constructor: "&builtinTrim1ArgSig{base}",
    },
    SignatureRecipe {
        signature: "Trim2Args",
        constructor: "&builtinTrim2ArgsSig{base}",
    },
    SignatureRecipe {
        signature: "Trim3Args",
        constructor: "&builtinTrim3ArgsSig{base}",
    },
    SignatureRecipe {
        signature: "UnHex",
        constructor: "&builtinUnHexSig{base}",
    },
    SignatureRecipe {
        signature: "Upper",
        constructor: "&builtinUpperSig{base}",
    },
    SignatureRecipe {
        signature: "UpperUTF8",
        constructor: "&builtinUpperUTF8Sig{base}",
    },
    SignatureRecipe {
        signature: "ToBinary",
        constructor: "&builtinInternalToBinarySig{base}",
    },
    SignatureRecipe {
        signature: "FromBinary",
        constructor: "&builtinInternalFromBinarySig{base, false}",
    },
    SignatureRecipe {
        signature: "CastVectorFloat32AsString",
        constructor: "&builtinCastVectorFloat32AsStringSig{base}",
    },
    SignatureRecipe {
        signature: "CastVectorFloat32AsVectorFloat32",
        constructor: "&builtinCastVectorFloat32AsVectorFloat32Sig{base}",
    },
    SignatureRecipe {
        signature: "LTVectorFloat32",
        constructor: "&builtinLTVectorFloat32Sig{base}",
    },
    SignatureRecipe {
        signature: "LEVectorFloat32",
        constructor: "&builtinLEVectorFloat32Sig{base}",
    },
    SignatureRecipe {
        signature: "GTVectorFloat32",
        constructor: "&builtinGTVectorFloat32Sig{base}",
    },
    SignatureRecipe {
        signature: "GEVectorFloat32",
        constructor: "&builtinGEVectorFloat32Sig{base}",
    },
    SignatureRecipe {
        signature: "NEVectorFloat32",
        constructor: "&builtinNEVectorFloat32Sig{base}",
    },
    SignatureRecipe {
        signature: "EQVectorFloat32",
        constructor: "&builtinEQVectorFloat32Sig{base}",
    },
    SignatureRecipe {
        signature: "NullEQVectorFloat32",
        constructor: "&builtinNullEQVectorFloat32Sig{base}",
    },
    SignatureRecipe {
        signature: "VectorFloat32AnyValue",
        constructor: "&builtinVectorFloat32AnyValueSig{base}",
    },
    SignatureRecipe {
        signature: "VectorFloat32IsNull",
        constructor: "&builtinVectorFloat32IsNullSig{base}",
    },
    SignatureRecipe {
        signature: "VecAsTextSig",
        constructor: "&builtinVecAsTextSig{base}",
    },
    SignatureRecipe {
        signature: "VecDimsSig",
        constructor: "&builtinVecDimsSig{base}",
    },
    SignatureRecipe {
        signature: "VecL1DistanceSig",
        constructor: "&builtinVecL1DistanceSig{base}",
    },
    SignatureRecipe {
        signature: "VecL2DistanceSig",
        constructor: "&builtinVecL2DistanceSig{base}",
    },
    SignatureRecipe {
        signature: "VecNegativeInnerProductSig",
        constructor: "&builtinVecNegativeInnerProductSig{base}",
    },
    SignatureRecipe {
        signature: "VecCosineDistanceSig",
        constructor: "&builtinVecCosineDistanceSig{base}",
    },
    SignatureRecipe {
        signature: "VecL2NormSig",
        constructor: "&builtinVecL2NormSig{base}",
    },
    SignatureRecipe {
        signature: "FTSMatchWord",
        constructor: "&builtinFtsMatchWordSig{base}",
    },
    SignatureRecipe {
        signature: "FTSMatchExpression",
        constructor: "&builtinFtsMysqlMatchAgainstSig{baseBuiltinFunc: base}",
    },
];

/// 对应 Go getSignatureByPB：先构造公共 base，再按协议签名选择具体 builtin 实现。
/// 独立迁移阶段以完整 recipe 表保留全部分派关系；package 集成时由各 builtin 类型接管实例化。
pub fn getSignatureByPB(
    ctx: &BuildContext,
    sig_code: tipb::ScalarFuncSig,
    tp: &tipb::FieldType,
    args: Vec<Expression>,
) -> Result<BuiltinFuncDraft, ExpressionError> {
    let field_tp = PbTypeToFieldType(tp);
    let base = newBaseBuiltinFuncWithFieldType(field_tp, args)?;
    let max_allowed_packet = ctx.get_eval_ctx().get_max_allowed_packet();
    let signature_name = format!("{sig_code:?}");

    // Go 的 default 分支返回 ErrFunctionNotExists；查表失败时保持同样的错误边界。
    let recipe = SIGNATURE_RECIPES
        .iter()
        .find(|recipe| recipe.signature.eq_ignore_ascii_case(&signature_name))
        .ok_or_else(|| ErrFunctionNotExists.gen_with_stack_by_args("FUNCTION", &signature_name))?;

    Ok(BuiltinFuncDraft {
        base,
        signature: sig_code,
        constructor: recipe.constructor,
        max_allowed_packet,
    })
}

/// 对应 Go newDistSQLFunctionBySig：签名实例化后，在隐式 cast 前推导字符串排序规则。
pub fn newDistSQLFunctionBySig(
    ctx: &BuildContext,
    sig_code: tipb::ScalarFuncSig,
    tp: &tipb::FieldType,
    args: Vec<Expression>,
) -> Result<Expression, ExpressionError> {
    let builtin = getSignatureByPB(ctx, sig_code, tp, args)?;
    let func_name = format!("{sig_code:?}");
    let arg_types = builtin
        .base
        .args()
        .iter()
        .map(|arg| arg.get_type(ctx.get_eval_ctx()).EvalType())
        .collect::<Vec<_>>();
    let ec = deriveCollation(
        ctx,
        &func_name,
        builtin.base.args(),
        builtin.return_type().EvalType(),
        &arg_types,
    )?;

    let mut scalar = ScalarFunction::new(
        ast::new_ci_str(format!("sig_{}", builtin.constructor)),
        builtin.return_type().clone(),
        builtin,
    );
    scalar.set_coercibility(ec.coer);
    Ok(Expression::ScalarFunction(scalar))
}

/// 对应 Go PBToExprs：保持输入顺序递归转换；任一子表达式失败立即返回。
pub fn PBToExprs(
    ctx: &BuildContext,
    pb_exprs: &[tipb::Expr],
    field_types: &[types::FieldType],
) -> Result<Vec<Expression>, ExpressionError> {
    let mut expressions = Vec::with_capacity(pb_exprs.len());
    for pb_expr in pb_exprs {
        let expression = PBToExpr(ctx, pb_expr, field_types)?.ok_or_else(|| {
            errors::errorf(format!(
                "pb to expression failed, pb expression is {:?}",
                pb_expr
            ))
        })?;
        expressions.push(expression);
    }
    Ok(expressions)
}

/// 对应 Go PBToExpr：先处理字面量，再递归处理标量函数的 children。
pub fn PBToExpr(
    ctx: &BuildContext,
    expr: &tipb::Expr,
    field_types: &[types::FieldType],
) -> Result<Option<Expression>, ExpressionError> {
    let eval_ctx = ctx.get_eval_ctx();
    let literal = match expr.get_tp() {
        tipb::ExprType::ColumnRef => {
            let (_, offset) = codec::decode_int(expr.get_val())?;
            // Go 直接按协议 offset 索引；越界仍属于上游 PB 不合法。
            Some(Expression::Column(Column::new(
                offset as usize,
                field_types[offset as usize].clone(),
            )))
        }
        tipb::ExprType::Null => Some(Expression::Constant(Constant::null(mysql::TYPE_NULL))),
        tipb::ExprType::Int64 => Some(convertInt(expr.get_val(), expr.get_field_type())?),
        tipb::ExprType::Uint64 => Some(convertUint(expr.get_val(), expr.get_field_type())?),
        tipb::ExprType::String => Some(convertString(expr.get_val(), expr.get_field_type())),
        tipb::ExprType::Bytes => Some(Expression::Constant(Constant::bytes(
            expr.get_val().to_vec(),
        ))),
        tipb::ExprType::MysqlBit => Some(Expression::Constant(Constant::mysql_bit(
            expr.get_val().to_vec(),
        ))),
        tipb::ExprType::Float32 => Some(convertFloat(expr.get_val(), true)?),
        tipb::ExprType::Float64 => Some(convertFloat(expr.get_val(), false)?),
        tipb::ExprType::MysqlDecimal => {
            Some(convertDecimal(expr.get_val(), expr.get_field_type())?)
        }
        tipb::ExprType::MysqlDuration => Some(convertDuration(expr.get_val())?),
        tipb::ExprType::MysqlTime => Some(convertTime(
            expr.get_val(),
            expr.get_field_type(),
            &eval_ctx.location(),
        )?),
        tipb::ExprType::MysqlJson => Some(convertJSON(expr.get_val())?),
        tipb::ExprType::MysqlEnum => Some(convertEnum(expr.get_val(), expr.get_field_type())?),
        tipb::ExprType::TiDbVectorFloat32 => Some(convertVectorFloat32(expr.get_val())?),
        _ => None,
    };
    if literal.is_some() {
        return Ok(literal);
    }
    if expr.get_tp() != tipb::ExprType::ScalarFunc {
        panic!("should be a tipb.ExprType_ScalarFunc");
    }

    let mut args = Vec::with_capacity(expr.get_children().len());
    for child in expr.get_children() {
        if child.get_tp() == tipb::ExprType::ValueList {
            let values = decodeValueList(child.get_val())?;
            // Go 把空 ValueList 折叠为 false 常量，避免生成无参数 IN。
            if values.is_empty() {
                return Ok(Some(Expression::Constant(Constant::boolean(false))));
            }
            args.extend(values);
            continue;
        }
        let arg = PBToExpr(ctx, child, field_types)?
            .ok_or_else(|| errors::errorf("child pb expression did not produce an expression"))?;
        args.push(arg);
    }
    Ok(Some(newDistSQLFunctionBySig(
        ctx,
        expr.get_sig(),
        expr.get_field_type(),
        args,
    )?))
}

/// 对应 Go convertTime：解码 packed uint，并只对 TIMESTAMP 做 UTC 到会话时区转换。
pub fn convertTime(
    data: &[u8],
    field_type_pb: &tipb::FieldType,
    timezone: &time::Location,
) -> Result<Expression, ExpressionError> {
    let field_type = PbTypeToFieldType(field_type_pb);
    let (_, packed) = codec::decode_uint(data)?;
    let mut value = types::Time::default();
    value.SetType(field_type.GetType());
    value.SetFsp(field_type.GetDecimal() as i32);
    value
        .FromPackedUint(packed)
        .map_err(ExpressionError::external)?;
    if field_type.GetType() == mysql::TYPE_TIMESTAMP && *timezone != time::UTC {
        value
            .ConvertTimeZone(time::UTC, *timezone)
            .map_err(ExpressionError::external)?;
    }
    Ok(Expression::Constant(Constant::with_type(
        types::NewTimeDatum(value),
        field_type,
    )))
}

/// 对应 Go decodeValueList：codec.Decode 返回的每个 Datum 都包装成 Constant。
pub fn decodeValueList(data: &[u8]) -> Result<Vec<Expression>, ExpressionError> {
    if data.is_empty() {
        return Ok(Vec::new());
    }
    let list = codec::decode(data, 1)?;
    Ok(list
        .into_iter()
        .map(|value| Expression::Constant(Constant::from_datum(value)))
        .collect())
}

/// 对应 Go convertInt；解码错误保留原十六进制上下文。
pub fn convertInt(value: &[u8], tp: &tipb::FieldType) -> Result<Expression, ExpressionError> {
    let (_, decoded) = codec::decode_int(value)
        .map_err(|_| errors::errorf(format!("invalid int {:x?}", value)))?;
    Ok(Expression::Constant(Constant::with_type(
        types::NewIntDatum(decoded),
        PbTypeToFieldType(tp),
    )))
}

/// 对应 Go convertUint；有符号与无符号路径保持分离。
pub fn convertUint(value: &[u8], tp: &tipb::FieldType) -> Result<Expression, ExpressionError> {
    let (_, decoded) = codec::decode_uint(value)
        .map_err(|_| errors::errorf(format!("invalid uint {:x?}", value)))?;
    Ok(Expression::Constant(Constant::with_type(
        types::NewUintDatum(decoded),
        PbTypeToFieldType(tp),
    )))
}

/// 对应 Go convertString：同时保留协议排序规则与 flen。
pub fn convertString(value: &[u8], tp: &tipb::FieldType) -> Expression {
    let mut datum = types::Datum::default();
    datum.SetBytesAsString(
        value.to_vec(),
        collate::ProtoToCollation(tp.get_collate()),
        tp.get_flen() as u32,
    );
    Expression::Constant(Constant::with_type(datum, PbTypeToFieldType(tp)))
}

/// 对应 Go convertFloat：协议统一解码为 f64，再按来源类型写入 Datum。
pub fn convertFloat(value: &[u8], float32: bool) -> Result<Expression, ExpressionError> {
    let (_, decoded) = codec::decode_float(value)
        .map_err(|_| errors::errorf(format!("invalid float {:x?}", value)))?;
    let datum = if float32 {
        types::NewFloat32Datum(decoded as f32)
    } else {
        types::NewFloat64Datum(decoded)
    };
    Ok(Expression::Constant(Constant::with_type(
        datum,
        *types::NewFieldType(mysql::TYPE_DOUBLE),
    )))
}

/// 对应 Go convertDecimal：精度与小数位来自编码结果，而返回类型来自 PB。
pub fn convertDecimal(value: &[u8], tp: &tipb::FieldType) -> Result<Expression, ExpressionError> {
    let field_type = PbTypeToFieldType(tp);
    let (_, decimal, precision, fraction) = codec::decode_decimal(value)
        .map_err(|_| errors::errorf(format!("invalid decimal {:x?}", value)))?;
    let mut datum = types::NewDecimalDatum(decimal);
    datum.SetLength(precision);
    datum.SetFrac(fraction);
    Ok(Expression::Constant(Constant::with_type(datum, field_type)))
}

/// 对应 Go convertDuration：纳秒整数转为最大 FSP 的 MySQL Duration。
pub fn convertDuration(value: &[u8]) -> Result<Expression, ExpressionError> {
    let (_, nanos) = codec::decode_int(value)
        .map_err(|_| errors::errorf(format!("invalid duration bytes {:x?}", value)))?;
    let duration = types::Duration {
        Duration: nanos,
        Fsp: types::MaxFsp,
    };
    Ok(Expression::Constant(Constant::with_type(
        types::NewDurationDatum(duration),
        *types::NewFieldType(mysql::TYPE_DURATION),
    )))
}

/// 对应 Go convertJSON：除了解码成功，还必须校验 Datum kind。
pub fn convertJSON(value: &[u8]) -> Result<Expression, ExpressionError> {
    let (_, datum) = codec::decode_one(value)
        .map_err(|_| errors::errorf(format!("invalid json {:x?}", value)))?;
    if datum.Kind() != types::KindMysqlJSON {
        return Err(errors::errorf(format!(
            "invalid Datum.Kind() {:?}",
            datum.Kind()
        )));
    }
    Ok(Expression::Constant(Constant::with_type(
        datum,
        *types::NewFieldType(mysql::TYPE_JSON),
    )))
}

/// 对应 Go convertVectorFloat32：采用零拷贝反序列化并保留向量专用字段类型。
pub fn convertVectorFloat32(value: &[u8]) -> Result<Expression, ExpressionError> {
    let (vector, _) = types::ZeroCopyDeserializeVectorFloat32(value)
        .map_err(|_| errors::errorf(format!("invalid VectorFloat32 {:x?}", value)))?;
    Ok(Expression::Constant(Constant::with_type(
        types::NewVectorFloat32Datum(vector),
        *types::NewFieldType(mysql::TYPE_TIDB_VECTOR_FLOAT32),
    )))
}

/// 对应 Go convertEnum：协议值 0 表示空 Enum，其余值按 FieldType elems 查表。
pub fn convertEnum(value: &[u8], tp: &tipb::FieldType) -> Result<Expression, ExpressionError> {
    let (_, enum_value) = codec::decode_uint(value)
        .map_err(|_| errors::errorf(format!("invalid enum {:x?}", value)))?;
    let parsed = if enum_value == 0 {
        types::Enum::default()
    } else {
        types::ParseEnumValue(tp.get_elems(), enum_value).map_err(ExpressionError::external)?
    };
    Ok(Expression::Constant(Constant::with_type(
        types::NewMysqlEnumDatum(parsed),
        FieldTypeFromPB(tp),
    )))
}
