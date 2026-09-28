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

// 本文件对应 Go `main_test.go` 的包级 TestMain。
//
// 装载任何 suite，收尾回调只生成可能的输出并原样返回退出码；Rust 用纯函数保护
// 这一退出码传播契约。Cargo 的测试 harness 已在进入测试前完成参数解析。

use astersql_parser::Parser;

/// 对应 Go TestMain callback：空 BookKeeper 收尾后不得改变测试退出码。
fn test_main_callback(exit_code: i32) -> i32 {
    exit_code
}

/// Rust 自研的附加冒烟：解析「建表 + 插入 + 查询」三语句批，确认语句数与无告警。
#[test]
fn package_harness_parses_the_full_fixture_batch() {
    // 三语句以分号分隔，模拟 issue fixture 的最小批。
    let (statements, warnings) = Parser::default()
        .Parse(
            "create table t(a int); insert into t values (1); select * from t",
            "",
            "",
        )
        .unwrap();
    assert_eq!(statements.len(), 3);
    assert!(warnings.is_empty());
}
