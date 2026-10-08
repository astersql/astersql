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

// CTE（Common Table Expression，公用表表达式）执行器。
//
// 支持非递归与递归 CTE：seed（种子）算子产出初始结果，recursive（递归）算子
// 迭代读写 `iterInTbl`/`iterOutTbl`，最终写入 `resTbl`。可选 DISTINCT 通过哈希表去重，
// 可选 LIMIT 在结果扫描阶段裁剪行。`CTEBackend` 抽象存储、内存/磁盘 tracker 与 spill 测试钩子。
#![allow(non_camel_case_types, non_snake_case)]

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// 指向 CTE 存储中某一行的位置（chunk 下标 + 行下标）。
pub struct RowPointer {
    pub chunk_index: usize,
    pub row_index: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// CTE 表统计日志级别。
pub enum CTELogLevel {
    Debug,
    Info,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// CTE 存储占用统计：内存字节、磁盘字节与行数。
pub struct StorageStats {
    pub memory_bytes: i64,
    pub disk_bytes: i64,
    pub rows: usize,
}

/// DISTINCT 去重用的哈希表：哈希键 -> 行指针列表（处理哈希冲突）。
type HashTable = HashMap<u64, Vec<RowPointer>>;

/// CTE 执行对会话、存储、chunk 与 tracker 的生产边界抽象。
pub trait CTEBackend: Send + Sync + 'static {
    type Context: Clone;
    type Error: Clone;
    type Chunk;
    type Row: Clone;
    type Executor: Clone;
    type Storage: Clone;
    type FieldType: Clone;
    type CorrelatedColumn: Clone;
    type MemoryTracker: Clone;
    type DiskTracker: Clone;
    type SpillAction: Clone;

    fn error(&self, message: String) -> Self::Error;
    fn recovered_panic_error(&self, panic: &(dyn Any + Send)) -> Self::Error;
    fn log_cte_error(&self, error: &Self::Error, message: &str);
    fn log_cte_tables(
        &self,
        context: &Self::Context,
        error: Option<&Self::Error>,
        iteration: u64,
        level: CTELogLevel,
        iter_in: StorageStats,
        iter_out: StorageStats,
        result: StorageStats,
    );

    fn base_open(&self, context: &Self::Context) -> Result<(), Self::Error>;
    fn base_close(&self) -> Result<(), Self::Error>;
    fn executor_id(&self) -> i32;
    fn max_chunk_size(&self) -> usize;
    fn return_field_types(&self) -> Vec<Self::FieldType>;
    fn statement_memory_tracker(&self) -> Self::MemoryTracker;
    fn statement_disk_tracker(&self) -> Self::DiskTracker;
    fn session_memory_tracker(&self) -> Self::MemoryTracker;
    fn max_recursion_depth(&self) -> i32;
    fn recursion_depth_error(&self, depth: i32) -> Self::Error;

    fn open_executor(
        &self,
        context: &Self::Context,
        executor: &Self::Executor,
    ) -> Result<(), Self::Error>;
    fn close_executor(&self, executor: &Self::Executor) -> Result<(), Self::Error>;
    fn next_executor(
        &self,
        context: &Self::Context,
        executor: &Self::Executor,
        chunk: &mut Self::Chunk,
    ) -> Result<(), Self::Error>;
    fn new_cache_chunk(&self, executor: &Self::Executor) -> Self::Chunk;
    fn executor_return_field_types(&self, executor: &Self::Executor) -> Vec<Self::FieldType>;

    fn new_memory_tracker(&self, executor_id: i32) -> Self::MemoryTracker;
    fn new_disk_tracker(&self, executor_id: i32) -> Self::DiskTracker;
    fn reset_memory_tracker(&self, tracker: &Self::MemoryTracker);
    fn reset_disk_tracker(&self, tracker: &Self::DiskTracker);
    fn attach_memory_tracker(&self, tracker: &Self::MemoryTracker, parent: &Self::MemoryTracker);
    fn attach_disk_tracker(&self, tracker: &Self::DiskTracker, parent: &Self::DiskTracker);

