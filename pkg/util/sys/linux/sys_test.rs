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

// `OSVersion` 冒烟测试：按目标 OS 选用对应实现并断言返回非空版本串。

#[cfg(target_os = "linux")]
use super::sys_linux::OSVersion;
#[cfg(all(not(target_os = "linux"), not(target_os = "windows")))]
use super::sys_other::OSVersion;
#[cfg(target_os = "windows")]
use super::sys_windows::OSVersion;

// TestGetOSVersion 对应 Go 测试函数：调用 OSVersion 并用 testify/require 检查结果。
/// 对应 Go `TestGetOSVersion`：确认 `OSVersion` 可读且非空。
#[test]
fn test_get_os_version() {
    let os_release = OSVersion().expect("OSVersion should read uname without error");
    assert!(!os_release.is_empty(), "OSVersion should not be empty");
}
