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

// `NewNoopRegistry` 契约测试：注册恒成功、注销恒为 true。
//
// 对应 Go `TestNoopRegistry`；验证空操作注册表忽略输入且不因重复注册失败。

use super::NewNoopRegistry;
use prometheus::{Counter, GaugeVec, Opts};

// test_noop_registry 对应 Go 的 TestNoopRegistry。
// NoopRegistry 的契约是所有注册操作忽略输入并成功，注销操作始终返回 true。
/// 验证 NoopRegistry：Register 成功、重复注册成功、Unregister 恒 true。
#[test]
fn test_noop_registry() {
    let reg = NewNoopRegistry();

    // Registering a metric should not fail.
    // 首次注册 Counter 应成功。
    let result = reg.Register(Box::new(
        Counter::with_opts(Opts::new("duplicate_counter", "duplicate counter"))
            .expect("counter options should be valid"),
    ));
    assert!(result.is_ok());

    // Registering a metric twice should not fail because this registry does nothing.
    // 用相同描述再次注册 Counter 仍应成功（空实现不查重）。
    let result = reg.Register(Box::new(
        Counter::with_opts(Opts::new("duplicate_counter", "duplicate counter"))
            .expect("counter options should be valid"),
    ));
    assert!(result.is_ok());

    // Unregistering a metric should always succeed.
    // 注销未真正登记过的指标也应返回 true。
    let unregistered = reg.Unregister(Box::new(
        Counter::with_opts(Opts::new("third_counter", "third counter"))
            .expect("counter options should be valid"),
    ));
    assert!(unregistered);

    // GaugeVec 注销同样恒成功。
    let unregistered = reg.Unregister(Box::new(
        GaugeVec::new(Opts::new("gauge", "gauge"), &[]).expect("gauge options should be valid"),
    ));
    assert!(unregistered);
}
