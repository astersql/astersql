// Copyright 2026 AsterSQL.

// 信号处理库入口：按平台条件编译并重导出实现。
//
// - Unix：`exit` + `signal_posix`（SIGUSR1 诊断栈、关机信号处理）
// - WASM：空操作桩（`signal_wasm`）
// - Windows：控制台 Ctrl+C 关机回调（`signal_windows`）
//
// 测试通过 `migration_aster_unit_test` 挂载。

extern crate self as astersql_util_signal;

#[cfg(unix)]
mod exit;
#[cfg(unix)]
mod signal_posix;
#[cfg(target_arch = "wasm32")]
mod signal_wasm;
#[cfg(windows)]
mod signal_windows;

#[cfg(unix)]
pub use exit::*;
#[cfg(unix)]
pub use signal_posix::*;
#[cfg(target_arch = "wasm32")]
pub use signal_wasm::*;
#[cfg(windows)]
pub use signal_windows::*;

#[cfg(test)]
#[path = "migration_aster_unit_test.rs"]
mod migration_aster_unit_test;
