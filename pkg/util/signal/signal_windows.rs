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

// Windows 控制台关机信号处理。
//
// - `SetupUSR1Handler`：Windows 无 SIGUSR1，空操作
// - `SetupSignalHandler`：通过 `ctrlc` 注册 Ctrl+C，以 SIGINT 语义调用关机回调一次
// - `TiDBExit`：Windows 不具备 POSIX 进程信号关机语义，忽略入参

use std::sync::{Arc, Mutex};

/// Windows 下信号编号占位类型（与 libc::SIGINT 等兼容的 i32）。
pub type Signal = i32;

// Go's Windows runtime maps both CTRL_C_EVENT and CTRL_BREAK_EVENT to os.Interrupt.
const WINDOWS_INTERRUPT: Signal = 2;

// SetupUSR1Handler is a no-op on Windows.
/// Windows 上 SIGUSR1 处理为空操作。
#[allow(non_snake_case)]
pub fn SetupUSR1Handler() {}

// SetupSignalHandler sets up the Windows console shutdown handler.
/// 注册控制台 Ctrl+C 处理器；首次触发时以 SIGINT 调用关机回调。
#[allow(non_snake_case)]
pub fn SetupSignalHandler<F>(shutdown_func: F)
where
    F: FnOnce(Signal) + Send + 'static,
{
    // Mutex+Option 保证回调只 take 一次，避免重复关机。
    let callback = Arc::new(Mutex::new(Some(shutdown_func)));
    if let Err(err) = ctrlc::set_handler(move || {
        if let Some(callback) = callback
            .lock()
            .expect("signal callback lock poisoned")
            .take()
        {
            eprintln!("got signal to exit: signal={WINDOWS_INTERRUPT}");
            callback(WINDOWS_INTERRUPT);
        }
    }) {
        eprintln!("failed to register shutdown signal handler: {err}");
    }
}

// TiDBExit sends a signal to the current process on a best-effort basis.
/// Windows 上尽力而为的退出信号发送；当前实现为空操作。
#[allow(non_snake_case)]
pub fn TiDBExit(sig: Signal) {
    // Windows does not support POSIX process-signal shutdown semantics.
    let _ = sig;
}
