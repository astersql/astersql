// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Store capability, blacklist and warning routing from `infer_pushdown.go`.
//
// 表达式下推（pushdown）能力推断：判断谓词/投影能否下推到 TiKV / TiFlash / TiDB。
//
// 下推指把计算移到存储层执行以减少数据搬运。本模块维护存储能力表、
// 黑名单位掩码与规划期警告路由（对应 Go `infer_pushdown.go`）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};

use crate::fts_to_like_kernel::{CastFamily, Expression, FieldKind, ScalarFunction, Signature};

/// 全局函数下推黑名单：函数名 → 禁用的 StoreType 位掩码。
static DEFAULT_EXPR_PUSH_DOWN_BLACKLIST: LazyLock<RwLock<HashMap<String, u32>>> =
    LazyLock::new(|| RwLock::new(HashMap::new()));

/// 黑名单重载序号；变更时递增，供缓存失效。
pub static EXPR_PUSH_DOWN_BLACKLIST_RELOAD_TIMESTAMP: AtomicI64 = AtomicI64::new(0);

/// 存储引擎类型；`Unspecified` 表示对所有引擎取并集判断。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
#[repr(u32)]
pub enum StoreType {
    Unspecified = 0,
    TiKV = 1,
    TiFlash = 2,
    TiDB = 3,
}

impl StoreType {
    /// 转为位掩码；Unspecified 覆盖 TiKV|TiFlash|TiDB。
    pub fn mask(self) -> u32 {
        if self == Self::Unspecified {
            (1 << Self::TiKV as u32) | (1 << Self::TiFlash as u32) | (1 << Self::TiDB as u32)
        } else {
            1 << self as u32
        }
    }

    /// 警告文案中的存储层显示名。
    fn name(self) -> &'static str {
        match self {
            Self::Unspecified => "storage layer",
            Self::TiKV => "tikv",
            Self::TiFlash => "tiflash",
            Self::TiDB => "tidb",
        }
    }
}

/// 警告收集器：规划期追加不可下推原因。
pub type WarningHandler = Arc<Mutex<Vec<String>>>;

/// 下推判定上下文：警告路由、GROUP_CONCAT 上限、强制下推与调试开关。
#[derive(Clone, Debug)]
pub struct PushDownContext {
    warning_handler: Option<WarningHandler>,
    group_concat_max_len: u64,
    force_pushdown: Option<String>,
    panic_on_unspecified: bool,
}

impl PushDownContext {
    /// Go chooses a warning handler only when both handlers are present. Explain
    /// uses the normal handler; other planning paths use the extra handler.
    ///
    /// 仅当 normal 与 extra 处理器都存在时才启用警告；EXPLAIN 用 normal，其它路径用 extra。
    pub fn new(
        in_explain_statement: bool,
        normal_handler: Option<WarningHandler>,
        extra_handler: Option<WarningHandler>,
        group_concat_max_len: u64,
    ) -> Self {
        let warning_handler = match (normal_handler, extra_handler) {
            (Some(normal), Some(extra)) => Some(if in_explain_statement { normal } else { extra }),
            _ => None,
        };
        Self {
            warning_handler,
            group_concat_max_len,
            force_pushdown: None,
            panic_on_unspecified: false,
        }
    }

    /// 返回会话 GROUP_CONCAT 最大长度配置。
    pub fn group_concat_max_len(&self) -> u64 {
        self.group_concat_max_len
    }

    /// 强制下推：`"all"` 或逗号分隔函数名列表。
    pub fn with_force_pushdown(mut self, functions: impl Into<String>) -> Self {
        self.force_pushdown = Some(functions.into());
        self
    }

    /// 调试：遇到 Unspecified PbCode 时 panic 而非仅警告。
    pub fn with_panic_on_unspecified(mut self, enabled: bool) -> Self {
        self.panic_on_unspecified = enabled;
        self
    }

    fn append_warning(&self, warning: String) {
        if let Some(handler) = &self.warning_handler {
            handler
                .lock()
                .expect("warning handler poisoned")
                .push(warning);
        }
    }
}

/// 整体替换黑名单并递增重载时间戳；函数名统一小写。
pub fn replace_pushdown_blacklist(entries: impl IntoIterator<Item = (String, u32)>) {
    let mut blacklist = DEFAULT_EXPR_PUSH_DOWN_BLACKLIST
        .write()
        .expect("blacklist poisoned");
    blacklist.clear();
    blacklist.extend(
        entries
            .into_iter()
            .map(|(name, mask)| (name.to_ascii_lowercase(), mask)),
    );
    EXPR_PUSH_DOWN_BLACKLIST_RELOAD_TIMESTAMP.fetch_add(1, Ordering::SeqCst);
}

