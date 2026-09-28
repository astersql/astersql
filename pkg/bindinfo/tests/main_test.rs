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

// bindinfo 集成测试的入口模块（对应 Go 的 `main_test.go`）。
//
// bindinfo（SQL 绑定）是数据库内核中把某条 SQL 语句固定到指定执行计划
// （执行计划：优化器为 SQL 生成的具体执行步骤）的机制，用于在不改业务
// SQL 的前提下干预优化器的选择。
//
/// 校验 bindinfo crate 对外暴露的日志器名称与错误信息格式保持稳定。
///
/// - `bindingLogger()` 应返回固定的日志分类名 "bindinfo"，
///   便于在内核日志中按模块过滤 SQL 绑定相关日志；
/// - `BindError` 的 `Display` 输出应与构造时传入的消息一致，
///   保证错误信息在向上层（如客户端报错）传播时不被改写。
#[test]
fn canonical_bindinfo_logger_and_error_surface_are_stable() {
    assert_eq!(astersql_bindinfo::bindingLogger(), "bindinfo");
    assert_eq!(
        astersql_bindinfo::BindError("storage unavailable".to_owned()).to_string(),
        "storage unavailable"
    );
}

/// Go `TestMain` 的公共测试初始化必须在 Rust 测试套件中可重复调用。
#[test]
fn test_main_setup() {
    astersql_testkit_testsetup::SetupForCommonTest();
    assert_eq!(
        astersql_bindinfo::bindingLogger(),
        "bindinfo",
        "common setup must leave the bindinfo logger surface available"
    );
}
