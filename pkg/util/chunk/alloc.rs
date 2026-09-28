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

// Chunk / Column 对象池分配器，降低执行器热路径上的堆分配。
//
// 对应 Go `util/chunk` 的 Allocator：`Alloc` 从池取 Chunk 并按字段类型
// 填充 Column；`Reset` 将 Chunk 与 Column 拆开归还。另提供加锁包装、
// 首次复用 hook 以及不缓存的空实现。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Once};

// ChunkRef 用 Arc<Mutex<_>> 表达 Go 里可跨线程复用的 *Chunk 指针。
/// 共享可变的 Chunk 引用，对齐 Go `*Chunk`。
pub type ChunkRef = Arc<Mutex<Chunk>>;

// Allocator is an interface defined to reduce object allocation.
// The typical usage is to call Reset() to recycle objects into a pool,
// and Alloc() allocates from the pool.
/// 减少对象分配的池化接口：`Alloc` 取用，`Reset` 回收。
pub trait Allocator {
    /// 按字段类型与容量分配（或复用）一个 Chunk。
    fn Alloc(
        &mut self,
        fields: &[Box<types::FieldType>],
        capacity: usize,
        maxChunkSize: usize,
    ) -> ChunkRef;
    /// 是否仍有可复用的 Chunk / Column 配额。
    fn CheckReuseAllocSize(&mut self) -> bool;
    /// 将本轮已分配对象拆解归还到池中。
    fn Reset(&mut self);
}

/// 全局：最多缓存的空闲 Chunk 数。
pub static mut maxFreeChunks: usize = 64;
/// 全局：每种列类型最多缓存的空闲 Column 数。
pub static mut maxFreeColumnsPerType: usize = 256;

// InitChunkAllocSize init the maximum cache size
// InitChunkAllocSize 保留 Go 的 uint32 到 int 上限保护，避免配置值超过 math.MaxInt32。
/// 初始化 Chunk / Column 缓存上限（带 MaxInt32 钳制）。
pub fn InitChunkAllocSize(mut setMaxFreeChunks: u32, mut setMaxFreeColumns: u32) {
    if setMaxFreeChunks > i32::MAX as u32 {
        setMaxFreeChunks = i32::MAX as u32;
    }
    if setMaxFreeColumns > i32::MAX as u32 {
        setMaxFreeColumns = i32::MAX as u32;
    }
    unsafe {
        maxFreeChunks = setMaxFreeChunks as usize;
        maxFreeColumnsPerType = setMaxFreeColumns as usize;
    }
}

// NewAllocator creates an Allocator.
// NewAllocator 初始化 chunk free list 容量和按列类型划分的 column allocator。
/// 创建默认池化 Allocator。
pub fn NewAllocator() -> Box<allocator> {
    let mut ret = Box::new(allocator {
        allocated: Vec::new(),
        free: Vec::new(),
        columnAlloc: poolColumnAllocator::default(),
        freeChunk: unsafe { maxFreeChunks },
    });
    ret.columnAlloc.init();
    ret
}

// MaxCachedLen Maximum cacheable length
/// Column `data` 可缓存的最大 capacity，超限则不入池。
pub static mut MaxCachedLen: usize = 16 * 1024;

// allocator try to reuse objects.
// It uses `poolColumnAllocator` to alloc chunk column objects.
// The allocated chunks are recorded in the `allocated` array.
// After Reset(), those chunks are decoupled into chunk column objects and get
// into `poolColumnAllocator` again for reuse.
// allocator 记录本轮已分配的 Chunk，Reset 时将 Chunk 自身和其中 Column 拆开归还到不同池中。
/// 默认池化实现：追踪已分配 Chunk，Reset 时拆列归还。
pub struct allocator {
    /// 本轮已分配、待 Reset 回收的 Chunk。
    allocated: Vec<ChunkRef>,
    /// 可立即复用的空闲 Chunk。
    free: Vec<ChunkRef>,
    columnAlloc: poolColumnAllocator,
    /// 允许缓存的空闲 Chunk 上限。
    freeChunk: usize,
}

// columnList keep column
// columnList 分别保存可立即复用的列和本轮已经分配出去、等待 Reset 回收的列。
/// 按类型分组的 Column 空闲/已分配列表。
pub struct columnList {
    freeColumns: Vec<Box<Column>>,
    allocColumns: Vec<Box<Column>>,
}

impl columnList {
    // columnList Len Get the number of elements in the list
    pub fn Len(&self) -> usize {
        self.freeColumns.len() + self.allocColumns.len()
    }
}

impl Allocator for allocator {
    // CheckReuseAllocSize return whether the cache can cache objects
    fn CheckReuseAllocSize(&mut self) -> bool {
        self.freeChunk > 0 || self.columnAlloc.freeColumnsPerType > 0
    }