/// 清空黑名单。
pub fn clear_pushdown_blacklist() {
    replace_pushdown_blacklist(std::iter::empty());
}

/// 查询函数名在指定存储上是否未被完整禁用。
pub fn is_push_down_enabled(name: &str, store_type: StoreType) -> bool {
    let blacklist = DEFAULT_EXPR_PUSH_DOWN_BLACKLIST
        .read()
        .expect("blacklist poisoned");
    is_push_down_enabled_with_map(name, store_type, &blacklist)
}

/// 黑名单命中且掩码覆盖目标存储则禁用；未登记则允许。
fn is_push_down_enabled_with_map(
    name: &str,
    store_type: StoreType,
    blacklist: &HashMap<String, u32>,
) -> bool {
    blacklist
        .get(&name.to_ascii_lowercase())
        .is_none_or(|disabled| disabled & store_type.mask() != store_type.mask())
}

/// 标量函数能否下推：强制列表 → 存储能力表 → 函数名/签名黑名单。
fn can_func_be_pushed(
    context: &PushDownContext,
    function: &ScalarFunction,
    store_type: StoreType,
) -> bool {
    if let Some(force) = &context.force_pushdown {
        return force == "all"
            || force
                .split(',')
                .any(|name| name.trim().eq_ignore_ascii_case(&function.name));
    }
    let supported = match store_type {
        StoreType::TiFlash => scalar_expr_supported_by_flash(function),
        StoreType::TiKV => scalar_expr_supported_by_tikv(function),
        StoreType::TiDB => scalar_expr_supported_by_tidb(function),
        StoreType::Unspecified => {
            scalar_expr_supported_by_tidb(function)
                || scalar_expr_supported_by_tikv(function)
                || scalar_expr_supported_by_flash(function)
        }
    };
    if !supported {
        return false;
    }
    let blacklist = DEFAULT_EXPR_PUSH_DOWN_BLACKLIST
        .read()
        .expect("blacklist poisoned");
    if !is_push_down_enabled_with_map(&function.name, store_type, &blacklist) {
        return false;
    }
    // 细粒度：`func.signature` 也可单独拉黑。
    let full_name = format!(
        "{}.{}",
        function.name,
        function.signature.name().to_ascii_lowercase()
    );
    is_push_down_enabled_with_map(&full_name, store_type, &blacklist)
}

/// 递归判定表达式能否下推到指定存储。
///
/// TiFlash 额外拒绝 ENUM（除非允许）/BIT/SET/GEOMETRY 及非法 DECIMAL；
/// 标量函数需签名可编码、能力表通过，且所有参数可下推。
pub fn can_expr_push_down(
    context: &PushDownContext,
    expression: &Expression,
    store_type: StoreType,
    can_enum_push: bool,
) -> bool {
    // TiFlash 对部分 MySQL 类型尚无列式实现，先按返回类型过滤。
    if store_type == StoreType::TiFlash {
        let field_type = expression.field_type();
        match field_type.kind {
            FieldKind::Enum if !can_enum_push => {
                context.append_warning(format!(
                    "Expression about '{expression}' can not be pushed to TiFlash because it contains unsupported calculation of type 'enum'."
                ));
                return false;
            }
            FieldKind::Bit | FieldKind::Set | FieldKind::Geometry | FieldKind::Unspecified => {
                context.append_warning(format!(
                    "Expression about '{expression}' can not be pushed to TiFlash because it contains unsupported calculation of type '{:?}'.",
                    field_type.kind
                ));
                return false;
            }
            FieldKind::Decimal if !field_type.is_decimal_valid() => {
                context.append_warning(format!(
                    "Expression about '{expression}' can not be pushed to TiFlash because it contains invalid decimal('{}','{}').",
                    field_type.flen, field_type.decimal
                ));
                return false;
            }
            _ => {}
        }
    }

    match expression {
        Expression::Constant(_) => true,
        Expression::Column(column) => column.encodable,
        Expression::Unsupported(_) => false,
        Expression::ScalarFunction(function) => {
            if function.signature == Signature::Unspecified {
                if context.panic_on_unspecified {
                    panic!("unspecified PbCode: {}", function.name);
                }
                context.append_warning(format!(
                    "Scalar function '{}' (signature: Unspecified, return type: {:?}) is not supported to push down to {} now.",
                    function.name, function.ret_type.kind, store_type.name()
                ));
                return false;
            }
            if !can_func_be_pushed(context, function, store_type) {
                context.append_warning(format!(
                    "Scalar function '{}' (signature: {}, return type: {:?}) is not supported to push down to {} now.",
                    function.name, function.signature.name(), function.ret_type.kind, store_type.name()
                ));
                return false;
            }
            // CAST 到数值时子树可携带 ENUM；否则禁止 ENUM 下推。
            let allow_enum = can_enum_pushdown_preliminarily(function);
            if function
                .args
                .iter()
                .any(|argument| !can_expr_push_down(context, argument, store_type, allow_enum))
            {
                return false;
            }
            function.metadata_serializable
        }
    }
}

