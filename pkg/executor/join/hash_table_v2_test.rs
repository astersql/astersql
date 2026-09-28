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

// Hash Join V2 哈希表容量、构建、lookup 与行迭代器的单元测试。
//
// 校验 `next_power_of_two` / 最小桶长边界、分区 build/lookup/replace/clear，
// 以及跨空分区与非空分区的 `RowIter` 边界行为。

/*
// 这段逻辑覆盖 hash table v2 的表大小、构建、并发构建、lookup 和 row iterator 校验。
// mock session、chunk 随机数据和 WaitGroupWrapper 均保留为 Go 语义占位。

#![allow(dead_code, non_snake_case, non_camel_case_types, non_upper_case_globals, unused_variables)]

use std::collections::HashSet;

// createMockRowTable 对应 Go 测试 helper：构造只含 rowStartOffset/validJoinKeyPos 的 mock rowTable。
// fixedSize=false 时 Go 使用 rand.Int31n 生成非空 segment；rows 为 0 的场景只由调用方显式传入。
pub fn createMockRowTable(maxRowsPerSeg: i32, segmentCount: i32, fixedSize: bool) -> Box<rowTable> {
    let mut ret = Box::new(rowTable::default());
    for _ in 0..segmentCount {
        // Go 测试要求 segment 不能是空段；随机分支最少生成 1 行。
        let mut rows = maxRowsPerSeg;
        if !fixedSize {
            rows = rand::Int31n(maxRowsPerSeg as i32) as i32 + 1;
        }

        let mut rowSeg = newRowTableSegment();
        rowSeg.rawData = make_byte_vec(rows);
        for j in 0..rows {
            rowSeg.rowStartOffset.push(j as u64);
            rowSeg.validJoinKeyPos.push(j);
        }
        ret.segments.push(rowSeg);
    }
    ret
}

// createRowTable 对应 Go 测试中的真实 row table 构造路径。
// 它通过随机 chunk、HashJoinCtxV2 和 rowTableBuilder 生成待测 rowTable，并返回最小 taggedBits。
pub fn createRowTable(rows: i32) -> Result<(Box<rowTable>, u8), Error> {
    let mut tinyTp = types::NewFieldType(mysql::TypeTiny);
    tinyTp.AddFlag(mysql::NotNullFlag);
    let buildKeyIndex = vec![0];
    let buildTypes = vec![tinyTp.clone()];
    let buildKeyTypes = vec![tinyTp.clone()];
    let probeKeyTypes = vec![tinyTp.clone()];

    let mut buildSchema = expression::Schema::default();
    for tp in &buildTypes {
        buildSchema.Append(Box::new(expression::Column { RetType: tp.clone(), ..Default::default() }));
    }

    let meta = newTableMeta(buildKeyIndex.clone(), buildTypes.clone(), buildKeyTypes.clone(), probeKeyTypes, None, vec![], false);
    let mut hasNullableKey = false;
    for buildKeyType in &buildKeyTypes {
        if !mysql::HasNotNullFlag(buildKeyType.GetFlag()) {
            hasNullableKey = true;
            break;
        }
    }

    let chk = testutil::GenRandomChunks(buildTypes.clone(), rows);
    let mut hashJoinCtx = HashJoinCtxV2 {
        hashTableMeta: meta.clone(),
        ..Default::default()
    };
    hashJoinCtx.Concurrency = 1;
    hashJoinCtx.SetupPartitionInfo();
    hashJoinCtx.initHashTableContext();
    hashJoinCtx.SessCtx = mock::NewContext();

    let mut builder = createRowTableBuilder(
        buildKeyIndex,
        buildKeyTypes.clone(),
        hashJoinCtx.partitionNumber,
        hasNullableKey,
        false,
        false,
        meta.nullMapLength,
    );
    // processOneChunk 依赖 session type context；保留错误向上传递形状。
    builder.processOneChunk(chk, hashJoinCtx.SessCtx.GetSessionVars().StmtCtx.TypeCtx(), &mut hashJoinCtx, 0)?;

    let mut taggedBits = maxTaggedBits as u8;
    for seg in &hashJoinCtx.hashTableContext.rowTables[0][0].segments {
        taggedBits = taggedBits.min(seg.taggedBits);
    }
    Ok((hashJoinCtx.hashTableContext.rowTables[0][0].clone(), taggedBits))
}

// TestHashTableSize 对应 Go 的 hash table 最小容量与扩容边界测试。
#[test]
pub fn TestHashTableSize() {
    let mut rowTable = createMockRowTable(2, 5, true);
    let mut subTable = newSubTable(rowTable);
    // Go 断言最小 hash table size 为 32。
    require::Equal(32, subTable.hashTable.len());

    rowTable = createMockRowTable(32, 1, true);
    subTable = newSubTable(rowTable);
    require::Equal(64, subTable.hashTable.len());

    rowTable = createMockRowTable(33, 1, true);
    subTable = newSubTable(rowTable);
    require::Equal(64, subTable.hashTable.len());

    rowTable = createMockRowTable(64, 1, true);
    subTable = newSubTable(rowTable);
    require::Equal(128, subTable.hashTable.len());

    rowTable = createMockRowTable(65, 1, true);
    subTable = newSubTable(rowTable);
    require::Equal(128, subTable.hashTable.len());
}

// TestBuild 对应 Go 的单线程 build 校验：每个 row pointer 必须且只能出现在 hash table 链上一次。
#[test]
pub fn TestBuild() -> Result<(), Error> {
    let (rowTable, taggedBits) = createRowTable(1_000_000)?;
    let mut tagHelper = tagPtrHelper::default();
    tagHelper.init(taggedBits);
    let mut subTable = newSubTable(rowTable.clone());

    // 单线程 build 走非 atomic 更新路径。
    subTable.build(0, rowTable.segments.len(), &tagHelper);

    let mut rowSet = HashSet::with_capacity(rowTable.rowCount() as usize);
    for seg in &rowTable.segments {
        for index in 0..seg.rowStartOffset.len() {
            let loc = seg.getRowPointer(index);
            require::False(rowSet.contains(&loc));
            rowSet.insert(loc);
        }
    }

    let mut rowCount = 0_u64;
    for mut locHolder in &subTable.hashTable {
        while *locHolder != 0 {
            rowCount += 1;
            let loc = tagHelper.toUnsafePointer(*locHolder);
            require::True(rowSet.contains(&loc));
            rowSet.remove(&loc);
            // Go 这里传入 0，避免 getNextRowAddress 因 hashvalue 不匹配提前退出。
            locHolder = &getNextRowAddress(loc, &tagHelper, 0);
        }
    }
    require::Equal(0, rowSet.len());
    require::Equal(rowTable.rowCount(), rowCount);
    Ok(())
}

// TestConcurrentBuild 对应 Go 的并发 build 校验。
// 原实现使用 util.WaitGroupWrapper 启动 goroutine；只保留线程切分和等待语义。
#[test]
pub fn TestConcurrentBuild() -> Result<(), Error> {
    let (rowTable, tagBits) = createRowTable(3_000_000)?;
    let mut subTable = newSubTable(rowTable.clone());
    let segmentCount = rowTable.segments.len();
    let buildThreads = 3;
    let mut tagHelper = tagPtrHelper::default();
    tagHelper.init(tagBits);
    let mut wg = util::WaitGroupWrapper::default();

    for i in 0..buildThreads {
        let segmentStart = segmentCount / buildThreads * i;
        let mut segmentEnd = segmentCount / buildThreads * (i + 1);
        if i == buildThreads - 1 {
            segmentEnd = segmentCount;
        }
        // Go 闭包捕获 segmentStart/segmentEnd 后并发执行；共享 subTable 的同步由 atomicUpdateHashValue 保证。
        wg.Run(|| {
            subTable.build(segmentStart, segmentEnd, &tagHelper);
        });
    }
    wg.Wait();

    let mut rowSet = HashSet::with_capacity(rowTable.rowCount() as usize);
    for seg in &rowTable.segments {
        for index in 0..seg.rowStartOffset.len() {
            let loc = seg.getRowPointer(index);
            require::False(rowSet.contains(&loc));
            rowSet.insert(loc);
        }
    }
    for mut locHolder in &subTable.hashTable {
        while *locHolder != 0 {
            let loc = tagHelper.toUnsafePointer(*locHolder);
            require::True(rowSet.contains(&loc));
            rowSet.remove(&loc);
            locHolder = &getNextRowAddress(loc, &tagHelper, 0);
        }
    }
    require::Equal(0, rowSet.len());
    Ok(())
}

// TestLookup 对应 Go 的 lookup 链校验：按 segment 中保存的 hashValue 必须能找回同一行地址。
#[test]
pub fn TestLookup() -> Result<(), Error> {
    let (rowTable, tagBits) = createRowTable(200_000)?;
    let mut tagHelper = tagPtrHelper::default();
    tagHelper.init(tagBits);
    let mut subTable = newSubTable(rowTable.clone());
    subTable.build(0, rowTable.segments.len(), &tagHelper);

    for seg in &rowTable.segments {
        for index in 0..seg.rowStartOffset.len() {
            let hashValue = seg.hashValues[index];
            let mut candidate = subTable.lookup(hashValue, &tagHelper);
            let loc = seg.getRowPointer(index);
            let mut found = false;
            while candidate != 0 {
                let candidatePtr = tagHelper.toUnsafePointer(candidate);
                if candidatePtr == loc {
                    found = true;
                    break;
                }
                candidate = getNextRowAddress(candidatePtr, &tagHelper, hashValue);
            }
            require::True(found);
        }
    }
    Ok(())
}

// checkRowIter 对应 Go 测试 helper：把全部 row pointer 建成集合，再按 scanConcurrency 切分 iterator。
pub fn checkRowIter(table: &mut hashTableV2, scanConcurrency: i32) {
    // 先创建包含所有 row location 的集合，用来证明 iterator 不丢、不重。
    let totalRowCount = table.totalRowCount();
    let mut rowSet = HashSet::with_capacity(totalRowCount as usize);
    for rt in &table.tables {
        for seg in &rt.rowData.segments {
            for index in 0..seg.rowStartOffset.len() {
                let loc = seg.getRowPointer(index);
                require::False(rowSet.contains(&loc));
                rowSet.insert(loc);
            }
        }
    }

    // Go 逻辑按总行数平均切分 scan range，最后一个 iterator 吃掉余数。
    let mut rowIters = Vec::with_capacity(scanConcurrency as usize);
    let rowPerScan = totalRowCount / scanConcurrency as u64;
    for i in 0..scanConcurrency as u64 {
        let startIndex = rowPerScan * i;
        let mut endIndex = rowPerScan * (i + 1);
        if i == scanConcurrency as u64 - 1 {
            endIndex = totalRowCount;
        }
        rowIters.push(table.createRowIter(startIndex, endIndex));
    }

    let mut locCount = 0_u64;
    for it in &mut rowIters {
        while !it.isEnd() {
            locCount += 1;
            let loc = it.getValue();
            require::True(rowSet.contains(&loc));
            rowSet.remove(&loc);
            it.next();
        }
    }
    require::Equal(table.totalRowCount(), locCount);
    require::Equal(0, rowSet.len());
}

// TestRowIter 对应 Go 的 row iterator 测试：覆盖正常分区和某个分区为空的情况。
#[test]
pub fn TestRowIter() {
    let partitionNumbers = vec![1, 4, 8];

    // 正常场景：每个分区都有随机长度的 row table。
    for partitionNumber in &partitionNumbers {
        let mut rowTables = Vec::with_capacity(*partitionNumber as usize);
        for _ in 0..*partitionNumber {
            rowTables.push(createMockRowTable(1024, 16, false));
        }
        let mut joinedHashTable = newJoinHashTableForTest(rowTables);
        checkRowIter(&mut joinedHashTable, *partitionNumber);
    }

    // 空 row table 场景：逐个让第 i 个分区为空，校验 iterator 边界不越界。
    for partitionNumber in &partitionNumbers {
        for i in 0..*partitionNumber {
            let mut rowTables = Vec::with_capacity(*partitionNumber as usize);
            for j in 0..*partitionNumber {
                if i == j {
                    rowTables.push(createMockRowTable(0, 0, true));
                } else {
                    rowTables.push(createMockRowTable(1024, 16, false));
                }
            }
            let mut joinedHashTable = newJoinHashTableForTest(rowTables);
            checkRowIter(&mut joinedHashTable, *partitionNumber);
        }
    }
}
*/

