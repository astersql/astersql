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

// 重复键检测用的内部键（InternalKey）表示与编解码。
//
// `InternalKey` 由用户键与来源 `key_id` 组成；编码时对用户键做
// memcomparable（可内存比较的字节序）编码，再追加原始 `key_id`，
// 使外排结果同时按键值与来源顺序排列，便于扫描相邻重复项。

use std::cmp::Ordering;
use std::fmt;

use crate::util::codec::{DecodeBytes, EncodeBytes};
use crate::util::extsort::external_sorter::Error;

/// The sortable representation used by the duplicate detector.
/// 重复检测器用的可排序内部键：用户键 + 来源标识 key_id。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct InternalKey {
    /// 用户可见/业务侧的键字节。
    pub(crate) key: Vec<u8>,
    /// 键来源标识（如写入序号），用于区分同一用户键的多条记录。
    pub(crate) key_id: Vec<u8>,
}

impl InternalKey {
    /// 构造内部键。
    pub fn new(key: Vec<u8>, key_id: Vec<u8>) -> Self {
        Self { key, key_id }
    }
}

impl fmt::Display for InternalKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // 以大写十六进制打印 key，若有 key_id 则用 `@` 分隔。
        write_upper_hex(f, &self.key)?;
        if !self.key_id.is_empty() {
            f.write_str("@")?;
            write_upper_hex(f, &self.key_id)?;
        }
        Ok(())
    }
}

/// 将字节切片格式化为大写十六进制写入 formatter。
fn write_upper_hex(f: &mut fmt::Formatter<'_>, bytes: &[u8]) -> fmt::Result {
    for byte in bytes {
        write!(f, "{byte:02X}")?;
    }
    Ok(())
}

/// 比较两个内部键：先比 key，再比 key_id；返回 -1/0/1（对齐 Go 侧语义）。
pub fn compare_internal_key(a: &InternalKey, b: &InternalKey) -> i32 {
    match a.key.cmp(&b.key).then_with(|| a.key_id.cmp(&b.key_id)) {
        Ordering::Less => -1,
        Ordering::Equal => 0,
        Ordering::Greater => 1,
    }
}

/// Encodes the user key using TiDB's memcomparable byte codec and appends the
/// source key ID verbatim, matching `encodeInternalKey` in Go.
/// 使用 TiDB memcomparable 字节编解码对用户键编码，再原样追加来源 key_id。
pub fn encode_internal_key(append_to: &mut Vec<u8>, internal_key: &InternalKey) {
    let encoded = EncodeBytes(std::mem::take(append_to), &internal_key.key);
    *append_to = encoded;
    append_to.extend_from_slice(&internal_key.key_id);
}

/// 解码外排存储中的内部键：先 DecodeBytes 取出用户键，剩余字节为 key_id。
pub fn decode_internal_key(data: &[u8], internal_key: &mut InternalKey) -> Result<(), Error> {
    // Decode into a temporary buffer: the owned codec drops its buffer on error,
    // whereas Go keeps the original slice header and any writes to its backing array.
    let (leftover, key) = match DecodeBytes(data, None) {
        Ok(decoded) => decoded,
        Err(error) => {
            let mut offset = 0;
            for group in data.chunks_exact(9) {
                let padding = usize::from(255 - group[8]);
                if padding > 8 {
                    break;
                }
                let count = 8 - padding;
                // Once append allocates a new Go backing array, later writes no
                // longer affect the original key, including on padding failure.
                if offset + count > internal_key.key.capacity() {
                    break;
                }
                let end = (offset + count).min(internal_key.key.len());
                if offset < end {
                    internal_key.key[offset..end].copy_from_slice(&group[..end - offset]);
                }
                offset += count;
                if padding != 0 {
                    break;
                }
            }
            return Err(error.into());
        }
    };
    internal_key.key.clear();
    internal_key.key.extend_from_slice(&key);
    internal_key.key_id.clear();
    internal_key.key_id.extend_from_slice(leftover);
    Ok(())
}