/// TiDB 侧能力：TiKV 与 TiFlash 支持集合的并集。
fn scalar_expr_supported_by_tidb(function: &ScalarFunction) -> bool {
    scalar_expr_supported_by_tikv(function) || scalar_expr_supported_by_flash(function)
}

/// TiKV 可下推的标量函数名白名单（与 Go 对齐）。
const TIKV_FUNCTIONS: &[&str] = &[
    "and",
    "or",
    "xor",
    "not",
    "bitneg",
    "leftshift",
    "rightshift",
    "unaryminus",
    "lt",
    "le",
    "eq",
    "ne",
    "ge",
    "gt",
    "nulleq",
    "in",
    "isnull",
    "like",
    "istrue",
    "istrue_with_null",
    "isfalse",
    "pi",
    "plus",
    "minus",
    "mul",
    "div",
    "abs",
    "mod",
    "intdiv",
    "ceil",
    "ceiling",
    "floor",
    "sqrt",
    "sign",
    "ln",
    "log",
    "log2",
    "log10",
    "exp",
    "pow",
    "power",
    "sin",
    "asin",
    "cos",
    "acos",
    "atan",
    "atan2",
    "cot",
    "radians",
    "degrees",
    "crc32",
    "case",
    "if",
    "ifnull",
    "coalesce",
    "upper",
    "lower",
    "length",
    "bit_length",
    "concat",
    "concat_ws",
    "replace",
    "ascii",
    "hex",
    "reverse",
    "ltrim",
    "rtrim",
    "strcmp",
    "space",
    "elt",
    "field",
    "from_binary",
    "to_binary",
    "mid",
    "substring",
    "substr",
    "char_length",
    "right",
    "json_type",
    "json_extract",
    "json_object",
    "json_array",
    "json_merge",
    "json_set",
    "json_insert",
    "json_replace",
    "json_remove",
    "json_length",
    "json_merge_patch",
    "json_unquote",
    "json_contains",
    "json_valid",
    "json_memberof",
    "json_array_append",
    "vec_dims",
    "vec_l1_distance",
    "vec_l2_distance",
    "vec_negative_inner_product",
    "vec_cosine_distance",
    "vec_l2_norm",
    "vec_as_text",
    "date",
    "week",
    "datediff",
    "monthname",
    "makedate",
    "time_to_sec",
    "maketime",
    "date_format",
    "date_add",
    "adddate",
    "date_sub",
    "subdate",
    "hour",
    "minute",
    "second",
    "microsecond",
    "month",
    "dayofmonth",
    "dayofweek",
    "dayofyear",
    "weekofyear",
    "year",
    "from_days",
    "period_add",
    "period_diff",
    "timestampdiff",
    "from_unixtime",
    "sysdate",
    "md5",
    "sha1",
    "sha2",
    "uncompressed_length",
    "cast",
    "uuid",
    "uuid_version",
    "uuid_timestamp",
];

/// TiKV 能力判定：白名单命中，或带签名/参数特例（unix_timestamp/conv/round/rand/regexp）。
fn scalar_expr_supported_by_tikv(function: &ScalarFunction) -> bool {
    if TIKV_FUNCTIONS.contains(&function.name.as_str()) {
        return true;
    }
    match function.name.as_str() {
        // 无参 CURRENT 形态不下推。
        "unix_timestamp" => !signature_is(function, "UnixTimestampCurrent"),
        // cast(hybrid/binary) 作 conv 首参时不下推。
        "conv" => !function
            .args
            .first()
            .is_some_and(|argument| match argument {
                Expression::ScalarFunction(cast) if cast.name == "cast" => cast
                    .args
                    .first()
                    .is_some_and(|source| source.field_type().hybrid || is_binary_literal(source)),
                _ => false,
            }),
        "round" => signature_in(function, &["RoundReal", "RoundInt", "RoundDec"]),
        "rand" => signature_is(function, "RandWithSeedFirstGen"),
        // binary charset/collation 的正则不下推。
        "regexp" | "regexp_like" | "regexp_substr" | "regexp_instr" | "regexp_replace" => {
            function.ret_type.charset != "binary" || function.ret_type.collation != "binary"
        }
        _ => false,
    }
}

