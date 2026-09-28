// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 游标服务端测试的统一初始化入口。
//
// Rust 测试框架没有 Go 的 `TestMain`，因此这里用进程级 `Once` 初始化公共测试环境、
// TopSQL 和指标注册，并验证这些初始化不会意外改写默认全局配置。

use std::sync::Once;

// 同一测试进程内只允许注册一次全局指标。
static SETUP: Once = Once::new();

/// 执行游标测试共用的进程级初始化；重复调用也只会生效一次。
fn setup_cursor_tests() {
    SETUP.call_once(|| {
        astersql_testkit_testsetup::SetupForCommonTest();
        astersql_util_topsql_state::EnableTopSQL();
        // SAFETY：指标初始化会修改进程级全局注册表；外层 `SETUP` 保证本闭包串行且仅执行一次。
        unsafe {
            astersql_metrics::metrics::InitMetrics().expect("initialize cursor test metrics");
            astersql_metrics::metrics::RegisterMetrics().expect("register cursor test metrics");
        }
    });
}

#[test]
/// 守护公共初始化：应启用 TopSQL，但不得改写默认全局配置。
fn test_main_initializes_harness_without_mutating_global_config() {
    let default = astersql_config::new_config();
    setup_cursor_tests();
    let global = astersql_config::get_global_config();

    assert!(astersql_util_topsql_state::TopSQLEnabled());
    assert_eq!(format!("{default:?}"), format!("{global:?}"));
}
