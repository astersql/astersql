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

//! Long-running test case registry (Go `tools/check/longtests.go`).
//! 该模块集中维护需要通过 `--long` 才执行的测试白名单。
//! 键是包路径，值是该包内耗时较长或资源占用较高的用例名列表。
//! 数据保持与 Go 版本一致，让检查工具在两端筛选长测时得到相同结果。

use std::collections::HashMap;

/// Go `longTests` — packages/cases scheduled only under `--long`.
/// 返回静态注册表，供检查脚本按包名查出需要延后到长测阶段的具体用例。
pub fn long_tests() -> HashMap<&'static str, Vec<&'static str>> {
    HashMap::from([
        (
            "pkg/ttl/ttlworker",
            vec![
                "TestParallelLockNewJob",
                "TestParallelLockNewTask",
                "TestJobManagerWithFault",
            ],
        ),
        ("pkg/ttl/cache", vec!["TestRegionDisappearDuringSplitRange"]),
    ])
}

/// Go `longTestWorkerCount` — lower concurrency for long tests.
/// 长测通常更耗时且更吃机器资源，因此默认并发度比常规测试更低。
pub const LONG_TEST_WORKER_COUNT: usize = 2;
