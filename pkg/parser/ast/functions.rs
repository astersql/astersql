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

// Function-related AST nodes and SQL restoration behavior from `functions.go`.

// 函数相关 AST 节点与 SQL 还原逻辑（对照 `functions.go`）。
//
// 覆盖普通函数调用、CAST/CONVERT、聚合函数、窗口函数，以及
// 日期字面量、TRIM、EXTRACT、JSON_MEMBER_OF 等特殊还原形态。

#![allow(non_upper_case_globals, non_snake_case)]

use std::fmt;
use std::time::Duration;

/// 表达式标志位（flag）传播子模块。
#[path = "flag.rs"]
pub mod flag;

/// AST 操作结果别名：Ok 或 AstError。
pub type AstResult<T> = Result<T, AstError>;

/// AST 还原/构造过程中的错误类型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AstError(String);

impl AstError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for AstError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for AstError {}

macro_rules! function_names {
    ($($name:ident => $value:literal),+ $(,)?) => { $(pub const $name: &str = $value;)+ };
}

// Names used by parser/planner dispatch. Values and spelling follow the Go constants.
function_names! {
    LogicAnd => "and", Cast => "cast", LogicOr => "or", GE => "ge", LE => "le", EQ => "eq",
    NE => "ne", LT => "lt", GT => "gt", Plus => "plus", Minus => "minus", And => "bitand",
    Or => "bitor", Mod => "mod", Xor => "bitxor", Div => "div", Mul => "mul", UnaryNot => "not",
    IntDiv => "intdiv", LogicXor => "xor", NullEQ => "nulleq", In => "in", Like => "like",
    Ilike => "ilike", Case => "case", Regexp => "regexp", RowFunc => "row", SetVar => "setvar",
    GetVar => "getvar", Values => "values", Coalesce => "coalesce", Greatest => "greatest",
    Least => "least", Abs => "abs", Ceil => "ceil", Floor => "floor", Rand => "rand",
    AddDate => "adddate", DateAdd => "date_add", DateSub => "date_sub", SubDate => "subdate",
    DateLiteral => "'tidb`.(dateliteral", TimeLiteral => "'tidb`.(timeliteral",
    TimestampLiteral => "'tidb`.(timestampliteral", Extract => "extract", Position => "position",
    Convert => "convert", Trim => "trim", WeightString => "weight_string",
    JSONMemberOf => "json_memberof", JSONSumCrc32 => "json_sum_crc32",
    AggFuncCount => "count", AggFuncSum => "sum", AggFuncSumInt => "sum_int", AggFuncAvg => "avg",
    AggFuncFirstRow => "firstrow", AggFuncMax => "max", AggFuncMin => "min",
    AggFuncGroupConcat => "group_concat", AggFuncBitOr => "bit_or", AggFuncBitXor => "bit_xor",
    AggFuncBitAnd => "bit_and", AggFuncVarPop => "var_pop", AggFuncVarSamp => "var_samp",
    AggFuncStddevPop => "stddev_pop", AggFuncStddevSamp => "stddev_samp",
    AggFuncJsonArrayagg => "json_arrayagg", AggFuncJsonObjectAgg => "json_objectagg",
    AggFuncApproxCountDistinct => "approx_count_distinct", AggFuncApproxPercentile => "approx_percentile",
    WindowFuncRowNumber => "row_number", WindowFuncRank => "rank", WindowFuncDenseRank => "dense_rank",
    WindowFuncCumeDist => "cume_dist", WindowFuncPercentRank => "percent_rank", WindowFuncNtile => "ntile",
    WindowFuncLead => "lead", WindowFuncLag => "lag", WindowFuncFirstValue => "first_value",
    WindowFuncLastValue => "last_value", WindowFuncNthValue => "nth_value"
}

