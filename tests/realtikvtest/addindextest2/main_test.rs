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

// 中文总览：本文件承担 RealTiKV 加索引、分布式回填与全局排序 中的 测试入口与生命周期装配。
// 中文总览：重点在于 TestMain 配置、参数传播和清理顺序。
// 中文总览：当前任务只补充注释，不改任何 SQL、断言、参数或桩实现。
// 中文总览：阅读时可先看公共搭建，再看核心动作，最后看状态观察和收尾清理。
// 中文总览：这类 RealTiKV 回归通常依赖时序、共享状态和外部副作用，因此顺序本身就是语义。
// 中文总览：Rust 版本继续保留与 Go 对照实现接近的行为边界，避免迁移后只剩表面通过。
// 中文总览：注释会优先解释为什么这样验证，而不是逐行翻译语法或重复函数名。
// 中文总览：下面的索引用于快速定位 helper、公共模块和场景 case 的职责分工。
// 中文总览：函数 `test_full_mode_flag` 负责 full mode flag。
// 中文总览：函数 `test_main` 负责 主流程。

//! Go-equivalent `TestMain` / `FullMode` for `addindextest2`.
//!
//! Mapping:
//! - `FullMode` → [`FULL_MODE`] / [`test_full_mode_flag`]
//! - `TestMain` → [`test_main`]

use astersql_tests_realtikvtest_addindextest2::harness::{
    FULL_MODE, FullMode, RunTestMain, TestMain, UpdateTiDBConfig, config, serial_guard,
};
use std::sync::atomic::Ordering;

/// `FullMode` flag default matches Go `flag.Bool("full-mode", false, ...)`.
// 该用例覆盖 full mode flag。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。

#[test]
fn test_full_mode_flag() {
    let _serial = serial_guard();
    assert!(!FullMode(), "Go FullMode defaults to false");
    assert!(!FULL_MODE.load(Ordering::SeqCst));
}

/// `TestMain`: set store=TiKV, UpdateTiDBConfig, RunTestMain.
// 该用例覆盖 主流程。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。

#[test]
fn test_main() {
    let _serial = serial_guard();
    config::UpdateGlobal(|conf| {
        conf.Store = config::StoreTypeTiKV.to_string();
    });
    assert_eq!(config::GetGlobalConfig().Store, config::StoreTypeTiKV);

    UpdateTiDBConfig();
    let cfg = config::GetGlobalConfig();
    assert_eq!(cfg.Path, "127.0.0.1:2379");

    let mut m = TestMain::new(0);
    let code = RunTestMain(&mut m);
    assert_eq!(code, 0);
}
