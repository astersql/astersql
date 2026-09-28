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

//! 中文说明开始（自动生成）
//! 中文总览：`util_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `util_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 5 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `test_import_into_suite_fixtures` 是当前文件里的辅助函数。
//! 阅读 `test_import_into_suite_fixtures` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_import_into_suite_fixtures` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 中文说明结束（自动生成）

//! Go-equivalent suite fixtures from `util_test.go`.
//!
//! Mapping:
//! - `TestImportInto` → suite covered by per-file tests via [`MockGCSSuite`]
//! - `SetupSuite` / `TearDownSuite` / `BeforeTest` / `cleanupSysTables`
//!   → [`test_import_into_suite_fixtures`]

use astersql_tests_realtikvtest_importintotest::harness::{
    MockGCSSuite, RunTestMain, TestMain, gcs_endpoint, max_wait_time, prepare_and_use_db,
    reset_engine, serial_guard, vardef,
};
use std::time::Duration;

/// Suite fixture smoke: SetupSuite / TearDownSuite / cleanupSysTables / BeforeTest.
#[test]
fn test_import_into_suite_fixtures() {
    let _serial = serial_guard();
    reset_engine();
    let _ = RunTestMain(&mut TestMain::new(0));
    let s = MockGCSSuite::setup();
    assert!(gcs_endpoint().starts_with("http://127.0.0.1:4443"));
    assert_eq!(max_wait_time(), Duration::from_secs(30));
    assert_eq!(vardef::GetStatsLease(), Duration::from_secs(1));

    s.prepare_and_use_db("fixture_db");
    s.cleanup_sys_tables();
    prepare_and_use_db("fixture_db2", &s.tk);
    // BeforeTest recreates TestKit
    let tk2 = astersql_tests_realtikvtest_importintotest::harness::testkit::NewTestKit(
        &s.t,
        s.store.clone(),
    );
    tk2.MustExec("use fixture_db");
    s.tear_down();
}
