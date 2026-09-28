// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// 有界最小堆单元测试：对齐 Go `bounded_min_heap_test.go`。
//
// 覆盖基础容量/超容量/重复值、单容量与零容量边界、自定义结构体、
// 反向比较器、满堆替换、大数据集 top-N，以及负容量构造 panic。

use super::NewBoundedMinHeap;

// testItem represents a simple test item with a value for comparison
// testItem 对应 Go 测试里的辅助结构，用 value 参与比较，用 name 校验元素内容未被打乱。
/// 测试用结构体：`value` 参与比较，`name` 校验排序后对应关系。
#[derive(Clone, Debug, PartialEq, Eq)]
struct testItem {
    value: i32,
    name: &'static str,
}

// intComparator compares integers (for max-heap behavior, return negative for smaller values)
// intComparator 保留 Go comparator 的三值约定：小于返回 -1，大于返回 1，相等返回 0。
/// 整数三值比较器（对齐 Go）：小返回 -1，大返回 1，等返回 0。
fn intComparator(a: &i32, b: &i32) -> i32 {
    if a < b {
        -1
    } else if a > b {
        1
    } else {
        0
    }
}

// testItemComparator compares testItems by value
// testItemComparator 只比较 value，name 字段用于后续断言验证排序后对应元素。
/// 仅按 `value` 比较的 `testItem` 比较器。
fn testItemComparator(a: &testItem, b: &testItem) -> i32 {
    intComparator(&a.value, &b.value)
}

// TestBoundedMinHeapBasic 对应 Go 的基础容量、超容量和重复值测试。
/// 空堆、容量内插入、超容量保留 top-3、重复值行为。
#[test]
fn TestBoundedMinHeapBasic() {
    let mut bmh = NewBoundedMinHeap(3, intComparator);

    // test empty state
    // Go 的 require.Nil 对应 ToSortedSlice 空堆返回 None 的语义。
    assert_eq!(0, bmh.Len());
    assert_eq!(None::<Vec<i32>>, bmh.ToSortedSlice());

    // test basic adding within capacity
    bmh.Add(5);
    bmh.Add(3);
    bmh.Add(8);
    assert_eq!(3, bmh.Len());
    assert_eq!(Some(vec![8, 5, 3]), bmh.ToSortedSlice());

    // test over capacity - should keep only top 3
    // 超过容量后只保留 comparator 认为最好的 3 个值。
    let items = vec![1, 9, 2, 7, 4];
    for item in items {
        bmh.Add(item);
    }
    assert_eq!(3, bmh.Len());
    assert_eq!(Some(vec![9, 8, 7]), bmh.ToSortedSlice());

    // test duplicate values
    // 重复值用于确认堆满后相等元素不会错误挤掉已有元素。
    let mut bmh2 = NewBoundedMinHeap(3, intComparator);
    let duplicates = vec![5, 5, 3, 8, 5];
    for item in duplicates {
        bmh2.Add(item);
    }
    assert_eq!(3, bmh2.Len());
    assert_eq!(Some(vec![8, 5, 5]), bmh2.ToSortedSlice());
}

// TestBoundedMinHeapEdgeCases 对应 Go 的单容量和零容量边界测试。
/// 容量 1 只留最大值；容量 0 永远为空。
#[test]
fn TestBoundedMinHeapEdgeCases() {
    // test single item capacity
    let mut bmh1 = NewBoundedMinHeap(1, intComparator);
    bmh1.Add(3);
    bmh1.Add(1);
    bmh1.Add(7);
    bmh1.Add(2);
    assert_eq!(1, bmh1.Len());
    assert_eq!(Some(vec![7]), bmh1.ToSortedSlice());

    // test zero capacity
    // Go 零容量 Add 直接返回；这里保留“永远为空”的断言。
    let mut bmh0 = NewBoundedMinHeap(0, intComparator);
    bmh0.Add(5);
    bmh0.Add(10);
    assert_eq!(0, bmh0.Len());
    assert_eq!(None::<Vec<i32>>, bmh0.ToSortedSlice());
}

// TestBoundedMinHeapCustomStruct 对应 Go 的自定义结构体排序测试。
/// 自定义结构体按 value 取 top-3，并校验 name 对应关系。
#[test]
fn TestBoundedMinHeapCustomStruct() {
    let mut bmh = NewBoundedMinHeap(3, testItemComparator);

    // add custom struct items
    bmh.Add(testItem {
        value: 10,
        name: "ten",
    });
    bmh.Add(testItem {
        value: 5,
        name: "five",
    });
    bmh.Add(testItem {
        value: 15,
        name: "fifteen",
    });
    bmh.Add(testItem {
        value: 8,
        name: "eight",
    });
    bmh.Add(testItem {
        value: 12,
        name: "twelve",
    });

    assert_eq!(3, bmh.Len());
    let result = bmh.ToSortedSlice().expect("Go 测试期望非空结果");

    // should have top 3 by value: 15, 12, 10
    // 这里逐字段断言，保留 Go 测试确认 name/value 对应关系的语义。
    assert_eq!(15, result[0].value);
    assert_eq!("fifteen", result[0].name);
    assert_eq!(12, result[1].value);
    assert_eq!("twelve", result[1].name);
    assert_eq!(10, result[2].value);
    assert_eq!("ten", result[2].name);
}

