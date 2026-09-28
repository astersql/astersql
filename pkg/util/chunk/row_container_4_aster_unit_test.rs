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

// AsterSQL 迁移补充：行访问、磁盘按行存储、缓存 Reader 与 RowContainer spill/排序。
//
// 覆盖 `Row` 取值与格式化、`DataInDiskByRows` 往返、`ReaderWithCache` 边界，
// 以及容器 spill、排序后拒绝 Add、failpoint 配额耗尽时错误传播。

use std::sync::Mutex;

use super::row_in_disk::{ReadAtError, ReaderWithCache, SliceReaderAt};
use super::*;
use parser_mysql::r#type as mysql;

/// 串行化依赖全局 failpoint / spill 状态的用例，避免并行干扰。
pub(crate) static SPILL_TEST_LOCK: Mutex<()> = Mutex::new(());

/// 测试字段：BIGINT + VARCHAR。
fn fields() -> Vec<types::FieldType> {
    vec![
        *types::NewFieldType(mysql::TypeLonglong),
        *types::NewFieldType(mysql::TypeVarString),
    ]
}

/// 按 (整数, 可选字符串) 列表构造 Chunk。
fn chunk(values: &[(i64, Option<&str>)]) -> Box<Chunk> {
    let mut chunk = NewChunkWithCapacity(fields(), values.len().max(1));
    for (number, text) in values {
        chunk.AppendInt64(0, *number);
        match text {
            Some(text) => chunk.AppendString(1, text),
            None => chunk.AppendNull(1),
        }
    }
    chunk
}

/// 验证行读写、ToString、CopyConstruct 与 Go 行为一致。
#[test]
fn row_access_and_format_match_go() {
    let chunk = chunk(&[(42, Some("西瓜")), (-7, None)]);
    let first = chunk.GetRow(0);
    assert_eq!(first.Len(), 2);
    assert_eq!(first.GetInt64(0), 42);
    assert_eq!(first.GetString(1), "西瓜");
    assert_eq!(first.ToString(&fields()), "42, 西瓜");
    assert_eq!(chunk.GetRow(1).ToString(&fields()), "-7, NULL");

    let copy = first.CopyConstruct();
    assert_eq!(copy.GetInt64(0), 42);
    assert_eq!(copy.GetString(1), "西瓜");
}

/// 验证按行磁盘存储的追加、按指针取行、空块拒绝与 Close 清零磁盘记账。
#[test]
fn disk_rows_round_trip_and_track_offsets() {
    let mut disk = DataInDiskByRows::New(fields());
    disk.Add(&chunk(&[(1, Some("a")), (2, None)])).unwrap();
    disk.Add(&chunk(&[(3, Some("西瓜"))])).unwrap();

    assert_eq!(disk.Len(), 3);
    assert_eq!(disk.NumChunks(), 2);
    assert_eq!(disk.NumRowsOfChunk(0), 2);
    assert!(disk.GetDiskTracker().BytesConsumed() > 0);
    assert_eq!(
        disk.GetRow(RowPtr {
            ChkIdx: 0,
            RowIdx: 1
        })
        .unwrap()
        .GetInt64(0),
        2
    );
    assert!(
        disk.GetRow(RowPtr {
            ChkIdx: 0,
            RowIdx: 1
        })
        .unwrap()
        .IsNull(1)
    );
    assert_eq!(disk.GetChunk(1).unwrap().GetRow(0).GetString(1), "西瓜");

    // 空 Chunk 不允许写入磁盘结构。
    let empty = NewChunkWithCapacity(fields(), 1);
    assert!(disk.Add(&empty).is_err());
    disk.Close().unwrap();
    assert_eq!(disk.GetDiskTracker().BytesConsumed(), 0);
}

/// 验证带尾部缓存的 `ReaderWithCache` 跨边界读与 EOF 行为。
#[test]
fn reader_with_cache_matches_go_boundaries() {
    let base = SliceReaderAt::new(b"01234567".to_vec());
    let reader = ReaderWithCache::New(Box::new(base), b"89abcd".to_vec(), 8);

    let mut all = vec![0; 14];
    let result = reader.ReadAt(&mut all, 0);
    assert_eq!(result.read, 14);
    assert!(result.error.is_none());
    assert_eq!(&all, b"0123456789abcd");

    let mut tail = vec![0; 8];
    let result = reader.ReadAt(&mut tail, 10);
    assert_eq!(result.read, 4);
    assert_eq!(result.error, Some(ReadAtError::Eof));
    assert_eq!(&tail[..4], b"abcd");
}

