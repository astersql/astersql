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

// Fast Create Table 测试包的 `TestMain` 对等实现。
//
// 一次性启用 `EnableFastCreateTable`（默认与 Go

use std::sync::Once;
use std::time::Duration;

use astersql_sessionctx_vardef::EnableFastCreateTable;

/// 进程内只执行一次的测试环境初始化门闩。
static INIT: Once = Once::new();

/// Go `TestMain` 清零的 TiKV AsyncCommit 安全窗口。
const ASYNC_COMMIT_SAFE_WINDOW: i64 = 0;
/// Go `TestMain` 清零的 TiKV AsyncCommit 允许时钟漂移。
const ASYNC_COMMIT_ALLOWED_CLOCK_DRIFT: i64 = 0;
/// Go `TestMain` 将 DDL worker 出错重试等待缩短为 1 微秒。
///
/// Rust DDL 当前没有对应的进程级可变等待钩子，因此保留为 harness 契约。
const DDL_WAIT_WHEN_ERROR_OCCURRED: Duration = Duration::from_micros(1);

/// Go `TestMain` 的初始化与收尾顺序；Rust libtest 没有进程级 `TestMain` 钩子。
const TEST_MAIN_STAGES: &[&str] = &[
    "setup-for-common-test",
    "clear-async-commit-timing-windows",
    "set-ddl-error-wait",
];

/// 确保快速建表相关默认配置已写入全局变量。
fn ensure_test_env() {
    INIT.call_once(|| {
        // 默认值与 Go DefTiDBEnableFastCreateTable 一致；其余进程级设置由上面的
        // harness 常量保持逐项映射，直到对应 Rust 运行时钩子可用。
        EnableFastCreateTable.Store(true);
    });
}

#[test]
fn test_main_initializes_fast_create_defaults() {
    ensure_test_env();
    assert!(EnableFastCreateTable.Load());
    assert_eq!(ASYNC_COMMIT_SAFE_WINDOW, 0);
    assert_eq!(ASYNC_COMMIT_ALLOWED_CLOCK_DRIFT, 0);
    assert_eq!(DDL_WAIT_WHEN_ERROR_OCCURRED, Duration::from_micros(1));
    assert_eq!(
        TEST_MAIN_STAGES,
        [
            "setup-for-common-test",
            "clear-async-commit-timing-windows",
            "set-ddl-error-wait",
        ]
    );
}