    fn new_storage(
        &self,
        field_types: Vec<Self::FieldType>,
        max_chunk_size: usize,
    ) -> Self::Storage;
    fn storage_open_and_ref(&self, storage: &Self::Storage) -> Result<(), Self::Error>;
    fn storage_deref_and_close(&self, storage: &Self::Storage) -> Result<(), Self::Error>;
    fn storage_reopen(&self, storage: &Self::Storage) -> Result<(), Self::Error>;
    fn storage_add(&self, storage: &Self::Storage, chunk: &Self::Chunk) -> Result<(), Self::Error>;
    fn storage_get_chunk(
        &self,
        storage: &Self::Storage,
        index: usize,
    ) -> Result<Self::Chunk, Self::Error>;
    fn storage_get_row(
        &self,
        storage: &Self::Storage,
        pointer: RowPointer,
    ) -> Result<Self::Row, Self::Error>;
    fn storage_num_chunks(&self, storage: &Self::Storage) -> usize;
    fn storage_num_rows(&self, storage: &Self::Storage) -> usize;
    fn storage_done(&self, storage: &Self::Storage) -> bool;
    fn storage_error(&self, storage: &Self::Storage) -> Option<Self::Error>;
    fn storage_set_error(&self, storage: &Self::Storage, error: Self::Error);
    fn storage_set_done(&self, storage: &Self::Storage);
    fn storage_set_iteration(&self, storage: &Self::Storage, iteration: i32);
    fn storage_swap_data(
        &self,
        left: &Self::Storage,
        right: &Self::Storage,
    ) -> Result<(), Self::Error>;
    fn storage_stats(&self, storage: &Self::Storage) -> StorageStats;
    fn configure_storage_trackers(
        &self,
        storage: &Self::Storage,
        parent_memory: &Self::MemoryTracker,
        parent_disk: &Self::DiskTracker,
        session_memory: &Self::MemoryTracker,
    ) -> Option<Self::SpillAction>;
    fn spill_test_enabled(&self) -> bool;
    fn wait_for_spill_test(&self, action: &Self::SpillAction);
    fn assert_iteration_spill(
        &self,
        iteration: u64,
        iter_in: StorageStats,
        iter_out: StorageStats,
        result: StorageStats,
    );

    fn reset_chunk(&self, chunk: &mut Self::Chunk);
    fn chunk_num_rows(&self, chunk: &Self::Chunk) -> usize;
    fn chunk_num_columns(&self, chunk: &Self::Chunk) -> usize;
    fn chunk_capacity(&self, chunk: &Self::Chunk) -> usize;
    fn chunk_selection(&self, chunk: &Self::Chunk) -> Option<Vec<usize>>;
    fn chunk_set_selection(&self, chunk: &mut Self::Chunk, selection: Vec<usize>);
    fn chunk_copy_selected(&self, chunk: &Self::Chunk) -> Self::Chunk;
    fn chunk_copy_all(&self, chunk: &Self::Chunk) -> Self::Chunk;
    fn chunk_swap_columns(&self, destination: &mut Self::Chunk, source: Self::Chunk);
    fn chunk_append(
        &self,
        destination: &mut Self::Chunk,
        source: Self::Chunk,
        begin: usize,
        end: usize,
    );
    fn chunk_row(&self, chunk: &Self::Chunk, logical_index: usize) -> Self::Row;
    fn hash_chunk_all_columns(
        &self,
        chunk: &Self::Chunk,
        field_types: &[Self::FieldType],
        selected_rows: Option<&[usize]>,
    ) -> Result<Vec<u64>, Self::Error>;
    fn rows_equal(
        &self,
        left: &Self::Row,
        right: &Self::Row,
        field_types: &[Self::FieldType],
        key_column_indexes: &[usize],
    ) -> Result<bool, Self::Error>;