use crate::hash_table_v2::{
    HashTableV2, SubTable, get_hash_table_length_by_row_len, get_hash_table_memory_usage,
    next_power_of_two,
};
use crate::join_row_table::{RowTable, RowTableSegment};
use crate::join_table_meta::EncodedRow;
use std::sync::atomic::AtomicBool;

/// 构造只含指定 hash 值与有效 key 数的单 segment `RowTable` 测试夹具。
fn table(hashes: &[u64], valid: usize) -> RowTable {
    let rows = hashes
        .iter()
        .map(|hash| EncodedRow {
            bytes: hash.to_le_bytes().to_vec(),
            null_map: vec![0],
            key_offset: 0,
            key_length: 8,
            row_data_offset: 8,
            used: AtomicBool::new(false),
        })
        .collect();
    let mut table = RowTable::default();
    table.segments_mut().push(RowTableSegment {
        rows,
        hash_values: hashes.to_vec(),
        valid_key_count: valid as u64,
        ..Default::default()
    });
    table
}

/// 对齐 Go 边界：2 的幂上取整、最小桶长 32、内存按 usize 宽度估算。
#[test]
fn hash_table_size_rounding_and_memory_match_go_boundaries() {
    assert_eq!(next_power_of_two(0), 2);
    assert_eq!(next_power_of_two(1), 2);
    assert_eq!(next_power_of_two(2), 4);
    assert_eq!(next_power_of_two(31), 32);
    assert_eq!(get_hash_table_length_by_row_len(0), 32);
    assert_eq!(get_hash_table_length_by_row_len(32), 64);
    assert_eq!(
        get_hash_table_memory_usage(64),
        64 * std::mem::size_of::<usize>() as i64
    );
}

