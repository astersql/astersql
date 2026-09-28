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

// Linux 平台系统信息与进程绑定辅助函数。
//
// 提供与 Go `pkg/util/sys/linux` 对齐的接口：读取 `uname` 版本串、
// 设置当前进程 CPU 亲和性（affinity，将线程/进程绑定到指定 CPU 集合）、
// 以及通过 Unix domain socket 的 `SO_PEERCRED` 获取对端有效用户 ID（UID）。

#![allow(non_snake_case)]

use std::io;
use std::mem;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;

/// 将 C 风格整型字符数组转换为 Rust 字符串，在首个 0 处截断。
pub(crate) fn charsToString<T>(ca: &[T]) -> String
where
    T: Copy + Into<i64>,
{
    // 对应 Go 侧遍历到 `\0` 为止再拼字符串；非法 UTF-8 用 lossy 替换以保持可打印。
    let bytes: Vec<u8> = ca
        .iter()
        .copied()
        .map(Into::into)
        .take_while(|value| *value != 0)
        .map(|value| value as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Returns operating-system version information in Go's
/// `Sysname Release.Machine` shape.
/// 返回操作系统版本信息，格式为 Go 风格的 `Sysname Release.Machine`。
pub fn OSVersion() -> io::Result<String> {
    // uname 写入 utsname；失败时透传 last_os_error。
    let mut name = unsafe { mem::zeroed::<libc::utsname>() };
    if unsafe { libc::uname(&mut name) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(format!(
        "{} {}.{}",
        charsToString(&name.sysname),
        charsToString(&name.release),
        charsToString(&name.machine)
    ))
}

/// Sets the CPU affinity of the current process.
/// 设置当前进程的 CPU 亲和性（绑定到 `cpus` 指定的 CPU 集合）。
pub fn SetAffinity(cpus: &[i32]) -> io::Result<()> {
    let mut set = unsafe { mem::zeroed::<libc::cpu_set_t>() };
    unsafe { libc::CPU_ZERO(&mut set) };
    for &cpu in cpus {
        // x/sys/unix.CPUSet.Set ignores values outside the backing bitset.
        // 越界 CPU 编号被忽略，与 Go CPUSet.Set 行为一致。
        if cpu >= 0 && (cpu as usize) < libc::CPU_SETSIZE as usize {
            unsafe { libc::CPU_SET(cpu as usize, &mut set) };
        }
    }
    // 对当前进程（getpid）调用 sched_setaffinity。
    let result =
        unsafe { libc::sched_setaffinity(libc::getpid(), mem::size_of::<libc::cpu_set_t>(), &set) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Returns the effective UID of the peer connected to a Unix-domain socket.
/// 返回 Unix domain socket 对端连接进程的有效 UID。
pub fn GetSockUID(socket: &UnixStream) -> io::Result<u32> {
    // SO_PEERCRED 返回对端 ucred；校验返回长度以防内核/ABI 不匹配。
    let mut credential = unsafe { mem::zeroed::<libc::ucred>() };
    let mut length = mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            socket.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credential as *mut libc::ucred).cast(),
            &mut length,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize != mem::size_of::<libc::ucred>() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "SO_PEERCRED returned an unexpected credential size",
        ));
    }
    Ok(credential.uid)
}