    fn correlated_column_hash(&self, column: &Self::CorrelatedColumn) -> Vec<u8>;
    fn trigger_seed_failpoint(&self);
    fn trigger_recursive_failpoint(&self);
}

/// DISTINCT 哈希上下文：列类型、键列下标与当前 chunk 的哈希值缓存。
struct HashContext<F> {
    all_types: Vec<F>,
    key_column_indexes: Vec<usize>,
    hash_values: Vec<u64>,
}

/// CTE 消费者执行器：从共享 `cteProducer` 读取已物化的结果 chunk。
pub struct CTEExec<B: CTEBackend> {
    pub backend: Arc<B>,
    pub chkIdx: usize,
    pub producer: Arc<Mutex<cteProducer<B>>>,
    pub cursor: u64,
    pub meetFirstBatch: bool,
}

impl<B: CTEBackend> CTEExec<B> {
    /// 打开 CTE：若相关列哈希变化则复位 producer，并按需打开生产侧执行器。
    pub fn Open(&mut self, context: &B::Context) -> Result<(), B::Error> {
        self.reset();
        self.backend.base_open(context)?;

        let producer = Arc::clone(&self.producer);
        let mut producer = producer
            .lock()
            .map_err(|_| self.backend.error("CTE storage lock poisoned".to_owned()))?;
        // 相关列变化意味着外层参数变了，需丢弃旧物化结果
        if producer.checkAndUpdateCorColHashCode() {
            producer.reset()?;
        }
        if let Some(error) = producer.openErr.clone() {
            return Err(error);
        }
        if !producer.hasCTEResult() && !producer.executorOpened {
            producer.openProducerExecutor(context, self)?;
        }
        Ok(())
    }

    /// 拉取结果：尚未物化时先 `genCTEResult`，再从 `resTbl` 取 chunk。
    pub fn Next(&mut self, context: &B::Context, request: &mut B::Chunk) -> Result<(), B::Error> {
        let producer = Arc::clone(&self.producer);
        let mut producer = producer
            .lock()
            .map_err(|_| self.backend.error("CTE storage lock poisoned".to_owned()))?;
        // 惰性物化：首次 Next 时生成完整 CTE 结果
        if !producer.hasCTEResult() {
            if !producer.executorOpened {
                producer.openProducerExecutor(context, self)?;
            }
            producer.genCTEResult(context)?;
        }
        producer.getChunk(self, request)
    }

    /// 关闭 producer 与基类子树，保留首个错误。
    pub fn Close(&mut self) -> Result<(), B::Error> {
        let mut first_error = None;
        {
            let producer = Arc::clone(&self.producer);
            let mut producer = producer
                .lock()
                .map_err(|_| self.backend.error("CTE storage lock poisoned".to_owned()))?;
            if producer.executorOpened {
                first_error = setFirstErr(
                    self.backend.as_ref(),
                    first_error,
                    producer.closeProducerExecutor(),
                    "close cte producer error",
                );
                if !producer.hasCTEResult() {
                    first_error = setFirstErr(
                        self.backend.as_ref(),
                        first_error,
                        producer.reset(),
                        "close cte producer error",
                    );
                }
            }
        }
        first_error = setFirstErr(
            self.backend.as_ref(),
            first_error,
            self.backend.base_close(),
            "close cte children error",
        );
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// 复位消费侧游标与 LIMIT 扫描状态。
    pub fn reset(&mut self) {
        self.chkIdx = 0;
        self.cursor = 0;
        self.meetFirstBatch = false;
    }
}

/// 合并错误：记录日志并保留最先出现的错误。
pub fn setFirstErr<B: CTEBackend>(
    backend: &B,
    first_error: Option<B::Error>,
    new_error: Result<(), B::Error>,
    message: &str,
) -> Option<B::Error> {
    match new_error {
        Ok(()) => first_error,
        Err(error) => {
            backend.log_cte_error(&error, message);
            first_error.or(Some(error))
        }
    }
}

/// CTE 生产者：负责 seed/recursive 执行、DISTINCT、LIMIT 与结果物化。
pub struct cteProducer<B: CTEBackend> {
    pub backend: Arc<B>,
    pub executorOpened: bool,
    pub openErr: Option<B::Error>,
    pub seedExec: Option<B::Executor>,
    pub recursiveExec: Option<B::Executor>,
    pub resTbl: B::Storage,
    pub iterInTbl: B::Storage,
    pub iterOutTbl: Option<B::Storage>,
    hashTbl: Option<HashTable>,
    pub isDistinct: bool,
    pub curIter: i32,
    hCtx: Option<HashContext<B::FieldType>>,
    pub sel: Vec<usize>,
    pub hasLimit: bool,
    pub limitBeg: u64,
    pub limitEnd: u64,
    pub memTracker: Option<B::MemoryTracker>,
    pub diskTracker: Option<B::DiskTracker>,
    pub corCols: Vec<B::CorrelatedColumn>,
    pub corColHashCodes: Vec<Vec<u8>>,
}

impl<B: CTEBackend> cteProducer<B> {
    /// 打开 seed/recursive、初始化 tracker、DISTINCT 哈希表与迭代输出存储。
    pub fn openProducerExecutor(
        &mut self,
        context: &B::Context,
        cte_exec: &CTEExec<B>,
    ) -> Result<(), B::Error> {
        let result = (|| {
            let seed = self
                .seedExec
                .as_ref()
                .ok_or_else(|| self.backend.error("seedExec for CTEExec is nil".to_owned()))?;
            self.backend.open_executor(context, seed)?;

            self.resetTracker();
            let memory_tracker = self.backend.new_memory_tracker(self.backend.executor_id());
            let disk_tracker = self.backend.new_disk_tracker(self.backend.executor_id());
            self.backend
                .attach_memory_tracker(&memory_tracker, &self.backend.statement_memory_tracker());
            self.backend
                .attach_disk_tracker(&disk_tracker, &self.backend.statement_disk_tracker());
            self.memTracker = Some(memory_tracker);
            self.diskTracker = Some(disk_tracker);

            if let Some(recursive) = self.recursiveExec.as_ref() {
                self.backend.open_executor(context, recursive)?;
                let storage = self.backend.new_storage(
                    self.backend.executor_return_field_types(recursive),
                    cte_exec.backend.max_chunk_size(),
                );
                self.backend.storage_open_and_ref(&storage)?;
                self.iterOutTbl = Some(storage);
            }

            if self.isDistinct {
                self.hashTbl = Some(HashMap::new());
                let all_types = cte_exec.backend.return_field_types();
                self.hCtx = Some(HashContext {
                    key_column_indexes: (0..all_types.len()).collect(),
                    all_types,
                    hash_values: Vec::new(),
                });
            }
            Ok(())
        })();
        self.openErr = result.clone().err();
        self.executorOpened = true;
        result
    }

