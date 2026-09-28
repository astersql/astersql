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

// 聚合函数包测试入口（对应 Go `TestMain`）。
//
// Rust 测试入口执行与 Go `TestMain` 相同的公共初始化。

/// 执行 Go `TestMain` 在测试主体前执行的公共初始化。
fn initialize_test_runtime() {
    astersql_testkit_testsetup::SetupForCommonTest();
}

/// 冒烟：真实 COUNT 聚合器可更新，且测试线程未处于 panic 状态。
#[test]
fn aggregate_test_runtime_initializes_real_production_state() {
    initialize_test_runtime();
    let mut count = crate::func_count::CountAggregator::default();
    count.update([Some(1_u8)]).unwrap();
    assert_eq!(count.value(), 1);
    assert_eq!(std::thread::panicking(), false);
}