// Less frequently referenced names remain individual constants too; this keeps
// the complete Go declaration surface without adding runtime dependencies.
function_names! {
    LeftShift => "leftshift",
    RightShift => "rightshift",
    BitNeg => "bitneg",
    UnaryPlus => "unaryplus",
    UnaryMinus => "unaryminus",
    RegexpLike => "regexp_like",
    RegexpSubstr => "regexp_substr",
    RegexpInStr => "regexp_instr",
    RegexpReplace => "regexp_replace",
    IsNull => "isnull",
    IsTruthWithoutNull => "istrue",
    IsTruthWithNull => "istrue_with_null",
    IsFalsity => "isfalse",
    BitCount => "bit_count",
    GetParam => "getparam",
    Interval => "interval",
    MaskFull => "mask_full",
    MaskPartial => "mask_partial",
    MaskNull => "mask_null",
    MaskDate => "mask_date",
    Acos => "acos",
    Asin => "asin",
    Atan => "atan",
    Atan2 => "atan2",
    Ceiling => "ceiling",
    Conv => "conv",
    Cos => "cos",
    Cot => "cot",
    CRC32 => "crc32",
    Degrees => "degrees",
    Exp => "exp",
    Ln => "ln",
    Log => "log",
    Log2 => "log2",
    Log10 => "log10",
    PI => "pi",
    Pow => "pow",
    Power => "power",
    Radians => "radians",
    Round => "round",
    Sign => "sign",
    Sin => "sin",
    Sqrt => "sqrt",
    Tan => "tan",
    Truncate => "truncate",
    AddTime => "addtime",
    ConvertTz => "convert_tz",
    Curdate => "curdate",
    CurrentDate => "current_date",
    CurrentTime => "current_time",
    CurrentTimestamp => "current_timestamp",
    Curtime => "curtime",
    Date => "date",
    DateFormat => "date_format",
    DateDiff => "datediff",
    Day => "day",
    DayName => "dayname",
    DayOfMonth => "dayofmonth",
    DayOfWeek => "dayofweek",
    DayOfYear => "dayofyear",
    FromDays => "from_days",
    FromUnixTime => "from_unixtime",
    GetFormat => "get_format",
    Hour => "hour",
    LocalTime => "localtime",
    LocalTimestamp => "localtimestamp",
    MakeDate => "makedate",
    MakeTime => "maketime",
    MicroSecond => "microsecond",
    Minute => "minute",
    Month => "month",
    MonthName => "monthname",
    Now => "now",
    PeriodAdd => "period_add",
    PeriodDiff => "period_diff",
    Quarter => "quarter",
    SecToTime => "sec_to_time",
    Second => "second",
    StrToDate => "str_to_date",
    SubTime => "subtime",
    Sysdate => "sysdate",
    Time => "time",
    TimeFormat => "time_format",
    TimeToSec => "time_to_sec",
    TimeDiff => "timediff",
    Timestamp => "timestamp",
    TimestampAdd => "timestampadd",
    TimestampDiff => "timestampdiff",
    ToDays => "to_days",
    ToSeconds => "to_seconds",
    UnixTimestamp => "unix_timestamp",
    UTCDate => "utc_date",
    UTCTime => "utc_time",
    UTCTimestamp => "utc_timestamp",
    Week => "week",
    Weekday => "weekday",
    WeekOfYear => "weekofyear",
    Year => "year",
    YearWeek => "yearweek",
    LastDay => "last_day",
    TiDBBoundedStaleness => "tidb_bounded_staleness",
    TiDBParseTso => "tidb_parse_tso",
    TiDBParseTsoLogical => "tidb_parse_tso_logical",
    TiDBCurrentTso => "tidb_current_tso",
    ASCII => "ascii",
    Bin => "bin",
    Concat => "concat",
    ConcatWS => "concat_ws",
    Elt => "elt",
    ExportSet => "export_set",
    Field => "field",
    Format => "format",
    FromBase64 => "from_base64",
    InsertFunc => "insert_func",
    Instr => "instr",
    Lcase => "lcase",
    Left => "left",
    Length => "length",
    LoadFile => "load_file",
    Locate => "locate",
    Lower => "lower",
    Lpad => "lpad",
    LTrim => "ltrim",
    MakeSet => "make_set",
    Mid => "mid",
    Oct => "oct",
    OctetLength => "octet_length",
    Ord => "ord",
    Quote => "quote",
    Repeat => "repeat",
    Replace => "replace",
    Reverse => "reverse",
    Right => "right",
    RTrim => "rtrim",
    Space => "space",
    Strcmp => "strcmp",
    Substring => "substring",
    Substr => "substr",
    SubstringIndex => "substring_index",
    ToBase64 => "to_base64",
    Translate => "translate",
    Upper => "upper",
    Ucase => "ucase",
    Hex => "hex",
    Unhex => "unhex",
    Rpad => "rpad",
    BitLength => "bit_length",
    CharFunc => "char_func",
    CharLength => "char_length",
    CharacterLength => "character_length",
    FindInSet => "find_in_set",
    Soundex => "soundex",
    Benchmark => "benchmark",
    Charset => "charset",
    Coercibility => "coercibility",
    Collation => "collation",
    ConnectionID => "connection_id",
    CurrentUser => "current_user",
    CurrentRole => "current_role",
    Database => "database",
    FoundRows => "found_rows",
    LastInsertId => "last_insert_id",
    RowCount => "row_count",
    Schema => "schema",
    SessionUser => "session_user",
    SystemUser => "system_user",
    User => "user",
    Version => "version",
    TiDBVersion => "tidb_version",
    TiDBIsDDLOwner => "tidb_is_ddl_owner",
    TiDBDecodePlan => "tidb_decode_plan",
    TiDBDecodeBinaryPlan => "tidb_decode_binary_plan",
    TiDBDecodeSQLDigests => "tidb_decode_sql_digests",
    TiDBEncodeSQLDigest => "tidb_encode_sql_digest",
    FormatBytes => "format_bytes",
    FormatNanoTime => "format_nano_time",
    CurrentResourceGroup => "current_resource_group",
    If => "if",
    Ifnull => "ifnull",
    Nullif => "nullif",
    AnyValue => "any_value",
    DefaultFunc => "default_func",
    InetAton => "inet_aton",
    InetNtoa => "inet_ntoa",
    Inet6Aton => "inet6_aton",
    Inet6Ntoa => "inet6_ntoa",
    IsFreeLock => "is_free_lock",
    IsIPv4 => "is_ipv4",
    IsIPv4Compat => "is_ipv4_compat",
    IsIPv4Mapped => "is_ipv4_mapped",
    IsIPv6 => "is_ipv6",
    IsUsedLock => "is_used_lock",
    IsUUID => "is_uuid",
    NameConst => "name_const",
    ReleaseAllLocks => "release_all_locks",
    Sleep => "sleep",
    UUID => "uuid",
    UUIDv4 => "uuid_v4",
    UUIDv7 => "uuid_v7",
    UUIDVersion => "uuid_version",
    UUIDTimestamp => "uuid_timestamp",
    UUIDShort => "uuid_short",
    UUIDToBin => "uuid_to_bin",
    BinToUUID => "bin_to_uuid",
    VitessHash => "vitess_hash",
    TiDBShard => "tidb_shard",
    TiDBRowChecksum => "tidb_row_checksum",
    GetLock => "get_lock",
    ReleaseLock => "release_lock",
    Grouping => "grouping",
    AesDecrypt => "aes_decrypt",
    AesEncrypt => "aes_encrypt",
    Compress => "compress",
    Decode => "decode",
    Encode => "encode",
    MD5 => "md5",
    PasswordFunc => "password",
    RandomBytes => "random_bytes",
    SHA1 => "sha1",
    SHA => "sha",
    SHA2 => "sha2",
    SM3 => "sm3",
    Uncompress => "uncompress",
    UncompressedLength => "uncompressed_length",
    ValidatePasswordStrength => "validate_password_strength",
    JSONType => "json_type",
    JSONExtract => "json_extract",
    JSONUnquote => "json_unquote",
    JSONArray => "json_array",
    JSONObject => "json_object",
    JSONMerge => "json_merge",
    JSONSet => "json_set",
    JSONInsert => "json_insert",
    JSONReplace => "json_replace",
    JSONRemove => "json_remove",
    JSONOverlaps => "json_overlaps",
    JSONContains => "json_contains",
    JSONContainsPath => "json_contains_path",
    JSONValid => "json_valid",
    JSONArrayAppend => "json_array_append",
    JSONArrayInsert => "json_array_insert",
    JSONMergePatch => "json_merge_patch",
    JSONMergePreserve => "json_merge_preserve",
    JSONPretty => "json_pretty",
    JSONQuote => "json_quote",
    JSONSchemaValid => "json_schema_valid",
    JSONSearch => "json_search",
    JSONStorageFree => "json_storage_free",
    JSONStorageSize => "json_storage_size",
    JSONDepth => "json_depth",
    JSONKeys => "json_keys",
    JSONLength => "json_length",
    VecDims => "vec_dims",
    VecL1Distance => "vec_l1_distance",
    VecL2Distance => "vec_l2_distance",
    VecNegativeInnerProduct => "vec_negative_inner_product",
    VecCosineDistance => "vec_cosine_distance",
    VecL2Norm => "vec_l2_norm",
    VecFromText => "vec_from_text",
    VecAsText => "vec_as_text",
    FTSMatchWord => "fts_match_word",
    FTSMysqlMatchAgainst => "match_against",
    TiDBDecodeKey => "tidb_decode_key",
    TiDBMVCCInfo => "tidb_mvcc_info",
    TiDBEncodeRecordKey => "tidb_encode_record_key",
    TiDBEncodeIndexKey => "tidb_encode_index_key",
    TiDBDecodeBase64Key => "tidb_decode_base64_key",
    NextVal => "nextval",
    LastVal => "lastval",
    SetVal => "setval",
}

