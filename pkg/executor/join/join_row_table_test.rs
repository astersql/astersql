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

// Hash Join 行表（row table）布局与合并行为的单元测试。
//
// 行表是 Hash Join v2 构建侧把构建行按固定内存布局编码后的容器；本测试覆盖：
// 平台字长能否容纳指针、段（segment）行数/有效键/内存统计，以及多段合并后的全局行定位。


use crate::join_row_table::{RowTable, RowTableSegment, SIZE_OF_ELEMENT_SIZE, SIZE_OF_NEXT_PTR};
use crate::join_table_meta::EncodedRow;
use std::sync::atomic::{AtomicBool, Ordering};

/// 构造最小可测的编码行：单字节载荷，used 标志默认未置位。
fn encoded(value: u8) -> EncodedRow {
    EncodedRow {
        bytes: vec![value],
        null_map: vec![0],
        key_offset: 0,
        key_length: 1,
        row_data_offset: 1,
        used: AtomicBool::new(false),
    }
}

/// 校验 next 指针宽度与元素长度字段宽度匹配当前目标平台字长。
#[test]
fn platform_row_layout_widths_can_store_pointer_and_element_length() {
    assert_eq!(SIZE_OF_NEXT_PTR, std::mem::size_of::<usize>());
    assert_eq!(SIZE_OF_ELEMENT_SIZE, std::mem::size_of::<u32>());
    assert!(std::mem::size_of::<usize>() >= std::mem::size_of::<*const u8>());
}

/// 校验段内行计数、有效键计数、内存占用，以及 used 原子标志读写。
#[test]
fn row_table_segment_tracks_rows_valid_keys_memory_and_next_links() {
    let mut segment = RowTableSegment {
        rows: vec![encoded(1), encoded(2)],
        hash_values: vec![11, 22],
        valid_key_count: 1,
        ..RowTableSegment::default()
    };
    let before = segment.total_used_bytes();
    segment.init_tagged_bits();
    segment.rows[0].used.store(true, Ordering::Release);
    assert_eq!(segment.row_count(), 2);
    assert_eq!(segment.valid_key_count(), 1);
    assert!(before > 0);
    assert!(segment.rows[0].used.load(Ordering::Acquire));
}

/// 校验多段 merge 后全局行序、有效 join key 位置映射，以及 clear 清空段。
#[test]
fn row_table_merge_resolves_global_rows_and_valid_positions() {
    let mut first = RowTable::default();
    first.segments_mut().push(RowTableSegment {
        rows: vec![encoded(1), encoded(2)],
        valid_key_count: 1,
        valid_join_key_positions: vec![1],
        ..Default::default()
    });
    let mut second = RowTable::default();
    second.segments_mut().push(RowTableSegment {
        rows: vec![encoded(3)],
        valid_key_count: 1,
        valid_join_key_positions: vec![0],
        ..Default::default()
    });
    first.merge(second);
    assert_eq!(first.row_count(), 3);
    assert_eq!(first.valid_key_count(), 2);
    assert_eq!(first.get_row(2).expect("third row").bytes, [3]);
    assert_eq!(first.valid_join_key_position(0), Some(1));
    assert_eq!(first.valid_join_key_position(1), Some(2));
    assert_eq!(first.valid_join_key_position(2), None);
    first.clear_segments();
    assert_eq!(first.row_count(), 0);
}

/// 校验 EncodedRow clone 后 used 原子标志相互独立（互不影响）。
#[test]
fn encoded_row_clone_keeps_independent_atomic_used_flag() {
    let row = encoded(9);
    let cloned = row.clone();
    cloned.used.store(true, Ordering::Release);
    assert!(!row.used.load(Ordering::Acquire));
    assert!(cloned.used.load(Ordering::Acquire));
}
