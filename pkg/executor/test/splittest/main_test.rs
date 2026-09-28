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

// splittest 的 Region 步长冒烟；功能测试在 `split_table_test.rs`。

/// 本包真正的功能覆盖在 `split_table_test.rs` 里对
/// `astersql-util-regionsplit`/`astersql-executor::split` 的真实调用，这里额外
/// 做一次最小烟雾测试，确认 `MinRegionStepValue`（`split_table_test.rs` 依赖的
/// 全局状态）在独立于其它测试模块之外也可用、真实可执行。
#[test]
fn min_region_step_value_smoke_check() {
    let value =
        astersql_util_regionsplit::MinRegionStepValue.load(std::sync::atomic::Ordering::Acquire);
    assert!(value > 0);
}
