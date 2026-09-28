// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 受限优化器 hint 单元测试。
//
// 覆盖：无守卫变量的 hint、变量已隐藏的 hint、变量仍可写的 hint、未列入受限列表的 hint。

/// 验证 `isRestrictedHint` 与 `hintGuardVars` / 系统变量可见性、只读性的联动语义。
#[test]
fn test_restricted_hint() {
    // memory_quota 对应变量被隐藏；max_execution_time 未在 RestrictedVariables 中，仍视为可调。
    let sem = buildSEMFromConfig(&Config {
        RestrictedVariables: vec![VariableRestriction {
            Name: vardef::TiDBMemQuotaQuery.to_string(),
            Hidden: true,
            ..Default::default()
        }],
        RestrictedHints: vec![
            "resource_group".to_string(),
            "memory_quota".to_string(),
            "max_execution_time".to_string(),
        ],
        ..Default::default()
    });

    // A hint with no backing variable is restricted unconditionally.
    // 无背后系统变量的 hint：只要列入 restricted_hints 即无条件限制。
    assert!(sem.isRestrictedHint("resource_group").is_err());
    // A variable-overriding hint whose variable is hidden is restricted.
    // 改写系统变量的 hint：变量已隐藏时仍限制。
    assert!(sem.isRestrictedHint("memory_quota").is_err());
    // A variable-overriding hint whose variable is still tunable is allowed.
    // 改写系统变量的 hint：变量仍可调时放行。
    assert!(sem.isRestrictedHint("max_execution_time").is_ok());
    // A hint not listed in restricted_hints is allowed.
    // 未配置为受限的 hint 始终放行。
    assert!(sem.isRestrictedHint("use_index").is_ok());
}
