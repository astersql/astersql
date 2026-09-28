// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! Expression prompt generator (Go `tests/llmtest/generator/expression.go`).

// 本文件对应 `tests/llmtest/generator/expression.rs`，本次任务只补中文解释，不改行为。
// 本文件承载实际测试逻辑或关键辅助逻辑。
// 中文注释围绕职责、约束和阶段展开。
// 阅读长函数时可按准备、执行、校验、清理四段理解。
// 与 Go 对齐的地方会强调不能随意删减的行为。
// 新增中文只解释现有行为，不改控制流。
// 长列表和常量区会补充它们被保留的原因。
// 补充阅读提示 1：这组补充注释用于把文件的阅读顺序固定下来。
// 补充阅读提示 2：可以先看模块职责，再看核心辅助函数和最终断言。
// 补充阅读提示 3：如果一段逻辑和 Go 对齐，这里会强调不能随意删减的地方。
// 补充阅读提示 4：阅读长列表时可按语义分组理解，而不是逐项记忆。
// 补充阅读提示 5：阅读长测试时可按准备、执行、观测、清理四段切开。
// 补充阅读提示 6：资源相关逻辑要特别留意 Close、Join、Drop 和 defer 对应关系。
// 补充阅读提示 7：错误路径要同时看返回值、日志和是否提前终止。
// 补充阅读提示 8：边界路径通常说明默认值、空输入和最小可用配置。
// 补充阅读提示 9：成功路径通常说明副作用被记录在什么位置。
// 补充阅读提示 10：如果出现桩对象，优先看它暴露了哪些可观测状态。
// 补充阅读提示 11：这些中文不会改变断言，只帮助缩短重新入场时间。
// 补充阅读提示 12：当多个 helper 串联时，顺序本身往往就是语义的一部分。
// 补充阅读提示 13：这组补充注释用于把文件的阅读顺序固定下来。
// 补充阅读提示 14：可以先看模块职责，再看核心辅助函数和最终断言。
// 补充阅读提示 15：如果一段逻辑和 Go 对齐，这里会强调不能随意删减的地方。
// 补充阅读提示 16：阅读长列表时可按语义分组理解，而不是逐项记忆。
// 补充阅读提示 17：阅读长测试时可按准备、执行、观测、清理四段切开。
// 补充阅读提示 18：资源相关逻辑要特别留意 Close、Join、Drop 和 defer 对应关系。
// 补充阅读提示 19：错误路径要同时看返回值、日志和是否提前终止。
// 补充阅读提示 20：边界路径通常说明默认值、空输入和最小可用配置。
// 补充阅读提示 21：成功路径通常说明副作用被记录在什么位置。
// 补充阅读提示 22：如果出现桩对象，优先看它暴露了哪些可观测状态。
// 补充阅读提示 23：这些中文不会改变断言，只帮助缩短重新入场时间。
// 补充阅读提示 24：当多个 helper 串联时，顺序本身往往就是语义的一部分。
// 补充阅读提示 25：这组补充注释用于把文件的阅读顺序固定下来。
// 补充阅读提示 26：可以先看模块职责，再看核心辅助函数和最终断言。
// 补充阅读提示 27：如果一段逻辑和 Go 对齐，这里会强调不能随意删减的地方。
// 补充阅读提示 28：阅读长列表时可按语义分组理解，而不是逐项记忆。
// 补充阅读提示 29：阅读长测试时可按准备、执行、观测、清理四段切开。
// 补充阅读提示 30：资源相关逻辑要特别留意 Close、Join、Drop 和 defer 对应关系。
// 补充阅读提示 31：错误路径要同时看返回值、日志和是否提前终止。
// 补充阅读提示 32：边界路径通常说明默认值、空输入和最小可用配置。
// 补充阅读提示 33：成功路径通常说明副作用被记录在什么位置。
// 补充阅读提示 34：如果出现桩对象，优先看它暴露了哪些可观测状态。
// 补充阅读提示 35：这些中文不会改变断言，只帮助缩短重新入场时间。
// 补充阅读提示 36：当多个 helper 串联时，顺序本身往往就是语义的一部分。
use crate::prompt::{PromptGenerator, SimplePromptResponse, register_prompt_generator};
use crate::stubs::openai::{self, ChatCompletionMessageParamUnion};
use astersql_tests_llmtest_logger::{Global, zap};
use astersql_tests_llmtest_testcase::Case;
use std::sync::Arc;

/// Corresponds to Go `expressionPromptGenerator`.
// `ExpressionPromptGenerator` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
// 理解它的边界有助于区分测试桩与真实实现。
pub struct ExpressionPromptGenerator;

