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

// testmain 迁移期单元测试。
//
// 覆盖 `WrapTestingM` 默认恒等回调与自定义回调，以及 `benchmark_exit_code`
// 对空/非空 `-test.bench` 过滤串的短路行为。

use std::cell::Cell;

use super::{TestingM, WrapTestingM, benchmark_exit_code};

/// 可记录调用次数并固定返回退出码的假测试运行器。
struct FakeTestingM {
    exit_code: i32,
    runs: Cell<usize>,
}

impl FakeTestingM {
    /// 构造固定退出码的假运行器。
    fn new(exit_code: i32) -> Self {
        Self {
            exit_code,
            runs: Cell::new(0),
        }
    }
}

impl TestingM for FakeTestingM {
    fn run(&self) -> i32 {
        self.runs.set(self.runs.get() + 1);
        self.exit_code
    }
}

/// 未提供回调时，包装器应原样返回底层 `run` 的退出码。
#[test]
fn wrap_testing_m_uses_identity_callback_when_none() {
    let runner = FakeTestingM::new(7);
    let wrapped = WrapTestingM::new(&runner, None);

    assert_eq!(wrapped.run(), 7);
    assert_eq!(runner.runs.get(), 1);
}

/// 提供回调时，应在底层 `run` 之后对退出码做变换。
#[test]
fn wrap_testing_m_applies_callback_after_underlying_run() {
    let runner = FakeTestingM::new(7);
    let wrapped = WrapTestingM::new(&runner, Some(Box::new(|exit_code| exit_code + 4)));

    assert_eq!(wrapped.run(), 11);
    assert_eq!(runner.runs.get(), 1);
}

/// 无 bench 标志或过滤串为空时不应触发运行。
#[test]
fn benchmark_without_a_nonempty_filter_does_not_run() {
    let runner = FakeTestingM::new(9);

    assert_eq!(benchmark_exit_code(&runner, ["tidb-test"]), None);
    assert_eq!(
        benchmark_exit_code(&runner, ["tidb-test", "-test.bench="]),
        None
    );
    assert_eq!(runner.runs.get(), 0);
}

/// 非空 bench 过滤串时应运行并返回底层退出码。
#[test]
fn benchmark_with_a_filter_runs_and_returns_its_exit_code() {
    let runner = FakeTestingM::new(9);

    assert_eq!(
        benchmark_exit_code(&runner, ["tidb-test", "-test.bench", "BenchmarkDDL"]),
        Some(9)
    );
    assert_eq!(runner.runs.get(), 1);
}
