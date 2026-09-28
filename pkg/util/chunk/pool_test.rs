// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// Chunk 列对象池（`Pool`）归还与清空行为的单元测试。
//
// 对应 Go `pool_test.go`：覆盖池构造、全部五种列宽的初始布局、
// `PutChunk` 的 reset/回收语义，以及 Go benchmark 中验证的并发往返能力。

fn go_pool_field_types() -> Vec<super::types::FieldType> {
    use super::{mysql, types};

    vec![
        *types::NewFieldType(mysql::TypeVarchar),
        *types::NewFieldType(mysql::TypeJSON),
        *types::NewFieldType(mysql::TypeFloat),
        *types::NewFieldType(mysql::TypeNewDecimal),
        *types::NewFieldType(mysql::TypeDouble),
        *types::NewFieldType(mysql::TypeLonglong),
        *types::NewFieldType(mysql::TypeTimestamp),
        *types::NewFieldType(mysql::TypeDatetime),
    ]
}

/// 对齐 Go `TestNewPool` 和 `TestPoolGetChunk`的容量与列布局断言。
#[test]
fn pool_constructs_all_go_column_shapes_with_the_requested_capacity() {
    use super::pool::NewPool;
    use super::{VarElemLen, getFixedLen};

    let init_cap = 1_024;
    let fields = go_pool_field_types();
    let pool = NewPool(init_cap);
    let chunk = pool.GetChunk(&fields);

    assert_eq!(chunk.capacity, init_cap);
    assert_eq!(chunk.requiredRows, init_cap);
    assert_eq!(chunk.NumCols(), fields.len());
    for (field, column) in fields.iter().zip(&chunk.columns) {
        let width = getFixedLen(field);
        if width == VarElemLen {
            assert!(column.elemBuf.is_empty());
        } else {
            assert_eq!(column.elemBuf.len(), width);
            assert_eq!(column.data.capacity(), init_cap * width);
        }
    }
}

/// 验证归还 Chunk 后列被清空，且池中缓存列数等于字段数。
#[test]
fn pool_returns_columns_after_clearing_the_chunk() {
    use super::pool::NewPool;
    use super::{mysql, types};
    let fields = vec![
        *types::NewFieldType(mysql::TypeVarchar),
        *types::NewFieldType(mysql::TypeLonglong),
    ];
    let pool = NewPool(4);
    let mut chunk = pool.GetChunk(&fields);
    chunk.AppendString(0, "x");
    chunk.AppendInt64(1, 1);
    // Put 后 chunk 不再持有列；两列分别进入变长池与 8 字节定长池。
    pool.PutChunk(&fields, &mut chunk);
    assert_eq!(chunk.NumCols(), 0);
    assert_eq!(pool.cached_columns(), 2);
}

/// 对齐 Go benchmark 的 `RunParallel`：同一 Pool 必须支持并发 Get/Put 往返。
#[test]
fn pool_supports_parallel_get_and_put_round_trips() {
    use super::pool::NewPool;
    use std::sync::{Arc, Barrier};

    let fields = Arc::new(go_pool_field_types());
    let pool = Arc::new(*NewPool(32));
    let first_checkout = Arc::new(Barrier::new(4));
    let workers: Vec<_> = (0..4)
        .map(|_| {
            let fields = Arc::clone(&fields);
            let pool = Arc::clone(&pool);
            let first_checkout = Arc::clone(&first_checkout);
            std::thread::spawn(move || {
                let mut chunk = pool.GetChunk(&fields);
                first_checkout.wait();
                pool.PutChunk(&fields, &mut chunk);
                for _ in 1..100 {
                    let mut chunk = pool.GetChunk(&fields);
                    pool.PutChunk(&fields, &mut chunk);
                    assert_eq!(chunk.NumCols(), 0);
                }
            })
        })
        .collect();

    for worker in workers {
        worker.join().expect("pool worker must not panic");
    }
    assert_eq!(pool.cached_columns(), fields.len() * 4);
}

/// Go 的 `Chunk.Destroy` 必须经全局 pool 归还列，下一次 `New` 才能复用。
#[test]
fn chunk_destroy_returns_columns_to_the_global_pool() {
    use super::{NewChunkFromPoolWithCapacity, mysql, pool, types};

    let init_cap = 12_347;
    let fields = vec![
        *types::NewFieldType(mysql::TypeVarchar),
        *types::NewFieldType(mysql::TypeLonglong),
    ];
    assert_eq!(pool::global_cached_columns_for_test(init_cap), 0);

    let mut chunk = NewChunkFromPoolWithCapacity(fields.clone(), init_cap);
    chunk.AppendString(0, "pooled");
    chunk.AppendInt64(1, 42);
    chunk.Destroy(init_cap, fields.clone());
    assert_eq!(pool::global_cached_columns_for_test(init_cap), 2);

    let chunk = NewChunkFromPoolWithCapacity(fields, init_cap);
    assert_eq!(pool::global_cached_columns_for_test(init_cap), 0);
    assert_eq!(chunk.NumRows(), 0);
}
