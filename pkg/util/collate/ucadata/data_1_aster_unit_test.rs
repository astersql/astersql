// Copyright 2023 PingCAP, Inc.
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

// `ucadata/data` 哨兵常量的迁移期单元测试。
//
// 校验 `LONG_RUNE_8` / `LongRune8` 与 Go 一致（`0xFFFD`），且可安全放入 `u16` 范围，
// 以便作为 MapTable 中的短整型哨兵使用。

use super::data::{LONG_RUNE_8, LongRune8};

/// 确认长权重哨兵数值、别名一致，且不超过 `u16::MAX`。
#[test]
fn long_rune_8_matches_go_sentinel_and_weight_storage() {
    let map_table_entry: u64 = LONG_RUNE_8;

    assert_eq!(map_table_entry, 0xFFFD);
    assert_eq!(LongRune8, LONG_RUNE_8);
    assert!(LONG_RUNE_8 <= u16::MAX as u64);
}
