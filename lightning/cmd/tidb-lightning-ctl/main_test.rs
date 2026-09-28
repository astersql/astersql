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

//! Go-equivalent tests for `lightning/cmd/tidb-lightning-ctl/main_test.go`.
//!
//! Mapping:
//! - `TestRunMain/run-main` → [`test_run_main`] worker: argument filtering,
//!   overridden exit, real CLI dispatch on a goroutine, wait-channel cleanup.
//! - `TestRunMain/checkpoint table not found does not print stack` → worker assertion.
//! - `TestRunMain/generic errors still print stack` → worker assertion.
//! 中文补充：本文件重点校验控制命令入口的进程边界语义，而不是覆盖各个子命令的业务细节。
//! 中文补充：测试关注点包括参数过滤、退出码分层、客户端关闭时机，以及不同错误类型的格式化差异。

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc;

use crate::entry::{formatFatalError, formatFatalErrorStacked, main_with_args, run_main};
use crate::stubs::{
    ErrCheckpointTableNotFound, StackError, clear_last_client_closed, reset_exit_fn, set_exit_fn,
    take_last_client_closed,
};

const WORKER_ENV: &str = "ASTERSQL_LIGHTNING_CTL_TEST_RUN_MAIN_WORKER";
// 中文补充：环境变量是父子进程之间的最小隔离信号，用来区分“调度者”和“真正执行者”。
// 中文补充：退出码通过原子变量记录，避免子线程或替换后的退出钩子丢失最后一次结果。
static EXIT_CODE: AtomicI32 = AtomicI32::new(-1);

// 中文补充：替代真实进程退出，便于测试在不终止用例的前提下观察 main 流程要返回的码值。
fn record_exit(code: i32) {
    EXIT_CODE.store(code, Ordering::SeqCst);
}

// 中文补充：过滤掉 Rust 测试运行器自带参数，只把真正属于 CLI 的 argv 传给控制命令主流程。
fn filter_run_main_args(args: impl IntoIterator<Item = String>) -> Vec<String> {
    args.into_iter()
        .filter(|arg| arg != "DEVEL" && !arg.starts_with("-test."))
        .collect()
}

fn run_main_worker() {
    // 中文补充：先证明参数清洗结果与 Go 版测试构造的 `os.Args` 一致。
    let filtered = filter_run_main_args([
        "tidb-lightning-ctl".into(),
        "DEVEL".into(),
        "-test.v".into(),
        "-test.run=TestRunMain".into(),
    ]);
    assert_eq!(filtered, vec!["tidb-lightning-ctl"]);

    // Go mutates os.Args and launches main in a goroutine. Rust passes the
    // filtered argv explicitly, retaining the same real CLI control flow.
    // 中文补充：这里保留“真实主流程在独立执行单元里运行”的结构，以验证资源释放和退出码传播。
    set_exit_fn(record_exit);
    EXIT_CODE.store(-1, Ordering::SeqCst);
    clear_last_client_closed();
    let (wait_tx, wait_rx) = mpsc::sync_channel::<i32>(1);
    std::thread::spawn(move || {
        let code = run_main(filtered.into_iter().skip(1).collect());
        wait_tx.send(code).expect("wait channel receiver");
    });
    assert_eq!(wait_rx.recv().expect("main worker must return"), 0);
    assert_eq!(EXIT_CODE.load(Ordering::SeqCst), -1);
    assert_eq!(
        take_last_client_closed(),
        Some(true),
        "Go defer cli.Close must run before the worker signals completion"
    );

    // Prove the process boundary keeps Go's config.Must and fatal exit codes.
    // 中文补充：未知参数属于装载阶段错误，应返回 2，而不是运行期失败码 1。
    EXIT_CODE.store(-1, Ordering::SeqCst);
    main_with_args(vec!["--definitely-unknown".into()]);
    assert_eq!(EXIT_CODE.load(Ordering::SeqCst), 2);

    // 中文补充：进入真实动作后再失败时，既要返回 1，也要确保 PD client 已按 Go 语义关闭。
    EXIT_CODE.store(-1, Ordering::SeqCst);
    clear_last_client_closed();
    main_with_args(vec!["--switch-mode".into(), "invalid".into()]);
    assert_eq!(EXIT_CODE.load(Ordering::SeqCst), 1);
    assert_eq!(
        take_last_client_closed(),
        Some(true),
        "runtime failure must still close the PD client"
    );
    reset_exit_fn();

    // 中文补充：checkpoint 表不存在属于可引导用户恢复的错误，断言它不会混入测试文件栈信息。
    let err = ErrCheckpointTableNotFound.GenWithStackByArgs("`db`.`table`");
    let formatted = formatFatalError(&err);
    assert!(formatted.contains(&err.Error()));
    assert!(!formatted.contains("main_test.rs"));
    assert!(formatted.contains("--checkpoint-error-ignore='`db`.`table`'"));
    assert!(formatted.contains("--checkpoint-error-destroy='`db`.`table`'"));

    // 中文补充：普通栈错误则应保留堆栈样式输出，用来和上面的“用户可修复错误”形成对照。
    let err = StackError::new("boom");
    let formatted = formatFatalErrorStacked(&err);
    // 中文补充：这里要求输出不同于裸 `Error()`，否则就说明栈格式化分支没有真正生效。
    assert_ne!(err.Error(), formatted);
    assert!(formatted.contains("main_test.rs"), "{formatted}");
}

/// `TestRunMain`: execute the process-global exit hook in an isolated child,
/// matching Go package-test serialization while Rust's test runner is parallel.
/// 中文补充：通过重新拉起当前测试二进制，把全局退出钩子限制在子进程内，避免并行测试相互污染。
#[test]
fn test_run_main() {
    if std::env::var_os(WORKER_ENV).is_some() {
        // 中文补充：子进程命中环境变量后直接执行 worker，本进程不再递归生成新的子进程。
        run_main_worker();
        return;
    }

    // 中文补充：父进程只负责启动精确匹配的单个测试用例，并把失败时的 stdout/stderr 原样带回断言。
    let exe = std::env::current_exe().expect("current test executable");
    let output = std::process::Command::new(exe)
        .env(WORKER_ENV, "1")
        .args(["--exact", "main_test::test_run_main"])
        .output()
        .expect("spawn TestRunMain worker");
    // 中文补充：这里只接受子进程整体成功，因为 worker 内部已经覆盖了所有需要观察的失败路径。
    assert!(
        output.status.success(),
        "TestRunMain worker failed: status={:?}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
