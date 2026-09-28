// Copyright 2026 AsterSQL.

//! `astersql::errors` 基准集合。
//!
//! 这里不验证功能正确性，而是固定几组典型输入，比较错误生成、堆栈展示
//! 和参数冻结路径的分配与格式化成本，便于与 Go 基线维持相近观测面。

use std::hint::black_box;

use astersql::errors::{ErrorArg, HackedStr, New, Normalize, RFCCodeText, SharedError};
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};

/// 递归构造带栈错误。
///
/// 使用 `#[inline(never)]` 与 `black_box` 避免编译器把递归深度折叠掉，
/// 让基准更接近真实调用链逐层下探后在叶子处 `New` 的开销。
#[inline(never)]
fn pingcap_error(depth: usize) -> SharedError {
    if depth == 0 {
        return New("ye error");
    }
    pingcap_error(black_box(depth - 1))
}

/// 递归构造标准库 `io::Error`。
///
/// 该对照组与 `pingcap_error` 使用同样的深度模型，只替换错误类型，
/// 用来分离“带栈错误链”相对普通错误对象的额外成本。
#[inline(never)]
fn std_error(depth: usize) -> std::io::Error {
    if depth == 0 {
        return std::io::Error::other("no error");
    }
    std_error(black_box(depth - 1))
}

/// 对比两类错误在不同递归深度下的构造成本。
///
/// 深度从 10 到 1000，覆盖浅链路到长调用链，观察带栈包装在深度增长后
/// 是否出现比标准库错误更明显的线性放大。
fn benchmark_errors(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("BenchmarkErrors");
    for depth in [10, 100, 1000] {
        group.bench_with_input(
            BenchmarkId::new("pkg-errors-stack", depth),
            &depth,
            |bencher, depth| bencher.iter(|| black_box(pingcap_error(*depth))),
        );
        group.bench_with_input(
            BenchmarkId::new("errors-stack", depth),
            &depth,
            |bencher, depth| bencher.iter(|| black_box(std_error(*depth))),
        );
    }
    group.finish();
}

/// 比较错误对象与栈跟踪对象的多种展示格式开销。
///
/// 先在循环外预生成 `error` 与 `trace`，避免把构造成本混入格式化测试；
/// 这里关注的是 `%s`/`%v`/`%+v` 风格输出在不同栈深上的纯格式化代价。
fn benchmark_stack_formatting(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("BenchmarkStackFormatting");
    for depth in [10, 30, 60] {
        let error = pingcap_error(depth);
        let trace = astersql::errors::GetStackTracer(&error)
            .expect("New captures a stack")
            .stack_trace();

        group.bench_function(format!("%s-stack-{depth}"), |bencher| {
            bencher.iter(|| black_box(format!("{}", black_box(&error))))
        });
        group.bench_function(format!("%v-stack-{depth}"), |bencher| {
            bencher.iter(|| black_box(format!("{:?}", black_box(&error))))
        });
        group.bench_function(format!("%+v-stack-{depth}"), |bencher| {
            bencher.iter(|| black_box(format!("{:#?}", black_box(&error))))
        });
        group.bench_function(format!("%s-stacktrace-{depth}"), |bencher| {
            bencher.iter(|| black_box(format!("{}", black_box(&trace))))
        });
        group.bench_function(format!("%v-stacktrace-{depth}"), |bencher| {
            bencher.iter(|| black_box(format!("{:?}", black_box(&trace))))
        });
        group.bench_function(format!("%+v-stacktrace-{depth}"), |bencher| {
            bencher.iter(|| black_box(format!("{:#?}", black_box(&trace))))
        });
    }
    group.finish();
}

/// 基准用 `HackedStr` 实现。
///
/// 该类型显式返回拥有所有权的字符串快照，模拟 Go 侧“底层存储可能随后被改写”
/// 的场景，从而测量 `FastGenByArgs` 在冻结参数时的额外成本。
struct BenchmarkHackedStr(String);

impl HackedStr for BenchmarkHackedStr {
    /// 每次都克隆一份字符串，确保基准测到真实冻结成本而非借用成本。
    fn FreezeStr(&self) -> String {
        self.0.clone()
    }
}

/// 对比普通字符串参数与 `HackedStr` 参数在 `FastGenByArgs` 下的生成成本。
///
/// 三层维度分别控制参数种类、参数个数和单个字符串长度，用于观察：
/// 1. 纯复制路径与冻结路径的差距；
/// 2. 参数数量增加时 `Vec<ErrorArg>` 构造与遍历的放大量；
/// 3. 长字符串下冻结快照是否成为主导成本。
fn benchmark_by_args_hacked_str_freeze(criterion: &mut Criterion) {
    let prototype = Normalize("bench", &[RFCCodeText("Internal:Bench".to_owned())]);
    let mut group = criterion.benchmark_group("BenchmarkByArgsHackedStrFreeze/FastGenByArgs");

    for profile in ["plain", "hacked"] {
        for count in [1, 4, 8] {
            for string_len in [16, 1024] {
                let value = "x".repeat(string_len);
                let hacked = BenchmarkHackedStr(value.clone());
                let benchmark_id = BenchmarkId::new(
                    format!("type-{profile}/count-{count}"),
                    format!("strlen-{string_len}"),
                );
                group.bench_with_input(benchmark_id, &(count, string_len), |bencher, _| {
                    bencher.iter(|| {
                        let args: Vec<ErrorArg> = (0..count)
                            .map(|_| {
                                if profile == "hacked" {
                                    ErrorArg::from_hacked(black_box(&hacked))
                                } else {
                                    ErrorArg::from(black_box(value.clone()))
                                }
                            })
                            .collect();
                        black_box(prototype.FastGenByArgs(black_box(&args)))
                    });
                });
            }
        }
    }
    group.finish();
}

criterion_group!(
    benches,
    benchmark_errors,
    benchmark_stack_formatting,
    benchmark_by_args_hacked_str_freeze
);
criterion_main!(benches);
