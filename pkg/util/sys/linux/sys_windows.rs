// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// Windows 平台的 `sys` 兜底实现。
//
// 版本串固定为 `windows.<GOARCH>`；CPU 亲和性为空操作；
// Unix domain socket 在 Windows 上不受支持，`GetSockUID` 始终返回 Unsupported。

#![allow(non_snake_case)]

use std::io;

/// 将 Rust 架构名映射为 Go 的 `GOARCH`（如 x86_64 → amd64）。
fn go_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86" => "386",
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        arch => arch,
    }
}

/// 返回 `windows.<GOARCH>` 形式的版本串，与 Go 兜底实现一致。
pub fn OSVersion() -> io::Result<String> {
    Ok(format!("windows.{}", go_arch()))
}

/// Windows 上 CPU 亲和性设置为空操作。
pub fn SetAffinity(_cpus: &[i32]) -> io::Result<()> {
    Ok(())
}

/// Windows 不支持 Unix domain socket 对端凭证，始终返回错误。
pub fn GetSockUID<T>(_socket: T) -> io::Result<u32> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "UNIX domain socket is not supported on Windows",
    ))
}
