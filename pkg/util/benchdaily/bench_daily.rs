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

// 日常基准结果采集（benchdaily）：运行一组微基准并把指标写成 JSON。
//
// 对应 Go `pkg/util/benchdaily`。用于 CI/日常回归收集 `ns/op` 等指标；
// 提供与 Go `testing.B` 类似的迭代驱动、函数指针解析名称、`--outfile` 入口，
// 以及读写结果文件的兼容 API。

use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::Path;
use std::time::Instant;

/// 本模块操作的统一结果类型（装箱错误便于跨调用传播）。
pub type BenchDailyResult<T> = Result<T, Box<dyn Error + Send + Sync>>;
/// 基准函数签名：接收可变的 `Benchmark` 状态（迭代次数等）。
pub type BenchmarkFn = fn(&mut Benchmark);

/// 具名基准用例：静态名称 + 函数指针。
#[derive(Clone, Copy)]
pub struct BenchmarkCase {
    name: &'static str,
    function: BenchmarkFn,
}

impl BenchmarkCase {
    /// 构造具名用例。
    pub const fn new(name: &'static str, function: BenchmarkFn) -> Self {
        Self { name, function }
    }
}

/// 用函数路径字面量生成 `BenchmarkCase`（`stringify!` 作名称）。
#[macro_export]
macro_rules! benchmark_case {
    ($function:path) => {
        $crate::BenchmarkCase::new(stringify!($function), $function)
    };
}

/// JSON format for the final output file.
/// 最终汇总输出文件的 JSON 结构（日期、提交、结果列表）。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct BenchOutput {
    /// 采集日期字符串。
    pub date: String,
    /// 关联的提交标识。
    pub commit: String,
    /// 各基准结果条目。
    pub result: Vec<BenchResult>,
}

/// One benchmark result, using the same JSON field names as Go's exported fields.
/// 单条基准结果，JSON 字段名与 Go 导出字段一致。
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct BenchResult {
    /// 基准名称。
    pub name: String,
    /// 每次操作的纳秒数。
    pub ns_per_op: i64,
    /// 每次操作的分配次数（本原生跑法当前固定为 0）。
    pub allocs_per_op: i64,
    /// 每次操作的分配字节数（本原生跑法当前固定为 0）。
    pub bytes_per_op: i64,
}

/// Metrics produced by the small native benchmark runner.
/// 原生小型跑法产出的内部指标。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BenchmarkResult {
    ns_per_op: i64,
    allocs_per_op: i64,
    bytes_per_op: i64,
}

impl BenchmarkResult {
    /// 由三项指标构造。
    pub const fn new(ns_per_op: i64, allocs_per_op: i64, bytes_per_op: i64) -> Self {
        Self {
            ns_per_op,
            allocs_per_op,
            bytes_per_op,
        }
    }

    /// 每次操作纳秒数。
    pub const fn ns_per_op(self) -> i64 {
        self.ns_per_op
    }

    /// 每次操作分配次数。
    pub const fn allocs_per_op(self) -> i64 {
        self.allocs_per_op
    }

    /// 每次操作分配字节数。
    pub const fn allocated_bytes_per_op(self) -> i64 {
        self.bytes_per_op
    }
}

/// Benchmark state corresponding to the useful part of Go's `testing.B`.
/// 对应 Go `testing.B` 中与迭代相关的最小状态。
pub struct Benchmark {
    iterations: u64,
}

impl Benchmark {
    /// 指定迭代次数构造。
    fn new(iterations: u64) -> Self {
        Self { iterations }
    }

    /// 返回计划迭代次数。
    pub const fn iterations(&self) -> u64 {
        self.iterations
    }

    /// Executes `operation` once per benchmark iteration.
    /// 每个迭代执行一次 `operation`，并用 `black_box` 防止优化吞掉计算。
    pub fn iter<T>(&mut self, mut operation: impl FnMut() -> T) {
        for _ in 0..self.iterations {
            std::hint::black_box(operation());
        }
    }
}

/// 将内部 `BenchmarkResult` 转为可序列化的 `BenchResult`。
pub fn benchmark_result_to_json(name: impl Into<String>, result: BenchmarkResult) -> BenchResult {
    BenchResult {
        name: name.into(),
        ns_per_op: result.ns_per_op(),
        allocs_per_op: result.allocs_per_op(),
        bytes_per_op: result.allocated_bytes_per_op(),
    }
}

/// Returns the unqualified function item name without requiring runtime symbols.
/// 从类型名解析未限定函数名，无需运行时符号表。
pub fn caller_name<F: 'static>(_: F) -> String {
    std::any::type_name::<F>()
        .rsplit("::")
        .next()
        .unwrap_or_default()
        .to_owned()
}