    // Alloc implements the Allocator interface.
    // Alloc 优先从 free chunk 取对象，再按字段类型从 column allocator 填充列。
    fn Alloc(
        &mut self,
        fields: &[Box<types::FieldType>],
        capacity: usize,
        maxChunkSize: usize,
    ) -> ChunkRef {
        let chk = if let Some(chk) = self.free.pop() {
            chk
        } else {
            Arc::new(Mutex::new(Chunk {
                sel: None,
                columns: Vec::with_capacity(fields.len()),
                numVirtualRows: 0,
                capacity: 0,
                requiredRows: 0,
                inCompleteChunk: false,
            }))
        };

        {
            let mut chk_mut = chk.lock().unwrap();
            // Init the chunk fields.
            chk_mut.capacity = std::cmp::min(capacity, maxChunkSize);
            chk_mut.requiredRows = maxChunkSize;
            let chunk_capacity = chk_mut.capacity;
            // Allocate the chunk columns from the pool column allocator.
            for f in fields {
                chk_mut.columns.push(*poolColumnAllocator::NewColumn(
                    &mut self.columnAlloc,
                    f,
                    chunk_capacity,
                ));
            }
        }

        // avoid OOM
        if self.freeChunk > self.allocated.len() {
            // Go 保存同一个 *Chunk； clone Rc 表达同一对象引用被 allocated 追踪。
            self.allocated.push(chk.clone());
        }

        chk
    }

    // Reset implements the Allocator interface.
    // Reset 先回收 Chunk 容器，再把本轮分配出的 Column 按类型归还到 column pool。
    fn Reset(&mut self) {
        // Go 的 allocColumns 保存分配时的 *Column，因此既保留原始分桶，又能观察
        // Chunk 中列的后续状态。Rust 的 Chunk 按值持有列：先按 reference_id 收集
        // 仍在 Chunk 中的实际值，再用它们替换 allocColumns 中的同源快照。
        let mut live_columns = HashMap::new();
        for chk in self.allocated.drain(..) {
            let columns = {
                let mut chunk = chk.lock().unwrap();
                let columns = std::mem::take(&mut chunk.columns);
                chunk.resetForReuse();
                columns
            };
            for column in columns {
                live_columns
                    .entry(column.reference_root())
                    .or_insert_with(|| Box::new(column));
            }
            if self.free.len() < self.freeChunk {
                // Don't cache too much data.
                self.free.push(chk);
            }
        }

        // column objects and put them to the column allocator for reuse.
        for (id, pool) in self.columnAlloc.pool.iter_mut() {
            let allocated = std::mem::take(&mut pool.allocColumns);
            for snapshot in allocated {
                let mut col = live_columns
                    .remove(&snapshot.reference_root())
                    .unwrap_or(snapshot);
                if pool.freeColumns.len() < self.columnAlloc.freeColumnsPerType
                    && checkColumnType(*id, &col)
                {
                    col.reset();
                    pool.freeColumns.push(col);
                }
                // Go 将 pool.allocColumns[i] 置 nil 后截断；这里 take 后丢弃未入池列，表达同样的资源释放点。
            }
        }
    }
}

// checkColumnType check whether the conditions for entering the corresponding queue are met
// column Reset may change type
// checkColumnType 防止类型已经变化或占用过大内存的 Column 被放回错误队列。
/// 判断 Column 是否仍可按 `id`（固定长或变长）放回对应池。
pub fn checkColumnType(id: usize, col: &Column) -> bool {
    if col.avoidReusing {
        return false;
    }

    if id == VarElemLen {
        // Take up too much memory,
        if col.data.capacity() > unsafe { MaxCachedLen } {
            return false;
        }
        // Go 判断 elemBuf == nil；用空 Vec 表达变长列没有固定元素缓冲。
        return col.elemBuf.is_empty();
    }

    if col.elemBuf.is_empty() {
        return false;
    }
    id == col.elemBuf.capacity()
}

#[derive(Default)]
/// 按 `typeSize` 分桶的 Column 池。
pub struct poolColumnAllocator {
    pool: HashMap<usize, Box<columnList>>,
    freeColumnsPerType: usize,
}

impl ColumnAllocator for poolColumnAllocator {
    // poolColumnAllocator implements the ColumnAllocator interface.
    fn NewColumn(&self, ft: &types::FieldType, count: usize) -> Box<Column> {
        // ColumnAllocator trait 需要 &self；真正的分配路径使用下面的固有方法以便记录 allocColumns。
        newColumn(getFixedLen(ft), count)
    }
}

