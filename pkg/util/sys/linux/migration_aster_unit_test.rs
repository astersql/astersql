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

// 跨平台迁移后的 `sys` API 单元测试。
//
// 按目标 OS 分别验证 `OSVersion`、`SetAffinity`、`GetSockUID`（以及 Linux 上的
// C 风格字符串截断辅助函数）与 Go 实现语义一致。

#[cfg(target_os = "linux")]
/// Linux 实现路径：真实调用 uname / sched_setaffinity / SO_PEERCRED。
mod linux_tests {
    use super::super::sys_linux::{GetSockUID, OSVersion, SetAffinity, charsToString};
    use std::os::unix::net::UnixStream;

    /// 校验 C 风格字符数组在首个 NUL 处截断，且不以 NUL 开头时得到空串。
    #[test]
    fn chars_to_string_stops_at_the_first_nul() {
        assert_eq!(
            charsToString(&[b'L' as i32, b'i' as i32, 0, b'x' as i32]),
            "Li"
        );
        assert_eq!(charsToString(&[0_i8, b'x' as i8]), "");
    }

    /// 校验 `OSVersion` 返回 Go 风格的 `Sysname Release.Machine` 形状。
    #[test]
    fn os_version_matches_the_go_uname_shape() {
        let version = OSVersion().expect("uname should succeed");
        assert!(!version.is_empty());
        assert!(
            version.contains(' '),
            "missing sysname/release separator: {version}"
        );
        assert!(
            version.contains('.'),
            "missing release/machine separator: {version}"
        );
    }

    /// 空 CPU 集合会把空 affinity mask 交给内核，应返回错误。
    #[test]
    fn empty_cpu_affinity_reports_the_kernel_error() {
        assert!(SetAffinity(&[]).is_err());
    }

    /// 通过 Unix domain socket 的 SO_PEERCRED 读取对端有效 UID。
    #[test]
    fn socket_uid_is_the_peer_process_uid() {
        let (left, _right) = UnixStream::pair().expect("create Unix socket pair");
        let uid = GetSockUID(&left).expect("read SO_PEERCRED");
        assert_eq!(uid, unsafe { libc::geteuid() });
    }
}

#[cfg(all(not(target_os = "linux"), not(target_os = "windows")))]
/// 非 Linux/Windows 的 Unix 兜底实现：版本串用 GOOS.GOARCH，亲和性为空操作。
mod other_unix_tests {
    use super::super::sys_other::{GetSockUID, OSVersion, SetAffinity};
    use std::os::unix::net::UnixStream;

    /// 校验版本串使用 Go runtime 命名（如 macOS → `darwin`）。
    #[test]
    fn os_version_uses_go_runtime_names() {
        let version = OSVersion().expect("OS version is infallible");
        assert!(version.contains('.'));
        #[cfg(target_os = "macos")]
        assert!(version.starts_with("darwin."));
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        assert_eq!(version, "darwin.arm64");
    }

    /// 非 Linux 上 CPU 亲和性设置为空操作，调用应成功。
    #[test]
    fn affinity_is_a_noop_off_linux() {
        SetAffinity(&[0, 1]).expect("non-Linux affinity should be a no-op");
    }

    #[cfg(any(
        target_vendor = "apple",
        target_os = "freebsd",
        target_os = "dragonfly"
    ))]
    /// 在支持 LocalPeerCred 的平台上校验对端 UID。
    #[test]
    fn socket_uid_is_the_peer_process_uid() {
        let (left, _right) = UnixStream::pair().expect("create Unix socket pair");
        assert_eq!(
            GetSockUID(&left).expect("read local peer credentials"),
            unsafe { libc::geteuid() }
        );
    }
}

#[cfg(target_os = "windows")]
/// Windows 兜底：版本串前缀 `windows.`，亲和性为空操作，Unix socket UID 不可用。
mod windows_tests {
    use super::super::sys_windows::{GetSockUID, OSVersion, SetAffinity};

    /// 校验 Windows 三条 API 与 Go 兜底语义一致。
    #[test]
    fn windows_fallbacks_match_go() {
        let version = OSVersion().expect("OS version is infallible");
        assert!(version.starts_with("windows."));
        SetAffinity(&[0, 1]).expect("Windows affinity should be a no-op");
        let error = GetSockUID(()).expect_err("Unix sockets are unsupported");
        assert_eq!(
            error.to_string(),
            "UNIX domain socket is not supported on Windows"
        );
    }
}
