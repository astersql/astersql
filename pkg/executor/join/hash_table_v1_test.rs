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

/*

#![allow(dead_code, non_snake_case, non_camel_case_types, unused_variables)]

// initBuildChunk 对应 Go 的构建侧 chunk fixture：6 列覆盖 NULL/int/string/decimal/json。
pub fn initBuildChunk(numRows: i32) -> (chunk::Chunk, Vec<types::FieldType>) {
    let numCols = 6;
    let mut colTypes = Vec::<types::FieldType>::with_capacity(numCols);
    colTypes.push(types::NewFieldTypeBuilder().SetType(mysql::TypeLonglong).BuildP());
    colTypes.push(types::NewFieldTypeBuilder().SetType(mysql::TypeLonglong).BuildP());
    colTypes.push(types::NewFieldTypeBuilder().SetType(mysql::TypeVarchar).BuildP());
    colTypes.push(types::NewFieldTypeBuilder().SetType(mysql::TypeVarchar).BuildP());
    colTypes.push(types::NewFieldTypeBuilder().SetType(mysql::TypeNewDecimal).BuildP());
    colTypes.push(types::NewFieldTypeBuilder().SetType(mysql::TypeJSON).BuildP());

    let mut oldChk = chunk::NewChunkWithCapacity(colTypes.clone(), numRows);
    for i in 0..numRows {
        let str_value = fmt::Sprintf("%d.12345", i);
        oldChk.AppendNull(0);
        oldChk.AppendInt64(1, i as i64);
        oldChk.AppendString(2, str_value.clone());
        oldChk.AppendString(3, str_value.clone());
        oldChk.AppendMyDecimal(4, types::NewDecFromStringForTest(str_value.clone()));
        oldChk.AppendJSON(5, types::CreateBinaryJSON(str_value));
    }
    (oldChk, colTypes)
}

// initProbeChunk 对应 Go 的探测侧 chunk fixture：只构造 join key 相关的 3 列。
pub fn initProbeChunk(numRows: i32) -> (chunk::Chunk, Vec<types::FieldType>) {
    let numCols = 3;
    let mut colTypes = Vec::<types::FieldType>::with_capacity(numCols);
    colTypes.push(types::NewFieldTypeBuilder().SetType(mysql::TypeLonglong).BuildP());
    colTypes.push(types::NewFieldTypeBuilder().SetType(mysql::TypeLonglong).BuildP());
    colTypes.push(types::NewFieldTypeBuilder().SetType(mysql::TypeVarchar).BuildP());

    let mut oldChk = chunk::NewChunkWithCapacity(colTypes.clone(), numRows);
    for i in 0..numRows {
        let str_value = fmt::Sprintf("%d.12345", i);
        oldChk.AppendNull(0);
        oldChk.AppendInt64(1, i as i64);
        oldChk.AppendString(2, str_value);
    }
    (oldChk, colTypes)
}

// hashCollision 对应 Go 的假 hash.Hash64：Sum64 总是返回 0，用于强制制造 hash collision。
pub struct hashCollision {
    pub count: i32,
}

impl hashCollision {
    pub fn Sum64(&mut self) -> u64 {
        self.count += 1;
        0
    }

    pub fn Write(&self, p: Vec<u8>) -> (i32, error) {
        (p.len() as i32, nil)
    }

    pub fn Reset(&self) {}

    pub fn Sum(&self, b: Vec<u8>) -> Vec<u8> {
        panic!("not implemented")
    }

    pub fn Size(&self) -> i32 {
        panic!("not implemented")
    }

    pub fn BlockSize(&self) -> i32 {
        panic!("not implemented")
    }
}

// TestHashRowContainer 对应 Go 的主测试：分别验证普通 hash、spill 路径和强碰撞路径下的统计字段。
#[test]
pub fn TestHashRowContainer() {
    let hashFunc = fnv::New64;
    let (mut rowContainer, mut copiedRC) = testHashRowContainer(hashFunc, false);
    require::Equal(0_i64, rowContainer.stat.probeCollision);
    // On windows time.Now() is imprecise, the elapse time may equal 0
    require::True(rowContainer.stat.buildTableElapse >= 0);
    require::Equal(rowContainer.stat.probeCollision, copiedRC.stat.probeCollision);
    require::Equal(rowContainer.stat.buildTableElapse, copiedRC.stat.buildTableElapse);

    let (rowContainer, copiedRC) = testHashRowContainer(hashFunc, true);
    require::Equal(0_i64, rowContainer.stat.probeCollision);
    require::True(rowContainer.stat.buildTableElapse >= 0);
    require::Equal(rowContainer.stat.probeCollision, copiedRC.stat.probeCollision);
    require::Equal(rowContainer.stat.buildTableElapse, copiedRC.stat.buildTableElapse);

    let mut h = hashCollision { count: 0 };
    let hashFuncCollision = || -> hash::Hash64 {
        &mut h
    };
    let (rowContainer, copiedRC) = testHashRowContainer(hashFuncCollision, false);
    require::True(h.count > 0);
    require::True(rowContainer.stat.probeCollision > 0_i64);
    require::True(rowContainer.stat.buildTableElapse >= 0);
    require::Equal(rowContainer.stat.probeCollision, copiedRC.stat.probeCollision);
    require::Equal(rowContainer.stat.buildTableElapse, copiedRC.stat.buildTableElapse);
}

// testHashRowContainer 对应 Go 的测试辅助：创建 build/probe chunk，写入 row container，再按 probe key 查回两份匹配行。
pub fn testHashRowContainer(hashFunc: fn() -> hash::Hash64, spill: bool) -> (hashRowContainer, hashRowContainer) {
    let sctx = mock::NewContext();
    let numRows = 10;

    let (chk0, colTypes) = initBuildChunk(numRows);
    let (chk1, _) = initBuildChunk(numRows);

    let mut hCtx = HashContext {
        AllTypes: colTypes[1..3].to_vec(),
        KeyColIdx: vec![1, 2],
        ..Default::default()
    };
    hCtx.HasNull = vec![false; numRows as usize];
    for _ in 0..numRows {
        hCtx.HashVals.push(hashFunc());
    }
    let mut rowContainer = newHashRowContainer(sctx, hCtx, colTypes.clone());
    let copiedRC = rowContainer.ShallowCopy();
    let tracker = rowContainer.GetMemTracker();
    tracker.SetLabel(memory::LabelForBuildSideResult);
    if spill {
        // Go 通过把内存阈值设为 1 触发 spill action；只保留触发条件和等待点。
        tracker.SetBytesLimit(1);
        rowContainer.rowContainer.ActionSpillForTest().Action(tracker);
    }
    let err = rowContainer.PutChunk(chk0, nil);
    require::NoError(err);
    let err = rowContainer.PutChunk(chk1, nil);
    require::NoError(err);
    rowContainer.ActionSpill().(*chunk::SpillDiskAction).WaitForTest();
    require::Equal(spill, rowContainer.AlreadySpilledSafeForTest());
    require::Equal(spill, rowContainer.rowContainer.GetMemTracker().BytesConsumed() == 0);
    require::Equal(!spill, rowContainer.rowContainer.GetMemTracker().BytesConsumed() > 0);
    require::True(rowContainer.GetMemTracker().BytesConsumed() > 0); // hashtable need memory
    if rowContainer.AlreadySpilledSafeForTest() {
        require::NotNil(rowContainer.GetDiskTracker());
        require::True(rowContainer.GetDiskTracker().BytesConsumed() > 0);
    }

    let (probeChk, probeColType) = initProbeChunk(2);
    let probeRow = probeChk.GetRow(1);
    let mut probeCtx = HashContext {
        AllTypes: probeColType[1..3].to_vec(),
        KeyColIdx: vec![1, 2],
        ..Default::default()
    };
    probeCtx.HasNull = vec![false; 1];
    probeCtx.HashVals.push(hashFunc());
    let (matched, _, err) = rowContainer.GetMatchedRowsAndPtrs(hCtx.HashVals[1].Sum64(), probeRow, probeCtx, nil, nil, false);
    require::NoError(err);
    require::Equal(2, matched.len());
    require::Equal(chk0.GetRow(1).GetDatumRow(colTypes), matched[0].GetDatumRow(colTypes));
    require::Equal(chk1.GetRow(1).GetDatumRow(colTypes), matched[1].GetDatumRow(colTypes));
    (rowContainer, copiedRC)
}

// concurrentMapHashTableTrackedMemoryUsage 对应 Go 的 tracked memory 计算：使用 arena 追踪容量和 entryStore slice cap。
pub fn concurrentMapHashTableTrackedMemoryUsage(m: &concurrentMapHashTable) -> i64 {
    let mut memoryUsage = unsafe::Sizeof::<concurrentMapHashTable>() as i64
        + (m.hashMap.len() as i64) * unsafe::Sizeof::<concurrentMapShared>() as i64;
    for shard in &m.hashMap {
        memoryUsage += shard.items.Bytes as i64;
    }
    memoryUsage += unsafe::Sizeof::<entryStore>() as i64;
    for store in &m.entryStore.slices {
        memoryUsage += unsafe::Sizeof::<entry>() as i64 * store.capacity() as i64;
    }
    memoryUsage
}

// concurrentMapHashTableRealMemoryUsage 对应 Go 的 real memory 计算：额外计入 map bucket 中 uintptr key 的实际占用。
pub fn concurrentMapHashTableRealMemoryUsage(m: &concurrentMapHashTable) -> i64 {
    let mut memoryUsage = unsafe::Sizeof::<concurrentMapHashTable>() as i64
        + (m.hashMap.len() as i64) * (unsafe::Sizeof::<usize>() as i64 + unsafe::Sizeof::<concurrentMapShared>() as i64);
    for shard in &m.hashMap {
        memoryUsage += shard.items.RealBytes() as i64;
    }
    memoryUsage += unsafe::Sizeof::<entryStore>() as i64;
    for store in &m.entryStore.slices {
        memoryUsage += unsafe::Sizeof::<entry>() as i64 * store.capacity() as i64;
    }
    memoryUsage
}

// TestConcurrentMapHashTableMemoryUsage 对应 Go 的内存回归测试：检查 entryStore 扩容序列和 GetAndCleanMemoryDelta 清零。
#[test]
pub fn TestConcurrentMapHashTableMemoryUsage() {
    let mut m = NewConcurrentMapHashTable();
    require::Equal(concurrentMapHashTableTrackedMemoryUsage(&m), m.memDelta);
    // Note: Now concurrentMapHashTable doesn't support inserting in parallel.
    for i in 0..6656 {
        // Add entry to map.
        m.Put((i * ShardCount) as u64, chunk::RowPtr { ChkIdx: i as u32, RowIdx: i as u32 });
    }
    require::Len(m.entryStore.slices, 7);
    for (idx, capacity) in vec![64, 128, 256, 512, 1024, 2048, 4096].iter().enumerate() {
        require::Equal(*capacity, m.entryStore.slices[idx].capacity());
    }
    let mut trackedMapMemoryUsage: i64 = 0;
    let mut realMapMemoryUsage: i64 = 0;
    for shard in &m.hashMap {
        trackedMapMemoryUsage += shard.items.Bytes as i64;
        realMapMemoryUsage += shard.items.RealBytes() as i64;
    }
    require::GreaterOrEqual(trackedMapMemoryUsage, realMapMemoryUsage * 75 / 100);
    require::Equal(concurrentMapHashTableTrackedMemoryUsage(&m), m.GetAndCleanMemoryDelta());
    require::Greater(concurrentMapHashTableRealMemoryUsage(&m), 0_i64);
    require::Equal(0_i64, m.GetAndCleanMemoryDelta());
}
*/

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
