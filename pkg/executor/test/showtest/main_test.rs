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

// SHOW 测试包级冒烟：库名列表排序语义。
//
// 对应 Go `pkg/executor/test/showtest/main_test.go` 中与 SHOW DATABASES
// 相关的顺序约定：系统库 `INFORMATION_SCHEMA` 应排在用户库之前，便于
// 客户端与兼容层固定看到元数据视图优先出现。

/// 与 Go `executor.TestMoveInfoSchemaToFront` 保持相同的表驱动输入/输出。
#[test]
fn show_database_order_places_information_schema_first() {
    let cases = [
        (vec![], vec![]),
        (
            vec!["A", "B", "C", "a", "b", "c"],
            vec!["A", "B", "C", "a", "b", "c"],
        ),
        (
            vec!["A", "B", "C", "INFORMATION_SCHEMA"],
            vec!["INFORMATION_SCHEMA", "A", "B", "C"],
        ),
        (
            vec!["A", "B", "INFORMATION_SCHEMA", "a"],
            vec!["INFORMATION_SCHEMA", "A", "B", "a"],
        ),
        (vec!["INFORMATION_SCHEMA"], vec!["INFORMATION_SCHEMA"]),
        (
            vec!["A", "B", "C", "INFORMATION_SCHEMA", "a", "b"],
            vec!["INFORMATION_SCHEMA", "A", "B", "C", "a", "b"],
        ),
    ];

    for (database_names, expected_names) in cases {
        let mut databases = database_names
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        let expected = expected_names
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        astersql_executor::show::moveInfoSchemaToFront(&mut databases);
        assert_eq!(databases, expected);
    }
}

/// 对应 Go `TestMain`：固定 autoid 步长并临时覆盖测试相关全局配置。
#[test]
fn test_main_applies_go_test_configuration_and_restores_it() {
    let original_step = astersql_meta_autoid::get_step();
    let restore = astersql_config::restore_func();

    astersql_meta_autoid::set_step(5_000);
    astersql_config::update_global(|config| {
        config.instance.slow_threshold = 30_000;
        config.tikv_client.async_commit.safe_window = 0;
        config.tikv_client.async_commit.allowed_clock_drift = 0;
        config.experimental.allows_expression_index = true;
    });

    assert_eq!(astersql_meta_autoid::get_step(), 5_000);
    let config = astersql_config::get_global_config();
    assert_eq!(config.instance.slow_threshold, 30_000);
    assert_eq!(config.tikv_client.async_commit.safe_window, 0);
    assert_eq!(config.tikv_client.async_commit.allowed_clock_drift, 0);
    assert!(config.experimental.allows_expression_index);

    restore();
    astersql_meta_autoid::set_step(original_step);
    assert_eq!(astersql_meta_autoid::get_step(), original_step);
}
