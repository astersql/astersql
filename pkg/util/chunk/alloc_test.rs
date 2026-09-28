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

// Chunk 分配器（Allocator）基础复用行为的单元测试。
//
// 验证：按相同 schema 分配、写入后 Reset，再分配应复用已清空的 Chunk，
// 行数归零但列结构保留。Chunk 是执行引擎中按列存储的一批行数据。

/// 断言 Allocator 在 Reset 后能复用同 schema 的 Chunk，且行数据被清空。
#[test]
fn allocator_reuses_reset_chunks_with_the_same_schema() {
    use super::{Allocator, NewAllocator, mysql, types};
    let fields = vec![types::NewFieldType(mysql::TypeLonglong)];
    let mut allocator = NewAllocator();
    // 首次分配并写入一行，随后 Reset 清空缓存供复用。
    let first = allocator.Alloc(&fields, 4, 8);
    first.lock().unwrap().AppendInt64(0, 7);
    allocator.Reset();
    let second = allocator.Alloc(&fields, 4, 8);
    assert_eq!(second.lock().unwrap().NumCols(), 1);
    assert_eq!(second.lock().unwrap().NumRows(), 0);
}

/// Go 的 allocated 列表持有每个原始列，即使 Chunk 后来把某列改成另一列的引用，
/// 回收池也不能出现两个指向同一列的条目。
#[test]
fn allocator_does_not_cache_duplicate_column_references() {
    use super::{Allocator, NewAllocator, mysql, types};

    let fields = vec![
        types::NewFieldType(mysql::TypeLonglong),
        types::NewFieldType(mysql::TypeLonglong),
    ];
    let mut allocator = NewAllocator();
    let chunk = allocator.Alloc(&fields, 4, 8);
    chunk.lock().unwrap().MakeRef(0, 1);
    allocator.Reset();

    let reused = allocator.Alloc(&fields, 4, 8);
    let reused = reused.lock().unwrap();
    assert!(!reused.columns[0].same_ref(&reused.columns[1]));
}

/// Go 按分配时的 typeSize 分桶；若列在使用期间改变类型，Reset 会从原桶拒收，
/// 而不是把它迁移进新类型的缓存桶。
#[test]
fn allocator_drops_columns_whose_type_changed() {
    use super::{Allocator, NewAllocator, getFixedLen, mysql, types};

    let float = types::NewFieldType(mysql::TypeFloat);
    let datetime = types::NewFieldType(mysql::TypeDatetime);
    let fields = vec![float.clone()];
    let mut allocator = NewAllocator();
    let chunk = allocator.Alloc(&fields, 4, 8);
    chunk.lock().unwrap().columns[0].Reset(types::ETDatetime);
    allocator.Reset();

    assert_eq!(allocator.cached_column_count(getFixedLen(&float)), 0);
    assert_eq!(allocator.cached_column_count(getFixedLen(&datetime)), 0);
}
