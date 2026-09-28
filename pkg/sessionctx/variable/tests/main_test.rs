// Copyright 2021 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// `variable` 测试包入口（对应 Go `TestMain`）与已移除系统变量冒烟用例。
//
// Rust 测试框架没有 Go 的进程级 `TestMain` 钩子，因此直接执行公共初始化。

#[test]
fn go_test_main_contract_is_preserved() {
    astersql_testkit_testsetup::SetupForCommonTest();
}

use astersql_sessionctx_variable as variable;

/// 确认已移除系统变量注册表对测试包可见，且检查文案与 Go 一致。
#[test]
fn removed_sysvar_registry_is_available_to_the_test_package() {
    assert!(variable::removed::IsRemovedSysVar("tidb_enable_streaming"));
    let error = variable::CheckSysVarIsRemoved("tidb_enable_streaming").unwrap_err();
    assert!(error.contains("streaming is no longer supported"));
    variable::CheckSysVarIsRemoved("autocommit").unwrap();
}
