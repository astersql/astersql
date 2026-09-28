// Copyright 2026 AsterSQL.
// Copyright 2019-present PingCAP, Inc.
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

// WASM 平台信号处理桩。
//
// WASM 环境通常不具备 POSIX 信号；此处提供与 Unix/Windows 相同的 API 形状，
// 实现为空操作且不会调用关机回调。

/// WASM 下信号编号的占位类型（i32）。
pub type Signal = i32;

// SetupUSR1Handler is a no-op on WASM.
/// WASM 上 SIGUSR1 处理为空操作。
#[allow(non_snake_case)]
pub fn SetupUSR1Handler() {}

// SetupSignalHandler is a no-op on WASM and never invokes shutdownFunc.
/// WASM 上关机信号注册为空操作；故意丢弃 `shutdown_func` 以免误调用。
#[allow(non_snake_case)]
pub fn SetupSignalHandler<F>(shutdown_func: F)
where
    F: FnOnce(Signal) + Send + 'static,
{
    let _ = shutdown_func;
}
