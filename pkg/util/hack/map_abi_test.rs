// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// Swiss map ABI / MemAwareMap 行为与布局公式测试。
//
// 对应 Go `pkg/util/hack/map_abi_test.go`。Rust HashMap 不能重解释为
// Go runtime map，故布局块核对 key/elem/slot 公式，其余通过 MemAwareMap
// 公开行为验证；benchmark 工作负载改为普通测试逐组比对。

use super::map_abi::{
    NewMemAwareMap, approxSize, groupSlotsOffset, maxTableCapacity, swissMapGroupSlots,
};
use super::map_abi_test_type_go125_test::{testMapGroupSlots, testMapTable};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::mem;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 用两个 u64 bit 模式模拟 Go complex128，便于 Hash/Eq。
struct Complex128 {
    real: u64,
    imag: u64,
}

impl Complex128 {
    fn new(real: f64, imag: f64) -> Self {
        Self {
            real: real.to_bits(),
            imag: imag.to_bits(),
        }
    }
}

impl Hash for Complex128 {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.real.hash(state);
        self.imag.hash(state);
    }
}

/// 按 Go group 公式返回 `(groupSize, slotSize, keySize)`。
fn group_layout<K, V>() -> (usize, usize, usize) {
    let key_size = mem::size_of::<K>();
    let elem_size = mem::size_of::<V>();
    let slot_size = key_size + elem_size;
    (
        groupSlotsOffset + swissMapGroupSlots as usize * slot_size,
        slot_size,
        key_size,
    )
}

// TestSwissTable 的 Rust 对照。Rust HashMap 不能重解释为 Go runtime map，
// 因此布局块核对同一组 key/elem/slot 公式，其余块通过 MemAwareMap 的公开行为验证。
#[test]
/// 总入口：常量、类型大小、group 布局与各 workload 子断言。
fn test_swiss_table() {
    assert_eq!(maxTableCapacity, 1024);
    assert_eq!(testMapGroupSlots, 8);
    assert_eq!(super::map_abi_go126::mapGroupSlots, 8);
    assert_eq!(
        mem::size_of::<testMapTable>(),
        mem::size_of::<super::map_abi::swissMapTable>()
    );
    assert_eq!(
        mem::size_of::<super::map_abi_test_type_go126_test::testMapTable>(),
        mem::size_of::<super::map_abi_go126::mapTable>()
    );

    assert_eq!(group_layout::<i64, i64>(), (136, 16, 8));
    assert_eq!(group_layout::<i32, i32>(), (72, 8, 4));
    assert_eq!(group_layout::<i8, i8>(), (24, 2, 1));
    assert_eq!(group_layout::<i64, f64>(), (136, 16, 8));
    assert_eq!(group_layout::<Complex128, Complex128>(), (264, 32, 16));

    assert_uint64_map_workload();
    assert_string_map_growth();
    assert_empty_then_growing_int_map();
    assert_mem_aware_complex_map();
}

// 对应 Go 的 uint64 directory 扫描块：特殊键和 1024 个普通键均可从包装 map
// 取回，元素计数准确，clear 后 map 为空但已分配容量仍可复用。
/// uint64 map：插入、查找、clear 后容量复用。
fn assert_uint64_map_workload() {
    const N: usize = 1024;
    let mut map = NewMemAwareMap::<u64, u64>(0);
    map.MockSeedForTest();
    map.Set(1234, 5678);
    for i in 0..N {
        map.Set(i as u64, (i * 2) as u64);
    }

    assert_eq!(map.Len(), N + 1);
    assert_eq!(map.Get(&1234), (Some(&5678), true));
    assert!(map.RealBytes() > 0);
    assert_eq!(map.M.seed(), super::map_abi::mockSeedForTest);

    let allocated = map.RealBytes();
    map.M.clear();
    assert_eq!(map.Len(), 0);
    assert_eq!(map.RealBytes(), allocated);
}

// 对应 Go 的 string map 增长块：保留 2000 次格式化 key 插入、元素计数和
// Go Swiss-table 分配的精确字节数。
/// string map：大量插入后长度与内存增长。
fn assert_string_map_growth() {
    const N: usize = 2000;
    let mut map = NewMemAwareMap::<String, i64>(0);
    map.MockSeedForTest();
    let initial = map.RealBytes();
    for i in 0..N {
        map.Set(format!("key-{i}"), i as i64);
    }

    assert_eq!(map.Len(), N);
    assert_eq!(map.Get(&"key-1234".to_owned()), (Some(&1234), true));
    assert!(map.RealBytes() > initial);
    assert_eq!(map.M.directory_len(), 4);
    assert_eq!(map.RealBytes(), 102608);
    assert!(map.Bytes > 0);
}

