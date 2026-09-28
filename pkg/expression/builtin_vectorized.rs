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

// 内置函数向量化求值的临时列缓冲池与逐行回退实现。
//
// 对应 Go `builtin_vectorized.go`：提供 `columnBufferAllocator`、本地/全局列池，
// 以及在缺少专用 `VecEval*` 实现时用 `evalInt`/`evalString` 逐行填充结果列的回退路径。
// 向量化指一次对 Chunk 中整列求值，而非逐行标量调用。

use crate::*;

use std::mem;
use std::sync::{LazyLock, Mutex};

/// columnBufferAllocator 对应 Go 接口，用于向量化求值期间借出和归还临时列。
pub trait columnBufferAllocator {
    /// get 只负责分配缓冲，不保证列类型、长度或 NULL 位图已经初始化。
    fn get(&self) -> Result<Box<chunk::Column>, Error>;

    /// put 把调用方不再使用的列缓冲归还给分配器。
    fn put(&self, buf: Box<chunk::Column>);

    /// MemoryUsage 返回分配器对象自身的内存占用，沿用 Go 的接口命名。
    fn MemoryUsage(&self) -> i64;
}

/// localColumnPool 对应 Go 的 sync.Pool 实现。
/// Rust 用互斥 Vec 表达并发安全复用；实际接线时可换成无锁池，但借还语义保持不变。
pub struct localColumnPool {
    pool: Mutex<Vec<Box<chunk::Column>>>,
}

/// columnTempl 是创建新缓冲时复制的列模板，类型为 MySQL LONG LONG，容量使用 chunk 初始值。
static columnTempl: LazyLock<Box<chunk::Column>> = LazyLock::new(|| {
    chunk::NewColumn(
        &types::NewFieldType(mysql::TypeLonglong),
        chunk::InitialCapacity,
    )
});

/// newLocalColumnPool 对应 Go 构造函数；空池在首次 get 时从 columnTempl 复制列。
pub fn newLocalColumnPool() -> localColumnPool {
    localColumnPool {
        pool: Mutex::new(Vec::new()),
    }
}

/// globalColumnAllocator 对应包级共享池，所有 GetColumn/PutColumn 调用复用它。
static globalColumnAllocator: LazyLock<localColumnPool> = LazyLock::new(newLocalColumnPool);

/// GetColumn 分配一个未初始化的列缓冲。
/// Go 当前忽略 EvalType 和容量参数，因此 Rust 也显式以下划线接收。
pub fn GetColumn(
    _eval_type: types::EvalType,
    _capacity: usize,
) -> Result<Box<chunk::Column>, Error> {
    globalColumnAllocator.get()
}

/// PutColumn 归还由 GetColumn 借出的缓冲；调用方之后不得再访问该列。
pub fn PutColumn(buf: Box<chunk::Column>) {
    globalColumnAllocator.put(buf);
}

impl columnBufferAllocator for localColumnPool {
    /// get 优先弹出已归还列；池为空时按 Go sync.Pool.New 的语义复制模板。
    fn get(&self) -> Result<Box<chunk::Column>, Error> {
        let mut pool = self
            .pool
            .lock()
            .map_err(|_| types::errors::New("localColumnPool lock poisoned"))?;
        Ok(pool
            .pop()
            .unwrap_or_else(|| columnTempl.CopyConstruct(None)))
    }

    /// put 将列压回并发安全容器，不在这里重置列内容。
    fn put(&self, col: Box<chunk::Column>) {
        if let Ok(mut pool) = self.pool.lock() {
            pool.push(col);
        }
        // Go 的 sync.Pool.Put 不返回错误；锁中毒时同样没有可向上传播的错误通道。
    }

    /// MemoryUsage 只统计空池结构大小，不累计池中列缓冲，与 Go 实现一致。
    fn MemoryUsage(&self) -> i64 {
        emptyLocalColumnPoolSize
    }
}

/// emptyLocalColumnPoolSize 对应 unsafe.Sizeof(localColumnPool{}) 的静态结构大小。
pub const emptyLocalColumnPoolSize: i64 = mem::size_of::<localColumnPool>() as i64;

/// 按本机字节序把 i64 写入列的原始 data 缓冲，跳过 Append 路径的长度调整。
fn write_i64(column: &mut chunk::Column, row: usize, value: i64) {
    let start = row * mem::size_of::<i64>();
    column.data[start..start + mem::size_of::<i64>()].copy_from_slice(&value.to_ne_bytes());
}

/// vecEvalIntByRows 在缺少专用向量化实现时，逐行调用 builtinFunc.evalInt。
pub fn vecEvalIntByRows(
    ctx: &dyn EvalContext,
    sig: &dyn builtinFunc,
    input: &chunk::Chunk,
    result: &mut chunk::Column,
) -> Result<(), Error> {
    let n = input.NumRows();
    result.ResizeInt64(n, false);
    for i in 0..n {
        // 任一行求值失败立即返回；此前已写入的结果不回滚，保留 Go 行为。
        let (res, isNull) = sig.evalInt(ctx, input.GetRow(i))?;
        result.SetNull(i, isNull);
        write_i64(result, i, res);
    }
    Ok(())
}

/// vecEvalStringByRows 在缺少专用向量化实现时，逐行调用 builtinFunc.evalString。
pub fn vecEvalStringByRows(
    sig: &dyn builtinFunc,
    ctx: &dyn EvalContext,
    input: &chunk::Chunk,
    result: &mut chunk::Column,
) -> Result<(), Error> {
    let n = input.NumRows();
    result.ReserveString(n);
    for i in 0..n {
        let (res, isNull) = sig.evalString(ctx, input.GetRow(i))?;
        if isNull {
            result.AppendNull();
            continue;
        }
        result.AppendString(&res);
    }
    Ok(())
}