/// 覆盖 build 后的 lookup、清空分区、替换分区以及越界错误路径。
#[test]
fn hash_table_v2_build_lookup_partition_replace_and_clear_are_real() {
    let mut hash_table = HashTableV2::new(vec![table(&[11, 22, 11], 3), table(&[33], 1)]);
    assert_eq!(hash_table.total_row_count(), 4);
    // 同一 hash 11 应命中两条冲突行。
    assert_eq!(hash_table.lookup(0, 11).len(), 2);
    assert_eq!(hash_table.lookup(0, 99).len(), 0);
    assert!(hash_table.total_memory_usage() > 0);
    hash_table.clear_partition_segments(0);
    assert_eq!(hash_table.total_row_count(), 1);
    hash_table
        .replace_partition(0, table(&[44, 44], 2))
        .unwrap();
    assert_eq!(hash_table.lookup(0, 44).len(), 2);
    assert!(hash_table.replace_partition(2, table(&[], 0)).is_err());
}

/// 行迭代器应跳过空分区，并在 total 边界处给出哨兵 `RowPos`。
#[test]
fn row_iterator_crosses_empty_and_nonempty_partitions_at_exact_boundaries() {
    let hash_table = HashTableV2::new(vec![table(&[], 0), table(&[1, 2], 2), table(&[3], 1)]);
    let positions: Vec<_> = hash_table.create_row_iter(0, 3).unwrap().collect();
    assert_eq!(positions.len(), 3);
    assert_eq!(positions[0].sub_table_index, 1);
    assert_eq!(positions[2].sub_table_index, 2);
    assert_eq!(
        hash_table.get_row(positions[2]).unwrap().bytes,
        3_u64.to_le_bytes()
    );
    // create_row_pos(total) 为结束哨兵；超过则报错。
    assert_eq!(hash_table.create_row_pos(3).unwrap().sub_table_index, 3);
    assert!(hash_table.create_row_pos(4).is_err());
}

