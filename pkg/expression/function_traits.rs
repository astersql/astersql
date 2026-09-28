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

// Optimizer function classifications from `function_traits.go`.
//
// 优化器对内置函数的分类表，对应 Go `function_traits.go`。
// 用于计划缓存、常量折叠、生成列合法性、分区函数白名单、延迟求值等决策：
// - 不可缓存 / 不可折叠 / 禁止折叠 / 尝试折叠；
// - 生成列非法函数集；
// - 延迟函数（含 SYSDATE 兼容开关）；
// - 分区表达式允许的函数与运算符；
// - 可变副作用、布尔谓词、noop 等标记。

use std::collections::HashSet;
use std::sync::LazyLock;

/// 从静态字符串切片构造 HashSet。
fn set(values: &[&'static str]) -> HashSet<&'static str> {
    values.iter().copied().collect()
}

/// 不可放入计划缓存的函数（依赖会话状态或非确定性语义）。
pub static UNCACHEABLE_FUNCTIONS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    set(&[
        "database",
        "current_user",
        "current_role",
        "current_resource_group",
        "user",
        "connection_id",
        "last_insert_id",
        "row_count",
        "version",
        "like",
        "json_object",
        "json_array",
        "coalesce",
        "convert",
        "time_literal",
        "date_literal",
        "timestamp_literal",
        "aes_encrypt",
        "aes_decrypt",
    ])
});

/// 不可常量折叠的函数（有状态、随机、会话变量等）。
static UNFOLDABLE_FUNCTIONS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    set(&[
        "sysdate",
        "found_rows",
        "rand",
        "uuid",
        "uuid_v4",
        "uuid_v7",
        "sleep",
        "row",
        "values",
        "setvar",
        "getvar",
        "getparam",
        "benchmark",
        "dayname",
        "nextval",
        "lastval",
        "setval",
        "any_value",
    ])
});

/// 显式禁用折叠的函数（即便其它路径可能尝试）。
pub static DISABLE_FOLD_FUNCTIONS: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| set(&["benchmark"]));

/// 控制流类：允许在安全条件下尝试折叠的函数。
pub static TRY_FOLD_FUNCTIONS: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| set(&["if", "ifnull", "case", "and", "or", "coalesce", "interval"]));

/// 生成列表达式中禁止出现的函数（非确定性、会话依赖或系统副作用）。
pub static ILLEGAL_FUNCTIONS_FOR_GENERATED_COLUMNS: LazyLock<HashSet<&'static str>> =
    LazyLock::new(|| {
        set(&[
            "benchmark",
            "connection_id",
            "curdate",
            "current_date",
            "current_resource_group",
            "current_role",
            "current_time",
            "current_timestamp",
            "current_user",
            "curtime",
            "database",
            "found_rows",
            "get_lock",
            "getvar",
            "is_free_lock",
            "is_used_lock",
            "json_merge",
            "last_insert_id",
            "load_file",
            "localtime",
            "localtimestamp",
            "name_const",
            "now",
            "rand",
            "random_bytes",
            "release_all_locks",
            "release_lock",
            "row_count",
            "row",
            "schema",
            "session_user",
            "setvar",
            "sleep",
            "sysdate",
            "system_user",
            "tidb_bounded_staleness",
            "tidb_current_tso",
            "tidb_is_ddl_owner",
            "tidb_row_checksum",
            "tidb_version",
            "unix_timestamp",
            "user",
            "utc_date",
            "utc_time",
            "utc_timestamp",
            "uuid",
            "uuid_v4",
            "uuid_v7",
            "uuid_short",
            "values",
            "version",
        ])
    });

/// 延迟到执行期求值的时间类函数集合。
static DEFERRED_FUNCTIONS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    set(&[
        "now",
        "random_bytes",
        "current_timestamp",
        "utc_time",
        "curtime",
        "current_time",
        "utc_timestamp",
        "unix_timestamp",
        "curdate",
        "current_date",
        "utc_date",
    ])
});

/// 分区表达式允许的确定性函数白名单。
pub static ALLOWED_PARTITION_FUNCTIONS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    set(&[
        "to_days",
        "to_seconds",
        "dayofmonth",
        "month",
        "dayofyear",
        "quarter",
        "yearweek",
        "year",
        "weekday",
        "dayofweek",
        "day",
        "hour",
        "minute",
        "second",
        "time_to_sec",
        "microsecond",
        "unix_timestamp",
        "from_days",
        "extract",
        "abs",
        "ceiling",
        "datediff",
        "floor",
        "mod",
    ])
});

