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

// `GetStorageSize` 冒烟测试：确认能从临时目录读到正的容量与可用空间。

use crate::GetStorageSize;

/// 对系统临时目录调用 GetStorageSize，断言 Capacity/Available 均大于 0。
#[test]
fn test_get_storage_size() {
    // only ensure we can get storage size.
    // 仅验证能取到磁盘容量，不校验具体数值
    let dir = std::env::temp_dir();
    let size = GetStorageSize(dir.to_str().unwrap()).expect("GetStorageSize");
    assert!(size.Capacity > 0);
    assert!(size.Available > 0);
}
