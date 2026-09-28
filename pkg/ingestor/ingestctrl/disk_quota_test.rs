// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// `CheckDiskQuota` 单元测试。
//
// 用固定一组 `EngineFileSize`（含 importing / 非 importing、磁盘与内存混合占用），
// 在不同 quota 下断言超额引擎集合与 in-progress 计数。

#![allow(dead_code)]
#![allow(non_snake_case)]

use crate::disk_quota::{CheckDiskQuota, DiskUsage};
use crate::{EngineFileSize, EngineId};

/// 测试用 DiskUsage：直接返回预置的引擎占用列表。
struct TestDiskUsage(Vec<EngineFileSize>);

impl DiskUsage for TestDiskUsage {
    fn EngineFileSizes(&self) -> Vec<EngineFileSize> {
        self.0.clone()
    }
}

// TestCheckDiskQuota 对应 Go 的同名测试：同一批 EngineFileSize 在不同 quota 下返回不同超额集合。
/// 覆盖无超额、仅最大非导入超额、多个非导入超额、以及 importing 只计 inProgress。
#[test]
pub fn TestCheckDiskQuota() {
    let uuid1 = EngineId(0x11111111111111111111111111111111);
    let uuid3 = EngineId(0x33333333333333333333333333333333);
    let uuid5 = EngineId(0x55555555555555555555555555555555);
    let uuid7 = EngineId(0x77777777777777777777777777777777);
    let uuid9 = EngineId(0x99999999999999999999999999999999);

    let disk_usage = TestDiskUsage(vec![
        EngineFileSize {
            UUID: uuid1,
            DiskSize: 1000,
            MemSize: 0,
            IsImporting: false,
        },
        EngineFileSize {
            UUID: uuid3,
            DiskSize: 2000,
            MemSize: 1000,
            IsImporting: true,
        },
        EngineFileSize {
            UUID: uuid5,
            DiskSize: 1500,
            MemSize: 3500,
            IsImporting: false,
        },
        EngineFileSize {
            UUID: uuid7,
            DiskSize: 0,
            MemSize: 7000,
            IsImporting: true,
        },
        EngineFileSize {
            UUID: uuid9,
            DiskSize: 4500,
            MemSize: 4500,
            IsImporting: false,
        },
    ]);

    // No quota exceeded：总占用未超过 30000，因此不会返回任何候选引擎。
    let result = CheckDiskQuota(&disk_usage, 30000);
    assert!(result.largeEngines.is_empty());
    assert_eq!(0, result.inProgressLargeEngines);
    assert_eq!(9000, result.totalDiskSize);
    assert_eq!(16000, result.totalMemSize);

    // Quota exceeded, the largest one is out：超额时先选择非 importing 且占用最大的 uuid9。
    let result = CheckDiskQuota(&disk_usage, 20000);
    assert_eq!(vec![uuid9], result.largeEngines);
    assert_eq!(0, result.inProgressLargeEngines);
    assert_eq!((9000, 16000), (result.totalDiskSize, result.totalMemSize));

    // Quota exceeded, the importing one should be ranked least priority：正在导入的引擎不优先暴露给调用方。
    let result = CheckDiskQuota(&disk_usage, 12000);
    assert_eq!(vec![uuid5, uuid9], result.largeEngines);
    assert_eq!(0, result.inProgressLargeEngines);
    assert_eq!((9000, 16000), (result.totalDiskSize, result.totalMemSize));

    // Quota exceeded, the importing ones should not be visible：importing 引擎只计入 inProgressLargeEngines。
    let result = CheckDiskQuota(&disk_usage, 5000);
    assert_eq!(vec![uuid1, uuid5, uuid9], result.largeEngines);
    assert_eq!(1, result.inProgressLargeEngines);
    assert_eq!((9000, 16000), (result.totalDiskSize, result.totalMemSize));
}

/// Go 的 `int64` 加法会按二进制补码回绕；配额判断必须保持同样语义。
#[test]
fn check_disk_quota_wraps_total_usage_like_go() {
    let uuid = EngineId(1);
    let disk_usage = TestDiskUsage(vec![EngineFileSize {
        UUID: uuid,
        DiskSize: i64::MAX,
        MemSize: 1,
        IsImporting: false,
    }]);

    let result = CheckDiskQuota(&disk_usage, 0);

    assert!(result.largeEngines.is_empty());
    assert_eq!(i64::MAX, result.totalDiskSize);
    assert_eq!(1, result.totalMemSize);
}