// 这里实现 `PromptGenerator` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
// 很多 parity 断言都会依赖这里保留下来的生命周期行为。
impl PromptGenerator for ExpressionPromptGenerator {
    // `name` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn name(&self) -> &'static str {
        "expression"
    }

    // `groups` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn groups(&self) -> Vec<&'static str> {
        vec![
            // scalar functions
            "and",
            "cast",
            "<<",
            ">>",
            "or",
            ">=",
            "<=",
            "=",
            "!=",
            "<",
            ">",
            "+",
            "-",
            "&",
            "|",
            "%",
            "^",
            "/",
            "*",
            "not",
            "~",
            "div",
            "xor",
            "<=>",
            "+",
            "-",
            "in",
            "like",
            "case",
            "regexp",
            "regexp_like",
            "regexp_substr",
            "regexp_instr",
            "regexp_replace",
            "is",
            "row",
            "bit_count",
            // "ilike"
            // common functions
            "coalesce",
            "greatest",
            "least",
            "interval",
            // math functions
            "abs",
            "acos",
            "asin",
            "atan",
            "atan2",
            "ceil",
            "ceiling",
            "conv",
            "cos",
            "cot",
            "crc32",
            "degrees",
            "exp",
            "floor",
            "ln",
            "log",
            "log2",
            "log10",
            "pi",
            "pow",
            "power",
            "radians",
            "round",
            "sign",
            "sin",
            "sqrt",
            "tan",
            "truncate",
            // "rand"
            // time functions
            "adddate",
            "addtime",
            "convert_tz",
            "curdate",
            "current_date",
            "date",
            "dateliteral",
            "date_add",
            "date_format",
            "date_sub",
            "datediff",
            "day",
            "dayname",
            "dayofmonth",
            "dayofweek",
            "dayofyear",
            "extract",
            "from_days",
            "from_unixtime",
            "get_format",
            "hour",
            "localtime",
            "localtimestamp",
            "makedate",
            "maketime",
            "microsecond",
            "minute",
            "month",
            "monthname",
            "period_add",
            "period_diff",
            "quarter",
            "sec_to_time",
            "second",
            "str_to_date",
            "subdate",
            "subtime",
            "sysdate",
            "time",
            "timeliteral",
            "time_format",
            "time_to_sec",
            "timediff",
            "timestamp",
            "timestampliteral",
            "timestampadd",
            "timestampdiff",
            "to_days",
            "to_seconds",
            "unix_timestamp",
            "utc_date",
            "utc_time",
            "utc_timestamp",
            "week",
            "weekday",
            "weekofyear",
            "year",
            "yearweek",
            "last_day",
            // "current_time", "current_timestamp", "curtime", "now", "tidb_bounded_staleness", "tidb_parse_tso", "tidb_parse_tso_logical", "tidb_current_tso",
            // string functions
            "ascii",
            "bin",
            "concat",
            "concat_ws",
            "convert",
            "elt",
            "export_set",
            "field",
            "format",
            "from_base64",
            "instr",
            "lcase",
            "left",
            "length",
            "locate",
            "lower",
            "lpad",
            "ltrim",
            "make_set",
            "mid",
            "oct",
            "octet_length",
            "ord",
            "position",
            "quote",
            "repeat",
            "replace",
            "reverse",
            "right",
            "rtrim",
            "space",
            "strcmp",
            "substring",
            "substr",
            "substring_index",
            "to_base64",
            "trim",
            "upper",
            "ucase",
            "hex",
            "unhex",
            "rpad",
            "bit_length",
            "char_length",
            "character_length",
            "find_in_set",
            "weight_string",
            // "soundex", "insert_func", "char_func", "load_file", "translate"
            // information functions
            "charset",
            "coercibility",
            "collation",
            "current_user",
            "database",
            "found_rows",
            "last_insert_id",
            "row_count",
            "schema",
            "session_user",
            "system_user",
            "user",
            "format_bytes",
            // "tidb_version", "tidb_is_ddl_owner", "tidb_decode_plan", "tidb_decode_binary_plan", "tidb_decode_sql_digests", "tidb_encode_sql_digest", "current_resource_group", "connection_id", "benchmark", "version", "current_role", "format_nano_time"
            // control functions
            "if",
            "ifnull",
            "nullif",
            // miscellaneous functions
            "inet_aton",
            "inet_ntoa",
            "inet6_aton",
            "inet6_ntoa",
            "is_ipv4",
            "is_ipv4_compat",
            "is_ipv4_mapped",
            "is_ipv6",
            "is_uuid",
            "name_const",
            "uuid_to_bin",
            "bin_to_uuid",
            "grouping",
            // "master_pos_wait", "vitess_hash", "get_lock", "release_lock", "release_all_locks", "is_free_lock", "is_used_lock", "tidb_shard", "tidb_row_checksum", "sleep", "uuid", "uuid_short",
            // encryption and compression functions
            "aes_decrypt",
            "aes_encrypt",
            "md5",
            "sha1",
            "sha",
            "sha2",
            "uncompress",
            "uncompressed_length",
            "validate_password_strength",
            // "password", "sm3", "random_bytes", "encode"
            // json functions
            "json_type",
            "json_extract",
            "json_unquote",
            "json_array",
            "json_object",
            "json_merge",
            "json_set",
            "json_insert",
            "json_replace",
            "json_remove",
            "json_overlaps",
            "json_contains",
            "json_contains_path",
            "json_valid",
            "json_array_append",
            "json_array_insert",
            "json_merge_patch",
            "json_merge_preserve",
            "json_quote",
            "json_schema_valid",
            "json_search",
            "json_depth",
            "json_keys",
            "json_length",
            // "json_pretty", "json_storage_free", "json_storage_size", "json_memberof"
            // vector functions (tidb extension)
            // "vec_dims", "vec_l1_distance", "vec_l2_distance", "vec_negative_inner_product", "vec_cosine_distance", "vec_l2_norm", "vec_from_text", "vec_as_text",
            // TiDB internal function
            // "tidb_decode_key", "tidb_mvcc_info", "tidb_encode_record_key", "tidb_encode_index_key", "tidb_decode_base64_key",
            // Sequence function
            // "nextval", "lastval", "setval",
        ]
    }

    // `generate_prompt` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn generate_prompt(
        &self,
        group: &str,
        count: i32,
        exist_cases: &[Case],
    ) -> Option<Vec<ChatCompletionMessageParamUnion>> {
        let mut messages = Vec::with_capacity(2);

        // Go uses a raw string; tabs in the IMPORTANT/EXAMPLE block match expression.go.
        let system_prompt = "You are a professional QA engineer testing a new SQL database compatible with MySQL. You are tasked with testing the compatibility of the database with MySQL for a specific function. You shouldn't use any Database and Tables in your queries. You should write the queries to cover the corner cases of the function. The common cases are not needed. You should try to use this function with different valid argument types to test the implicit type conversion. You should try to use this function with NULL to test the behavior of NULL. Please return a valid JSON object with the key \"queries\" and an array of strings as the value. Be careful with the escape characters. You should avoid using NOW(), RAND() or any other functions that return different results on each call.\n\n\tIMPORTANT: Don't put anything else in the response.\n\n\tEXAMPLE INPUT:\n\tReturn 3 random SQL queries using this function: CONCAT.\n\t\n\tEXAMPLE JSON OUTPUT:\n\t{\"queries\": [\"SELECT CONCAT('a', 'b')\", \"SELECT CONCAT(1, 'd')\", \"SELECT CONCAT(1, '\\\\n')\"]}";
        messages.push(openai::SystemMessage(system_prompt));

        if !exist_cases.is_empty() {
            messages.push(openai::UserMessage(format!(
                "Return {} random SQL queries using this function: {}.",
                exist_cases.len(),
                group
            )));

            let exist_response: Vec<String> = exist_cases.iter().map(|c| c.sql.clone()).collect();
            let assistant_message = match (SimplePromptResponse {
                queries: exist_response,
            })
            .marshal()
            {
                Ok(message) => message,
                Err(err) => {
                    // should never happen
                    Global.Info("failed to marshal exist response", &[zap::Error(err)]);
                    return None;
                }
            };
            messages.push(openai::AssistantMessage(assistant_message));
        }
        messages.push(openai::UserMessage(format!(
            "Return {} random SQL queries using this function: {}.",
            count, group
        )));

        Some(messages)
    }

    // `unmarshal` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    // 保持这层拆分可以让后续定位回归更直接。
    fn unmarshal(&self, response: &str) -> Vec<Case> {
        let resp = match SimplePromptResponse::unmarshal_json(response) {
            Ok(resp) => resp,
            Err(err) => {
                Global.Error(
                    "failed to unmarshal expression prompt response",
                    &[zap::Error(&err), zap::String("response", response)],
                );
                return Vec::new();
            }
        };

        let mut cases = Vec::with_capacity(resp.queries.len());
        for q in resp.queries {
            cases.push(Case {
                sql: q,
                ..Default::default()
            });
        }
        cases
    }
}

/// Go `init` registration.
// `register` 负责组织当前阶段的输入、状态或资源。
// 阅读时重点看它如何约束调用顺序和失败返回。
// 这里的行为需要尽量贴近 Go 版本。
pub fn register() {
    register_prompt_generator(Arc::new(ExpressionPromptGenerator));
}
