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

// Hash Join V1 哈希行容器与并发 map 哈希表的单元测试。
//
// 覆盖：join key 哈希与碰撞（hash collision）、NULL key 过滤、
// NULL-aware（感知 NULL）匹配、内存 spill（落盘）以及 unsafe/并发两类
// `BaseHashTable` 的桶内多行与 memory delta 清零行为。

use crate::hash_table_v1::{
    BaseHashTable, ConcurrentMapHashTable, HashContext, HashRowContainer, RowPointer,
    UnsafeHashTable,
};
use crate::row_table_builder::Value;

/// 将值切片克隆为测试用行向量。
fn row(values: &[Value]) -> Vec<Value> {
    values.to_vec()
}

/// 校验 HashContext 对相同 key 产生相同哈希、NULL 标记正确，
/// 以及 HashRowContainer 在普通/并发模式下的匹配、NA 行、mark_used 与 spill。
#[test]
fn hash_context_and_row_container_match_keys_collisions_nulls_and_spill() {
    let rows = vec![
        row(&[Value::Int(1), Value::Text("a".into())]),
        row(&[Value::Int(1), Value::Text("b".into())]),
        row(&[Value::Null, Value::Text("n".into())]),
        row(&[Value::Int(2), Value::Text("c".into())]),
    ];
    let mut context = HashContext::new(vec![0]);
    context.init_hash(&rows).unwrap();
    assert_eq!(context.has_null, [false, false, true, false]);
    assert_eq!(context.hash_values[0], context.hash_values[1]);

    // 分别走 unsafe 与 concurrent 两种行容器实现。
    for concurrent in [false, true] {
        let mut container = HashRowContainer::new(vec![0], concurrent, rows.len());
        container.put_chunk(rows.clone()).unwrap();
        let matched = container.get_matched_rows(&row(&[Value::Int(1)])).unwrap();
        assert_eq!(matched.len(), 2);
        assert_eq!(container.row(matched[0]).unwrap()[0], Value::Int(1));
        // NULL probe key 在普通匹配路径上应被过滤为空。
        assert!(
            container
                .get_matched_rows(&row(&[Value::Null]))
                .unwrap()
                .is_empty()
        );
        // NULL-aware 路径仍能命中含 NULL 的构建侧行。
        assert_eq!(
            container.get_na_rows(&row(&[Value::Int(9)])).unwrap().len(),
            1
        );
        container.mark_used(matched[0]);
        assert_eq!(container.unmatched_rows().len(), 3);
        assert!(container.memory_bytes() > 0);
        // spill：内存不足时将构建侧数据落到磁盘。
        container.spill();
        assert!(container.already_spilled());
        assert_eq!(container.memory_bytes(), 0);
        assert!(container.disk_bytes() > 0);
        container.close();
        assert!(container.is_empty());
    }
}

/// NAAJ NULL 桶应把 NULL 当作未知值，只比较构建/探测两侧均非 NULL 的键列。
#[test]
fn na_null_bucket_matches_only_equal_non_null_key_positions() {
    let rows = vec![
        row(&[Value::Int(1), Value::Null]),
        row(&[Value::Int(2), Value::Null]),
        row(&[Value::Null, Value::Int(2)]),
        row(&[Value::Null, Value::Null]),
    ];
    let mut container = HashRowContainer::new(vec![0, 1], false, rows.len());
    container.put_chunk(rows).unwrap();

    let pointers = container
        .get_na_rows(&row(&[Value::Int(1), Value::Int(3)]))
        .unwrap();
    assert_eq!(pointers.len(), 2);
    assert_eq!(
        container.row(pointers[0]).unwrap(),
        &row(&[Value::Int(1), Value::Null])
    );
    assert_eq!(
        container.row(pointers[1]).unwrap(),
        &row(&[Value::Null, Value::Null])
    );

    let pointers = container
        .get_na_rows(&row(&[Value::Int(1), Value::Null]))
        .unwrap();
    assert_eq!(pointers.len(), 3);
    assert!(
        pointers.iter().any(|pointer| {
            container.row(*pointer) == Some(&row(&[Value::Int(1), Value::Null]))
        })
    );
    assert!(
        pointers.iter().any(|pointer| {
            container.row(*pointer) == Some(&row(&[Value::Null, Value::Int(2)]))
        })
    );
    assert!(
        pointers
            .iter()
            .any(|pointer| { container.row(*pointer) == Some(&row(&[Value::Null, Value::Null])) })
    );
}

/// Go 的动态位图支持超过 64 个 NAAJ 键；Rust 至少必须保持完整的 NULL 行分类。
#[test]
fn hash_context_marks_null_keys_beyond_first_bitmap_word() {
    let mut values = vec![Value::Int(1); 65];
    values[64] = Value::Null;
    let mut context = HashContext::new((0..65).collect());
    context.init_hash(&[values]).unwrap();
    assert_eq!(context.has_null, [true]);
}

/// 校验 UnsafeHashTable 与 ConcurrentMapHashTable 在冲突桶、遍历与 memory delta 上的一致性。
#[test]
fn unsafe_and_concurrent_hash_tables_preserve_duplicate_bucket_rows_and_delta() {
    /// 向表中写入大量带冲突的条目，并断言长度、桶密度、遍历与 delta 清零。
    fn exercise(table: &mut dyn BaseHashTable) {
        // 用取模制造大量 hash collision，验证冲突链完整性。
        for index in 0..6656 {
            table.put(
                (index % 111) as u64,
                RowPointer {
                    chunk_index: index / 128,
                    row_index: index,
                },
            );
        }
        assert_eq!(table.len(), 6656);
        assert_eq!(table.get(0).len(), 60);
        let mut visited = 0;
        table.for_each(&mut |_, _| visited += 1);
        assert_eq!(visited, 6656);
        // GetAndCleanMemoryDelta 第一次取走增量后应清零。
        assert!(table.get_and_clean_memory_delta() > 0);
        assert_eq!(table.get_and_clean_memory_delta(), 0);
    }
    exercise(&mut UnsafeHashTable::with_capacity(6656));
    exercise(&mut ConcurrentMapHashTable::default());
}