/// 通过 backtrace 解析函数指针符号名，去掉 hash 后缀后取最后一段标识符。
fn caller_name_from_pointer(function: BenchmarkFn) -> String {
    let mut resolved = None;
    backtrace::resolve(function as *mut std::ffi::c_void, |symbol| {
        // A single address can have inline debug records. The first record is the
        // requested function; later records can name an inlined helper.
        // 同一地址可能有多条内联调试记录；取第一条为目标函数名。
        if resolved.is_none() {
            let Some(name) = symbol.name() else { return };
            resolved = Some(name.to_string());
        }
    });

    let full_name = resolved.unwrap_or_else(|| format!("{function:p}"));
    let without_hash = full_name
        .rsplit_once("::h")
        .map_or(full_name.as_str(), |(name, _)| name);
    without_hash
        .rsplit([':', '.'])
        .find(|part| !part.is_empty())
        .unwrap_or(without_hash)
        .to_owned()
}

/// 固定迭代次数运行基准函数，返回平均每次操作的纳秒数（分配指标置 0）。
fn execute_benchmark(function: BenchmarkFn) -> BenchmarkResult {
    // A fixed native sample makes the result deterministic in duration while retaining
    // Go's central behavior: execute the benchmark repeatedly and report nanoseconds/op.
    // 固定采样次数使耗时结果可复现，同时保留“重复执行并报告 ns/op”的核心行为。
    const ITERATIONS: u64 = 100;
    let mut benchmark = Benchmark::new(ITERATIONS);
    let started = Instant::now();
    function(&mut benchmark);
    let elapsed = started.elapsed().as_nanos() / u128::from(ITERATIONS);
    BenchmarkResult::new(i64::try_from(elapsed).unwrap_or(i64::MAX), 0, 0)
}

/// Runs benchmarks and writes their results. An empty path is the Go `outfile == ""` fast path.
/// 运行用例并写入结果；空路径对应 Go `outfile == ""` 的快速返回。
pub fn run_to_file(tests: &[BenchmarkCase], outfile: impl AsRef<Path>) -> BenchDailyResult<()> {
    if outfile.as_ref().as_os_str().is_empty() {
        return Ok(());
    }

    let results = tests
        .iter()
        .map(|test| benchmark_result_to_json(test.name, execute_benchmark(test.function)))
        .collect::<Vec<_>>();
    write_bench_result_to_file(&results, outfile)
}

/// Go-compatible entry point. It recognizes `--outfile value` and `--outfile=value`.
/// Go 兼容入口：识别 `--outfile value` / `--outfile=value`（及单横线形式）。
#[allow(non_snake_case)]
pub fn Run(tests: Vec<BenchmarkFn>) {
    let mut args = std::env::args().skip(1);
    let mut outfile = String::new();
    while let Some(arg) = args.next() {
        if arg == "--outfile" || arg == "-outfile" {
            outfile = args.next().unwrap_or_default();
        } else if let Some(value) = arg
            .strip_prefix("--outfile=")
            .or_else(|| arg.strip_prefix("-outfile="))
        {
            outfile = value.to_owned();
        }
    }
    let tests = tests
        .into_iter()
        .map(|function| {
            let name = caller_name_from_pointer(function);
            // Compatibility callers do not carry names at the type level. Keep the
            // resolved name alive for the duration of this short-lived command.
            // 兼容调用方无类型级名称；泄漏为 `'static` 以满足 `BenchmarkCase`。
            let name = Box::leak(name.into_boxed_str());
            BenchmarkCase::new(name, function)
        })
        .collect::<Vec<_>>();
    run_to_file(&tests, outfile).unwrap_or_else(|error| panic!("{error}"));
}

/// 从文件反序列化基准结果列表。
pub fn read_bench_result_from_file(file: impl AsRef<Path>) -> BenchDailyResult<Vec<BenchResult>> {
    let input = BufReader::new(File::open(file)?);
    Ok(serde_json::from_reader(input)?)
}

/// 将结果列表序列化为 JSON 并写入文件（末尾补换行）。
pub fn write_bench_result_to_file(
    results: &[BenchResult],
    file: impl AsRef<Path>,
) -> BenchDailyResult<()> {
    let mut output = BufWriter::new(File::create(file)?);
    serde_json::to_writer(&mut output, results)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

// Compatibility spellings for callers migrated mechanically from Go.
/// Go 风格命名：`benchmarkResultToJSON` → `benchmark_result_to_json`。
#[allow(non_snake_case)]
pub fn benchmarkResultToJSON(name: String, result: BenchmarkResult) -> BenchResult {
    benchmark_result_to_json(name, result)
}

/// Go 风格命名：按函数指针解析调用方名称。
#[allow(non_snake_case)]
pub fn callerName(function: BenchmarkFn) -> String {
    caller_name_from_pointer(function)
}

/// Go 风格命名：读结果文件，失败则 panic。
#[allow(non_snake_case)]
pub fn readBenchResultFromFile(file: &str) -> Vec<BenchResult> {
    read_bench_result_from_file(file).unwrap_or_else(|error| panic!("{error}"))
}

/// Go 风格命名：写结果文件，失败则 panic。
#[allow(non_snake_case)]
pub fn writeBenchResultToFile(results: Vec<BenchResult>, file: &str) {
    write_bench_result_to_file(&results, file).unwrap_or_else(|error| panic!("{error}"));
}
