// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// Unix 平台下进程打开文件数（rlimit）的查询与提升。
//
// local 后端在大量并发写 SST / 打开引擎文件时需要足够的 `RLIMIT_NOFILE`。
// 本模块封装 `getrlimit`/`setrlimit`，在导入前按预估文件数抬高软限制，
// 并与 Go 侧一致：硬限制可能失败时仍以复读到的实际软限制为准。

use crate::local_unix_generic::RlimT;
use crate::{Error, Result};

/// 允许请求的打开文件数上限，防止无界抬升 soft/hard limit。
pub const maxRLimit: RlimT = 1_000_000;

/// 与 libc `rlimit` 布局对应的原始结构（软/硬限制）。
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct RawRLimit {
    pub(crate) current: u64,
    pub(crate) maximum: u64,
}

/// macOS 上 `RLIMIT_NOFILE` 的资源编号。
#[cfg(target_os = "macos")]
const RLIMIT_NOFILE: i32 = 8;
/// 非 macOS Unix 上 `RLIMIT_NOFILE` 的资源编号。
#[cfg(not(target_os = "macos"))]
const RLIMIT_NOFILE: i32 = 7;

unsafe extern "C" {
    fn getrlimit(resource: i32, limit: *mut RawRLimit) -> i32;
    fn setrlimit(resource: i32, limit: *const RawRLimit) -> i32;
}

/// 读取当前进程的 `RLIMIT_NOFILE`。
fn read_limit() -> Result<RawRLimit> {
    let mut limit = RawRLimit::default();
    // SAFETY: `limit` points to writable storage with the platform rlimit layout.
    if unsafe { getrlimit(RLIMIT_NOFILE, &mut limit) } != 0 {
        return Err(Error::Io(std::io::Error::last_os_error().to_string()));
    }
    Ok(limit)
}

/// 返回当前打开文件数的软限制（soft rlimit）。
pub fn GetSystemRLimit() -> Result<RlimT> {
    Ok(read_limit()?.current)
}

pub(crate) fn verify_rlimit_with(
    estimateMaxFiles: RlimT,
    mut get_limit: impl FnMut() -> Result<RawRLimit>,
    mut set_limit: impl FnMut(&RawRLimit) -> std::io::Result<()>,
) -> Result<()> {
    let requested = estimateMaxFiles.min(maxRLimit);
    let mut limit = get_limit()?;
    if limit.current >= requested {
        return Ok(());
    }
    let previous = limit.current;
    limit.current = requested;
    limit.maximum = limit.maximum.max(requested);
    set_limit(&limit).map_err(|error| {
        Error::Io(format!(
            "the maximum number of open file descriptors is too small, got {previous}, expect greater or equal to {requested}: {error}"
        ))
    })?;

    let actual = get_limit()?.current;
    if actual < requested {
        return Err(Error::Io(format!(
            "cannot update the maximum number of open file descriptors, expected: {requested}, got: {actual}. Please manually execute `ulimit -n {requested}` to increase the open files limit."
        )));
    }
    Ok(())
}

/// 按预估最大文件数校验并尝试抬高 `RLIMIT_NOFILE`。
///
/// 请求值会截断到 `maxRLimit`；若当前 soft 已足够则直接成功。
pub fn VerifyRLimit(estimateMaxFiles: RlimT) -> Result<()> {
    verify_rlimit_with(estimateMaxFiles, read_limit, |limit| {
        if unsafe { setrlimit(RLIMIT_NOFILE, limit) } == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error())
        }
    })
}