/// TiFlash 可下推的标量函数名白名单。
const TIFLASH_FUNCTIONS: &[&str] = &[
    "or",
    "and",
    "not",
    "bitneg",
    "xor",
    "rightshift",
    "leftshift",
    "ge",
    "le",
    "eq",
    "ne",
    "nulleq",
    "lt",
    "gt",
    "in",
    "isnull",
    "like",
    "ilike",
    "strcmp",
    "plus",
    "minus",
    "div",
    "mul",
    "abs",
    "mod",
    "if",
    "ifnull",
    "case",
    "concat",
    "concat_ws",
    "date",
    "year",
    "month",
    "day",
    "quarter",
    "dayname",
    "monthname",
    "datediff",
    "timestampdiff",
    "date_format",
    "from_unixtime",
    "dayofweek",
    "dayofmonth",
    "dayofyear",
    "last_day",
    "weekofyear",
    "to_seconds",
    "from_days",
    "to_days",
    "sqrt",
    "log",
    "log2",
    "log10",
    "ln",
    "exp",
    "pow",
    "power",
    "sign",
    "radians",
    "degrees",
    "conv",
    "crc32",
    "json_length",
    "json_depth",
    "json_extract",
    "json_unquote",
    "json_object",
    "json_array",
    "json_contains_path",
    "json_valid",
    "json_keys",
    "repeat",
    "inet_ntoa",
    "inet_aton",
    "inet6_ntoa",
    "inet6_aton",
    "coalesce",
    "ascii",
    "length",
    "trim",
    "position",
    "format",
    "elt",
    "ltrim",
    "rtrim",
    "lpad",
    "rpad",
    "hour",
    "minute",
    "second",
    "microsecond",
    "time_to_sec",
    "upper",
    "ucase",
    "lower",
    "lcase",
    "space",
    "sysdate",
    "istrue",
    "istrue_with_null",
    "isfalse",
    "hex",
    "unhex",
    "bin",
    "get_format",
    "is_ipv4",
    "is_ipv6",
    "vec_dims",
    "vec_l1_distance",
    "vec_l2_distance",
    "vec_negative_inner_product",
    "vec_cosine_distance",
    "vec_l2_norm",
    "vec_as_text",
    "fts_match_word",
    "grouping",
    "fts_mysql_match_against",
];

/// TiFlash 能力判定：排除部分 Duration/JSON 签名与布尔 FTS 模式，其余按白名单/签名细则。
fn scalar_expr_supported_by_flash(function: &ScalarFunction) -> bool {
    // Int→Dec 的 floor/ceil 在 Flash 侧未实现。
    if matches!(function.name.as_str(), "floor" | "ceil" | "ceiling")
        && signature_in(function, &["FloorIntToDec", "CeilIntToDec"])
    {
        return false;
    }
    if TIFLASH_FUNCTIONS.contains(&function.name.as_str()) {
        if signature_in(
            function,
            &[
                "InDuration",
                "CoalesceDuration",
                "IfNullDuration",
                "IfDuration",
                "CaseWhenDuration",
                "LTJson",
                "LEJson",
                "GTJson",
                "GEJson",
                "EQJson",
                "NEJson",
                "JsonIsNull",
                "InJson",
            ],
        ) {
            return false;
        }
        // FULLTEXT：非 BOOLEAN MODE 且无 query expansion 才可下推。
        if function.name == "fts_mysql_match_against" {
            return function.modifier.is_some_and(|modifier| {
                !modifier.is_boolean_mode() && !modifier.with_query_expansion()
            });
        }
        return true;
    }
    match function.name.as_str() {
        "regexp" | "regexp_like" | "regexp_instr" | "regexp_substr" | "regexp_replace" => {
            function.ret_type.charset != "binary" || function.ret_type.collation != "binary"
        }
        "substr" | "substring" | "left" | "right" | "char_length" | "substring_index"
        | "reverse" => signature_in(
            function,
            &[
                "LeftUTF8",
                "RightUTF8",
                "CharLengthUTF8",
                "Substring2ArgsUTF8",
                "Substring3ArgsUTF8",
                "SubstringIndex",
                "ReverseUTF8",
                "Reverse",
            ],
        ),
        "cast" => flash_supports_cast(function),
        "date_add" | "adddate" => signature_in(
            function,
            &[
                "AddDateDatetimeInt",
                "AddDateStringInt",
                "AddDateStringReal",
            ],
        ),
        "date_sub" | "subdate" => signature_in(
            function,
            &[
                "SubDateDatetimeInt",
                "SubDateStringInt",
                "SubDateStringReal",
            ],
        ),
        "unix_timestamp" => signature_in(function, &["UnixTimestampInt", "UnixTimestampDec"]),
        "round" => signature_in(
            function,
            &[
                "RoundInt",
                "RoundReal",
                "RoundDec",
                "RoundWithFracInt",
                "RoundWithFracReal",
                "RoundWithFracDec",
            ],
        ),
        "truncate" => signature_in(
            function,
            &[
                "TruncateUint",
                "TruncateInt",
                "TruncateReal",
                "TruncateDecimal",
            ],
        ),
        "extract" => signature_in(function, &["ExtractDatetime", "ExtractDuration"]),
        "replace" => signature_is(function, "Replace"),
        "str_to_date" => signature_in(function, &["StrToDateDate", "StrToDateDatetime"]),
        "least" | "greatest" => signature_in(
            function,
            &[
                "GreatestInt",
                "GreatestReal",
                "GreatestString",
                "LeastInt",
                "LeastReal",
                "LeastString",
            ],
        ),
        _ => false,
    }
}