#[test]
/// Disjoint row iterator ranges cover every row exactly once when split at a boundary.
fn row_iter_split_ranges_cover_all_rows_once() {
    let hash_table = HashTableV2::new(vec![table(&[7, 8], 2), table(&[9, 10], 2)]);
    let left: Vec<_> = hash_table.create_row_iter(0, 2).unwrap().collect();
    let right: Vec<_> = hash_table.create_row_iter(2, 4).unwrap().collect();
    assert_eq!(left.len() + right.len(), 4);
    assert!(left.iter().all(|position| position.sub_table_index == 0));
    assert!(right.iter().all(|position| position.sub_table_index == 1));
    assert!(left.iter().all(|position| !right.contains(position)));
}

#[test]
/// Clearing a partition removes its rows and memory while preserving other partitions.
fn clearing_one_hash_table_partition_preserves_other_partition_rows() {
    let mut hash_table = HashTableV2::new(vec![table(&[1, 2], 2), table(&[3, 4], 2)]);
    let memory_before = hash_table.total_memory_usage();
    hash_table.clear_partition_segments(1);
    assert_eq!(hash_table.total_row_count(), 2);
    assert_eq!(hash_table.lookup(0, 1).len(), 1);
    assert!(hash_table.lookup(1, 3).is_empty());
    assert!(hash_table.total_memory_usage() < memory_before);
}

