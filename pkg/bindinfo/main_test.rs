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

// bindinfo 测试套件的入口文件（对应 Go 版 TiDB 的 `main_test.go`）。
//
// bindinfo（SQL 绑定，SQL Plan Binding）模块负责把某条 SQL 语句与一组优化器
// 提示（hint）绑定在一起，使优化器在生成执行计划（Execution Plan，即数据库
// 实际执行查询的步骤序列）时强制采用指定的索引或连接方式，从而稳定查询性能。
//
// 本文件包含两部分内容：
// 本文件加载 golden 测试数据，并验证绑定 SQL 的规范化生成逻辑。

/// Rust 测试均只读 golden 数据，因此无需 Go `BookKeeper` 的进程级可变状态和
/// 退出时写回回调。编译期嵌入同时保证测试不依赖当前工作目录。
fn binding_auto_suite_data() -> (serde_json::Value, serde_json::Value) {
    let input = serde_json::from_str(include_str!("testdata/binding_auto_suite_in.json"))
        .expect("binding_auto_suite input must be valid JSON");
    let output = serde_json::from_str(include_str!("testdata/binding_auto_suite_out.json"))
        .expect("binding_auto_suite output must be valid JSON");
    (input, output)
}

/// 验证绑定 SQL 的规范化生成：`GenerateBindingSQL` 应当在原始 SQL 中
/// 注入优化器提示注释（`/*+ ... */`，用于指示优化器选择特定索引等策略），
/// 并把语句中引用的表名补全为 `库名.表名` 的完全限定形式，
/// 以保证绑定在任意当前数据库上下文中都能正确匹配。
#[test]
fn canonical_binding_sql_generation_restores_db_and_injects_hint() {
    let statement = crate::Statement {
        SQL: "select * from orders".to_owned(),
        Tables: vec![crate::TableName {
            Name: "orders".to_owned(),
            ..crate::TableName::default()
        }],
        ..crate::Statement::default()
    };
    assert_eq!(
        crate::GenerateBindingSQL(&statement, "use_index(orders, idx)", "app"),
        "SELECT /*+ use_index(orders, idx) */ * FROM `app`.`orders`"
    );
}

#[test]
fn test_main_contract_is_executable_and_loads_binding_auto_suite() {
    let (input, output) = binding_auto_suite_data();
    let input = input.as_array().expect("suite input must be an array");
    let output = output.as_array().expect("suite output must be an array");
    assert_eq!(input.len(), 2);
    assert_eq!(input.len(), output.len());

    for (input_suite, output_suite) in input.iter().zip(output) {
        assert_eq!(input_suite["name"], output_suite["Name"]);
        assert_eq!(
            input_suite["cases"].as_array().map(Vec::len),
            output_suite["Cases"].as_array().map(Vec::len)
        );
    }
}