/// TiFlash CAST 支持：按 CastFamily 检查源/目标类型兼容性。
fn flash_supports_cast(function: &ScalarFunction) -> bool {
    let Some(source) = function.args.first().map(Expression::field_type) else {
        return false;
    };
    let target = &function.ret_type;
    match function.signature {
        Signature::Cast(CastFamily::Int) => {
            (source.kind == target.kind && source.unsigned == target.unsigned)
                || target.kind == FieldKind::Int
        }
        Signature::Cast(CastFamily::Real) => {
            source.kind == target.kind || target.kind == FieldKind::Real
        }
        Signature::Cast(CastFamily::Decimal) => target.is_decimal_valid(),
        Signature::Cast(CastFamily::String | CastFamily::Json | CastFamily::Vector) => true,
        // YEAR 源类型不可 CAST 为 Time。
        Signature::Cast(CastFamily::Time) => source.kind != FieldKind::Year,
        Signature::Cast(CastFamily::Duration) => target.kind == FieldKind::Duration,
        Signature::Cast(CastFamily::Unsupported)
        | Signature::Generic(_)
        | Signature::Unspecified => false,
    }
}

/// CAST 到 Int/Real/Decimal 时，允许子表达式携带 ENUM。
fn can_enum_pushdown_preliminarily(function: &ScalarFunction) -> bool {
    function.name == "cast"
        && matches!(
            function.ret_type.kind,
            FieldKind::Int | FieldKind::Real | FieldKind::Decimal
        )
}

/// 将表达式列表分为可下推与不可下推两组。
pub fn push_down_exprs_with_extra_info(
    context: &PushDownContext,
    expressions: Vec<Expression>,
    store_type: StoreType,
    can_enum_push: bool,
) -> (Vec<Expression>, Vec<Expression>) {
    let mut pushed = Vec::with_capacity(expressions.len());
    let mut remained = Vec::with_capacity(expressions.len());
    for expression in expressions {
        if can_expr_push_down(context, &expression, store_type, can_enum_push) {
            pushed.push(expression);
        } else {
            remained.push(expression);
        }
    }
    (pushed, remained)
}

/// 默认禁止 ENUM 下推的分组接口。
pub fn push_down_exprs(
    context: &PushDownContext,
    expressions: Vec<Expression>,
    store_type: StoreType,
) -> (Vec<Expression>, Vec<Expression>) {
    push_down_exprs_with_extra_info(context, expressions, store_type, false)
}

/// 全部表达式均可下推时返回 true（可指定 ENUM 策略）。
pub fn can_exprs_push_down_with_extra_info(
    context: &PushDownContext,
    expressions: Vec<Expression>,
    store_type: StoreType,
    can_enum_push: bool,
) -> bool {
    push_down_exprs_with_extra_info(context, expressions, store_type, can_enum_push)
        .1
        .is_empty()
}

/// 全部表达式均可下推（默认禁止 ENUM）。
pub fn can_exprs_push_down(
    context: &PushDownContext,
    expressions: Vec<Expression>,
    store_type: StoreType,
) -> bool {
    can_exprs_push_down_with_extra_info(context, expressions, store_type, false)
}

fn signature_is(function: &ScalarFunction, expected: &str) -> bool {
    function.signature.name() == expected
}

fn signature_in(function: &ScalarFunction, expected: &[&str]) -> bool {
    expected.contains(&function.signature.name())
}

/// 识别 binary 字面量节点（conv 下推特例）。
fn is_binary_literal(expression: &Expression) -> bool {
    matches!(expression, Expression::Unsupported(name) if name == "binary_literal")
}
