// Copyright 2026 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// TopSQL/TopRU 全局状态单测：引用计数、interval 后写覆盖与非法值拒绝。
//
// 因 `GlobalState` 为进程级单例，测试间用互斥锁串行化并先重置引用计数。

use crate::test_util::{lock_global_state, reset_global_state};
use topsql_state::*;

// TestTopRUEnableDisableAndResetInterval 验证 TopRU enablement 使用引用计数，
// 且只有最后一个消费者退出时才把 item interval 重置为默认值。
#[test]
fn TestTopRUEnableDisableAndResetInterval() {
    let _guard = lock_global_state();
    reset_global_state();

    assert!(!TopRUEnabled());
    EnableTopRU();
    assert!(TopRUEnabled());
    EnableTopRU();
    assert!(TopRUEnabled());

    SetTopRUItemInterval(15).unwrap();
    assert_eq!(15, GetTopRUItemInterval());

    // 第一次 Disable 后仍有一个消费者，因此 interval 应保持 15。
    DisableTopRU();
    assert!(TopRUEnabled());
    assert_eq!(15, GetTopRUItemInterval());

    // 第二次 Disable 让引用计数归零，Go 逻辑会重置 interval 为默认 60。
    DisableTopRU();
    assert!(!TopRUEnabled());
    assert_eq!(DefTiDBTopRUItemIntervalSeconds, GetTopRUItemInterval());

    // 额外 Disable 不能让引用计数下溢，也不能重新开启 TopRU。
    DisableTopRU();
    assert!(!TopRUEnabled());
}

// TestTopRUItemIntervalLastWriteWins 验证合法 setter 的最后一次写入生效。
#[test]
fn TestTopRUItemIntervalLastWriteWins() {
    let _guard = lock_global_state();
    reset_global_state();

    SetTopRUItemInterval(30).unwrap();
    assert_eq!(30, GetTopRUItemInterval());

    SetTopRUItemInterval(60).unwrap();
    assert_eq!(60, GetTopRUItemInterval());

    SetTopRUItemInterval(15).unwrap();
    assert_eq!(15, GetTopRUItemInterval());
}

// TestTopRUItemIntervalRejectsInvalid 验证非法 interval 返回 ErrInvalidTopRUItemInterval，
// 并且不会覆盖现有合法值；0 会按 Go 语义归一化为默认 60。
#[test]
fn TestTopRUItemIntervalRejectsInvalid() {
    let _guard = lock_global_state();
    reset_global_state();

    let error = SetTopRUItemInterval(1).unwrap_err();
    assert_eq!(TopRUStateError::InvalidItemInterval(1), error);
    assert!(error.to_string().starts_with(ErrInvalidTopRUItemInterval));
    assert_eq!(DefTiDBTopRUItemIntervalSeconds, GetTopRUItemInterval());

    SetTopRUItemInterval(0).unwrap();
    assert_eq!(DefTiDBTopRUItemIntervalSeconds, GetTopRUItemInterval());

    SetTopRUItemInterval(15).unwrap();
    assert_eq!(15, GetTopRUItemInterval());

    let error = SetTopRUItemInterval(99).unwrap_err();
    assert_eq!(TopRUStateError::InvalidItemInterval(99), error);
    assert!(error.to_string().starts_with(ErrInvalidTopRUItemInterval));
    assert_eq!(15, GetTopRUItemInterval());

    // Go 注释说明 0 会 normalized to 60，即回到默认 TopRU item interval。
    SetTopRUItemInterval(0).unwrap();
    assert_eq!(DefTiDBTopRUItemIntervalSeconds, GetTopRUItemInterval());
}
