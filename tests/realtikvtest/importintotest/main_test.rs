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
//! 中文总览：`main_test.rs` 只补充中文说明，不改任何可执行逻辑。
//! 该文件围绕 `main_test` 主题组织测试入口、辅助封装或模块接线。
//! 阅读时可优先关注前置准备、主路径执行、结果断言和资源收尾四个层次。
//! 这些注释补充职责、边界和 Go 对齐意图，不重复 Rust 语法本身。
//! 如果文件同时包含 SQL、对象存储、任务状态或锁语义，应把它们视为同一场景的不同观察面。
//! 本轮工作保持许可证、英文注释、现有断言和所有代码路径原样不动。
//! 计划要求本文件至少达到 4 行中文注释，下面用索引式说明补足阅读背景。
//! 当 Rust 与 Go 同名文件并存时，建议优先将同名场景视为语义参照。
//! 符号 `test_main` 是当前文件里的辅助函数。
//! 阅读 `test_main` 时可先判断它服务的是哪一个测试阶段或哪一类前置条件。
//! `test_main` 保留现有调用顺序和返回契约，本次只补中文背景说明。
//! 中文说明结束（自动生成）

//! Go-equivalent `TestMain` for `importintotest`.
//!
//! Mapping:
//! - `init` / `TestMain` → [`test_main`]

use astersql_tests_realtikvtest_importintotest::harness::{
    RunTestMain, TestMain, UpdateTiDBConfig, config, reset_engine, serial_guard,
};

/// `TestMain`: UpdateTiDBConfig + RunTestMain (Go entry).
#[test]
fn test_main() {
    let _serial = serial_guard();
    reset_engine();
    config::UpdateGlobal(|conf| {
        conf.Store = config::StoreTypeTiKV.to_string();
    });
    UpdateTiDBConfig();
    let cfg = config::GetGlobalConfig();
    assert_eq!(cfg.Path, "127.0.0.1:2379");

    let mut m = TestMain::new(0);
    let code = RunTestMain(&mut m);
    assert_eq!(code, 0);
}