impl poolColumnAllocator {
    // poolColumnAllocator implements the ColumnAllocator interface.
    // NewColumn 从按 typeSize 分组的池里取列，并保留 Go 的 allocColumns 计数。
    // allocator::Reset 会用 Chunk 中的实际 Column 替换这里的快照后再回收。
    pub fn NewColumn(&mut self, ft: &types::FieldType, count: usize) -> Box<Column> {
        let typeSize = getFixedLen(ft);
        let column = self.NewSizeColumn(typeSize, count);
        self.put(Box::new(column.reference_clone()));
        column
    }

    // poolColumnAllocator implements the ColumnAllocator interface.
    // NewSizeColumn 复用同类型 free column；容量不足时丢弃旧列并新建。
    pub fn NewSizeColumn(&mut self, typeSize: usize, count: usize) -> Box<Column> {
        if let Some(l) = self.pool.get_mut(&typeSize) {
            if !l.empty() {
                if let Some(mut col) = l.pop() {
                    if col.data.capacity() < count {
                        col = newColumn(typeSize, count);
                    }
                    return col;
                }
            }
        }
        newColumn(typeSize, count)
    }

    /// 用全局上限初始化空池。
    pub fn init(&mut self) {
        self.pool = HashMap::new();
        self.freeColumnsPerType = unsafe { maxFreeColumnsPerType };
    }

    /// 将 Column 记入对应 typeSize 的 allocColumns（受容量限制）。
    pub fn put(&mut self, col: Box<Column>) {
        if col.avoidReusing {
            return;
        }
        let typeSize = col.typeSize();
        if typeSize <= 0 && typeSize != VarElemLen {
            return;
        }

        let l = self.pool.entry(typeSize).or_insert_with(|| {
            Box::new(columnList {
                freeColumns: Vec::with_capacity(self.freeColumnsPerType),
                allocColumns: Vec::with_capacity(self.freeColumnsPerType),
            })
        });
        if l.allocColumns.len() < self.freeColumnsPerType {
            l.push(col);
        }
    }

    /// 某 typeSize 下 free+alloc 列总数。
    pub fn column_count(&self, type_size: usize) -> usize {
        self.pool.get(&type_size).map_or(0, |columns| columns.Len())
    }

    /// 各 typeSize 桶的列总数列表。
    pub fn all_column_counts(&self) -> Vec<usize> {
        self.pool.values().map(|columns| columns.Len()).collect()
    }

    /// 某 typeSize 下已分配（待回收）列数。
    pub fn allocated_column_count(&self, type_size: usize) -> usize {
        self.pool
            .get(&type_size)
            .map_or(0, |columns| columns.allocColumns.len())
    }
}

impl allocator {
    /// 当前空闲 Chunk 数量。
    pub fn cached_chunk_count(&self) -> usize {
        self.free.len()
    }

    /// 某 typeSize 空闲 Column 数量。
    pub fn cached_column_count(&self, type_size: usize) -> usize {
        self.columnAlloc
            .pool
            .get(&type_size)
            .map_or(0, |columns| columns.freeColumns.len())
    }

    /// 某 typeSize 池顶空闲列的 data capacity。
    pub fn cached_column_capacity(&self, type_size: usize) -> Option<usize> {
        self.columnAlloc
            .pool
            .get(&type_size)
            .and_then(|columns| columns.freeColumns.last())
            .map(|column| column.data.capacity())
    }


    pub fn all_column_counts(&self) -> Vec<usize> {
        self.columnAlloc.all_column_counts()
    }

    pub fn allocated_column_count(&self, type_size: usize) -> usize {
        self.columnAlloc.allocated_column_count(type_size)
    }

    /// 所有空闲 Column 的地址列表（测试用）。
    pub fn cached_column_addresses(&self) -> Vec<usize> {
        self.columnAlloc
            .pool
            .values()
            .flat_map(|columns| columns.freeColumns.iter())
            .map(|column| &**column as *const Column as usize)
            .collect()
    }
}

impl columnList {
    pub fn pop(&mut self) -> Option<Box<Column>> {
        let col = self.freeColumns.pop();
        if col.is_none() {
            return None;
        }
        col
    }

    pub fn empty(&self) -> bool {
        self.freeColumns.is_empty()
    }

    pub fn push(&mut self, col: Box<Column>) {
        if col.data.capacity() < unsafe { MaxCachedLen } {
            self.allocColumns.push(col);
        }
    }
}

// syncAllocator uses a mutex to protect the allocator.
// syncAllocator 对应 Go 的加锁包装器，所有接口方法都在 Mutex 临界区内转调底层 allocator。
/// 线程安全的 Allocator 包装。
pub struct syncAllocator {
    alloc: Mutex<Box<dyn Allocator + Send>>,
}

// NewSyncAllocator creates the synchronized version of the `alloc`
/// 用 Mutex 包装任意 `Allocator + Send`。
pub fn NewSyncAllocator(alloc: Box<dyn Allocator + Send>) -> Box<syncAllocator> {
    Box::new(syncAllocator {
        alloc: Mutex::new(alloc),
    })
}