/// 分区表达式中允许出现的二元/一元运算符种类。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum PartitionOp {
    Plus,
    Minus,
    Mul,
    IntDiv,
    Mod,
}

/// 不等式比较类函数（如 ISNULL）。
static INEQUAL_FUNCTIONS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| set(&["isnull"]));

/// 具有可变副作用或非确定性结果的函数（影响并行/缓存安全）。
static MUTABLE_EFFECT_FUNCTIONS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    set(&[
        "now",
        "current_timestamp",
        "utc_time",
        "curtime",
        "current_time",
        "utc_timestamp",
        "unix_timestamp",
        "sysdate",
        "curdate",
        "current_date",
        "utc_date",
        "rand",
        "random_bytes",
        "uuid",
        "uuid_v4",
        "uuid_v7",
        "uuid_short",
        "sleep",
        "setvar",
        "getvar",
        "any_value",
    ])
});

/// noop 实现占位函数集合（当前为空，与 Go 初始态一致）。
static NOOP_FUNCTIONS: LazyLock<HashSet<&'static str>> = LazyLock::new(HashSet::new);

/// 返回布尔语义的谓词/比较函数集合。
static BOOLEAN_FUNCTIONS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    set(&[
        "not",
        "eq",
        "ne",
        "nulleq",
        "lt",
        "le",
        "gt",
        "ge",
        "in",
        "and",
        "or",
        "xor",
        "istrue_with_null",
        "istrue",
        "isfalse",
        "isnull",
        "like",
        "regexp",
        "is_ipv4",
        "is_ipv4_compat",
        "is_ipv4_mapped",
        "is_ipv6",
        "json_schema_valid",
        "json_valid",
        "regexp_like",
    ])
});

/// 函数是否不可放入计划缓存。
pub fn is_uncacheable(name: &str) -> bool {
    UNCACHEABLE_FUNCTIONS.contains(name)
}

/// 函数是否不可常量折叠。
pub fn is_unfoldable(name: &str) -> bool {
    UNFOLDABLE_FUNCTIONS.contains(name)
}

/// 函数是否禁止出现在生成列定义中。
pub fn is_illegal_generated_column_function(name: &str) -> bool {
    ILLEGAL_FUNCTIONS_FOR_GENERATED_COLUMNS.contains(name)
}

/// 函数是否延迟求值；`sysdate_is_now` 为真时将 SYSDATE 视为 NOW。
pub fn is_deferred(name: &str, sysdate_is_now: bool) -> bool {
    DEFERRED_FUNCTIONS.contains(name) || (name == "sysdate" && sysdate_is_now)
}

/// IsDeferredFunctions matches Go's plan-cache classification, including the
/// session-controlled `SYSDATE()` compatibility branch.
/// 从会话 BuildContext 读取 `sysdate_is_now` 后判定是否延迟函数。
pub fn IsDeferredFunctions(ctx: &dyn crate::BuildContext, name: &str) -> bool {
    is_deferred(name, ctx.GetSysdateIsNow())
}

/// 函数是否允许出现在分区表达式中。
pub fn is_allowed_partition_function(name: &str) -> bool {
    ALLOWED_PARTITION_FUNCTIONS.contains(name)
}

/// 二元运算符是否允许用于分区表达式（+ - * DIV MOD）。
pub fn is_allowed_partition_binary_op(operation: PartitionOp) -> bool {
    matches!(
        operation,
        PartitionOp::Plus
            | PartitionOp::Minus
            | PartitionOp::Mul
            | PartitionOp::IntDiv
            | PartitionOp::Mod
    )
}

/// 一元运算符是否允许用于分区表达式（仅 + / -）。
pub fn is_allowed_partition_unary_op(operation: PartitionOp) -> bool {
    matches!(operation, PartitionOp::Plus | PartitionOp::Minus)
}

/// 是否为不等式比较类函数。
pub fn is_inequal_function(name: &str) -> bool {
    INEQUAL_FUNCTIONS.contains(name)
}

/// 函数是否具有可变副作用。
pub fn has_mutable_effect(name: &str) -> bool {
    MUTABLE_EFFECT_FUNCTIONS.contains(name)
}

/// 函数是否为 noop 占位实现。
pub fn is_noop_function(name: &str) -> bool {
    NOOP_FUNCTIONS.contains(name)
}

/// 函数是否产生布尔语义结果。
pub fn is_boolean_function(name: &str) -> bool {
    BOOLEAN_FUNCTIONS.contains(name)
}
