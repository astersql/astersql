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

// 会话 `vars` 测试包入口 harness。
//
// Go 侧 `TestMain` 会短路 bench、初始化环境、清零 AsyncCommit 窗口并启用 failpoint。
// Rust 测试框架没有同构的 TestMain；本模块验证可移植的 benchmark 短路与测试
// runner 回调边界，环境初始化由 testkit/testsetup 的进程级入口负责。

use astersql_testkit_testmain::{TestingM, WrapTestingM, benchmark_exit_code};

#[derive(Clone, Copy)]
struct Runner(i32);

impl TestingM for Runner {
    fn run(&self) -> i32 {
        self.0
    }
}

/// 对应 Go `TestMain` 的可移植部分：仅非空 benchmark 过滤条件会短路执行，
/// 包装器会在底层测试结束后应用 goleak 等后置回调的退出码。
#[test]
fn vars_harness_short_circuits_bench_and_applies_runner_callback() {
    let runner = Runner(7);
    assert_eq!(benchmark_exit_code(&runner, ["vars"]), None);
    assert_eq!(
        benchmark_exit_code(&runner, ["vars", "--test.bench="]),
        None
    );
    assert_eq!(
        benchmark_exit_code(&runner, ["vars", "--test.bench=TestKVVars"]),
        Some(7)
    );

    let wrapped = WrapTestingM(runner, Some(Box::new(|status| status + 1)));
    assert_eq!(wrapped.run(), 8);
}
