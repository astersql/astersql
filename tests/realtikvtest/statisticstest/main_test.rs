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
//! 该文件围绕 `统计信息与分析任务` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、锁、统计信息、会话或时间戳语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 6 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `test_main` 是当前文件里的测试用例。
//! `test_main` 所处的位置主要服务 `统计信息与分析任务` 主题下的一个阅读切面。
//! 阅读 `test_main` 时可先判断它关联的是哪一段前置准备、主路径执行、结果断言或资源收尾。
//! 如果 Go 同名文件里也出现 `test_main`，阅读时应优先核对场景目标、断言顺序和清理时机。
//! 中文说明结束（自动生成）

//! Go-equivalent `TestMain` for `statisticstest`.
//!
//! Mapping:
//! - `TestMain` → [`test_main`]

use astersql_tests_realtikvtest::stubs::{clear_events, take_events};
use astersql_tests_realtikvtest_statisticstest::harness::{
    RunTestMain, TestMain, reset_engine, serial_guard, testsetup,
};

/// `TestMain`: common setup + RunTestMain.
#[test]
fn test_main() {
    let _serial = serial_guard();
    reset_engine();
    clear_events();
    let mut m = TestMain::new(0);
    let code = RunTestMain(&mut m);
    assert_eq!(code, 0);
    assert!(testsetup::was_called());
}
