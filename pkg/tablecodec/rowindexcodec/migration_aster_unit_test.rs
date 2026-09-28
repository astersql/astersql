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

// 迁移用单元测试：验证 `GetKeyKind` 对表 key 布局的轻量分类。
//
// 表 key 布局为 `t` + 8 字节 table id + `_r`/`_i` 分隔符 + 后缀；
// 分类只看前缀与分隔符，不解析完整行/索引内容。

use super::{GetKeyKind, KeyKindIndex, KeyKindRow, KeyKindUnknown};

/// 构造以固定 table id 前缀开头的测试 key：`t` + 8 字节零填充 id + 分隔符 + 后缀。
fn key(separator: &[u8], suffix: &[u8]) -> Vec<u8> {
    // 与 Go 测试一致：table id 全零时仍满足 1+8 字节最小前缀长度。
    let mut key = Vec::from(b"t\x80\0\0\0\0\0\0\0" as &[u8]);
    key.extend_from_slice(separator);
    key.extend_from_slice(suffix);
    key
}

/// 验证典型 `_r`（行）与 `_i`（索引）示例能正确分类。
#[test]
fn migration_classifies_go_row_and_index_examples() {
    assert_eq!(GetKeyKind(&key(b"_r", b"")), KeyKindRow);
    assert_eq!(GetKeyKind(&key(b"_i", b"\x80\0\0\0\0\0\0\0")), KeyKindIndex);
}

/// 长度不足 11 字节或非表前缀时，应返回 Unknown。
#[test]
fn migration_rejects_short_and_non_table_keys() {
    // 表前缀 + table id + 分隔符最少 1+8+2=11 字节，更短一律 Unknown。
    for length in 0..11 {
        assert_eq!(GetKeyKind(&vec![b't'; length]), KeyKindUnknown);
    }

    // 首字节不是 `t` 时，即使后续形似行 key 也判为 Unknown。
    let mut non_table = key(b"_r", b"");
    non_table[0] = b'x';
    assert_eq!(GetKeyKind(&non_table), KeyKindUnknown);
}

/// 分隔符必须位于跳过 table id 之后的位置，错位或未知分隔符均为 Unknown。
#[test]
fn migration_reads_the_separator_after_the_eight_byte_table_id() {
    assert_eq!(GetKeyKind(&key(b"_x", b"")), KeyKindUnknown);

    // `_r` 被放进 table id 区域时，真实分隔符位置变成 `_x`，不应误判为行 key。
    let mut misplaced = Vec::from(b"t_r\0\0\0\0\0\0" as &[u8]);
    misplaced.extend_from_slice(b"_x");
    assert_eq!(GetKeyKind(&misplaced), KeyKindUnknown);
}

/// 分类只依赖分隔符本身，后缀任意字节不影响 row/index 判定。
#[test]
fn migration_classification_ignores_bytes_after_the_separator() {
    assert_eq!(GetKeyKind(&key(b"_r", b"arbitrary-row-data")), KeyKindRow);
    assert_eq!(
        GetKeyKind(&key(b"_i", b"arbitrary-index-data")),
        KeyKindIndex
    );
}
