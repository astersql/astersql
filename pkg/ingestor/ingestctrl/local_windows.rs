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

// Windows 平台下 local 后端的 rlimit 桩实现。
//
// Windows 无 Unix `RLIMIT_NOFILE` 语义；`GetSystemRLimit` 返回占位上限，
// `VerifyRLimit` 直接报错，提示 local-backend 未在 Windows 上验证。

use crate::{Error, Result};

/// 进程资源限制数值类型（Windows 桩：无符号 64 位）。
pub type RlimT = u64;

/// 返回占位的系统打开文件上限（`i32::MAX`），非真实 OS 限制。
pub fn GetSystemRLimit() -> Result<RlimT> {
    Ok(i32::MAX as u64)
}

/// Windows 上拒绝执行 local 后端的 rlimit 校验。
///
/// 与 Go 一致：未在 Windows 上测试 local-backend，要求关闭相关检查或换平台。
pub fn VerifyRLimit(_estimateMaxFiles: RlimT) -> Result<()> {
    Err(Error::InvalidData(
        "local-backend is not tested on Windows. Run with --check-requirements=false to disable this check, but you are on your own risk".into(),
    ))
}
