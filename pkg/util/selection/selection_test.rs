// Copyright 2020 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// selection 功能与基准辅助测试。
//
// 覆盖基本 Select、重复值、随机/序列大数据正确性，并保留 Go benchmark
// 的规模与复制-再测量辅助函数（不作为普通测试运行）。

use super::{Interface, Select};
use crate::selection::quickselect;
use rand::Rng;

/// 实现 Interface 的整数切片测试夹具。
#[derive(Clone)]
struct TestSlice(Vec<i32>);

impl Interface for TestSlice {
    fn Len(&self) -> isize {
        self.0.len() as isize
    }

    fn Swap(&mut self, i: isize, j: isize) {
        self.0.swap(i as usize, j as usize);
    }

    fn Less(&self, i: isize, j: isize) -> bool {
        self.0[i as usize] < self.0[j as usize]
    }
}

impl TestSlice {
    fn stable_sort(&mut self) {
        self.0.sort();
    }

    fn reverse_sort(&mut self) {
        self.0.sort_by(|a, b| b.cmp(a));
    }
}

/// 基本路径：对有序五元组取第 3 小，期望值为 3。
#[test]
fn test_selection() {
    let mut data = TestSlice(vec![1, 2, 3, 4, 5]);
    let index = Select(&mut data, 3);
    assert_eq!(3, data.0[index as usize]);
}

/// 含重复值时按 1-based 排名取第 3、第 5 小。
#[test]
fn test_selection_with_duplicate() {
    let mut data = TestSlice(vec![1, 2, 3, 3, 5]);
    let mut index = Select(&mut data, 3);
    assert_eq!(3, data.0[index as usize]);
    index = Select(&mut data, 5);
    assert_eq!(5, data.0[index as usize]);
}

/// 百万随机数据取中位附近元素，与全排序结果比对。
#[test]
fn test_selection_with_random_case() {
    let mut data = random_test_case(1_000_000);
    let index = Select(&mut data, 500_000);
    let actual = data.0[index as usize];
    data.stable_sort();
    let expected = data.0[499_999];
    assert_eq!(expected, actual);
}

/// 逆序序列大数据取中位附近元素，与全排序结果比对。
#[test]
fn test_selection_with_serial_case() {
    let mut data = serial_test_case(1_000_000);
    data.reverse_sort();
    let index = Select(&mut data, 500_000);
    let actual = data.0[index as usize];
    data.stable_sort();
    let expected = data.0[499_999];
    assert_eq!(expected, actual);
}

/// 生成指定规模的随机整数测试数据（值域 0..100）。
fn random_test_case(size: usize) -> TestSlice {
    let mut rng = rand::thread_rng();
    TestSlice((0..size).map(|_| rng.gen_range(0..100)).collect())
}

/// 生成 `0..size` 的序列测试数据。
fn serial_test_case(size: usize) -> TestSlice {
    TestSlice((0..size).map(|i| i as i32).collect())
}

// Rust's stable test harness has no benchmark API. These helpers preserve the
// Go benchmark's copy-before-measure behavior and rank calculation for use by
// a benchmark harness without turning benchmarks into ordinary tests.
/// Go 基准用例规模列表，保留供外部基准 harness 使用。
#[allow(dead_code)]
const GLOBAL_CASE_SIZES: &[usize] = &[10_000_000, 1_000_000, 100_000, 10_000, 1_000, 100, 50];

/// 按规模循环对比 introselect、quickselect 与全排序的基准辅助入口。
#[allow(dead_code)]
fn benchmark_selection(iterations: usize) {
    for &size in GLOBAL_CASE_SIZES {
        let test_case = random_test_case(size);
        run_select(iterations, &test_case, benchmark_intro_selection);
        run_select(iterations, &test_case, benchmark_quick_selection);
        for _ in 0..iterations {
            benchmark_sort(&test_case);
        }
    }
}

/// 按迭代次数计算排名 k，并对同一用例反复调用 bench_func。
#[allow(dead_code)]
fn run_select(iterations: usize, test_case: &TestSlice, bench_func: fn(&TestSlice, usize)) {
    for i in 1..=iterations {
        let k = if iterations < test_case.0.len() {
            test_case.0.len() / iterations * i
        } else {
            i % test_case.0.len() + 1
        };
        bench_func(test_case, k);
    }
}

/// 复制数据后调用 introselect 版 Select。
fn benchmark_intro_selection(test_case: &TestSlice, k: usize) {
    let mut data = test_case.clone();
    Select(&mut data, k as isize);
}

/// 复制数据后调用 quickselect（k 转为 0-based）。
fn benchmark_quick_selection(test_case: &TestSlice, k: usize) {
    let mut data = test_case.clone();
    let right = data.Len() - 1;
    quickselect(&mut data, 0, right, k as isize - 1);
}

/// 复制数据后做全排序，作为基准对照。
fn benchmark_sort(test_case: &TestSlice) {
    let mut data = test_case.clone();
    data.stable_sort();
}
