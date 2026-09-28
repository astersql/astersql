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

// `GetTargetDirectoryCapacity` 迁移后的跨平台单元测试。
//
// 覆盖：已存在目录容量为正；缺失路径返回 NotFound；Unix 上非 UTF-8 路径仍能进入底层查询。

#[cfg(test)]
/// 容量查询行为断言集合。
mod tests {
    use super::super::GetTargetDirectoryCapacity;

    /// 临时目录应可查询且可用容量大于 0。
    #[test]
    fn capacity_of_existing_directory_is_positive() {
        let capacity = GetTargetDirectoryCapacity(std::env::temp_dir())
            .expect("the temporary directory must be queryable");
        assert!(capacity > 0, "available capacity must be positive");
    }

    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
    /// 不存在的路径应返回 `NotFound`（真实查询实现，非兜底平台）。
    #[test]
    fn missing_path_returns_the_os_error() {
        let missing = std::env::temp_dir().join(format!(
            "tidb-storage-migration-missing-{}",
            std::process::id()
        ));
        let error =
            GetTargetDirectoryCapacity(missing).expect_err("a path that does not exist must fail");
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
    }

    #[cfg(unix)]
    /// Go 字符串可含任意字节；非 UTF-8 路径不应在进入 `statfs` 前被当成 InvalidInput。
    #[test]
    fn posix_path_accepts_non_utf8_bytes_like_go_string() {
        use std::os::unix::ffi::OsStringExt;

        let path = std::path::PathBuf::from(std::ffi::OsString::from_vec(vec![0xff]));
        let error = GetTargetDirectoryCapacity(path)
            .expect_err("the non-UTF-8 path does not exist, but must reach statfs");
        assert_ne!(error.kind(), std::io::ErrorKind::InvalidInput);
    }
}
