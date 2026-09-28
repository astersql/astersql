// Copyright 2026 AsterSQL.
// Copyright 2020-present PingCAP, Inc.
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

// fastrand 随机 API 单元测试与并行基准对照。
//
// 对应 Go `random_test.go`：校验有界随机与桶覆盖率，并用多线程并行调用
// 对比 `Buf`/`Uint32N`/`Uint32` 与标准库 `rand` 的吞吐路径。

use super::{Buf, Uint32, Uint32N, Uint64N};
use rand::Rng;
use std::hint::black_box;
use std::thread;

/// 每个工作线程内重复调用随机 API 的次数。
const PARALLEL_ITERATIONS: usize = 256;

/// 按可用并行度启动线程，各自执行 `PARALLEL_ITERATIONS` 次 `operation`。
fn run_parallel(operation: impl Fn() + Sync) {
    let workers = thread::available_parallelism()
        .map(|parallelism| parallelism.get())
        .unwrap_or(1);
    thread::scope(|scope| {
        for _ in 0..workers {
            let operation = &operation;
            scope.spawn(move || {
                for _ in 0..PARALLEL_ITERATIONS {
                    operation();
                }
            });
        }
    });
}

// test_rand 对应 Go 的 TestRand。
// Go 先检查 Uint32N/Uint64N 上界，再用 1024 次抽样估算 256 个桶的覆盖率。
#[test]
fn test_rand() {
    super::main_test::setup_for_common_test();
    let x = Uint32N(1024);
    assert!(x < 1024_u32);
    let y = Uint64N(1_u64 << 63);
    assert!(y < 1_u64 << 63);

    // Buf(20) 的返回值只用于触发随机缓冲区生成路径，Go 原测试不检查内容。
    let _ = Buf(20);
    let mut arr = [false; 256];
    for _ in 0..1024 {
        let idx = Uint32N(256) as usize;
        arr[idx] = true;
    }

    let sum = arr.iter().filter(|occupied| !**occupied).count();
    assert!(sum < 24, "too many unvisited buckets: {sum}");
}

// benchmark_fast_rand_buf 对应 Go 的 BenchmarkFastRandBuf。
#[test]
fn benchmark_fast_rand_buf() {
    super::main_test::setup_for_common_test();
    run_parallel(|| {
        black_box(Buf(20));
    });
}

// benchmark_fast_rand_uint32_n 对应 Go 的 BenchmarkFastRandUint32N。
#[test]
fn benchmark_fast_rand_uint32_n() {
    super::main_test::setup_for_common_test();
    run_parallel(|| {
        black_box(Uint32N(127));
    });
}

// benchmark_fast_rand 对应 Go 的 BenchmarkFastRand。
#[test]
fn benchmark_fast_rand() {
    super::main_test::setup_for_common_test();
    run_parallel(|| {
        black_box(Uint32());
    });
    eprintln!("fast random sample: {}", Uint32());
}

// benchmark_global_rand 对应 Go 的 BenchmarkGlobalRand，用标准库 math/rand 作为对照。
#[test]
fn benchmark_global_rand() {
    super::main_test::setup_for_common_test();
    run_parallel(|| {
        black_box(rand::thread_rng().gen_range(0..=i64::MAX));
    });
    eprintln!(
        "global random sample: {}",
        rand::thread_rng().gen_range(0..=i64::MAX)
    );
}
