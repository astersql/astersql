// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// Copyright 2010 The Go Authors. All rights reserved.
// Use of this source code is governed by a BSD-style
// license that can be found in the LICENSE file.

// POSIX/Unix 信号处理实现。
//
// - `SetupUSR1Handler`：后台线程监听 SIGUSR1，打印 goroutine 风格诊断栈（此处为 Backtrace）
// - `SetupSignalHandler`：监听 SIGHUP/INT/TERM/QUIT，首次收到后调用关机回调
//
// Rust 无 goroutine，用 `Backtrace::force_capture` 近似 Go 的栈转储文本。

use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGQUIT, SIGTERM, SIGUSR1};
use signal_hook::iterator::Signals;
use std::backtrace::Backtrace;

/// 与 libc 信号编号一致的类型别名。
pub type Signal = libc::c_int;

// Rust has no goroutine runtime; capture the signal-listener thread's diagnostic stack.
/// 捕获当前线程诊断栈，格式贴近 Go 侧 dump 文本。
fn get_goroutine_stacks() -> String {
    format!("stack backtrace:\n{}", Backtrace::force_capture())
}

/// 若为 SIGUSR1 则生成诊断 dump 字符串，否则返回 None。
#[doc(hidden)]
pub fn handle_usr1_signal(sig: Signal) -> Option<String> {
    (sig == SIGUSR1).then(|| {
        format!(
            "\n=== Got signal [{sig}] to dump goroutine stack. ===\n{}\n=== Finished dumping goroutine stack. ===\n",
            get_goroutine_stacks()
        )
    })
}

// SetupUSR1Handler sets up a signal handler for SIGUSR1.
// When SIGUSR1 is received, it dumps a diagnostic stack to the log.
/// 注册 SIGUSR1 处理器：在独立线程中 forever 监听并打印 dump。
#[allow(non_snake_case)]
pub fn SetupUSR1Handler() {
    let Ok(mut signals) = Signals::new([SIGUSR1]) else {
        eprintln!("failed to register SIGUSR1 handler");
        return;
    };
    std::thread::spawn(move || {
        for sig in signals.forever() {
            if let Some(dump) = handle_usr1_signal(sig) {
                eprint!("{dump}");
            }
        }
    });
}

/// 打印退出信号日志并调用一次关机回调。
#[doc(hidden)]
pub fn dispatch_shutdown_signal<F>(sig: Signal, shutdown_func: F)
where
    F: FnOnce(Signal),
{
    eprintln!("got signal to exit: signal={sig}");
    shutdown_func(sig);
}

// SetupSignalHandler sets up the signal handler for TiDB Server.
/// 注册关机信号处理器；首次收到 SIGHUP/INT/TERM/QUIT 即触发 `shutdownFunc`。
#[allow(non_snake_case)]
pub fn SetupSignalHandler<F>(shutdownFunc: F)
where
    F: FnOnce(Signal) + Send + 'static,
{
    let Ok(mut signals) = Signals::new([SIGHUP, SIGINT, SIGTERM, SIGQUIT]) else {
        eprintln!("failed to register shutdown signal handler");
        return;
    };
    // 只取 forever 迭代的第一个信号，对齐 Go 侧“收到即关机”的行为。
    std::thread::spawn(move || {
        if let Some(sig) = signals.forever().next() {
            dispatch_shutdown_signal(sig, shutdownFunc);
        }
    });
}
