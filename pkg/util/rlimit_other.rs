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

// 非 Windows 平台的进程资源限制（rlimit）查询。
//
// 对应 Go `pkg/util/rlimit_other.go`：通过 `getrlimit(RLIMIT_NOFILE)` 读取
// 当前进程可打开文件数的软限制（soft limit），供连接池、文件句柄配额等使用。

#![cfg(not(windows))]

use std::io;
use std::mem::MaybeUninit;

// GenRLimit get RLIMIT_NOFILE limit
// GenRLimit 对应 Go 的同名函数，返回当前进程可打开文件数软限制。
/// 读取 `RLIMIT_NOFILE` 软限制；失败时打日志并回退为 1024。
pub fn GenRLimit(source: &str) -> u64 {
    gen_rlimit_with(
        source,
        || {
            // `getrlimit` initializes the output only when it succeeds.
            let mut limit = MaybeUninit::<libc::rlimit>::uninit();
            if unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, limit.as_mut_ptr()) } != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(unsafe { limit.assume_init() })
        },
        |source, err| {
            log::warn!("[{source}] get system open file limit error: {err}; default=1024");
        },
    )
}

pub(crate) fn gen_rlimit_with(
    source: &str,
    getrlimit: impl FnOnce() -> io::Result<libc::rlimit>,
    warn: impl FnOnce(&str, &io::Error),
) -> u64 {
    match getrlimit() {
        Ok(limit) => limit.rlim_cur as u64,
        Err(err) => {
            warn(source, &err);
            1024
        }
    }
}
