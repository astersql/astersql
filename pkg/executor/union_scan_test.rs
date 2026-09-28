// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
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

// UnionScan 行比较器单元测试。
//
// 验证 `compareExec`：先按索引列比较，再按 handle（行标识）比较；
// `descending` 为 true 时翻转比较结果（对应 ORDER BY DESC）。

use crate::union_scan::{UnionScanExec, UnionScanRuntime, compareExec};
use astersql_util_chunk::Chunk;
use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::{HashSet, VecDeque};

#[derive(Default)]
struct MockRuntime {
    added: VecDeque<i32>,
    snapshot_batches: VecDeque<Vec<i32>>,
    modified: HashSet<i32>,
    output: RefCell<Vec<i32>>,
    output_rows: Cell<usize>,
    capacity: usize,
    cached: bool,
    opened: bool,
    iterator_built: bool,
    iterator_closed: bool,
    child_closed: bool,
    fail_compare: bool,
}

impl UnionScanRuntime for MockRuntime {
    type Context = ();
    type Row = i32;
    type Error = &'static str;

    fn open_base(&mut self, _ctx: &mut Self::Context) -> Result<(), Self::Error> {
        self.opened = true;
        Ok(())
    }

    fn build_added_rows_iterator(&mut self, _ctx: &mut Self::Context) -> Result<(), Self::Error> {
        self.iterator_built = true;
        Ok(())
    }

    fn physical_table_id_column(&self) -> Option<usize> {
        Some(3)
    }

    fn new_snapshot_chunk(&mut self) -> Chunk {
        Chunk::default()
    }

    fn maximum_chunk_size(&self) -> usize {
        self.capacity
    }

    fn reset_output_chunk(&self, _chunk: &mut Chunk, _maximum_size: usize) {
        self.output.borrow_mut().clear();
        self.output_rows.set(0);
    }

    fn output_capacity(&self, _chunk: &Chunk) -> usize {
        self.capacity
    }

    fn output_rows(&self, _chunk: &Chunk) -> usize {
        self.output_rows.get()
    }

    fn next_added_row(&mut self) -> Result<Option<Self::Row>, Self::Error> {
        Ok(self.added.pop_front())
    }

    fn next_snapshot_rows(
        &mut self,
        _ctx: &mut Self::Context,
        _chunk: &mut Chunk,
    ) -> Result<Vec<Self::Row>, Self::Error> {
        Ok(self.snapshot_batches.pop_front().unwrap_or_default())
    }

    fn snapshot_row_was_modified(
        &self,
        row: &Self::Row,
        physical_table_id_column: Option<usize>,
    ) -> Result<bool, Self::Error> {
        assert_eq!(physical_table_id_column, Some(3));
        Ok(self.modified.contains(row))
    }

    fn compare_rows(&self, left: &Self::Row, right: &Self::Row) -> Result<Ordering, Self::Error> {
        if self.fail_compare {
            return Err("compare failed");
        }
        Ok(left.cmp(right))
    }

    fn evaluate_virtual_columns_and_conditions(
        &mut self,
        row: Self::Row,
    ) -> Result<Option<Self::Row>, Self::Error> {
        Ok((row >= 0).then_some(row * 10))
    }

    fn append_row(&self, _chunk: &mut Chunk, row: &Self::Row) {
        self.output.borrow_mut().push(*row);
        self.output_rows.set(self.output_rows.get() + 1);
    }

    fn reading_cached_table(&self) -> bool {
        self.cached
    }

    fn close_added_rows_iterator(&mut self) {
        self.iterator_closed = true;
    }

    fn close_child(&mut self) -> Result<(), Self::Error> {
        self.child_closed = true;
        Ok(())
    }
}

fn executor(runtime: MockRuntime) -> UnionScanExec<MockRuntime> {
    UnionScanExec {
        runtime,
        added_row: None,
        snapshot_rows: Vec::new(),
        snapshot_cursor: 0,
        snapshot_chunk: None,
        physical_table_id_column: None,
    }
}

#[test]
/// 升序时索引列决定顺序；降序时比较结果取反。
fn union_scan_comparator_uses_index_then_handle_and_reverses_descending() {
    // 仅用第 0 列作索引键；相等时再比较 handle（此处用第二列模拟）。
    let compare = compareExec {
        collators: vec![()],
        used_index: vec![0],
        descending: false,
        need_extra_sorting: true,
    };
    assert_eq!(
        compare.compare(
            &[1, 9],
            &[2, 1],
            |i, l, r, _| Ok::<_, ()>(l.cmp(r)),
            |l, r, _| Ok(l[1].cmp(&r[1]))
        ),
        Ok(Ordering::Less)
    );
    let descending = compareExec {
        descending: true,
        ..compare
    };
    assert_eq!(
        descending.compare(
            &[1, 9],
            &[2, 1],
            |i, l, r, _| {
                let _ = i;
                Ok::<_, ()>(l.cmp(r))
            },
            |l, r, _| Ok(l[1].cmp(&r[1]))
        ),
        Ok(Ordering::Greater)
    );
}

#[test]
fn union_scan_merges_rows_masks_modified_snapshot_and_filters_after_merge() {
    let runtime = MockRuntime {
        added: VecDeque::from([-1, 2, 4]),
        snapshot_batches: VecDeque::from([vec![1, 2, 3], vec![]]),
        modified: HashSet::from([2]),
        capacity: 3,
        ..MockRuntime::default()
    };
    let mut union_scan = executor(runtime);

    union_scan.Open(&mut ()).unwrap();
    assert!(union_scan.runtime.opened);
    assert!(union_scan.runtime.iterator_built);
    assert!(union_scan.snapshot_chunk.is_some());

    union_scan.Next(&mut (), &mut Chunk::default()).unwrap();
    assert_eq!(*union_scan.runtime.output.borrow(), vec![10, 20, 30]);

    union_scan.Next(&mut (), &mut Chunk::default()).unwrap();
    assert_eq!(*union_scan.runtime.output.borrow(), vec![40]);
}

#[test]
fn union_scan_cached_table_uses_only_added_rows_and_close_releases_both_sides() {
    let runtime = MockRuntime {
        added: VecDeque::from([7]),
        snapshot_batches: VecDeque::from([vec![1]]),
        capacity: 8,
        cached: true,
        ..MockRuntime::default()
    };
    let mut union_scan = executor(runtime);

    union_scan.Open(&mut ()).unwrap();
    union_scan.Next(&mut (), &mut Chunk::default()).unwrap();
    assert_eq!(*union_scan.runtime.output.borrow(), vec![70]);
    assert_eq!(union_scan.runtime.snapshot_batches.len(), 1);

    union_scan.Close().unwrap();
    assert!(union_scan.runtime.iterator_closed);
    assert!(union_scan.runtime.child_closed);
    assert!(union_scan.added_row.is_none());
    assert!(union_scan.snapshot_rows.is_empty());
}

#[test]
fn union_scan_propagates_merge_comparison_errors_without_consuming_rows() {
    let runtime = MockRuntime {
        added: VecDeque::from([2]),
        snapshot_batches: VecDeque::from([vec![1]]),
        capacity: 1,
        fail_compare: true,
        ..MockRuntime::default()
    };
    let mut union_scan = executor(runtime);
    union_scan.Open(&mut ()).unwrap();

    assert_eq!(
        union_scan.Next(&mut (), &mut Chunk::default()),
        Err("compare failed")
    );
    assert_eq!(union_scan.added_row, Some(2));
    assert_eq!(union_scan.snapshot_cursor, 0);
}
