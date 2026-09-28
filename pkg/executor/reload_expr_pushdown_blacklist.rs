// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 重载表达式下推黑名单（expr pushdown blacklist）。
//
// 从系统表 `mysql.expr_pushdown_blacklist` 读取禁止下推到存储引擎的表达式名，
// 按 store_type 位掩码聚合后原子替换内存中的黑名单；无变化则跳过。
// 下推（pushdown）指将表达式计算下沉到 TiKV/TiFlash 等存储侧执行。

#![allow(non_snake_case)]

use std::collections::HashMap;

/// 加载黑名单所用的系统表查询（HIGH_PRIORITY 优先调度）。
pub const LOAD_EXPR_PUSHDOWN_BLACKLIST_SQL: &str =
    "select HIGH_PRIORITY name, store_type from mysql.expr_pushdown_blacklist";

/// Restricted-SQL and atomic-expression-state boundary used by the executor.
/// 受限 SQL 查询与原子替换表达式黑名单状态的运行时边界。
pub trait ExprPushdownBlacklistRuntime {
    type Context;
    type Error;

    fn query_blacklist(
        &mut self,
        context: &mut Self::Context,
        sql: &str,
    ) -> Result<Vec<(String, String)>, Self::Error>;
    fn store_mask(&self, store_name: &str) -> Option<u32>;
    fn current_blacklist(&self) -> HashMap<String, u32>;
    fn unix_nanos(&self) -> i64;
    fn replace_blacklist(&mut self, blacklist: HashMap<String, u32>, reload_time: i64);
}

/// RELOAD EXPR_PUSHDOWN_BLACKLIST 执行器：Next 时触发一次加载。
pub struct ReloadExprPushdownBlacklistExec<R: ExprPushdownBlacklistRuntime> {
    pub runtime: R,
}

impl<R: ExprPushdownBlacklistRuntime> ReloadExprPushdownBlacklistExec<R> {
    /// 委托 `LoadExprPushdownBlacklist` 完成查询与替换。
    pub fn Next<T>(&mut self, context: &mut R::Context, _request: &mut T) -> Result<(), R::Error> {
        LoadExprPushdownBlacklist(&mut self.runtime, context)
    }
}

/// 查询系统表、归一化函数名并按 store 掩码聚合，若与当前黑名单不同则替换。
pub fn LoadExprPushdownBlacklist<R: ExprPushdownBlacklistRuntime>(
    runtime: &mut R,
    context: &mut R::Context,
) -> Result<(), R::Error> {
    // 拉取 name + store_type，转小写后经别名映射得到规范函数名。
    let rows = runtime.query_blacklist(context, LOAD_EXPR_PUSHDOWN_BLACKLIST_SQL)?;
    let mut new_blacklist = HashMap::with_capacity(rows.len());
    for (name, stores) in rows {
        let lowered = name.to_lowercase();
        let canonical = func_name_to_alias(&lowered).unwrap_or(&lowered).to_owned();
        let mut value = new_blacklist.get(&canonical).copied().unwrap_or(0);
        // 同一函数可对应多个 store，掩码按位或合并。
        let lowered_stores = stores.to_lowercase();
        for store in lowered_stores.split(',') {
            if let Some(mask) = runtime.store_mask(store) {
                value |= mask;
            }
        }
        new_blacklist.insert(canonical, value);
    }

    // 内容未变则跳过，避免无意义的全局替换与时间戳更新。
    if isSameExprPushDownBlackList(&new_blacklist, &runtime.current_blacklist()) {
        return Ok(());
    }
    let reload_time = runtime.unix_nanos();
    runtime.replace_blacklist(new_blacklist, reload_time);
    Ok(())
}

// isSameExprPushDownBlackList checks whether two exprPushDownBlacklist are the same.
// 对应 Go 的 map 长度和值比较，保持“任意 key 缺失或 value 不同即 false”的语义。
/// 比较两份黑名单 map 是否完全一致（长度与每个 key/value）。
pub fn isSameExprPushDownBlackList(l1: &HashMap<String, u32>, l2: &HashMap<String, u32>) -> bool {
    if l1.len() != l2.len() {
        return false;
    }
    for (k, v1) in l1 {
        match l2.get(k) {
            Some(v2) if v2 == v1 => {}
            _ => return false,
        }
    }
    true
}