    /// 关闭 seed/recursive 与 iterOutTbl，清空 tracker。
    pub fn closeProducerExecutor(&mut self) -> Result<(), B::Error> {
        let mut first_error = None;
        if let Some(seed) = self.seedExec.as_ref() {
            first_error = setFirstErr(
                self.backend.as_ref(),
                first_error,
                self.backend.close_executor(seed),
                "close seedExec err",
            );
        }
        if let Some(recursive) = self.recursiveExec.as_ref() {
            first_error = setFirstErr(
                self.backend.as_ref(),
                first_error,
                self.backend.close_executor(recursive),
                "close recursiveExec err",
            );
            if let Some(iter_out) = self.iterOutTbl.as_ref() {
                first_error = setFirstErr(
                    self.backend.as_ref(),
                    first_error,
                    self.backend.storage_deref_and_close(iter_out),
                    "deref iterOutTbl err",
                );
            }
        }
        self.memTracker = None;
        self.diskTracker = None;
        self.executorOpened = false;
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// 向消费者填充下一结果 chunk；有 LIMIT 时走 `nextChunkLimit`。
    pub fn getChunk(
        &mut self,
        cte_exec: &mut CTEExec<B>,
        request: &mut B::Chunk,
    ) -> Result<(), B::Error> {
        self.backend.reset_chunk(request);
        if self.hasLimit {
            return self.nextChunkLimit(cte_exec, request);
        }
        if cte_exec.chkIdx < self.backend.storage_num_chunks(&self.resTbl) {
            let result = self
                .backend
                .storage_get_chunk(&self.resTbl, cte_exec.chkIdx)?;
            self.backend
                .chunk_swap_columns(request, self.backend.chunk_copy_selected(&result));
            cte_exec.chkIdx += 1;
        }
        Ok(())
    }

    /// 带 LIMIT 的结果扫描：跳过 `limitBeg` 之前的行，截断到 `limitEnd`。
    pub fn nextChunkLimit(
        &mut self,
        cte_exec: &mut CTEExec<B>,
        request: &mut B::Chunk,
    ) -> Result<(), B::Error> {
        if !cte_exec.meetFirstBatch {
            while cte_exec.chkIdx < self.backend.storage_num_chunks(&self.resTbl) {
                let result = self
                    .backend
                    .storage_get_chunk(&self.resTbl, cte_exec.chkIdx)?;
                cte_exec.chkIdx += 1;
                let row_count = self.backend.chunk_num_rows(&result) as u64;
                let new_cursor = cte_exec.cursor + row_count;
                if new_cursor >= self.limitBeg {
                    cte_exec.meetFirstBatch = true;
                    let begin = self.limitBeg - cte_exec.cursor;
                    let mut end = row_count;
                    if new_cursor > self.limitEnd {
                        end = self.limitEnd - cte_exec.cursor;
                    }
                    cte_exec.cursor += end;
                    if begin == end {
                        break;
                    }
                    self.backend.chunk_append(
                        request,
                        self.backend.chunk_copy_selected(&result),
                        begin as usize,
                        end as usize,
                    );
                    return Ok(());
                }
                cte_exec.cursor += row_count;
            }
        }

        if cte_exec.chkIdx < self.backend.storage_num_chunks(&self.resTbl)
            && cte_exec.cursor < self.limitEnd
        {
            let result = self
                .backend
                .storage_get_chunk(&self.resTbl, cte_exec.chkIdx)?;
            cte_exec.chkIdx += 1;
            let mut row_count = self.backend.chunk_num_rows(&result) as u64;
            if cte_exec.cursor + row_count > self.limitEnd {
                row_count = self.limitEnd - cte_exec.cursor;
                self.backend.chunk_append(
                    request,
                    self.backend.chunk_copy_selected(&result),
                    0,
                    row_count as usize,
                );
            } else {
                self.backend
                    .chunk_swap_columns(request, self.backend.chunk_copy_selected(&result));
            }
            cte_exec.cursor += row_count;
        }
        Ok(())
    }

    /// 结果表是否已标记为完成（`storage_done`）。
    pub fn hasCTEResult(&self) -> bool {
        self.backend.storage_done(&self.resTbl)
    }

    /// 计算 seed + recursive 并标记结果完成；可选等待 spill 测试。
    pub fn genCTEResult(&mut self, context: &B::Context) -> Result<(), B::Error> {
        if let Some(error) = self.backend.storage_error(&self.resTbl) {
            return Err(error);
        }
        let result_action = setupCTEStorageTracker(
            self.backend.as_ref(),
            &self.resTbl,
            self.memTracker
                .as_ref()
                .expect("CTE memory tracker must be initialized"),
            self.diskTracker
                .as_ref()
                .expect("CTE disk tracker must be initialized"),
        );
        let iter_in_action = setupCTEStorageTracker(
            self.backend.as_ref(),
            &self.iterInTbl,
            self.memTracker
                .as_ref()
                .expect("CTE memory tracker must be initialized"),
            self.diskTracker
                .as_ref()
                .expect("CTE disk tracker must be initialized"),
        );
        let iter_out_action = self.iterOutTbl.as_ref().map(|storage| {
            setupCTEStorageTracker(
                self.backend.as_ref(),
                storage,
                self.memTracker
                    .as_ref()
                    .expect("CTE memory tracker must be initialized"),
                self.diskTracker
                    .as_ref()
                    .expect("CTE disk tracker must be initialized"),
            )
        });

        let result = (|| {
            if let Err(error) = self.computeSeedPart(context) {
                self.backend.storage_set_error(&self.resTbl, error.clone());
                return Err(error);
            }
            if let Err(error) = self.computeRecursivePart(context) {
                self.backend.storage_set_error(&self.resTbl, error.clone());
                return Err(error);
            }
            self.backend.storage_set_done(&self.resTbl);
            Ok(())
        })();

        // Preserve the executor error before running spill-test synchronization. The Go
        // failpoint callback (and its deferred waits) finishes before CTE computation, so
        // an overflow from the recursive executor cannot be displaced by spill test state.
        result?;

        if self.backend.spill_test_enabled() {
            for action in [result_action.as_ref(), iter_in_action.as_ref()]
                .into_iter()
                .flatten()
            {
                self.backend.wait_for_spill_test(action);
            }
            if let Some(action) = iter_out_action.as_ref().and_then(Option::as_ref) {
                self.backend.wait_for_spill_test(action);
            }
        }
        Ok(())
    }

    /// 执行 seed 算子，将去重后的行写入 iterInTbl 与 resTbl。
    pub fn computeSeedPart(&mut self, context: &B::Context) -> Result<(), B::Error> {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.backend.trigger_seed_failpoint();
            self.curIter = 0;
            self.backend
                .storage_set_iteration(&self.iterInTbl, self.curIter);
            let mut chunks = Vec::with_capacity(10);
            loop {
                if self.limitDone(&self.iterInTbl) {
                    break;
                }
                let seed = self
                    .seedExec
                    .as_ref()
                    .expect("seed executor must be initialized");
                let mut chunk = self.backend.new_cache_chunk(seed);
                self.backend.next_executor(context, seed, &mut chunk)?;
                if self.backend.chunk_num_rows(&chunk) == 0 {
                    break;
                }
                let chunk = self.tryDedupAndAdd(
                    chunk,
                    &self.iterInTbl.clone(),
                    self.hashTbl.clone().unwrap_or_default(),
                )?;
                chunks.push(chunk);
            }
            for chunk in &chunks {
                self.backend.storage_add(&self.resTbl, chunk)?;
            }
            self.curIter += 1;
            self.backend
                .storage_set_iteration(&self.iterInTbl, self.curIter);
            Ok(())
        }));
        match result {
            Ok(result) => result,
            Err(panic) => Err(self.backend.recovered_panic_error(panic.as_ref())),
        }
    }