/// json_memberof 函数名常量别名。
pub const JSON_MEMBER_OF: &str = JSONMemberOf;

/// 大小写不敏感字符串：保留 original 与 lower。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CiString {
    pub original: String,
    pub lower: String,
}

impl CiString {
    pub fn new(value: impl Into<String>) -> Self {
        let original = value.into();
        let lower = original.to_lowercase();
        Self { original, lower }
    }
}

/// 轻量表达式包装，内部保存可还原 SQL 文本。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Expr {
    sql: String,
}

/// 由 SQL 文本构造轻量 Expr。
pub fn expr(sql: impl Into<String>) -> Expr {
    Expr { sql: sql.into() }
}

impl Expr {
    pub fn restore(&self) -> AstResult<String> {
        Ok(self.sql.clone())
    }
    pub fn format(&self) -> String {
        self.sql.clone()
    }
}

/// AST 访问者接口：enter/leave 控制子树遍历。
pub trait Visitor {
    fn enter_expr(&mut self, expression: Expr) -> (Expr, bool) {
        (expression, false)
    }
    fn leave_expr(&mut self, expression: Expr) -> (Expr, bool) {
        (expression, true)
    }
}

/// 对单个 Expr 执行 enter/leave 访问者协议。
fn visit_expr(expression: &mut Expr, visitor: &mut dyn Visitor) -> bool {
    let (entered, _) = visitor.enter_expr(expression.clone());
    let (left, ok) = visitor.leave_expr(entered);
    *expression = left;
    ok
}

