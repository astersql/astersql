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

// 行/索引 key 轻量分类（对应 Go `rowindexcodec`）。
//
// 在不全量解码 key 的前提下，仅根据表前缀、table id 与 `_r`/`_i` 分隔符
// 判断 key 属于行、索引或未知类型。

// This module preserves the row/index key classification semantics of the Go package.

// KeyKind is a specific type of key, mainly used to distinguish row/index.
// KeyKind 对应 Go 的 int 枚举，用于用最小解析成本区分未知、行 key 和索引 key。
/// key 类型枚举别名：未知 / 行 / 索引。
pub type KeyKind = i32;

// KeyKindUnknown indicates that this key is unknown type.
/// 无法识别为表行或索引的 key。
pub const KeyKindUnknown: KeyKind = 0;
// KeyKindRow means that this key belongs to row.
/// 表记录（行）key。
pub const KeyKindRow: KeyKind = 1;
// KeyKindIndex means that this key belongs to index.
/// 二级索引 key。
pub const KeyKindIndex: KeyKind = 2;

// Go var 块里的三个前缀保持为字节常量；后续判断只做 HasPrefix 级别的轻量匹配。
/// 表 key 全局前缀字节 `t`。
pub static tablePrefix: &[u8] = b"t";
/// 行记录分隔前缀 `_r`。
pub static rowPrefix: &[u8] = b"_r";
/// 索引分隔前缀 `_i`。
pub static indexPrefix: &[u8] = b"_i";

// GetKeyKind returns the KeyKind that matches the key in the minimalist way.
// GetKeyKind 按 Go 源码顺序检查表前缀、跳过 table id，再判断 row/index 分隔符。
/// 以最小解析成本返回 key 的行/索引/未知分类。
pub fn GetKeyKind(mut key: &[u8]) -> KeyKind {
    // [ TABLE_PREFIX | TABLE_ID | ROW_PREFIX (INDEX_PREFIX) | ROW_ID (INDEX_ID) | ... ]   (name)
    // [      1       |    8     |            2              |         8         | ... ]   (byte)
    if key.len() < 11 {
        // Go 在长度不足时直接返回 Unknown，避免后续切片越界。
        return KeyKindUnknown;
    }
    if !key.starts_with(tablePrefix) {
        return KeyKindUnknown;
    }

    // Go 的 `key = key[9:]` 跳过 1 字节表前缀和 8 字节 table id。
    key = &key[9..];
    if key.starts_with(rowPrefix) {
        return KeyKindRow;
    }
    if key.starts_with(indexPrefix) {
        return KeyKindIndex;
    }
    KeyKindUnknown
}

#[cfg(test)]
#[path = "rowindexcodec_test.rs"]
mod tests;