// The second field records the Go AST constant for one-to-one auditability.
// `func_name_to_alias` below returns the constants' actual SQL spelling.
/// 函数名到 Go AST 常量名的对照表，便于与上游一对一审计；别名见 `func_name_to_alias`。
pub const FUNC_NAME_2_ALIAS_SYMBOLS: &[(&str, &str)] = &[
    ("and", "ast.LogicAnd"),
    ("cast", "ast.Cast"),
    ("<<", "ast.LeftShift"),
    (">>", "ast.RightShift"),
    ("or", "ast.LogicOr"),
    (">=", "ast.GE"),
    ("<=", "ast.LE"),
    ("=", "ast.EQ"),
    ("!=", "ast.NE"),
    ("<>", "ast.NE"),
    ("<", "ast.LT"),
    (">", "ast.GT"),
    ("+", "ast.Plus"),
    ("-", "ast.Minus"),
    ("&&", "ast.And"),
    ("||", "ast.Or"),
    ("%", "ast.Mod"),
    ("xor_bit", "ast.Xor"),
    ("/", "ast.Div"),
    ("*", "ast.Mul"),
    ("!", "ast.UnaryNot"),
    ("~", "ast.BitNeg"),
    ("div", "ast.IntDiv"),
    ("xor_logic", "ast.LogicXor"), // 避免和 xor_bit 名称冲突。
    ("<=>", "ast.NullEQ"),
    ("+_unary", "ast.UnaryPlus"), // 避免和 plus 名称冲突。
    ("-_unary", "ast.UnaryMinus"),
    ("in", "ast.In"),
    ("like", "ast.Like"),
    ("case", "ast.Case"),
    ("regexp", "ast.Regexp"),
    ("is null", "ast.IsNull"),
    ("is true", "ast.IsTruthWithoutNull"),
    ("is false", "ast.IsFalsity"),
    ("values", "ast.Values"),
    ("bit_count", "ast.BitCount"),
    ("coalesce", "ast.Coalesce"),
    ("greatest", "ast.Greatest"),
    ("least", "ast.Least"),
    ("interval", "ast.Interval"),
    ("abs", "ast.Abs"),
    ("acos", "ast.Acos"),
    ("asin", "ast.Asin"),
    ("atan", "ast.Atan"),
    ("atan2", "ast.Atan2"),
    ("ceil", "ast.Ceil"),
    ("ceiling", "ast.Ceiling"),
    ("conv", "ast.Conv"),
    ("cos", "ast.Cos"),
    ("cot", "ast.Cot"),
    ("crc32", "ast.CRC32"),
    ("degrees", "ast.Degrees"),
    ("exp", "ast.Exp"),
    ("floor", "ast.Floor"),
    ("ln", "ast.Ln"),
    ("log", "ast.Log"),
    ("log2", "ast.Log2"),
    ("log10", "ast.Log10"),
    ("pi", "ast.PI"),
    ("pow", "ast.Pow"),
    ("power", "ast.Power"),
    ("radians", "ast.Radians"),
    ("rand", "ast.Rand"),
    ("round", "ast.Round"),
    ("sign", "ast.Sign"),
    ("sin", "ast.Sin"),
    ("sqrt", "ast.Sqrt"),
    ("tan", "ast.Tan"),
    ("truncate", "ast.Truncate"),
    ("adddate", "ast.AddDate"),
    ("addtime", "ast.AddTime"),
    ("convert_tz", "ast.ConvertTz"),
    ("curdate", "ast.Curdate"),
    ("current_date", "ast.CurrentDate"),
    ("current_time", "ast.CurrentTime"),
    ("current_timestamp", "ast.CurrentTimestamp"),
    ("curtime", "ast.Curtime"),
    ("date", "ast.Date"),
    ("date_add", "ast.DateAdd"),
    ("date_format", "ast.DateFormat"),
    ("date_sub", "ast.DateSub"),
    ("datediff", "ast.DateDiff"),
    ("day", "ast.Day"),
    ("dayname", "ast.DayName"),
    ("dayofmonth", "ast.DayOfMonth"),
    ("dayofweek", "ast.DayOfWeek"),
    ("dayofyear", "ast.DayOfYear"),
    ("extract", "ast.Extract"),
    ("from_days", "ast.FromDays"),
    ("from_unixtime", "ast.FromUnixTime"),
    ("get_format", "ast.GetFormat"),
    ("hour", "ast.Hour"),
    ("localtime", "ast.LocalTime"),
    ("localtimestamp", "ast.LocalTimestamp"),
    ("makedate", "ast.MakeDate"),
    ("maketime", "ast.MakeTime"),
    ("microsecond", "ast.MicroSecond"),
    ("minute", "ast.Minute"),
    ("month", "ast.Month"),
    ("monthname", "ast.MonthName"),
    ("now", "ast.Now"),
    ("period_add", "ast.PeriodAdd"),
    ("period_diff", "ast.PeriodDiff"),
    ("quarter", "ast.Quarter"),
    ("sec_to_time", "ast.SecToTime"),
    ("second", "ast.Second"),
    ("str_to_date", "ast.StrToDate"),
    ("subdate", "ast.SubDate"),
    ("subtime", "ast.SubTime"),
    ("sysdate", "ast.Sysdate"),
    ("time", "ast.Time"),
    ("time_format", "ast.TimeFormat"),
    ("time_to_sec", "ast.TimeToSec"),
    ("timediff", "ast.TimeDiff"),
    ("timestamp", "ast.Timestamp"),
    ("timestampadd", "ast.TimestampAdd"),
    ("timestampdiff", "ast.TimestampDiff"),
    ("to_days", "ast.ToDays"),
    ("to_seconds", "ast.ToSeconds"),
    ("unix_timestamp", "ast.UnixTimestamp"),
    ("utc_date", "ast.UTCDate"),
    ("utc_time", "ast.UTCTime"),
    ("utc_timestamp", "ast.UTCTimestamp"),
    ("week", "ast.Week"),
    ("weekday", "ast.Weekday"),
    ("weekofyear", "ast.WeekOfYear"),
    ("year", "ast.Year"),
    ("yearweek", "ast.YearWeek"),
    ("last_day", "ast.LastDay"),
    ("ascii", "ast.ASCII"),
    ("bin", "ast.Bin"),
    ("concat", "ast.Concat"),
    ("concat_ws", "ast.ConcatWS"),
    ("convert", "ast.Convert"),
    ("elt", "ast.Elt"),
    ("export_set", "ast.ExportSet"),
    ("field", "ast.Field"),
    ("format", "ast.Format"),
    ("from_base64", "ast.FromBase64"),
    ("insert_func", "ast.InsertFunc"),
    ("instr", "ast.Instr"),
    ("lcase", "ast.Lcase"),
    ("left", "ast.Left"),
    ("length", "ast.Length"),
    ("load_file", "ast.LoadFile"),
    ("locate", "ast.Locate"),
    ("lower", "ast.Lower"),
    ("lpad", "ast.Lpad"),
    ("ltrim", "ast.LTrim"),
    ("make_set", "ast.MakeSet"),
    ("mid", "ast.Mid"),
    ("oct", "ast.Oct"),
    ("octet_length", "ast.OctetLength"),
    ("ord", "ast.Ord"),
    ("position", "ast.Position"),
    ("quote", "ast.Quote"),
    ("repeat", "ast.Repeat"),
    ("replace", "ast.Replace"),
    ("reverse", "ast.Reverse"),
    ("right", "ast.Right"),
    ("rtrim", "ast.RTrim"),
    ("space", "ast.Space"),
    ("strcmp", "ast.Strcmp"),
    ("substring", "ast.Substring"),
    ("substr", "ast.Substr"),
    ("substring_index", "ast.SubstringIndex"),
    ("to_base64", "ast.ToBase64"),
    ("trim", "ast.Trim"),
    ("upper", "ast.Upper"),
    ("ucase", "ast.Ucase"),
    ("hex", "ast.Hex"),
    ("unhex", "ast.Unhex"),
    ("rpad", "ast.Rpad"),
    ("bit_length", "ast.BitLength"),
    ("char_func", "ast.CharFunc"),
    ("char_length", "ast.CharLength"),
    ("character_length", "ast.CharacterLength"),
    ("find_in_set", "ast.FindInSet"),
    ("benchmark", "ast.Benchmark"),
    ("charset", "ast.Charset"),
    ("coercibility", "ast.Coercibility"),
    ("collation", "ast.Collation"),
    ("connection_id", "ast.ConnectionID"),
    ("current_user", "ast.CurrentUser"),
    ("current_resource_group", "ast.CurrentResourceGroup"),
    ("current_role", "ast.CurrentRole"),
    ("database", "ast.Database"),
    ("found_rows", "ast.FoundRows"),
    ("last_insert_id", "ast.LastInsertId"),
    ("row_count", "ast.RowCount"),
    ("schema", "ast.Schema"),
    ("session_user", "ast.SessionUser"),
    ("system_user", "ast.SystemUser"),
    ("user", "ast.User"),
    ("if", "ast.If"),
    ("ifnull", "ast.Ifnull"),
    ("nullif", "ast.Nullif"),
    ("any_value", "ast.AnyValue"),
    ("default_func", "ast.DefaultFunc"),
    ("inet_aton", "ast.InetAton"),
    ("inet_ntoa", "ast.InetNtoa"),
    ("inet6_aton", "ast.Inet6Aton"),
    ("inet6_ntoa", "ast.Inet6Ntoa"),
    ("is_free_lock", "ast.IsFreeLock"),
    ("is_ipv4", "ast.IsIPv4"),
    ("is_ipv4_compat", "ast.IsIPv4Compat"),
    ("is_ipv4_mapped", "ast.IsIPv4Mapped"),
    ("is_ipv6", "ast.IsIPv6"),
    ("is_used_lock", "ast.IsUsedLock"),
    ("name_const", "ast.NameConst"),
    ("release_all_locks", "ast.ReleaseAllLocks"),
    ("sleep", "ast.Sleep"),
    ("uuid", "ast.UUID"),
    ("uuid_v4", "ast.UUIDv4"),
    ("uuid_v7", "ast.UUIDv7"),
    ("uuid_version", "ast.UUIDVersion"),
    ("uuid_timestamp", "ast.UUIDTimestamp"),
    ("uuid_short", "ast.UUIDShort"),
    ("get_lock", "ast.GetLock"),
    ("release_lock", "ast.ReleaseLock"),
    ("aes_decrypt", "ast.AesDecrypt"),
    ("aes_encrypt", "ast.AesEncrypt"),
    ("compress", "ast.Compress"),
    ("decode", "ast.Decode"),
    ("encode", "ast.Encode"),
    ("md5", "ast.MD5"),
    ("password", "ast.PasswordFunc"),
    ("random_bytes", "ast.RandomBytes"),
    ("sha1", "ast.SHA1"),
    ("sha", "ast.SHA"),
    ("sha2", "ast.SHA2"),
    ("sm3", "ast.SM3"),
    ("uncompress", "ast.Uncompress"),
    ("uncompressed_length", "ast.UncompressedLength"),
    ("validate_password_strength", "ast.ValidatePasswordStrength"),
    ("json_type", "ast.JSONType"),
    ("json_extract", "ast.JSONExtract"),
    ("json_unquote", "ast.JSONUnquote"),
    ("json_array", "ast.JSONArray"),
    ("json_object", "ast.JSONObject"),
    ("json_merge", "ast.JSONMerge"),
    ("json_set", "ast.JSONSet"),
    ("json_insert", "ast.JSONInsert"),
    ("json_replace", "ast.JSONReplace"),
    ("json_remove", "ast.JSONRemove"),
    ("json_contains", "ast.JSONContains"),
    ("json_contains_path", "ast.JSONContainsPath"),
    ("json_valid", "ast.JSONValid"),
    ("json_array_append", "ast.JSONArrayAppend"),
    ("json_array_insert", "ast.JSONArrayInsert"),
    ("json_merge_patch", "ast.JSONMergePatch"),
    ("json_merge_preserve", "ast.JSONMergePreserve"),
    ("json_pretty", "ast.JSONPretty"),
    ("json_quote", "ast.JSONQuote"),
    ("json_schema_valid", "ast.JSONSchemaValid"),
    ("json_search", "ast.JSONSearch"),
    ("json_storage_size", "ast.JSONStorageSize"),
    ("json_depth", "ast.JSONDepth"),
    ("json_keys", "ast.JSONKeys"),
    ("json_length", "ast.JSONLength"),
    ("vec_dims", "ast.VecDims"),
    ("vec_l1_distance", "ast.VecL1Distance"),
    ("vec_l2_distance", "ast.VecL2Distance"),
    ("vec_negative_inner_product", "ast.VecNegativeInnerProduct"),
    ("vec_cosine_distance", "ast.VecCosineDistance"),
    ("vec_l2_norm", "ast.VecL2Norm"),
    ("vec_from_text", "ast.VecFromText"),
    ("vec_as_text", "ast.VecAsText"),
];

/// 将内部/符号形式的函数名映射为规范 SQL 拼写（未在表中则返回 None）。
pub fn func_name_to_alias<'a>(name: &'a str) -> Option<&'a str> {
    if !FUNC_NAME_2_ALIAS_SYMBOLS
        .iter()
        .any(|(origin, _)| *origin == name)
    {
        return None;
    }
    Some(match name {
        "<<" => "leftshift",
        ">>" => "rightshift",
        ">=" => "ge",
        "<=" => "le",
        "=" => "eq",
        "!=" | "<>" => "ne",
        "<" => "lt",
        ">" => "gt",
        "+" => "plus",
        "-" => "minus",
        "&&" => "bitand",
        "||" => "bitor",
        "%" => "mod",
        "xor_bit" => "bitxor",
        "*" => "mul",
        "!" => "not",
        "~" => "bitneg",
        "div" => "intdiv",
        "xor_logic" => "xor",
        "<=>" => "nulleq",
        "+_unary" => "unaryplus",
        "-_unary" => "unaryminus",
        "is null" => "isnull",
        "is true" => "istrue",
        "is false" => "isfalse",
        _ => name,
    })
}
