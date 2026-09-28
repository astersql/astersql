// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// function_traits 分类表的单元测试。
//
// 验证不可折叠集合、生成列非法/合法函数划分，以及 builtin 注册表
// 有序唯一并包含分类样例。

use std::collections::HashSet;

use crate::function_traits_kernel::{
    PartitionOp, has_mutable_effect, is_allowed_partition_binary_op, is_allowed_partition_function,
    is_allowed_partition_unary_op, is_boolean_function, is_deferred,
    is_illegal_generated_column_function, is_inequal_function, is_noop_function, is_uncacheable,
    is_unfoldable,
};

#[test]
fn optimizer_function_trait_queries_cover_every_go_classification_branch() {
    assert!(is_uncacheable("database"));
    assert!(is_uncacheable("aes_decrypt"));
    assert!(!is_uncacheable("abs"));

    assert!(is_deferred("now", false));
    assert!(!is_deferred("sysdate", false));
    assert!(is_deferred("sysdate", true));
    assert!(!is_deferred("abs", true));

    assert!(is_allowed_partition_function("to_days"));
    assert!(is_allowed_partition_function("mod"));
    assert!(!is_allowed_partition_function("rand"));
    for operation in [
        PartitionOp::Plus,
        PartitionOp::Minus,
        PartitionOp::Mul,
        PartitionOp::IntDiv,
        PartitionOp::Mod,
    ] {
        assert!(is_allowed_partition_binary_op(operation));
    }
    assert!(is_allowed_partition_unary_op(PartitionOp::Plus));
    assert!(is_allowed_partition_unary_op(PartitionOp::Minus));
    assert!(!is_allowed_partition_unary_op(PartitionOp::Mul));

    assert!(is_inequal_function("isnull"));
    assert!(!is_inequal_function("eq"));
    assert!(has_mutable_effect("current_timestamp"));
    assert!(has_mutable_effect("any_value"));
    assert!(!has_mutable_effect("abs"));
    assert!(!is_noop_function("unknown_function"));
    assert!(is_boolean_function("regexp_like"));
    assert!(!is_boolean_function("abs"));
}

/// 有状态/非确定性内置应不可折叠；纯函数应可折叠。
#[test]
fn unfoldable_functions_cover_stateful_and_nondeterministic_builtins() {
    for name in ["sysdate", "rand", "uuid", "sleep", "getvar"] {
        assert!(is_unfoldable(name), "{name} must not be constant-folded");
    }
    for name in ["abs", "concat", "json_extract"] {
        assert!(
            is_unfoldable(name) == false,
            "{name} should remain foldable"
        );
    }
}

/// 生成列显式拒绝非确定性、会话依赖或系统副作用函数。
#[test]
fn generated_columns_reject_explicit_stateful_function_set() {
    for name in [
        "rand",
        "uuid",
        "current_timestamp",
        "connection_id",
        "get_lock",
        "sleep",
        "setvar",
    ] {
        assert!(
            is_illegal_generated_column_function(name),
            "{name} is nondeterministic, session-dependent, or has system effects"
        );
    }
}

/// 生成列允许确定性且与会话无关的函数。
#[test]
fn generated_columns_allow_explicit_deterministic_function_set() {
    for name in [
        "abs",
        "concat",
        "date_add",
        "json_extract",
        "lower",
        "plus",
        "sha2",
    ] {
        assert!(
            !is_illegal_generated_column_function(name),
            "{name} is deterministic and independent of session state"
        );
    }
}

/// 与 Go `TestIllegalFunctions4GeneratedColumns` 一样，对所有已注册 builtin
/// 的生成列合法集合做显式快照，避免新函数绕过安全决策。
/// Rust 注册表尚未覆盖全部 Go builtin，因此只对已注册交集做精确比较。
#[test]
fn every_registered_generated_column_builtin_has_an_explicit_go_decision() {
    let builtins = crate::expression_builtin::GetBuiltinList();
    assert!(!builtins.is_empty());
    assert!(builtins.windows(2).all(|pair| pair[0] < pair[1]));

    let registered: HashSet<&str> = builtins.iter().map(String::as_str).collect();
    assert_eq!(registered.len(), builtins.len());

    let legal: Vec<&str> = builtins
        .iter()
        .map(String::as_str)
        .filter(|name| !is_illegal_generated_column_function(name))
        .collect();
    let mut known_good: HashSet<&str> = "
        abs acos adddate addtime aes_decrypt aes_encrypt and any_value ascii asin atan atan2
        bin bin_to_uuid bit_count bit_length bitand bitneg bitor bitxor case ceil ceiling
        char_func char_length character_length charset coalesce coercibility collation compress
        concat concat_ws conv convert convert_tz cos cot crc32 date date_add date_format date_sub
        datediff day dayname dayofmonth dayofweek dayofyear decode default_func degrees div elt
        encode eq exp export_set extract field find_in_set floor format format_bytes
        format_nano_time from_base64 from_days from_unixtime fts_match_word ge get_format getparam
        greatest grouping gt hex hour if ifnull ilike in inet6_aton inet6_ntoa inet_aton
        inet_ntoa insert_func instr intdiv interval is_ipv4 is_ipv4_compat is_ipv4_mapped is_ipv6
        is_uuid isfalse isnull istrue json_array json_array_append json_array_insert json_contains
        json_contains_path json_depth json_extract json_insert json_keys json_length json_memberof
        json_merge_patch json_merge_preserve json_object json_overlaps json_pretty json_quote
        json_remove json_replace json_schema_valid json_search json_set json_storage_free
        json_storage_size json_type json_unquote json_valid last_day lastval lcase le least left
        leftshift length like ln locate log log10 log2 lower lpad lt ltrim make_set makedate
        maketime match_against md5 microsecond mid minus minute mod month monthname mul ne nextval
        not nulleq oct octet_length or ord password period_add period_diff pi plus position pow
        power quarter quote radians regexp regexp_instr regexp_like regexp_replace regexp_substr
        repeat replace reverse right rightshift round rpad rtrim sec_to_time second setval sha sha1
        sha2 sign sin sm3 space sqrt str_to_date strcmp subdate substr substring substring_index
        subtime tan tidb_decode_binary_plan tidb_decode_key tidb_decode_plan tidb_decode_sql_digests
        tidb_encode_index_key tidb_encode_record_key tidb_encode_sql_digest tidb_mvcc_info
        tidb_parse_tso tidb_parse_tso_logical tidb_shard time time_format time_to_sec timediff
        timestamp timestampadd timestampdiff to_base64 to_days to_seconds translate trim truncate
        ucase unaryminus uncompress uncompressed_length unhex upper uuid_timestamp uuid_to_bin
        uuid_version validate_password_strength vec_as_text vec_cosine_distance vec_dims
        vec_from_text vec_l1_distance vec_l2_distance vec_l2_norm vec_negative_inner_product
        vitess_hash week weekday weekofyear weight_string xor year yearweek
    "
    .split_whitespace()
    .collect();
    // Rust registry spellings (`cast`, `char`, `insert`) for parser-level
    // aliases are independently deterministic.
    known_good.extend(["cast", "char", "insert"]);

    let expected: Vec<&str> = builtins
        .iter()
        .map(String::as_str)
        .filter(|name| known_good.contains(name))
        .collect();
    assert_eq!(legal, expected);
}