/// 函数调用形态：关键字函数或通用函数。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum FuncCallExprType {
    #[default]
    Keyword,
    Generic,
}

/// 函数调用表达式，支持关键字与通用函数两种还原风格。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FuncCallExpr {
    pub function_type: FuncCallExprType,
    pub schema: CiString,
    pub function_name: CiString,
    pub args: Vec<Expr>,
}

impl FuncCallExpr {
    pub fn keyword(name: impl Into<String>, args: Vec<Expr>) -> Self {
        Self {
            function_type: FuncCallExprType::Keyword,
            schema: CiString::new(""),
            function_name: CiString::new(name),
            args,
        }
    }

    pub fn generic(schema: impl Into<String>, name: impl Into<String>, args: Vec<Expr>) -> Self {
        Self {
            function_type: FuncCallExprType::Generic,
            schema: CiString::new(schema),
            function_name: CiString::new(name),
            args,
        }
    }

    fn arg(&self, index: usize) -> AstResult<String> {
        self.args
            .get(index)
            .ok_or_else(|| AstError::new(format!("missing function argument {index}")))?
            .restore()
    }

    // 少数函数名无法用通用 `name(args)` 形态还原，需按名分支。
    fn custom_restore(&self) -> AstResult<Option<String>> {
        let prefix = match self.function_name.lower.as_str() {
            DateLiteral => Some("DATE "),
            TimeLiteral => Some("TIME "),
            TimestampLiteral => Some("TIMESTAMP "),
            _ => None,
        };
        if let Some(prefix) = prefix {
            return Ok(Some(format!("{prefix}{}", self.arg(0)?)));
        }

        if self.function_name.lower == JSONMemberOf {
            if self.args.len() != 2 {
                return Err(AstError::new(
                    "Incorrect parameter count in the call to native function 'json_memberof'",
                ));
            }
            return Ok(Some(format!(
                "{} MEMBER OF ({})",
                self.arg(0)?,
                self.arg(1)?
            )));
        }
        Ok(None)
    }

