// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// 每日基准注册入口。Go 的 benchdaily.Run 在未指定 outfile 时直接返回，
// 因此常规 cargo test 只验证注册清单，不重复执行昂贵的 benchmark 数据准备。

/// 对应 Go `TestBenchDaily` 传给 `benchdaily.Run` 的完整且有序的基准清单。
const DAILY_BENCHMARKS: &[&str] = &["BenchmarkBuildHistAndTopN"];

/// 注册统计模块的每日基准；常规测试环境没有 outfile，语义对应 Go 的快速返回路径。
fn benchdaily_run(benchmarks: &[&str]) -> usize {
    benchmarks.len()
}

/// 保证每日任务注册 Go `BenchmarkBuildHistAndTopN`，且不混入无关 CMSketch 场景。
#[test]
#[allow(non_snake_case)]
fn TestBenchDaily() {
    assert_eq!(DAILY_BENCHMARKS, &["BenchmarkBuildHistAndTopN"]);
    assert_eq!(benchdaily_run(DAILY_BENCHMARKS), 1);
}
