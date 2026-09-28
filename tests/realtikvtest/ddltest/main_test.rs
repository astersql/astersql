// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 中文总览：本文件承担 DDL 元信息、region scatter 与版本推进 中的 测试入口与生命周期装配。
// 中文总览：重点在于 TestMain 配置、参数传播和清理顺序。
// 中文总览：当前任务只补充注释，不改任何 SQL、断言、参数或桩实现。
// 中文总览：阅读时可先看公共搭建，再看核心动作，最后看状态观察和收尾清理。
// 中文总览：这类 RealTiKV 回归通常依赖时序、共享状态和外部副作用，因此顺序本身就是语义。
// 中文总览：Rust 版本继续保留与 Go 对照实现接近的行为边界，避免迁移后只剩表面通过。
// 中文总览：注释会优先解释为什么这样验证，而不是逐行翻译语法或重复函数名。
// 中文总览：下面的索引用于快速定位 helper、公共模块和场景 case 的职责分工。
// 中文总览：类型 `GlobalResetGuard` 负责 GlobalResetGuard。
// 中文总览：函数 `enter` 负责 进入生命周期。
// 中文总览：函数 `drop` 负责 收尾删除。
// 中文总览：函数 `test_main_configures_tikv_and_runs_shared_lifecycle` 负责 主流程 configures tikv and runs shared lifecycle。

//! Go-equivalent package `TestMain` lifecycle.

use std::time::Duration;

use astersql_tests_realtikvtest::stubs::{
    TestMain, config, goleak, reset_test_globals, take_events, testsetup, tikv, vardef,
};
use astersql_tests_realtikvtest::{RunTestMain, UpdateTiDBConfig, WithRealTiKV};
use astersql_tests_realtikvtest_ddltest::serial_guard;

// 该类型围绕 GlobalResetGuard 组织字段或状态视图。
// 它的存在通常是为了让测试更容易携带中间结果，而不是为了扩展业务模型本身。
// 通过把观测值收拢成结构化对象，断言层就不必反复解析底层返回内容。

struct GlobalResetGuard;

impl GlobalResetGuard {
    // 该辅助函数负责 进入生命周期。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    fn enter() -> Self {
        reset_test_globals();
        let _ = take_events();
        Self
    }
}

impl Drop for GlobalResetGuard {
    // 该辅助函数负责 收尾删除。
    // 它把重复出现的准备、读取、校验或清理步骤集中到一处，减少 case 间样板差异。
    // 这样调用方可以把注意力放在场景本身，而不是散落的底层桩对象拼装细节。

    fn drop(&mut self) {
        reset_test_globals();
        let _ = take_events();
    }
}

/// Go `TestMain`: select the TiKV store, apply the RealTiKV endpoint
/// configuration, then run the shared package harness.
// 该用例覆盖 主流程 configures tikv and runs shared lifecycle。
// 它负责把当前文件最关键的一条产品语义固定成可回归的观察序列。
// 开始阶段通常会准备会话、存储、failpoint 或串行守卫，先把环境噪声降到最低。

#[test]
fn test_main_configures_tikv_and_runs_shared_lifecycle() {
    let _serial = serial_guard();
    let _reset = GlobalResetGuard::enter();

    config::UpdateGlobal(|conf| conf.Store = config::StoreTypeTiKV.to_owned());
    UpdateTiDBConfig();

    let mut main = TestMain::new(37);
    assert_eq!(RunTestMain(&mut main), 37);

    let conf = config::GetGlobalConfig();
    assert_eq!(conf.Store, config::StoreTypeTiKV);
    assert_eq!(conf.Path, "127.0.0.1:2379");
    assert_eq!(conf.TiKVClient.AsyncCommit.SafeWindow, 0);
    assert_eq!(conf.TiKVClient.AsyncCommit.AllowedClockDrift, 0);
    assert!(main.wrapped);
    assert!(WithRealTiKV());
    assert!(testsetup::was_called());
    assert!(tikv::failpoints_enabled());
    assert!(goleak::verify_called());
    assert_eq!(vardef::schema_lease(), Duration::from_secs(5));

    let events = take_events();
    let config_updates = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| (event == "config.UpdateGlobal").then_some(index))
        .collect::<Vec<_>>();
    assert_eq!(
        config_updates.len(),
        3,
        "store, endpoint, and shared harness updates"
    );
    let setup = events
        .iter()
        .position(|event| event == "testsetup.SetupForCommonTest")
        .expect("common setup event");
    let failpoints = events
        .iter()
        .position(|event| event == "tikv.EnableFailpoints")
        .expect("failpoint setup event");
    let verify = events
        .iter()
        .position(|event| event == "goleak.VerifyTestMain:37")
        .expect("goleak verification event");
    assert!(
        config_updates[0] < config_updates[1]
            && config_updates[1] < setup
            && setup < config_updates[2]
            && config_updates[2] < failpoints
            && failpoints < verify
    );
}
