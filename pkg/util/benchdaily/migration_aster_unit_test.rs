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

// `benchdaily` 迁移期单元测试：对齐 Go 基准指标序列化与文件 IO 语义。
//
// 覆盖：`BenchmarkResult` → JSON 字段映射、截断写回、非法 JSON 报错、
// caller 短名解析，以及 `run_to_file` 执行并落盘。

use super::{
    BenchResult, Benchmark, BenchmarkResult, benchmark_result_to_json, caller_name,
    read_bench_result_from_file, run_to_file, write_bench_result_to_file,
};

/// 样例基准函数：对黑盒乘法做一次迭代，供 `run_to_file` / `caller_name` 使用。
fn sample_benchmark(bench: &mut Benchmark) {
    bench.iter(|| std::hint::black_box(2_u64.wrapping_mul(3)));
}

/// 验证指标转换与 Go 字段名/数值一致（ns/op、allocs、bytes）。
#[test]
fn converts_benchmark_metrics_like_go() {
    let result = benchmark_result_to_json("BenchmarkPointGet", BenchmarkResult::new(40, 3, 128));

    assert_eq!(
        result,
        BenchResult {
            name: "BenchmarkPointGet".to_owned(),
            ns_per_op: 40,
            allocs_per_op: 3,
            bytes_per_op: 128,
        }
    );
}

/// 验证写入会截断旧内容，读回后与 Go 兼容 JSON（含尾部换行）一致。
#[test]
fn round_trips_go_compatible_json_and_truncates_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bench_daily.json");
    std::fs::write(&path, b"stale bytes that must be truncated").unwrap();
    let expected = vec![BenchResult {
        name: "BenchmarkTxn".to_owned(),
        ns_per_op: 17,
        allocs_per_op: 2,
        bytes_per_op: 64,
    }];

    write_bench_result_to_file(&expected, &path).unwrap();

    assert_eq!(read_bench_result_from_file(&path).unwrap(), expected);
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "[{\"Name\":\"BenchmarkTxn\",\"NsPerOp\":17,\"AllocsPerOp\":2,\"BytesPerOp\":64}]\n"
    );
}

/// 验证非法 JSON 直接返回错误，而非给出部分解析结果。
#[test]
fn reports_invalid_json_instead_of_returning_partial_data() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken.json");
    std::fs::write(&path, b"[{").unwrap();

    assert!(read_bench_result_from_file(&path).is_err());
}

/// 验证从函数指针解析出短函数名（对应 Go 基准命名）。
#[test]
fn resolves_the_short_benchmark_function_name() {
    assert_eq!(caller_name(sample_benchmark), "sample_benchmark");
}

/// 验证 `run_to_file` 执行基准用例并写出至少一条非负 ns/op 结果。
#[test]
fn run_executes_benchmarks_and_writes_results() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("result.json");

    run_to_file(&[super::benchmark_case!(sample_benchmark)], &path).unwrap();

    let results = read_bench_result_from_file(path).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].name, "sample_benchmark");
    assert!(results[0].ns_per_op >= 0);
}
