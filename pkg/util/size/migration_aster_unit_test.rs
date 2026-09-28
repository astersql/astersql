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

// size 迁移回归：校验容量单位与 Go unsafe.Sizeof 头部大小常量。
//
// 同时用编译期断言确认 `size.rs` 含 AsterSQL 版权标记。

use super::*;

/// `size.rs` 源文本，供编译期版权检查嵌入。
const SOURCE: &str = include_str!("size.rs");

/// 朴素字节子串搜索（const 上下文可用），用于版权字符串断言。
const fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    let mut start = 0;
    // 从每个起始位置尝试匹配 needle。
    while start + needle.len() <= haystack.len() {
        let mut offset = 0;
        while offset < needle.len() && haystack[start + offset] == needle[offset] {
            offset += 1;
        }
        if offset == needle.len() {
            return true;
        }
        start += 1;
    }
    false
}

const _: () = assert!(contains(SOURCE.as_bytes(), b"// Copyright 2026 AsterSQL."));

/// 验证 KB/MB/GB/TB/PB 与 Go 二进制容量常量一致。
#[test]
fn binary_capacity_units_match_go_constants() {
    assert_eq!(KB, 1_024);
    assert_eq!(MB, 1_048_576);
    assert_eq!(GB, 1_073_741_824);
    assert_eq!(TB, 1_099_511_627_776);
    assert_eq!(PB, 1_125_899_906_842_624);
}

/// 验证标量类型 SizeOf* 与 Go unsafe.Sizeof / 平台字宽语义一致。
#[test]
fn scalar_sizes_match_go_unsafe_sizeof() {
    assert_eq!(SizeOfByte, 1);
    assert_eq!(SizeOfUint8, 1);
    assert_eq!(SizeOfBool, 1);
    assert_eq!(SizeOfInt32, 4);
    assert_eq!(SizeOfFloat64, 8);
    assert_eq!(SizeOfUint64, 8);
    assert_eq!(SizeOfInt64, 8);
    assert_eq!(SizeOfInt, std::mem::size_of::<isize>() as i64);
    assert_eq!(SizeOfUint, std::mem::size_of::<usize>() as i64);
}

/// 验证 slice/string/interface 等记录的是 Go 头部大小，不含底层数据。
#[test]
fn go_runtime_header_sizes_exclude_backing_storage() {
    let word = std::mem::size_of::<usize>() as i64;
    assert_eq!(SizeOfSlice, 3 * word);
    assert_eq!(SizeOfString, 2 * word);
    assert_eq!(SizeOfInterface, 2 * word);
    assert_eq!(SizeOfPointer, word);
    assert_eq!(SizeOfFunc, word);
    assert_eq!(SizeOfMap, word);
}
