// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// Windows 平台的进程资源限制占位实现。
//
// 对应 Go `pkg/util/rlimit_windows.go`：Windows 无等价 `getrlimit`，固定返回 1024，
// 与非 Windows 失败回退值保持一致，保证调用方配额逻辑跨平台可编译。

#![cfg(windows)]

// GenRLimit always return 1024.
// GenRLimit 对应 Go 函数 `GenRLimit(source string) uint64`。
// Windows 分支没有调用 syscall.Getrlimit，因此 source 仅保留调用形状，不参与参数解析或日志输出。
/// Windows 下固定返回 1024；`source` 仅保留与 Go 相同的调用形状。
pub fn GenRLimit(source: &str) -> u64 {
    // 保留 Go 参数名以方便对照；这里显式标记未使用，说明 Windows 实现不依赖调用来源。
    let _ = source;
    // 对应 Go 代码中的固定返回值 `return 1024`，没有错误处理、IO、并发或资源收尾逻辑。
    1024
}