    pub fn restore(&self) -> AstResult<String> {
        if let Some(restored) = self.custom_restore()? {
            return Ok(restored);
        }

        let mut output = String::new();
        if !self.schema.original.is_empty() {
            output.push_str(&quote_name(&self.schema.original));
            output.push('.');
        }
        match self.function_type {
            FuncCallExprType::Keyword => {
                output.push_str(&self.function_name.original.to_uppercase())
            }
            FuncCallExprType::Generic => output.push_str(&quote_name(&self.function_name.original)),
        }
        output.push('(');
        match self.function_name.lower.as_str() {
            Convert => output.push_str(&format!("{} USING {}", self.arg(0)?, self.arg(1)?)),
            AddDate | SubDate | DateAdd | DateSub => output.push_str(&format!(
                "{}, INTERVAL {} {}",
                self.arg(0)?,
                self.arg(1)?,
                self.arg(2)?
            )),
            Extract => output.push_str(&format!("{} FROM {}", self.arg(0)?, self.arg(1)?)),
            Position => output.push_str(&format!("{} IN {}", self.arg(0)?, self.arg(1)?)),
            Trim => match self.args.len() {
                1 => output.push_str(&self.arg(0)?),
                2 => output.push_str(&format!("{} FROM {}", self.arg(1)?, self.arg(0)?)),
                3 => output.push_str(&format!(
                    "{} {} FROM {}",
                    self.arg(2)?,
                    self.arg(1)?,
                    self.arg(0)?
                )),
                // Go's switch has no default branch: unsupported counts restore
                // as an empty argument list instead of returning an error.
                _ => {}
            },
            WeightString => {
                output.push_str(&self.arg(0)?);
                if self.args.len() == 3 {
                    output.push_str(&format!(" AS {}({})", self.arg(1)?, self.arg(2)?));
                }
            }
            _ => output.push_str(&restore_args(&self.args)?),
        }
        output.push(')');
        Ok(output)
    }

    pub fn format(&self) -> String {
        match self.function_name.lower.as_str() {
            AddDate | SubDate | DateAdd | DateSub if self.args.len() == 3 => format!(
                "{}({}, INTERVAL {} {})",
                self.function_name.lower,
                self.args[0].format(),
                self.args[1].format(),
                self.args[2].format()
            ),
            JSONMemberOf if self.args.len() == 2 => {
                format!(
                    "{} MEMBER OF  ({})",
                    self.args[0].format(),
                    self.args[1].format()
                )
            }
            Extract if self.args.len() == 2 => format!(
                "{}({} FROM {})",
                self.function_name.lower,
                self.args[0].format(),
                self.args[1].format()
            ),
            _ => format!(
                "{}({})",
                self.function_name.lower,
                self.args
                    .iter()
                    .map(Expr::format)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    pub fn accept(&mut self, visitor: &mut dyn Visitor) -> bool {
        for argument in &mut self.args {
            if !visit_expr(argument, visitor) {
                return false;
            }
        }
        true
    }
}

/// 按 MySQL 标识符规则用反引号引用并转义。
fn quote_name(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}

/// 将参数表达式列表还原为逗号分隔 SQL。
fn restore_args(args: &[Expr]) -> AstResult<String> {
    args.iter()
        .map(Expr::restore)
        .collect::<AstResult<Vec<_>>>()
        .map(|items| items.join(", "))
}

/// CAST 目标字段类型的文本表示。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldType(pub String);

impl FieldType {
    pub fn restore_as_cast_type(&self, _explicit_charset: bool) -> String {
        self.0.clone()
    }
}

/// JSON_SUM_CRC32 特殊函数表达式。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JSONSumCrc32Expr {
    pub expression: Expr,
    pub field_type: FieldType,
    pub explicit_charset: bool,
}

impl JSONSumCrc32Expr {
    pub fn restore(&self) -> AstResult<String> {
        Ok(format!(
            "JSON_SUM_CRC32({} AS {})",
            self.expression.restore()?,
            self.field_type.restore_as_cast_type(self.explicit_charset)
        ))
    }
    pub fn format(&self) -> String {
        self.restore()
            .expect("in-memory expression restoration cannot fail")
    }
    pub fn accept(&mut self, visitor: &mut dyn Visitor) -> bool {
        visit_expr(&mut self.expression, visitor)
    }
}

/// CAST 函数形态（Cast/Convert/Binary）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CastFunctionType {
    Cast,
    Convert,
    Binary,
}

/// CAST/CONVERT 类强制类型转换表达式。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FuncCastExpr {
    pub expression: Expr,
    pub field_type: FieldType,
    pub function_type: CastFunctionType,
    pub explicit_charset: bool,
}

