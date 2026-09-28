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

// 基准测试短路入口，对齐 Go `test.bench` 标志行为。
//
// 当命令行带有非空的 `-test.bench` / `--test.bench` 时，直接运行 `TestingM`
// 并返回/退出进程，绕过 TestMain 里其它套件配置。

use std::ffi::OsStr;

use super::TestingM;

fn test_flag_takes_value(argument: &str) -> bool {
    matches!(
        argument,
        "-test.benchtime"
            | "--test.benchtime"
            | "-test.blockprofile"
            | "--test.blockprofile"
            | "-test.blockprofilerate"
            | "--test.blockprofilerate"
            | "-test.count"
            | "--test.count"
            | "-test.coverprofile"
            | "--test.coverprofile"
            | "-test.cpu"
            | "--test.cpu"
            | "-test.cpuprofile"
            | "--test.cpuprofile"
            | "-test.fuzz"
            | "--test.fuzz"
            | "-test.fuzzcachedir"
            | "--test.fuzzcachedir"
            | "-test.fuzzminimizetime"
            | "--test.fuzzminimizetime"
            | "-test.fuzztime"
            | "--test.fuzztime"
            | "-test.gocoverdir"
            | "--test.gocoverdir"
            | "-test.list"
            | "--test.list"
            | "-test.memprofile"
            | "--test.memprofile"
            | "-test.memprofilerate"
            | "--test.memprofilerate"
            | "-test.mutexprofile"
            | "--test.mutexprofile"
            | "-test.mutexprofilefraction"
            | "--test.mutexprofilefraction"
            | "-test.outputdir"
            | "--test.outputdir"
            | "-test.parallel"
            | "--test.parallel"
            | "-test.run"
            | "--test.run"
            | "-test.shuffle"
            | "--test.shuffle"
            | "-test.skip"
            | "--test.skip"
            | "-test.timeout"
            | "--test.timeout"
            | "-test.trace"
            | "--test.trace"
    )
}

/// Runs the test runner when `args` contains a non-empty `test.bench` flag.
///
/// Returning the exit code separately keeps the flag behavior testable; the
/// process-facing wrapper below performs the same immediate exit as Go.
/// 若参数含非空 `test.bench` 标志则运行测试并返回退出码；否则返回 None 以便单测验证。
pub fn benchmark_exit_code<I, S>(testing_m: &dyn TestingM, args: I) -> Option<i32>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    // 跳过可执行文件名，再按 Go flag 包的顺序语义扫描两种 test.bench 写法。
    let mut args = args.into_iter().skip(1).peekable();
    let mut benchmark_enabled = None;

    while let Some(argument) = args.next() {
        let argument = argument.as_ref().to_string_lossy();

        // Go flag.Parse 遇到 `--` 或首个非 flag 参数即停止解析。
        if argument == "--" || !argument.starts_with('-') || argument == "-" {
            break;
        }

        let benchmark = argument
            .strip_prefix("-test.bench=")
            .or_else(|| argument.strip_prefix("--test.bench="));

        if let Some(benchmark) = benchmark {
            // flag 包允许重复设置，最终值覆盖先前值。
            benchmark_enabled = Some(!benchmark.is_empty());
            continue;
        }

        if argument == "-test.bench" || argument == "--test.bench" {
            benchmark_enabled = Some(
                args.next()
                    .map(|value| !value.as_ref().is_empty())
                    .unwrap_or(false),
            );
            continue;
        }

        // testing 包注册的非布尔标志会消费紧随其后的独立值。
        if test_flag_takes_value(&argument) {
            let _ = args.next();
        }
    }

    benchmark_enabled.unwrap_or(false).then(|| testing_m.run())
}

/// Runs benchmarks directly despite how `TestMain` configures the test suite.
/// 进程侧包装：检测到基准标志后立即以对应退出码结束进程。
#[allow(non_snake_case)]
pub fn ShortCircuitForBench(testing_m: &dyn TestingM) {
    if let Some(exit_code) = benchmark_exit_code(testing_m, std::env::args_os()) {
        std::process::exit(exit_code);
    }
}