impl syncAllocator {
    pub fn Alloc(
        &self,
        fields: &[Box<types::FieldType>],
        capacity: usize,
        maxChunkSize: usize,
    ) -> ChunkRef {
        self.alloc
            .lock()
            .unwrap()
            .Alloc(fields, capacity, maxChunkSize)
    }

    pub fn CheckReuseAllocSize(&self) -> bool {
        self.alloc.lock().unwrap().CheckReuseAllocSize()
    }

    pub fn Reset(&self) {
        self.alloc.lock().unwrap().Reset();
    }
}

impl Allocator for syncAllocator {
    // Alloc implements `Allocator` for `*syncAllocator`
    fn Alloc(
        &mut self,
        fields: &[Box<types::FieldType>],
        capacity: usize,
        maxChunkSize: usize,
    ) -> ChunkRef {
        syncAllocator::Alloc(self, fields, capacity, maxChunkSize)
    }

    // CheckReuseAllocSize implements `Allocator` for `*syncAllocator`
    fn CheckReuseAllocSize(&mut self) -> bool {
        syncAllocator::CheckReuseAllocSize(self)
    }

    // Reset implements `Allocator` for `*syncAllocator`
    fn Reset(&mut self) {
        syncAllocator::Reset(self)
    }
}

// reuseHookAllocator will run the function hook when it allocates the first chunk from reused part
// reuseHookAllocator 在第一次发现底层 allocator 已有可复用对象时执行 hook，之后由 sync.Once 保证只执行一次。
/// 首次从复用池分配时触发一次性 hook 的包装器。
pub struct reuseHookAllocator {
    once: Once,
    f: Box<dyn Fn()>,

    alloc: Box<dyn Allocator>,
}

// NewReuseHookAllocator creates an allocator, which will call the function `f` when the first reused chunk is allocated.
/// 包装底层 allocator，首次复用时调用 `f`。
pub fn NewReuseHookAllocator(alloc: Box<dyn Allocator>, f: Box<dyn Fn()>) -> Box<dyn Allocator> {
    Box::new(reuseHookAllocator {
        once: Once::new(),
        f,
        alloc,
    })
}

impl Allocator for reuseHookAllocator {
    // Alloc implements `Allocator` for `*reuseHookAllocator`
    fn Alloc(
        &mut self,
        fields: &[Box<types::FieldType>],
        capacity: usize,
        maxChunkSize: usize,
    ) -> ChunkRef {
        if self.alloc.CheckReuseAllocSize() {
            // Go 的 once.Do(r.f) 在首次复用前触发；保留相同并发语义入口。
            self.once.call_once(|| (self.f)());
        }

        self.alloc.Alloc(fields, capacity, maxChunkSize)
    }

    // CheckReuseAllocSize implements `Allocator` for `*reuseHookAllocator`
    fn CheckReuseAllocSize(&mut self) -> bool {
        self.alloc.CheckReuseAllocSize()
    }

    // Reset implements `Allocator` for `*reuseHookAllocator`
    fn Reset(&mut self) {
        self.alloc.Reset()
    }
}

#[derive(Clone, Copy)]
/// 不缓存对象的空 Allocator；每次直接新建 Chunk。
pub struct emptyAllocator {}

/// 全局默认空 Allocator 单例。
pub static defaultEmptyAllocator: emptyAllocator = emptyAllocator {};

// NewEmptyAllocator creates an empty pool, which will always call `chunk.New` to create a new chunk
/// 返回始终新建 Chunk 的空池实现。
pub fn NewEmptyAllocator() -> Box<dyn Allocator> {
    Box::new(defaultEmptyAllocator)
}

impl Allocator for emptyAllocator {
    // Alloc implements `Allocator` for `*emptyAllocator`
    // emptyAllocator 不缓存对象；每次都走 chunk.New 等价的直接构造路径。
    fn Alloc(
        &mut self,
        fields: &[Box<types::FieldType>],
        capacity: usize,
        maxChunkSize: usize,
    ) -> ChunkRef {
        let real_capacity = std::cmp::min(capacity, maxChunkSize);
        let mut chk = Chunk {
            sel: None,
            columns: Vec::with_capacity(fields.len()),
            numVirtualRows: 0,
            capacity: real_capacity,
            requiredRows: maxChunkSize,
            inCompleteChunk: false,
        };
        for f in fields {
            chk.columns.push(*NewColumn(f, real_capacity));
        }
        Arc::new(Mutex::new(chk))
    }

    // CheckReuseAllocSize implements `Allocator` for `*emptyAllocator`
    fn CheckReuseAllocSize(&mut self) -> bool {
        false
    }

    // Reset implements `Allocator` for `*emptyAllocator`
    fn Reset(&mut self) {}
}
