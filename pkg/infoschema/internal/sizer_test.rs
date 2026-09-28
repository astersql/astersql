// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// `MemoryUsage` / `sizeof` 的单元测试。
//
// 对应 Go `TestSize`：用表驱动用例核对数组、切片、字符串、map、嵌套结构体的手工计算结果，
// 覆盖 capacity 大于 length 的切片与结构体 padding。

use std::collections::HashMap;
use std::mem::size_of;
use std::rc::Rc;
use std::sync::Arc;

use super::{MemoryUsage, SizeCache, sizeof};

/// NestedInner 对应 Go TestSize 里匿名嵌套 struct { i int8; s string }。
#[repr(C)]
struct NestedInner {
    i: i8,
    s: &'static str,
}

/// NestedOuter 对应 Go TestSize 里外层匿名 struct。
#[repr(C)]
struct NestedOuter {
    slice: Vec<i64>,
    array: [bool; 2],
    structure: NestedInner,
}

impl MemoryUsage for NestedInner {
    fn memory_usage(&self, cache: &mut SizeCache) -> isize {
        // Go struct: sum(sizeOf(field)) + (Type.Size() - sum(Field.Type().Size())).
        // 字段占用之和，再加上结构体对齐 padding。
        let mut sum = 0isize;
        let si = self.i.memory_usage(cache);
        if si < 0 {
            return -1;
        }
        sum += si;
        let ss = self.s.memory_usage(cache);
        if ss < 0 {
            return -1;
        }
        sum += ss;
        let padding =
            size_of::<Self>() as isize - size_of::<i8>() as isize - size_of::<&str>() as isize;
        sum + padding
    }
}

impl MemoryUsage for NestedOuter {
    fn memory_usage(&self, cache: &mut SizeCache) -> isize {
        // 依次累加 slice / array / 内嵌结构体，再补外层 padding。
        let mut sum = 0isize;
        let s_slice = self.slice.memory_usage(cache);
        if s_slice < 0 {
            return -1;
        }
        sum += s_slice;
        let s_array = self.array.memory_usage(cache);
        if s_array < 0 {
            return -1;
        }
        sum += s_array;
        let s_structure = self.structure.memory_usage(cache);
        if s_structure < 0 {
            return -1;
        }
        sum += s_structure;
        let padding = size_of::<Self>() as isize
            - size_of::<Vec<i64>>() as isize
            - size_of::<[bool; 2]>() as isize
            - size_of::<NestedInner>() as isize;
        sum + padding
    }
}

/// SizeCase 对应 Go 表驱动测试中的匿名 struct。
struct SizeCase {
    name: &'static str,
    got: isize,
    want: isize,
}

struct FailingSize;

impl MemoryUsage for FailingSize {
    fn memory_usage(&self, _cache: &mut SizeCache) -> isize {
        -1
    }
}

// test_size 对应 Go 的 TestSize。
// 每个 want 沿用 Go 注释中的手工计算，覆盖容量大于长度的切片和结构体 padding。
#[test]
fn test_size() {
    // capacity=5、仅 push 2 个字符串，验证未用槽位计入 sizeof。
    let mut string_slice: Vec<&str> = Vec::with_capacity(5);
    string_slice.push("AAAAAAAAAA");
    string_slice.push("BBBBBBBBBBBB");

    let mut int_slice: Vec<i64> = Vec::with_capacity(5);
    int_slice.resize(2, 0);

    // map 记账含 Go 头大小与 bucket 开销系数 10.79。
    let mut map_val: HashMap<i64, &str> = HashMap::new();
    map_val.insert(0, "ABC");
    map_val.insert(1, "DEFG");

    let nested = NestedOuter {
        slice: vec![12345, 67890], // 2 * 8 + 24 = 40
        array: [true, false],      // 2 * 1 = 2
        structure: NestedInner {
            i: 5,     // 1
            s: "abc", // 3 + 16 = 19
        }, // 20 + 7 (padding) = 27
    }; // 40 + 2 + 27 = 69 + 6 (padding) = 75

    let tests = [
        SizeCase {
            name: "Array",
            got: sizeof(&[1i32, 2, 3]), // 3 * 4 = 12
            want: 12,
        },
        SizeCase {
            name: "Array 2",
            got: sizeof(&[""; 5]), // 5 * 16 = 80
            want: 80,
        },
        SizeCase {
            name: "Slice",
            got: sizeof(&int_slice), // 5 * 8 + 24 = 64
            want: 64,
        },
        SizeCase {
            name: "string Slice",
            got: sizeof(&string_slice), // 5 * 16 + 10 + 12 + 24 = 126
            want: 126,
        },
        SizeCase {
            name: "String",
            got: sizeof("ABCdef"), // 6 + 16 = 22
            want: 22,
        },
        SizeCase {
            name: "Map",
            // (8 + 3 + 16) + (8 + 4 + 16) = 55
            // 55 + 8 + 10.79 * 2 = 84
            got: sizeof(&map_val),
            want: 84,
        },
        SizeCase {
            name: "Struct",
            got: sizeof(&nested),
            want: 75,
        },
    ];

    for tt in tests {
        // Go 使用 t.Run(tt.name, ...)，这里保留子用例名称和 Sizeof(tt.v) == want 的断言。
        assert_eq!(
            tt.got, tt.want,
            "Of() = {}, want {} ({})",
            tt.got, tt.want, tt.name
        );
    }
}

#[test]
fn test_owned_string_uses_length_not_capacity() {
    let mut value = String::with_capacity(32);
    value.push_str("abc");

    assert_eq!(
        sizeof(&value),
        size_of::<String>() as isize + value.len() as isize
    );
}

#[test]
fn test_tuple_propagates_size_failure() {
    assert_eq!(sizeof(&(FailingSize, 0u8)), -1);
}

#[test]
fn test_shared_pointers_match_go_pointer_accounting() {
    let arc = Arc::new(7u64);
    let arcs = [Arc::clone(&arc), Arc::clone(&arc)];
    let rc = Rc::new(7u64);
    let rcs = [Rc::clone(&rc), Rc::clone(&rc)];

    // Go counts the pointer word for every field, but follows a shared pointee
    // only once. Runtime reference counters are not part of reflect.Ptr.
    let want = (3 * size_of::<usize>()) as isize;
    assert_eq!(sizeof(&arcs), want);
    assert_eq!(sizeof(&rcs), want);
}