/// Go builds buckets from `validJoinKeyPos`, not from the first N rows.
#[test]
fn build_indexes_exact_valid_join_key_positions() {
    let rows = [10_u64, 20, 30]
        .into_iter()
        .map(|hash| EncodedRow {
            bytes: hash.to_le_bytes().to_vec(),
            null_map: vec![0],
            key_offset: 0,
            key_length: 8,
            row_data_offset: 8,
            used: AtomicBool::new(false),
        })
        .collect();
    let mut row_table = RowTable::default();
    row_table.segments_mut().push(RowTableSegment {
        rows,
        hash_values: vec![10, 20, 30],
        valid_join_key_positions: vec![2],
        valid_key_count: 1,
        ..Default::default()
    });

    let hash_table = HashTableV2::new(vec![row_table]);
    assert!(hash_table.lookup(0, 10).is_empty());
    assert_eq!(hash_table.lookup(0, 30).len(), 1);
    assert_eq!(hash_table.lookup(0, 30)[0].row_index, 2);
}

/// Go clears storage during spill without rewriting the cached empty flags.
#[test]
fn clear_segments_preserves_go_empty_flags_and_memory_formula() {
    let row_table = table(&[1, 2], 2);
    let row_memory = row_table.total_memory_usage();
    let mut sub_table = SubTable::new(row_table, 0);
    assert_eq!(
        sub_table.total_memory_usage(),
        row_memory + get_hash_table_memory_usage(32)
    );
    assert!(!sub_table.is_row_table_empty);
    assert!(!sub_table.is_hash_table_empty);

    sub_table.clear_segments();
    assert!(!sub_table.is_row_table_empty);
    assert!(!sub_table.is_hash_table_empty);
    assert_eq!(sub_table.total_memory_usage(), 0);
}
