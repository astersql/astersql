// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

//! Go-equivalent tests for `br/cmd/br` (`main_test.go`).
//!
//! Reference: https://dzone.com/articles/measuring-integration-test-coverage-rate-in-pouchc
//!
//! Mapping:
//! - `TestMain` → [`test_main`] (`--skip-goleak` argument filter) and
//!   [`go_commit_231dad5225_cleans_global_memory_arbitrator_before_leak_check`]
//! - `TestRunMain` → [`test_run_main`] (filter DEVEL/`-test.*`, run `main` on a thread, wait)
//! - `TestCalculateMemoryLimit` → [`test_calculate_memory_limit`]
//!
//! 中文补充：本文件不是对 BR 做完整集成测试，
//! 而是把 Go `main_test.go` 中最关键的入口契约迁移成 Rust 等价断言。
//! 关注点包括测试参数过滤、根入口可返回性，以及内存上限公式与 Go 表值一致。

use std::sync::mpsc;

use crate::cmd::calculateMemoryLimit;
use crate::stubs::os_stub;

fn cleanup_main_test_resources() {
    astersql_util_memory::global_arbitrator::CleanupGlobalMemArbitratorForTest();
}

/// Go `TestMain` arg rewrite: drop `--skip-goleak`, return whether leak checks are skipped.
///
/// 中文补充：Rust 测试无法原样复刻 Go 的 `os.Args` 原地改写，
/// 因此先把参数处理逻辑抽成纯函数单独验证。
fn filter_skip_goleak(args: impl IntoIterator<Item = String>) -> (bool, Vec<String>) {
    let mut skip_leak_test = false;
    let mut new_args = Vec::new();
    for arg in args {
        if arg == "--skip-goleak" {
            skip_leak_test = true;
        } else {
            new_args.push(arg);
        }
    }
    (skip_leak_test, new_args)
}

/// Go `TestRunMain` arg rewrite: drop `DEVEL` and any `-test.*` harness flag.
///
/// 中文补充：这些参数会干扰命令行解析，必须在调用 `main()` 前过滤掉。
fn filter_run_main_args(args: impl IntoIterator<Item = String>) -> Vec<String> {
    args.into_iter()
        .filter(|arg| arg != "DEVEL" && !arg.starts_with("-test."))
        .collect()
}

/// `TestMain`: parse and remove the Go-only `--skip-goleak` compatibility flag.
#[test]
fn test_main() {
    // 先验证带 `--skip-goleak` 时，标记会被识别且参数本身会被剔除。
    let (skip, filtered) = filter_skip_goleak([
        "br-test".into(),
        "--skip-goleak".into(),
        "-test.v".into(),
        "kept".into(),
    ]);
    assert!(skip);
    assert_eq!(filtered, vec!["br-test", "-test.v", "kept"]);

    // 不携带该参数时必须保持默认行为，即不跳过泄漏检查。
    // 这保证普通 `cargo test` 路径不会因为参数过滤逻辑而误关闭检查。
    let (skip, filtered) = filter_skip_goleak(["br-test".into(), "-test.run=TestMain".into()]);
    assert!(!skip);
    assert_eq!(filtered, vec!["br-test", "-test.run=TestMain"]);
}

/// `TestRunMain`: filter DEVEL/`-test.*`, run `main` on a worker thread, wait for return.
#[test]
fn test_run_main() {
    // 这一步对应 Go 测试里对 `os.Args` 的预清洗。
    let filtered = filter_run_main_args([
        "br".into(),
        "DEVEL".into(),
        "-test.v".into(),
        "-test.run=TestRunMain".into(),
        "kept".into(),
    ]);
    assert_eq!(filtered, vec!["br", "kept"]);

    // Apply the same filter to the live process argv (Go mutates os.Args; stub Args is read-only).
    // 这里不直接覆写真实 argv，而是确保过滤逻辑能在当前测试进程输入上执行。
    let _filtered_live = filter_run_main_args(std::env::args());
    // 清空 Exit stub，避免前序测试残留状态影响本次断言。
    let _ = os_stub::take_exit();

    let (wait_tx, wait_rx) = mpsc::sync_channel::<()>(1);
    std::thread::spawn(move || {
        // 关键约束：`main()` 在线程中运行后必须返回，否则测试会一直阻塞。
        // 采用线程而不是当前线程执行，可以模拟 Go 版本 goroutine + channel 的等待结构。
        crate::main();
        let _ = wait_tx.send(());
    });
    wait_rx
        .recv()
        .expect("main() must return so waitCh closes like Go");
    // Clear any Exit stub side-effect from cargo/libtest argv reaching cobra.
    // 收尾再清一次，防止 cargo/libtest 自身参数触发的退出码污染后续用例。
    let _ = os_stub::take_exit();
}

/// `TestCalculateMemoryLimit`: exact Go table values for BR memory headroom.
#[test]
fn test_calculate_memory_limit() {
    // 这里直接复用 Go 表格中的标称值，确保 Rust 公式不是“近似正确”而是逐点一致。
    // f(0 Byte) = 0 Byte
    // 边界 0 的意义是确认实现没有因为位运算或除法保护而返回非零最小值。
    assert_eq!(calculateMemoryLimit(0), 0_u64);
    // f(100 KB) = 87.5 KB
    assert_eq!(calculateMemoryLimit(100 * 1024), 89_600_u64);
    // f(100 MB) = 87.5 MB
    assert_eq!(calculateMemoryLimit(100 * 1024 * 1024), 91_763_188_u64);
    // f(3.99 GB) = 3.74 GB
    assert_eq!(
        calculateMemoryLimit(4 * 1024 * 1024 * 1024 - 1),
        4_026_531_839_u64
    );
    // f(4 GB) = 3.5 GB
    // 4 GiB 是公式拐点，对应预留空间正好达到 512 MiB 的一半。
    assert_eq!(
        calculateMemoryLimit(4 * 1024 * 1024 * 1024),
        3_758_096_384_u64
    );
    // f(32 GB) = 31.5 GB
    // 大内存样本用于确认预留值渐近到 512 MiB，而不是继续线性放大。
    assert_eq!(
        calculateMemoryLimit(32 * 1024 * 1024 * 1024),
        33_822_867_456_u64
    );
}

#[test]
fn go_commit_231dad5225_cleans_global_memory_arbitrator_before_leak_check() {
    astersql_util_memory::global_arbitrator::CleanupGlobalMemArbitratorForTest();
    astersql_util_memory::global_arbitrator::SetupGlobalMemArbitratorForTest(
        std::env::temp_dir()
            .join("go_commit_231dad5225")
            .display()
            .to_string(),
    );
    assert!(
        astersql_util_memory::global_arbitrator::SetGlobalMemArbitratorWorkMode(
            "standard".to_owned()
        )
    );
    assert!(astersql_util_memory::global_arbitrator::GlobalMemArbitrator().is_some());

    cleanup_main_test_resources();

    assert!(astersql_util_memory::global_arbitrator::GlobalMemArbitrator().is_none());
}
