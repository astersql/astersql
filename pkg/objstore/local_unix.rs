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

// Unix 平台本地存储目录创建辅助。
//
// 通过临时清除 umask 并以 `0o777` 递归建目录，对齐 Go 侧 `mkdirAll` 行为。

#[cfg(not(windows))]
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

use anyhow::Result;

/// mkdirAll temporarily clears umask exactly like local_unix.go, then restores it.
#[cfg(not(windows))]
/// 临时清零 umask 后以 0o777 递归建目录；`UmaskGuard` 在 Drop 时恢复原 umask。
pub fn mkdirAll(base: &Path) -> Result<()> {
    // RAII：离开作用域时恢复进程 umask，避免影响后续文件创建权限。
    struct UmaskGuard(libc::mode_t);
    impl Drop for UmaskGuard {
        fn drop(&mut self) {
            unsafe { libc::umask(self.0) };
        }
    }

    // umask(0) 使后续创建不受掩码裁剪，对齐 Go 行为。
    let guard = UmaskGuard(unsafe { libc::umask(0) });
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true).mode(0o777);
    let result = builder.create(base).map_err(Into::into);
    drop(guard);
    result
}
