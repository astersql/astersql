// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// `GetKeyKind` 的 Go 对齐单测：固定字节序列验证行/索引/未知分类。

// Keep the Go test inputs and assertions in the same order.

use super::{GetKeyKind, KeyKindIndex, KeyKindRow, KeyKindUnknown};

// TestGetKeyKind 对应 Go 的同名测试，逐项验证 row/index/unknown key 分类。
/// 覆盖典型行 key、索引 key、空切片与非表前缀等边界输入。
#[test]
pub fn TestGetKeyKind() {
    // require.Equal 在 实现中用 assert_eq! 保留比较值和失败语义。
    assert_eq!(
        KeyKindRow,
        GetKeyKind(&[116, 128, 0, 0, 0, 0, 0, 0, 0, 95, 114]),
    );
    assert_eq!(
        KeyKindIndex,
        GetKeyKind(&[
            116, 128, 0, 0, 0, 0, 0, 0, 0, 95, 105, 128, 0, 0, 0, 0, 0, 0, 0
        ]),
    );
    assert_eq!(KeyKindUnknown, GetKeyKind(&[]));
    // Go 的 nil []byte 在这里用空字节切片表达；两者都应得到 Unknown。
    assert_eq!(KeyKindUnknown, GetKeyKind(&[]));
    assert_eq!(
        KeyKindUnknown,
        GetKeyKind(&[120, 128, 0, 0, 0, 0, 0, 0, 0, 95, 114]),
    );
}
