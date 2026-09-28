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

// 非 Linux、非 Windows 平台的 `sys` 兜底实现。
//
// 版本串使用 Go runtime 风格的 `GOOS.GOARCH`；CPU 亲和性为空操作；
// 仅在 apple/freebsd/dragonfly 上通过 LocalPeerCred 读取对端 UID，其余目标返回 Unsupported。

#![allow(non_snake_case)]

use std::io;
use std::os::unix::net::UnixStream;

/// 将 Rust `std::env::consts::OS` 映射为 Go 的 `GOOS` 命名（macos → darwin）。
fn go_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        os => os,
    }
}

/// 将 Rust 架构名映射为 Go 的 `GOARCH`（如 x86_64 → amd64）。
fn go_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86" => "386",
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        arch => arch,
    }
}

/// Returns the Go runtime-style `GOOS.GOARCH` value.
/// 返回 Go runtime 风格的 `GOOS.GOARCH` 版本串。
pub fn OSVersion() -> io::Result<String> {
    Ok(format!("{}.{}", go_os(), go_arch()))
}

/// Affinity is intentionally a no-op on non-Linux systems, as in Go.
/// 非 Linux 上亲和性设置为空操作，与 Go 一致。
pub fn SetAffinity(_cpus: &[i32]) -> io::Result<()> {
    Ok(())
}

/// Returns the effective UID of the peer connected to a Unix-domain socket.
/// 在支持 LocalPeerCred 的平台上返回 Unix domain socket 对端有效 UID。
#[cfg(any(
    target_vendor = "apple",
    target_os = "freebsd",
    target_os = "dragonfly"
))]
pub fn GetSockUID(socket: &UnixStream) -> io::Result<u32> {
    use nix::sys::socket::{getsockopt, sockopt::LocalPeerCred};

    getsockopt(socket, LocalPeerCred)
        .map(|credential| credential.uid())
        .map_err(|errno| io::Error::from_raw_os_error(errno as i32))
}

/// 其他 Unix 目标不支持读取对端凭证，返回 Unsupported。
#[cfg(not(any(
    target_vendor = "apple",
    target_os = "freebsd",
    target_os = "dragonfly"
)))]
pub fn GetSockUID(_socket: &UnixStream) -> io::Result<u32> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "peer credentials are unsupported on this target",
    ))
}
