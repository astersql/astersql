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

// Handle（行句柄）测试辅助：构造 common handle，以及对整型 handle 做分片位掩码排序。
//
// Common handle 是非整数主键的编码形式；分片（shard）位位于高位，排序时常需先掩掉后再比较低位。

use std::any::Any;

// MustNewCommonHandle creates a common handle with the given values.
/// 将任意类型值编码为 common handle；编码或构造失败时直接 panic（测试专用）。
pub fn MustNewCommonHandle(values: Vec<&dyn Any>) -> Box<dyn kv::Handle> {
    let encoded = codec::EncodeKey(chrono_tz::UTC, Vec::new(), types::MakeDatums(values))
        .expect("encode common handle values");
    Box::new(kv::NewCommonHandle(encoded).expect("construct common handle"))
}

// MaskSortHandles sorts the handles by the lowest
// (fieldTypeBits - 1 - shardBitsCount) bits.
/// 按字段类型位宽与分片位数，掩掉符号位与分片高位后，对 handle 低位排序。
///
/// `fieldType` 为 MySQL 字段类型码；`shardBitsCount` 为表级 `shard_row_id_bits`。
pub fn MaskSortHandles(handles: Vec<i64>, shardBitsCount: isize, fieldType: u8) -> Vec<i64> {
    // 由字段类型字节长度换算为位宽，再计算需左移丢弃的高位数量。
    let typeBitsLength = mysql::DefaultLengthOfMysqlTypes
        .iter()
        .find_map(|(field_type, byte_length)| {
            (*field_type == fieldType).then_some((*byte_length * 8) as isize)
        })
        .unwrap_or(0);
    const signBitCount: isize = 1;
    let shiftBitsCount = 64 - typeBitsLength + shardBitsCount + signBitCount;
    assert!(shiftBitsCount >= 0, "negative shift count");

    // 先算术移位保留低位，再不稳定排序；移位量过大时低位视为 0。
    let mut ordered = handles
        .into_iter()
        .map(|handle| {
            if shiftBitsCount >= i64::BITS as isize {
                0
            } else {
                (handle << shiftBitsCount as u32) >> shiftBitsCount as u32
            }
        })
        .collect::<Vec<_>>();
    ordered.sort_unstable();
    ordered
}