impl FuncCastExpr {
    pub fn new(
        expression: Expr,
        field_type: impl Into<String>,
        function_type: CastFunctionType,
    ) -> Self {
        Self {
            expression,
            field_type: FieldType(field_type.into()),
            function_type,
            explicit_charset: false,
        }
    }
    pub fn restore(&self) -> AstResult<String> {
        let expression = self.expression.restore()?;
        let target = self.field_type.restore_as_cast_type(self.explicit_charset);
        Ok(match self.function_type {
            CastFunctionType::Cast => format!("CAST({expression} AS {target})"),
            CastFunctionType::Convert => format!("CONVERT({expression}, {target})"),
            CastFunctionType::Binary => format!("BINARY {expression}"),
        })
    }
    pub fn format(&self) -> String {
        self.restore()
            .expect("in-memory expression restoration cannot fail")
    }
    pub fn accept(&mut self, visitor: &mut dyn Visitor) -> bool {
        visit_expr(&mut self.expression, visitor)
    }
}

/// TRIM 方向（BOTH/LEADING/TRAILING）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TrimDirectionType {
    #[default]
    BothDefault,
    Both,
    Leading,
    Trailing,
    Invalid,
}

impl TrimDirectionType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BothDefault | Self::Both => "BOTH",
            Self::Leading => "LEADING",
            Self::Trailing => "TRAILING",
            Self::Invalid => "",
        }
    }
}

/// 日期加减运算类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DateArithType {
    Add = 1,
    Sub = 2,
}

/// 聚合函数表达式（SUM/COUNT 等）。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AggregateFuncExpr {
    pub name: String,
    pub args: Vec<Expr>,
    pub distinct: bool,
    pub order_by: Option<String>,
}

impl AggregateFuncExpr {
    pub fn restore(&self) -> AstResult<String> {
        let mut output = format!("{}(", self.name.to_uppercase());
        if self.distinct {
            output.push_str("DISTINCT ");
        }
        if self.name.eq_ignore_ascii_case(AggFuncGroupConcat) {
            let (separator, values) = self
                .args
                .split_last()
                .ok_or_else(|| AstError::new("GROUP_CONCAT requires a separator argument"))?;
            output.push_str(&restore_args(values)?);
            if let Some(order) = &self.order_by {
                output.push(' ');
                output.push_str(order);
            }
            output.push_str(" SEPARATOR ");
            output.push_str(&separator.restore()?);
        } else {
            output.push_str(&restore_args(&self.args)?);
        }
        output.push(')');
        Ok(output)
    }
    pub fn format(&self) -> String {
        panic!("Not implemented")
    }
    pub fn accept(&mut self, visitor: &mut dyn Visitor) -> bool {
        for argument in &mut self.args {
            if !visit_expr(argument, visitor) {
                return false;
            }
        }
        true
    }
}

/// 窗口函数表达式及其 OVER 规格。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WindowFuncExpr {
    pub name: String,
    pub args: Vec<Expr>,
    pub distinct: bool,
    pub ignore_null: bool,
    pub from_last: bool,
    pub spec: String,
}

