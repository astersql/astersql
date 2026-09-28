// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// `RowContainer` 溢写（spill）与重置行为的单元测试。
//
// Spill 指内存压力下把行数据落到磁盘；本测试确认 spill 后按 `RowPtr` 读回仍正确，
// 且 `Reset` 后行数归零。

/// 验证 spill 保留行内容，Reset 清空容器状态。
#[test]
fn row_container_spill_preserves_rows_and_reset_clears_state() {
    use super::{NewChunkWithCapacity, RowContainer, RowPtr, mysql, types};
    let _serial = super::row_container_4_aster_unit_test::SPILL_TEST_LOCK
        .lock()
        .unwrap();
    let fields = vec![*types::NewFieldType(mysql::TypeLonglong)];
    let mut chunk = NewChunkWithCapacity(fields.clone(), 2);
    chunk.AppendInt64(0, 4);
    chunk.AppendInt64(0, 6);
    let container = RowContainer::New(fields, 2);
    container.Add(*chunk).unwrap();
    // 触发溢写：此后 GetRow 应从磁盘路径读回。
    container.SpillToDisk();
    assert_eq!(
        container
            .GetRow(RowPtr {
                ChkIdx: 0,
                RowIdx: 1
            })
            .unwrap()
            .GetInt64(0),
        6
    );
    // Reset 应释放内存/磁盘状态，行数变为 0。
    container.Reset().unwrap();
    assert_eq!(container.NumRow(), 0);
}

/// Go attaches the underlying RowContainer tracker to the sorted container's
/// tracker, so callers see both chunk storage and the row-pointer reservation.
#[test]
fn sorted_row_container_tracker_includes_rows_and_chunk_storage() {
    use super::{NewChunkWithCapacity, SortedRowContainer, mysql, types};

    let fields = vec![*types::NewFieldType(mysql::TypeLonglong)];
    let mut chunk = NewChunkWithCapacity(fields.clone(), 2);
    chunk.AppendInt64(0, 4);
    chunk.AppendInt64(0, 6);
    let expected = chunk.MemoryUsage() + (chunk.NumRows() * 8) as i64;

    let container = SortedRowContainer::New(fields, 2, Vec::new(), Vec::new(), Vec::new());
    container.Add(*chunk).unwrap();

    assert_eq!(container.GetMemTracker().BytesConsumed(), expected);
}

/// Go installs the row-pointer slice before sorting, so even a recovered sort
/// panic transitions the container to the sorted/no-more-additions state.
#[test]
fn failed_sort_still_prevents_later_additions() {
    use super::{
        CompareFunc, ErrCannotAddBecauseSorted, NewChunkWithCapacity, SortedRowContainer, mysql,
        types,
    };

    let _serial = super::row_container_4_aster_unit_test::SPILL_TEST_LOCK
        .lock()
        .unwrap();
    let _scenario = fail::FailScenario::setup();
    let fields = vec![*types::NewFieldType(mysql::TypeLonglong)];
    let cmp: CompareFunc =
        Box::new(
            |left, lcol, right, rcol| match left.GetInt64(lcol).cmp(&right.GetInt64(rcol)) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            },
        );
    let container = SortedRowContainer::New(fields.clone(), 2, vec![false], vec![0], vec![cmp]);
    let mut first = NewChunkWithCapacity(fields.clone(), 2);
    first.AppendInt64(0, 2);
    first.AppendInt64(0, 1);
    container.Add(*first).unwrap();

    fail::cfg("errorDuringSortRowContainer", "return(true)").unwrap();
    assert_eq!(container.Sort().unwrap_err().to_string(), "sort meet error");
    fail::remove("errorDuringSortRowContainer");

    let mut later = NewChunkWithCapacity(fields, 1);
    later.AppendInt64(0, 3);
    assert_eq!(
        container.Add(*later).unwrap_err().to_string(),
        ErrCannotAddBecauseSorted
    );
}