// 对应 Go 的 empty/small/growing int map 块，验证小 map 初值、覆盖写不增加
// 长度，以及跨过 checkpoint 后 Bytes 只增不减。
/// 空/小/增长 int map：checkpoint 与覆盖写行为。
fn assert_empty_then_growing_int_map() {
    let mut map = NewMemAwareMap::<i64, i64>(0);
    let initial = map.Bytes;
    assert_eq!(map.Len(), 0);
    assert_eq!(initial, 184);

    for i in 0..8 {
        assert_eq!(map.Set(i, i), 0);
    }
    assert_eq!(map.Len(), 8);
    assert_eq!(map.RealBytes(), 184);
    map.Set(9, 9);
    assert_eq!(map.RealBytes(), 360);
    let before_growth = map.Bytes;

    for i in 8..32 {
        map.Set(i, i);
    }
    assert_eq!(map.Len(), 32);
    assert!(map.Bytes >= before_growth);

    let before_replace = map.Bytes;
    assert_eq!(map.Set(9, 99), 0);
    assert_eq!(map.Len(), 32);
    assert_eq!(map.Bytes, before_replace);
    assert_eq!(map.Get(&9), (Some(&99), true));
}

// 对应 Go 的 MemAwareMap[complex128, complex128] 块，保留 51,199 次插入、
// delta 累加、估算下界、clear 后容量复用和 SetExt insert 判定。
/// complex128 MemAwareMap：大量插入、delta、SetExt 判定。
fn assert_mem_aware_complex_map() {
    const N: usize = 1024 * 50 - 1;
    let mut map = NewMemAwareMap::<Complex128, Complex128>(0);
    map.MockSeedForTest();
    let initial = map.Bytes as i64;
    let mut delta = initial;

    for i in 0..N {
        let key = Complex128::new(i as f64, i as f64);
        let change = map.Set(key, key);
        delta += change;
        if change > 0 {
            let expected_min = map.RealBytes() * 75 / 100;
            assert!(map.Bytes >= expected_min);
            assert!(approxSize(264, map.Len() as u64) >= expected_min);
        }
    }

    assert_eq!(map.Len(), N);
    assert_eq!(delta, map.Bytes as i64);
    assert_eq!(delta, 2702278);
    let allocated = map.RealBytes();
    assert_eq!(allocated, 2165296);
    assert_eq!(map.M.seed(), super::map_abi::mockSeedForTest);
    let clear_seq = map.M.clear_seq();
    map.M.clear();
    assert_eq!(map.M.clear_seq(), clear_seq + 1);
    assert_ne!(map.M.seed(), super::map_abi::mockSeedForTest);
    assert_eq!(map.Len(), 0);
    assert_eq!(map.RealBytes(), allocated);
    assert_eq!(delta, map.Bytes as i64);

    map.MockSeedForTest();
    for i in 0..1024 {
        let key = Complex128::new(i as f64, i as f64);
        let (change, inserted) = map.SetExt(key, key);
        assert_eq!(change, 0);
        assert!(inserted);
    }
    let duplicate = Complex128::new(100.0, 100.0);
    let (_change, inserted) = map.SetExt(duplicate, duplicate);
    assert!(!inserted);
    assert_eq!(map.Len(), 1024);
}

/// 原 Go benchmark 的四组规模输入。
static INPUTS: [usize; 4] = [1, 100, 10_000, 1_000_000];

/// MemAwareMap 写入再读回，返回最后一个值。
fn mem_aware_int_map(size: usize) -> usize {
    let mut result = 0;
    let mut map = NewMemAwareMap::<usize, usize>(0);
    for value in 0..size {
        map.Set(value, value);
    }
    for value in 0..size {
        result = *map.Get(&value).0.expect("inserted key must exist");
    }
    result
}

/// 原生 HashMap 对照 workload。
fn native_int_map(size: usize) -> usize {
    let mut result = 0;
    let mut map = HashMap::new();
    for value in 0..size {
        map.insert(value, value);
    }
    for value in 0..size {
        result = map[&value];
    }
    result
}

// Rust stable test harness 没有 Go testing.B；两个函数保留相同的四组 workload，
// 测试逐组执行并比较结果，避免 benchmark helper 退化成未运行代码。
/// 对 INPUTS 逐组跑 MemAwareMap workload。
fn benchmark_mem_aware_int_map() -> Vec<usize> {
    INPUTS.iter().map(|size| mem_aware_int_map(*size)).collect()
}

/// 对 INPUTS 逐组跑原生 HashMap workload。
fn benchmark_native_int_map() -> Vec<usize> {
    INPUTS.iter().map(|size| native_int_map(*size)).collect()
}

#[test]
/// 断言两套 workload 结果一致（替代 Go testing.B）。
fn benchmark_workloads_match_native_map() {
    assert_eq!(benchmark_mem_aware_int_map(), benchmark_native_int_map());
}
