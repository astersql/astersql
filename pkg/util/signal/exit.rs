// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 进程退出信号发送（POSIX/Unix）。
//
// 提供向当前进程投递信号的辅助：`send_to_current_process` 供测试与内部使用，
// `TiDBExit` 对齐 Go 侧同名 API，失败时仅打印错误而不 panic。

use std::io;

/// Sends `sig` to the current process.
/// 向当前进程发送信号 `sig`（`kill(getpid(), sig)`）。
#[doc(hidden)]
pub fn send_to_current_process(sig: libc::c_int) -> io::Result<()> {
    // SAFETY: `getpid` has no preconditions and `kill` receives that live PID.
    let result = unsafe { libc::kill(libc::getpid(), sig) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

// TiDBExit sends a signal to the current process.
/// 向当前进程发送退出相关信号；发送失败时打印错误信息。
#[allow(non_snake_case)]
pub fn TiDBExit(sig: libc::c_int) {
    if let Err(err) = send_to_current_process(sig) {
        eprintln!("failed to send signal: signal={sig}, error={err}");
    }
}
