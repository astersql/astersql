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
// Update 执行器辅助逻辑的单元测试。
//
// 覆盖重复键检查模式选择、外连接未匹配行判定，以及 UpdateRuntimeStats 的合并与格式化。

use crate::update::{
    UpdateDupKeyCheckMode, UpdateRuntimeStats, optimizeDupKeyCheckForUpdate, unmatchedOuterRow,
};
use std::time::Duration;
#[test]
/// 验证 Lazy/InPlace 选择、unmatchedOuterRow 与运行时统计 Merge/String。
fn update_modes_and_runtime_stats_cover_lazy_and_merge_paths() {
    assert_eq!(
        optimizeDupKeyCheckForUpdate(false, false, false),
        UpdateDupKeyCheckMode::InPlace
    );
    assert_eq!(
        optimizeDupKeyCheckForUpdate(true, false, false),
        UpdateDupKeyCheckMode::Lazy
    );
    assert_eq!(
        optimizeDupKeyCheckForUpdate(false, false, true),
        UpdateDupKeyCheckMode::InPlace
    );
    assert_eq!(
        optimizeDupKeyCheckForUpdate(true, false, true),
        UpdateDupKeyCheckMode::InPlace
    );
    assert_eq!(
        optimizeDupKeyCheckForUpdate(false, true, true),
        UpdateDupKeyCheckMode::Lazy
    );
    assert!(unmatchedOuterRow(true));
    let mut stats = UpdateRuntimeStats {
        fetch: Duration::from_secs(1),
        compose: Duration::from_secs(2),
        check_and_update: Duration::from_secs(3),
    };
    stats.Merge(&stats.Clone());
    assert_eq!(stats.fetch, Duration::from_secs(2));
    assert!(stats.String().contains("check-and-update:6s"));
}