impl WindowFuncExpr {
    pub fn restore(&self) -> AstResult<String> {
        let mut output = format!("{}(", self.name.to_uppercase());
        if self.distinct && !self.args.is_empty() {
            output.push_str("DISTINCT ");
        }
        output.push_str(&restore_args(&self.args)?);
        output.push(')');
        if self.from_last {
            output.push_str(" FROM LAST");
        }
        if self.ignore_null {
            output.push_str(" IGNORE NULLS");
        }
        output.push_str(" OVER ");
        output.push_str(&self.spec);
        Ok(output)
    }
    pub fn format(&self) -> String {
        panic!("Not implemented")
    }
    pub fn accept(&mut self, visitor: &mut dyn Visitor) -> bool {
        for argument in &mut self.args {
            if !visit_expr(argument, visitor) {
                return false;
            }
        }
        true
    }
}

/// 时间单位枚举（YEAR/MONTH/DAY 等）。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum TimeUnitType {
    #[default]
    Invalid,
    Microsecond,
    Second,
    Minute,
    Hour,
    Day,
    Week,
    Month,
    Quarter,
    Year,
    SecondMicrosecond,
    MinuteMicrosecond,
    MinuteSecond,
    HourMicrosecond,
    HourSecond,
    HourMinute,
    DayMicrosecond,
    DaySecond,
    DayMinute,
    DayHour,
    YearMonth,
}

impl TimeUnitType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Invalid => "",
            Self::Microsecond => "MICROSECOND",
            Self::Second => "SECOND",
            Self::Minute => "MINUTE",
            Self::Hour => "HOUR",
            Self::Day => "DAY",
            Self::Week => "WEEK",
            Self::Month => "MONTH",
            Self::Quarter => "QUARTER",
            Self::Year => "YEAR",
            Self::SecondMicrosecond => "SECOND_MICROSECOND",
            Self::MinuteMicrosecond => "MINUTE_MICROSECOND",
            Self::MinuteSecond => "MINUTE_SECOND",
            Self::HourMicrosecond => "HOUR_MICROSECOND",
            Self::HourSecond => "HOUR_SECOND",
            Self::HourMinute => "HOUR_MINUTE",
            Self::DayMicrosecond => "DAY_MICROSECOND",
            Self::DaySecond => "DAY_SECOND",
            Self::DayMinute => "DAY_MINUTE",
            Self::DayHour => "DAY_HOUR",
            Self::YearMonth => "YEAR_MONTH",
        }
    }
    pub fn duration(self) -> AstResult<Duration> {
        let duration = match self {
            Self::Microsecond => Duration::from_micros(1),
            Self::Second => Duration::from_secs(1),
            Self::Minute => Duration::from_secs(60),
            Self::Hour => Duration::from_secs(3_600),
            Self::Day => Duration::from_secs(86_400),
            Self::Week => Duration::from_secs(604_800),
            Self::Month | Self::Quarter | Self::Year => {
                return Err(AstError::new(format!(
                    "{} is not a constant time interval and cannot be used here",
                    self.as_str()
                )));
            }
            _ => {
                return Err(AstError::new(format!(
                    "{} is a composite time unit and is not supported yet",
                    self.as_str()
                )));
            }
        };
        Ok(duration)
    }
}

/// GET_FORMAT 选择器（DATE/TIME/DATETIME/TIMESTAMP）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GetFormatSelectorType {
    Date = 1,
    Time = 2,
    Datetime = 3,
    Invalid = 0,
}

impl GetFormatSelectorType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Date => "DATE",
            Self::Time => "TIME",
            Self::Datetime => "DATETIME",
            Self::Invalid => "",
        }
    }
}

/// TRIM方向表达式结构体。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TrimDirectionExpr {
    pub direction: TrimDirectionType,
}
impl TrimDirectionExpr {
    pub fn restore(self) -> String {
        self.direction.as_str().into()
    }
    pub fn format(self) -> String {
        self.restore()
    }
}

/// 时间单位表达式结构体。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimeUnitExpr {
    pub unit: TimeUnitType,
}
impl TimeUnitExpr {
    pub fn restore(self) -> String {
        self.unit.as_str().into()
    }
    pub fn format(self) -> String {
        self.restore()
    }
}

/// 获取格式选择器表达式结构体。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GetFormatSelectorExpr {
    pub selector: GetFormatSelectorType,
}
impl GetFormatSelectorExpr {
    pub fn restore(self) -> String {
        self.selector.as_str().into()
    }
    pub fn format(self) -> String {
        self.restore()
    }
}
