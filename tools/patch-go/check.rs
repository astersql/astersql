// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

//! Patched-Go runtime probe (Go package `main` / `tools/patch-go/check.go`).
//!
//! Go uses `//go:linkname grunningnanos runtime.grunningnanos` so building
//! this program succeeds only when the toolchain carries the running-time
//! patch. Rust calls the local `stubs` stand-in for that runtime symbol.
//! 该模块对应 Go 版最小探针程序：唯一职责是触发 `runtime.grunningnanos`
//! 的链接与调用，从而验证补丁版 Go 运行时是否已经注入目标符号。
//! Rust 侧不自行实现运行时逻辑，只保留与 Go `main` 相同的可观察调用路径。

use crate::stubs;

/// Go `grunningnanos` — linkname to `runtime.grunningnanos`.
///
/// # Safety
/// Mirrors Go's linkname binding: callers treat this as an external runtime
/// entry (Go `main` invokes it unsafely via the linked symbol).
/// 这里故意经由本地 `stubs` 转发，语义上仍视为对 Go 运行时符号的探测；
/// 若符号不存在，失败应体现在链接或调用阶段，而不是被 Rust 包装层吞掉。
pub unsafe fn grunningnanos() -> i64 {
    stubs::runtime_grunningnanos()
}

/// Core of Go `main` without process exit — call and discard return value.
/// 单独拆出该入口，便于测试或其他调用方复用，同时保持“只触发调用、不消费结果”的
/// Go 原始行为，避免把探针误改成带业务语义的返回值检查。
pub fn run_main() {
    unsafe {
        let _ = grunningnanos();
    }
}

/// Go `main`: invoke `grunningnanos()` and ignore the result.
pub fn main() {
    run_main();
}