    /// 迭代执行 recursive 算子直至无输入或触达递归深度/LIMIT。
    pub fn computeRecursivePart(&mut self, context: &B::Context) -> Result<(), B::Error> {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.backend.trigger_recursive_failpoint();
            let Some(recursive) = self.recursiveExec.clone() else {
                return Ok(());
            };
            if self.backend.storage_num_chunks(&self.iterInTbl) == 0 {
                return Ok(());
            }
            if self.curIter > self.backend.max_recursion_depth() {
                return Err(self.backend.recursion_depth_error(self.curIter));
            }
            if self.limitDone(&self.resTbl) {
                return Ok(());
            }

            // 递归迭代：空 chunk 表示一轮结束，交换表后继续
            let mut iteration = 0_u64;
            loop {
                let mut chunk = self.backend.new_cache_chunk(&recursive);
                self.backend
                    .next_executor(context, &recursive, &mut chunk)?;
                if self.backend.chunk_num_rows(&chunk) == 0 {
                    if iteration.is_multiple_of(1000) {
                        self.logTbls(context, None, iteration, CTELogLevel::Debug);
                    }
                    iteration += 1;
                    self.backend.assert_iteration_spill(
                        iteration,
                        self.backend.storage_stats(&self.iterInTbl),
                        self.backend.storage_stats(
                            self.iterOutTbl
                                .as_ref()
                                .expect("recursive output storage must exist"),
                        ),
                        self.backend.storage_stats(&self.resTbl),
                    );

                    self.setupTblsForNewIteration()?;
                    if self.limitDone(&self.resTbl)
                        || self.backend.storage_num_chunks(&self.iterInTbl) == 0
                    {
                        break;
                    }
                    self.curIter += 1;
                    self.backend
                        .storage_set_iteration(&self.iterInTbl, self.curIter);
                    if self.curIter > self.backend.max_recursion_depth() {
                        return Err(self.backend.recursion_depth_error(self.curIter));
                    }
                    self.backend.close_executor(&recursive)?;
                    self.backend.open_executor(context, &recursive)?;
                } else {
                    self.backend.storage_add(
                        self.iterOutTbl
                            .as_ref()
                            .expect("recursive output storage must exist"),
                        &chunk,
                    )?;
                }
            }
            Ok(())
        }));
        match result {
            Ok(result) => result,
            Err(panic) => Err(self.backend.recovered_panic_error(panic.as_ref())),
        }
    }

    /// 将本轮 iterOut 并入结果，准备下一轮 iterIn，并清空 iterOut。
    pub fn setupTblsForNewIteration(&mut self) -> Result<(), B::Error> {
        let iter_out = self
            .iterOutTbl
            .as_ref()
            .expect("recursive output storage must exist")
            .clone();
        let chunk_count = self.backend.storage_num_chunks(&iter_out);
        let mut chunks = Vec::with_capacity(chunk_count);
        for index in 0..chunk_count {
            let mut chunk = self.backend.storage_get_chunk(&iter_out, index)?;
            if self.isDistinct {
                chunk = self.backend.chunk_copy_all(&chunk);
            }
            let chunk = self.tryDedupAndAdd(
                chunk,
                &self.resTbl.clone(),
                self.hashTbl.clone().unwrap_or_default(),
            )?;
            chunks.push(chunk);
        }

        self.backend.storage_reopen(&self.iterInTbl)?;
        setupCTEStorageTracker(
            self.backend.as_ref(),
            &self.iterInTbl,
            self.memTracker
                .as_ref()
                .expect("CTE memory tracker must be initialized"),
            self.diskTracker
                .as_ref()
                .expect("CTE disk tracker must be initialized"),
        );
        if self.isDistinct {
            for chunk in &chunks {
                self.backend.storage_add(&self.iterInTbl, chunk)?;
            }
        } else {
            self.backend.storage_swap_data(&self.iterInTbl, &iter_out)?;
        }

        self.backend.storage_reopen(&iter_out)?;
        setupCTEStorageTracker(
            self.backend.as_ref(),
            &iter_out,
            self.memTracker
                .as_ref()
                .expect("CTE memory tracker must be initialized"),
            self.diskTracker
                .as_ref()
                .expect("CTE disk tracker must be initialized"),
        );
        Ok(())
    }

    /// 复位迭代状态并 reopen 结果/输入存储。
    pub fn reset(&mut self) -> Result<(), B::Error> {
        self.curIter = 0;
        self.hashTbl = None;
        self.executorOpened = false;
        self.openErr = None;
        self.backend.storage_reopen(&self.resTbl)?;
        self.backend.storage_reopen(&self.iterInTbl)
    }

    /// 释放并复位内存/磁盘 tracker。
    pub fn resetTracker(&mut self) {
        if let Some(tracker) = self.memTracker.take() {
            self.backend.reset_memory_tracker(&tracker);
        }
        if let Some(tracker) = self.diskTracker.take() {
            self.backend.reset_disk_tracker(&tracker);
        }
    }

    /// 若启用 LIMIT 且存储行数已达 `limitEnd` 则返回 true。
    pub fn limitDone(&self, storage: &B::Storage) -> bool {
        self.hasLimit && self.backend.storage_num_rows(storage) as u64 >= self.limitEnd
    }

    /// 可选 DISTINCT 去重后写入目标存储。
    pub fn tryDedupAndAdd(
        &mut self,
        mut chunk: B::Chunk,
        storage: &B::Storage,
        hash_table: HashTable,
    ) -> Result<B::Chunk, B::Error> {
        if self.isDistinct {
            chunk = self.deduplicate(chunk, storage, hash_table)?;
        }
        self.backend.storage_add(storage, &chunk)?;
        Ok(chunk)
    }

    /// 计算 chunk 各行哈希，返回逻辑选择向量。
    pub fn computeChunkHash(&mut self, chunk: &B::Chunk) -> Result<Vec<usize>, B::Error> {
        let row_count = self.backend.chunk_num_rows(chunk);
        let original_selection = self.backend.chunk_selection(chunk);
        let selection = match original_selection.as_ref() {
            Some(selection) => selection.clone(),
            None => {
                if self.sel.len() < row_count {
                    self.sel.extend(self.sel.len()..row_count);
                }
                self.sel[..row_count].to_vec()
            }
        };
        let hash_context = self
            .hCtx
            .as_mut()
            .expect("distinct hash context must be initialized");
        hash_context.hash_values = self.backend.hash_chunk_all_columns(
            chunk,
            &hash_context.all_types,
            original_selection.as_deref(),
        )?;
        Ok(selection)
    }

    /// 两阶段去重：先相对本 chunk，再相对已有存储哈希表。
    pub fn deduplicate(
        &mut self,
        mut chunk: B::Chunk,
        storage: &B::Storage,
        mut hash_table: HashTable,
    ) -> Result<B::Chunk, B::Error> {
        let row_count = self.backend.chunk_num_rows(&chunk);
        if row_count == 0 {
            return Ok(chunk);
        }

        // 阶段 1：相对本 chunk 内去重
        let mut chunk_hash_table = HashMap::new();
        let original_selection = self.computeChunkHash(&chunk)?;
        let mut chunk_selection = Vec::with_capacity(row_count);
        for index in 0..row_count {
            let key = self
                .hCtx
                .as_ref()
                .expect("distinct hash context must be initialized")
                .hash_values[original_selection[index]];
            let row = self.backend.chunk_row(&chunk, index);
            if self.checkHasDup(key, &row, Some(&chunk), storage, &chunk_hash_table)? {
                continue;
            }
            chunk_selection.push(original_selection[index]);
            chunk_hash_table
                .entry(key)
                .or_insert_with(Vec::new)
                .push(RowPointer {
                    chunk_index: 0,
                    row_index: index,
                });
        }
        self.backend
            .chunk_set_selection(&mut chunk, chunk_selection.clone());
        let chunk_index = self.backend.storage_num_chunks(storage);

        // 阶段 2：相对已写入存储的哈希表去重
        let mut storage_selection = Vec::with_capacity(chunk_selection.len());
        for (index, selected) in chunk_selection.iter().copied().enumerate() {
            let key = self
                .hCtx
                .as_ref()
                .expect("distinct hash context must be initialized")
                .hash_values[selected];
            let row = self.backend.chunk_row(&chunk, index);
            if self.checkHasDup(key, &row, None, storage, &hash_table)? {
                continue;
            }
            let row_index = storage_selection.len();
            storage_selection.push(selected);
            hash_table
                .entry(key)
                .or_insert_with(Vec::new)
                .push(RowPointer {
                    chunk_index,
                    row_index,
                });
        }
        self.backend
            .chunk_set_selection(&mut chunk, storage_selection);
        self.hashTbl = Some(hash_table);
        Ok(chunk)
    }

    /// 在哈希桶中探测是否存在与当前行键列相等的重复行。
    pub fn checkHasDup(
        &self,
        probe_key: u64,
        row: &B::Row,
        current_chunk: Option<&B::Chunk>,
        storage: &B::Storage,
        hash_table: &HashTable,
    ) -> Result<bool, B::Error> {
        let Some(entries) = hash_table.get(&probe_key) else {
            return Ok(false);
        };
        let hash_context = self
            .hCtx
            .as_ref()
            .expect("distinct hash context must be initialized");
        for pointer in entries {
            let matched = match current_chunk {
                Some(chunk) => self.backend.chunk_row(chunk, pointer.row_index),
                None => self.backend.storage_get_row(storage, *pointer)?,
            };
            if self.backend.rows_equal(
                row,
                &matched,
                &hash_context.all_types,
                &hash_context.key_column_indexes,
            )? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// 检测相关列哈希是否变化；变化则更新并返回 true（需复位）。
    pub fn checkAndUpdateCorColHashCode(&mut self) -> bool {
        let mut changed = false;
        for (index, column) in self.corCols.iter().enumerate() {
            let hash = getCorColHashCode(self.backend.as_ref(), column);
            if hash != self.corColHashCodes[index] {
                changed = true;
                self.corColHashCodes[index] = hash;
            }
        }
        changed
    }

    /// 输出当前迭代的 iterIn/iterOut/result 存储统计。
    pub fn logTbls(
        &self,
        context: &B::Context,
        error: Option<&B::Error>,
        iteration: u64,
        level: CTELogLevel,
    ) {
        self.backend.log_cte_tables(
            context,
            error,
            iteration,
            level,
            self.backend.storage_stats(&self.iterInTbl),
            self.backend.storage_stats(
                self.iterOutTbl
                    .as_ref()
                    .expect("recursive output storage must exist"),
            ),
            self.backend.storage_stats(&self.resTbl),
        );
    }
}

/// 为 CTE 存储挂接父级内存/磁盘 tracker，并返回可选的 spill 动作。
pub fn setupCTEStorageTracker<B: CTEBackend>(
    backend: &B,
    storage: &B::Storage,
    parent_memory: &B::MemoryTracker,
    parent_disk: &B::DiskTracker,
) -> Option<B::SpillAction> {
    backend.configure_storage_trackers(
        storage,
        parent_memory,
        parent_disk,
        &backend.session_memory_tracker(),
    )
}

/// 计算相关列（correlated column）的哈希码，用于检测外层绑定是否变化。
pub fn getCorColHashCode<B: CTEBackend>(backend: &B, column: &B::CorrelatedColumn) -> Vec<u8> {
    backend.correlated_column_hash(column)
}
