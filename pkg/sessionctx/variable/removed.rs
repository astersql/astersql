// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 已从产品中移除的系统变量清单及查询/报错辅助函数。
//
// 用户若仍 SET/SHOW 这些变量，应得到明确的「不再支持」错误及迁移建议（Reason）。

/// 已移除系统变量表：`(变量名, 移除原因/替代建议)`；变量名大小写敏感匹配。
pub const REMOVED_SYS_VARS: &[(&str, &str)] = &[
    (
        "tidb_enable_alter_placement",
        "alter placement is now always enabled",
    ),
    (
        "tidb_enable_global_temporary_table",
        "temporary table support is now always enabled",
    ),
    ("tidb_slow_log_masking", "use tidb_redact_log instead"),
    (
        "placement_checks",
        "placement_checks is removed and use tidb_placement_mode instead",
    ),
    (
        "tidb_mem_quota_hashjoin",
        "use tidb_mem_quota_query instead",
    ),
    (
        "tidb_mem_quota_mergejoin",
        "use tidb_mem_quota_query instead",
    ),
    ("tidb_mem_quota_sort", "use tidb_mem_quota_query instead"),
    ("tidb_mem_quota_topn", "use tidb_mem_quota_query instead"),
    (
        "tidb_mem_quota_indexlookupreader",
        "use tidb_mem_quota_query instead",
    ),
    (
        "tidb_mem_quota_indexlookupjoin",
        "use tidb_mem_quota_query instead",
    ),
    ("tidb_enable_streaming", "streaming is no longer supported"),
    (
        "tidb_opt_broadcast_join",
        "tidb_opt_broadcast_join is removed and use tidb_allow_mpp instead",
    ),
    (
        "tidb_enable_change_multi_schema",
        "alter multiple schema objects in a table is now always enabled",
    ),
];

/// 判断变量名是否在已移除列表中（精确字符串匹配）。
pub fn IsRemovedSysVar(var_name: &str) -> bool {
    REMOVED_SYS_VARS.iter().any(|(name, _)| *name == var_name)
}

/// 若变量已移除则返回含原因的错误；否则 Ok(())。
pub fn CheckSysVarIsRemoved(var_name: &str) -> Result<(), String> {
    match REMOVED_SYS_VARS.iter().find(|(name, _)| *name == var_name) {
        Some((_, reason)) => Err(format!(
            "option '{var_name}' is no longer supported. Reason: {reason}"
        )),
        None => Ok(()),
    }
}