// TestBoundedMinHeapReverseComparator 对应 Go 的反向比较器测试，用最小值集合验证 comparator 可替换。
/// 反向比较器下保留最小的 3 个值。
#[test]
fn TestBoundedMinHeapReverseComparator() {
    // reverse comparator for min-heap behavior (keeping smallest values)
    let reverseComparator = |a: &i32, b: &i32| -> i32 {
        -intComparator(a, b) // reverse the comparison
    };

    let mut bmh = NewBoundedMinHeap(3, reverseComparator);

    let items = vec![9, 2, 7, 1, 8, 3];
    for item in items {
        bmh.Add(item);
    }

    assert_eq!(3, bmh.Len());
    let result = bmh.ToSortedSlice();
    // with reverse comparator, should keep smallest 3: 1, 2, 3
    assert_eq!(Some(vec![1, 2, 3]), result);
}

// TestBoundedMinHeapItemReplacement 对应 Go 的满堆替换和劣质元素忽略测试。
/// 满堆时更优替换、更差忽略，以及相等值不挤占。
#[test]
fn TestBoundedMinHeapItemReplacement() {
    let mut bmh = NewBoundedMinHeap(2, intComparator);

    bmh.Add(5);
    bmh.Add(3);

    // add better items - should replace worse ones
    // 满堆时新元素更好才替换根部最差元素。
    bmh.Add(10);
    bmh.Add(8);
    assert_eq!(2, bmh.Len());
    assert_eq!(Some(vec![10, 8]), bmh.ToSortedSlice());

    // try to add worse items - should be ignored
    // 较差元素不会改变已有 top N 集合。
    bmh.Add(2);
    bmh.Add(1);
    bmh.Add(4);
    assert_eq!(2, bmh.Len());
    assert_eq!(Some(vec![10, 8]), bmh.ToSortedSlice());

    // test equal values behavior
    let mut bmh2 = NewBoundedMinHeap(3, intComparator);
    bmh2.Add(5);
    bmh2.Add(5);
    bmh2.Add(5);
    bmh2.Add(5); // should not be added since heap is full and item is not better
    assert_eq!(3, bmh2.Len());
    assert_eq!(Some(vec![5, 5, 5]), bmh2.ToSortedSlice());
}

// TestBoundedMinHeap_LargeDataset 对应 Go 的大批量数据测试，确认只保留最高的 capacity 个值。
/// 1000 个递增输入后仅保留最大的 capacity 个值。
#[test]
fn TestBoundedMinHeap_LargeDataset() {
    const capacity: isize = 10;
    const dataSize: i32 = 1000;

    let mut bmh = NewBoundedMinHeap(capacity, intComparator);

    // add many items
    for i in 0..dataSize {
        bmh.Add(i);
    }

    assert_eq!(capacity as usize, bmh.Len());
    let result = bmh.ToSortedSlice().expect("Go 测试期望大数据集结果非空");

    // should have the top 10 values: 999, 998, ..., 990
    assert_eq!(capacity as usize, result.len());
    for i in 0..capacity as usize {
        let expected = dataSize - 1 - i as i32; // 999, 998, 997, ...
        assert_eq!(expected, result[i]);
    }
}

// TestNewBoundedMinHeapSafetyChecks 对应 Go 的构造器 panic 安全检查。
/// 负容量 panic；零/正容量可构造；零容量 Add 不保存元素。
#[test]
fn TestNewBoundedMinHeapSafetyChecks() {
    // test nil comparison function panic
    // Go 可以传 nil 函数触发 panic；Rust 泛型函数参数没有 nil 等价物，此处保留原测试意图说明。
    // require.Panics(t, func() { NewBoundedMinHeap[int](10, nil) })

    // test negative maxSize panic
    assert!(
        std::panic::catch_unwind(|| {
            NewBoundedMinHeap(-1, intComparator);
        })
        .is_err()
    );

    // test valid cases should not panic
    assert!(
        std::panic::catch_unwind(|| {
            NewBoundedMinHeap(0, intComparator);
        })
        .is_ok()
    );

    assert!(
        std::panic::catch_unwind(|| {
            NewBoundedMinHeap(10, intComparator);
        })
        .is_ok()
    );

    // verify that zero capacity heap works correctly
    let mut bmh = NewBoundedMinHeap(0, intComparator);
    bmh.Add(5);
    assert_eq!(0, bmh.Len());
}