/// 验证 RowContainer spill/Reset，以及 SortedRowContainer 排序与排序后拒绝 Add。
#[test]
fn row_container_spills_and_sorted_container_rejects_add() {
    let _serial = SPILL_TEST_LOCK.lock().unwrap();
    let container = RowContainer::New(fields(), 2);
    container
        .Add(*chunk(&[(2, Some("b")), (1, Some("a"))]))
        .unwrap();
    assert_eq!(container.NumRow(), 2);
    container.SpillToDisk();
    assert!(container.AlreadySpilledSafeForTest());
    assert_eq!(
        container
            .GetRow(RowPtr {
                ChkIdx: 0,
                RowIdx: 1
            })
            .unwrap()
            .GetInt64(0),
        1
    );

    container.Reset().unwrap();
    assert!(!container.AlreadySpilledSafeForTest());
    assert_eq!(container.NumRow(), 0);

    // 升序排序：比较函数返回 -1/0/1，对齐 Go CompareFunc。
    let cmp: CompareFunc =
        Box::new(
            |left, lcol, right, rcol| match left.GetInt64(lcol).cmp(&right.GetInt64(rcol)) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            },
        );
    let sorted = SortedRowContainer::New(fields(), 2, vec![false], vec![0], vec![cmp]);
    sorted
        .Add(*chunk(&[(2, Some("b")), (1, Some("a"))]))
        .unwrap();
    sorted.Sort().unwrap();
    assert_eq!(sorted.GetSortedRow(0).unwrap().GetInt64(0), 1);
    assert_eq!(sorted.GetSortedRow(1).unwrap().GetInt64(0), 2);
    assert_eq!(
        sorted
            .Add(*chunk(&[(3, Some("c"))]))
            .unwrap_err()
            .to_string(),
        ErrCannotAddBecauseSorted
    );

    // ByItemsDesc=true：降序，首行应为较大键。
    let desc: CompareFunc =
        Box::new(
            |left, lcol, right, rcol| match left.GetInt64(lcol).cmp(&right.GetInt64(rcol)) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            },
        );
    let descending = SortedRowContainer::New(fields(), 2, vec![true], vec![0], vec![desc]);
    descending
        .Add(*chunk(&[(2, Some("b")), (1, Some("a"))]))
        .unwrap();
    descending.Sort().unwrap();
    assert_eq!(descending.GetSortedRow(0).unwrap().GetInt64(0), 2);
}

/// failpoint 模拟磁盘配额耗尽：spill 后 GetRow 应返回错误（与 Go 一致）。
#[test]
fn spill_panic_is_recorded_and_returned_like_go() {
    let _serial = SPILL_TEST_LOCK.lock().unwrap();
    let _scenario = fail::FailScenario::setup();
    fail::cfg("spillToDiskOutOfDiskQuota", "return(true)").unwrap();
    let container = RowContainer::New(fields(), 1);
    container.Add(*chunk(&[(1, Some("a"))])).unwrap();
    container.SpillToDisk();
    fail::remove("spillToDiskOutOfDiskQuota");

    assert!(container.AlreadySpilledSafeForTest());
    assert!(
        container
            .GetRow(RowPtr {
                ChkIdx: 0,
                RowIdx: 0
            })
            .is_err()
    );
}

/// Go 的三个 RowContainer failpoint 都只在布尔值为 true 时生效。
#[test]
fn false_row_container_failpoints_do_not_change_behavior() {
    let _serial = SPILL_TEST_LOCK.lock().unwrap();
    let _scenario = fail::FailScenario::setup();

    fail::cfg("testRowContainerDeadLock", "return(false)").unwrap();
    let container = RowContainer::New(fields(), 1);
    container.Add(*chunk(&[(1, Some("a"))])).unwrap();
    fail::remove("testRowContainerDeadLock");

    fail::cfg("spillToDiskOutOfDiskQuota", "return(false)").unwrap();
    container.SpillToDisk();
    fail::remove("spillToDiskOutOfDiskQuota");
    assert_eq!(
        container
            .GetRow(RowPtr {
                ChkIdx: 0,
                RowIdx: 0,
            })
            .unwrap()
            .GetInt64(0),
        1
    );

    let cmp: CompareFunc =
        Box::new(|left, lcol, right, rcol| left.GetInt64(lcol).cmp(&right.GetInt64(rcol)) as i32);
    let sorted = SortedRowContainer::New(fields(), 1, vec![false], vec![0], vec![cmp]);
    sorted.Add(*chunk(&[(1, Some("a"))])).unwrap();
    fail::cfg("errorDuringSortRowContainer", "return(false)").unwrap();
    assert!(sorted.Sort().is_ok());
}
