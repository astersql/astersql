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

//! Go-equivalent tests for `lightning/cmd/tidb-lightning` (`main_test.go`).
//!
//! 这里的测试专门对齐 Go 版 `TestRunMain`，验证二进制入口在测试环境下的
//! 参数过滤、退出钩子替换以及等待主逻辑返回的时序，而不是覆盖业务功能本身。
//! Rust 端不能像 Go 一样直接重写 `os.Args` 和包级 `exit` 变量，因此测试通过
//! 子进程与桩函数近似出相同约束，重点保证行为边界一致。
//!
//! Reference: https://dzone.com/articles/measuring-integration-test-coverage-rate-in-pouchc
//!
//! Mapping:
//! - `TestRunMain` → [`test_run_main`] (filter DEVEL/`-test.*`, override exit, run `main` on a thread, wait)

use std::sync::{Arc, mpsc};

use crate::stubs;

/// Env flag: child process executes the Go `TestRunMain` body in isolation.
/// 该环境变量把“父进程负责拉起、子进程负责真实执行”分成两段，避免测试框架本身
/// 被入口逻辑里的退出路径影响。
/// Sibling `parity_test` mutates the process-global exit hook; Go package tests are
/// serial by default, so isolate the real `run`/`exit` path in a subprocess.
/// 这样可以保持与 Go 包级测试近似的串行隔离语义，避免并发测试互相踩踏全局状态。
const WORKER_ENV: &str = "ASTERSQL_LIGHTNING_TEST_RUN_MAIN_WORKER";

/// Go `TestRunMain` arg rewrite: drop `DEVEL` and any `-test.*` harness flag.
/// 过滤结果要保留真正会传给 Lightning 的参数，同时剥离只属于测试框架的控制位。
fn filter_run_main_args(args: impl IntoIterator<Item = String>) -> Vec<String> {
    args.into_iter()
        .filter(|arg| arg != "DEVEL" && !arg.starts_with("-test."))
        .collect()
}

fn run_main_body() {
    // 先用静态样例固定过滤规则，确保 Go 版最关键的参数裁剪行为没有漂移。
    let filtered = filter_run_main_args([
        "tidb-lightning".into(),
        "DEVEL".into(),
        "-test.v".into(),
        "-test.run=TestRunMain".into(),
        "kept".into(),
    ]);
    assert_eq!(filtered, vec!["tidb-lightning", "kept"]);

    // Live argv after the same Go filter (Rust cannot assign `os.Args`).
    // Rust 只能读取当前进程参数，因此这里验证“经过同一套过滤后仍至少保留程序名”。
    let filtered_live = filter_run_main_args(std::env::args());
    assert!(
        !filtered_live.is_empty(),
        "process argv must retain the program name after DEVEL/-test.* filter"
    );
    // Go `os.Args = args` then `LoadGlobalConfig(os.Args[1:], nil)`.
    // Go 单测过滤后通常只剩二进制名，因此业务入口看到的是空参数切片。
    // Typical Go unit-test argv after filter is only the binary → Args[1:] is empty.
    // Rust libtest flags are not `-test.*`, so do not forward them into Lightning.
    // 这里显式传空参数，避免把 Rust 测试框架自身选项误当成 Lightning CLI 输入。
    let run_args: Vec<String> = Vec::new();
    let _ = filtered_live;

    // Go: if INTEGRATION_TEST unset, `exit = func(code int) {}`.
    // 单元测试场景下把退出行为替换为空实现，只验证是否走到退出路径，不让测试进程真的结束。
    if std::env::var_os("INTEGRATION_TEST").is_none() {
        stubs::set_exit_hook(Some(Arc::new(|_code| {})));
    }
    let _ = stubs::take_exit_code();

    let (wait_tx, wait_rx) = mpsc::sync_channel::<()>(1);
    std::thread::spawn(move || {
        // `run` is the body of Go/`crate::main` after FIPS init (signals included).
        // 线程模型对应 Go 版 goroutine + waitCh：入口返回后发信号，测试再继续断言。
        let _code = crate::entry::run(run_args);
        let _ = wait_tx.send(());
    });
    wait_rx
        .recv()
        .expect("main() must return so waitCh closes like Go");

    let _ = stubs::take_exit_code();
    stubs::set_exit_hook(None);
}

/// `TestRunMain`: filter DEVEL/`-test.*`, override `exit` outside INTEGRATION_TEST,
/// run `main` on a worker thread, wait for return (Go `waitCh`).
/// 父测试只负责验证过滤规则与拉起子进程，真正可能触碰全局退出钩子的部分放到
/// 子进程中执行，从而把风险限制在隔离环境里。
#[test]
fn test_run_main() {
    if std::env::var_os(WORKER_ENV).is_some() {
        run_main_body();
        return;
    }

    // Parent: prove the Go arg filter, then run the body in a child process so a
    // concurrent parity test clearing `exit` cannot `process::exit` this harness.
    // 这里先在父进程完成纯文本级断言，再把真实入口执行委托给子进程，减少共享全局状态。
    let filtered = filter_run_main_args([
        "tidb-lightning".into(),
        "DEVEL".into(),
        "-test.v".into(),
        "-test.run=TestRunMain".into(),
        "kept".into(),
    ]);
    assert_eq!(filtered, vec!["tidb-lightning", "kept"]);

    let exe = std::env::current_exe().expect("current_exe");
    let output = std::process::Command::new(&exe)
        .env(WORKER_ENV, "1")
        .args(["--exact", "main_test::test_run_main"])
        .output()
        .expect("spawn TestRunMain worker");
    assert!(
        output.status.success(),
        "TestRunMain worker failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
