// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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
//! 中文总览：`main_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `会话生命周期与信息模式` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 12 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `GlobalResetGuard` 是当前文件里的状态类型。
//! `GlobalResetGuard` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `GlobalResetGuard` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `GlobalResetGuard`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `enter` 是当前文件里的辅助函数。
//! `enter` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `enter` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `enter`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `drop` 是当前文件里的辅助函数。
//! `drop` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `drop` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `drop`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 符号 `test_main_runs_shared_realtikv_lifecycle` 是当前文件里的测试用例。
//! `test_main_runs_shared_realtikv_lifecycle` 所处的位置主要服务 `会话生命周期与信息模式` 主题下的一个阅读切面。
//! 阅读 `test_main_runs_shared_realtikv_lifecycle` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_main_runs_shared_realtikv_lifecycle`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 中文说明结束（自动生成）

//! Go-equivalent package `TestMain` lifecycle.

use std::time::Duration;

use astersql_tests_realtikvtest::RunTestMain;
use astersql_tests_realtikvtest::stubs::{
    TestMain as HarnessMain, config, goleak, reset_test_globals, set_mvcc_wait, take_events,
    testsetup, tikv, vardef,
};
use astersql_tests_realtikvtest_sessiontest::serial_guard;

struct GlobalResetGuard;

impl GlobalResetGuard {
    fn enter() -> Self {
        reset_test_globals();
        let _ = take_events();
        set_mvcc_wait(Duration::ZERO);
        Self
    }
}

impl Drop for GlobalResetGuard {
    fn drop(&mut self) {
        reset_test_globals();
        let _ = take_events();
        set_mvcc_wait(Duration::ZERO);
    }
}

/// Go `TestMain`: the shared RealTiKV setup must run before the wrapped test
/// process and preserve its exit status.
#[test]
fn test_main_runs_shared_realtikv_lifecycle() {
    let _serial = serial_guard();
    let _reset = GlobalResetGuard::enter();

    let mut main = HarnessMain::new(43);
    assert_eq!(RunTestMain(&mut main), 43);
    assert!(main.wrapped);
    assert!(testsetup::was_called());
    assert!(tikv::failpoints_enabled());
    assert!(goleak::verify_called());
    assert_eq!(vardef::schema_lease(), Duration::from_secs(5));

    let config = config::GetGlobalConfig();
    assert_eq!(config.TiKVClient.AsyncCommit.SafeWindow, 0);
    assert_eq!(config.TiKVClient.AsyncCommit.AllowedClockDrift, 0);

    let events = take_events();
    let setup = events
        .iter()
        .position(|event| event == "testsetup.SetupForCommonTest")
        .expect("common setup event");
    let update = events
        .iter()
        .position(|event| event == "config.UpdateGlobal")
        .expect("config update event");
    let failpoints = events
        .iter()
        .position(|event| event == "tikv.EnableFailpoints")
        .expect("failpoint enable event");
    let verify = events
        .iter()
        .position(|event| event == "goleak.VerifyTestMain:43")
        .expect("goleak verification event");
    assert!(setup < update && update < failpoints && failpoints < verify);
}
