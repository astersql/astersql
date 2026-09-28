// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// DDL ingest（数据摄入）引擎模块。
//
// DDL（Data Definition Language，数据定义语言，如建表、加索引）在执行如
// “添加索引”这类操作时，会把大量索引键值对先写入本地的 ingest 引擎，再批量
// 导入存储层，以提升回填（backfill）性能。本模块提供了该引擎的内存版实现：
// - `Engine`：引擎抽象，负责管理索引数据、刷盘（flush）与关闭；
// - `Writer`：写入器抽象，供各回填 worker 并发写入行数据；
// - `EngineInfo` / `WriterContext`：基于 `BTreeMap` 的具体实现，
//   并通过 `MemRoot` 与 `ResourceTracker` 跟踪内存与磁盘占用。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::disk_root::ResourceTracker;
use crate::mem_root::MemRoot;

/// 写入器抽象：向 ingest 引擎写入单行索引键值对。
///
/// 一个引擎可创建多个写入器，供不同回填 worker 并发使用。
pub trait Writer: Send {
    /// 写入一条键值对（key/value 均为编码后的字节序列）。
    fn write_row(&mut self, key: &[u8], value: &[u8]) -> Result<(), String>;
    /// 返回该写入器累计写入的字节数。
    fn written_bytes(&self) -> i64;
}
/// ingest 引擎抽象：管理某个索引的数据缓冲、刷盘与生命周期。
pub trait Engine: Send + Sync {
    /// 将缓冲的数据刷盘（flush），标记数据已就绪待导入存储层。
    fn flush(&self) -> Result<(), String>;
    /// 关闭引擎；`cleanup` 为 true 时清空已缓冲的数据。
    fn close(&self, cleanup: bool);
    /// 为指定 worker 创建一个写入器，同时预留其内存配额。
    fn create_writer(&self, worker_id: usize) -> Result<Box<dyn Writer>, String>;
    /// 返回该引擎对应的索引 ID。
    fn index_id(&self) -> i64;
}

/// 引擎的可变内部状态，由 `Mutex` 保护以支持多写入器并发访问。
#[derive(Default)]
struct EngineState {
    /// 已缓冲的索引键值对，用有序的 `BTreeMap` 保存以便按 key 有序导入。
    rows: BTreeMap<Vec<u8>, Vec<u8>>,
    /// 当前存活的写入器数量。
    writers: usize,
    /// 引擎是否已关闭。
    closed: bool,
    /// 数据是否已刷盘。
    flushed: bool,
}

/// ingest 引擎的具体实现，对应某个待构建的索引。
pub struct EngineInfo {
    /// 索引 ID。
    index_id: i64,
    /// 该索引是否为唯一索引（unique）。
    unique: bool,
    /// 内存跟踪标签，用于在 `MemRoot` 中标识本引擎占用的内存。
    tag: String,
    /// 共享的内部状态。
    state: Arc<Mutex<EngineState>>,
    /// 内存根：统一跟踪与限制内存使用。
    mem_root: Arc<dyn MemRoot>,
    /// 单个写入器预留的内存字节数。
    writer_memory: i64,
}

impl EngineInfo {
    /// 创建一个新的 ingest 引擎实例。
    ///
    /// `writer_memory` 会被约束为非负值，避免出现负的内存配额。
    pub fn new(
        index_id: i64,
        unique: bool,
        tag: impl Into<String>,
        mem_root: Arc<dyn MemRoot>,
        writer_memory: i64,
    ) -> Self {
        Self {
            index_id,
            unique,
            tag: tag.into(),
            state: Arc::new(Mutex::new(EngineState::default())),
            mem_root,
            writer_memory: writer_memory.max(0),
        }
    }
    /// 返回当前已缓冲行的一个克隆副本（供导入或测试检查）。
    pub fn rows(&self) -> BTreeMap<Vec<u8>, Vec<u8>> {
        self.state.lock().unwrap().rows.clone()
    }
    /// 返回该引擎对应的索引是否为唯一索引。
    pub fn unique(&self) -> bool {
        self.unique
    }
}

impl Engine for EngineInfo {
    fn flush(&self) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        // 引擎已关闭则不允许再刷盘。
        if state.closed {
            return Err("engine closed".into());
        }
        state.flushed = true;
        Ok(())
    }
    fn close(&self, cleanup: bool) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        // cleanup 为 true 时丢弃缓冲数据，释放内存占用。
        if cleanup {
            state.rows.clear();
        }
        // 归还本引擎在 MemRoot 中登记的内存。
        self.mem_root.release_with_tag(&self.tag);
    }
    fn create_writer(&self, worker_id: usize) -> Result<Box<dyn Writer>, String> {
        // Keep the lifecycle check and writer registration under the same lock so
        // `close` cannot race with creation and leave a writer attached to a
        // closed engine.
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return Err("engine closed".into());
        }
        // 先检查内存配额是否足够容纳新写入器，不足则拒绝创建。
        if !self.mem_root.check_consume(self.writer_memory) {
            return Err("memory used up".into());
        }
        // 为该写入器生成唯一标签并登记其内存占用。
        let tag = format!("{}-writer-{worker_id}", self.tag);
        self.mem_root.consume_with_tag(&tag, self.writer_memory);
        state.writers += 1;
        drop(state);
        Ok(Box::new(WriterContext {
            state: Arc::clone(&self.state),
            mem_root: Arc::clone(&self.mem_root),
            tag,
            bytes: 0,
        }))
    }
    fn index_id(&self) -> i64 {
        self.index_id
    }
}
impl ResourceTracker for EngineInfo {
    /// 估算引擎当前的磁盘/空间占用：所有缓冲键值对的字节数之和。
    fn disk_usage(&self) -> u64 {
        self.state
            .lock()
            .unwrap()
            .rows
            .iter()
            .map(|(key, value)| key.len() as u64 + value.len() as u64)
            .sum()
    }
}

/// 写入器的上下文，持有对共享引擎状态与内存根的引用。
struct WriterContext {
    /// 共享的引擎内部状态。
    state: Arc<Mutex<EngineState>>,
    /// 内存根，用于在写入器销毁时释放其内存配额。
    mem_root: Arc<dyn MemRoot>,
    /// 本写入器的内存跟踪标签。
    tag: String,
    /// 本写入器累计写入的字节数。
    bytes: i64,
}
impl WriterContext {
    /// 获取写入锁，返回受保护的引擎状态守卫。
    pub fn lock_for_write(&self) -> MutexGuard<'_, EngineState> {
        self.state.lock().unwrap()
    }
}
impl Writer for WriterContext {
    fn write_row(&mut self, key: &[u8], value: &[u8]) -> Result<(), String> {
        let mut state = self.state.lock().unwrap();
        // 引擎已关闭则拒绝写入。
        if state.closed {
            return Err("engine closed".into());
        }
        // 插入键值对（相同 key 会覆盖），并累加已写字节数（saturating 防溢出）。
        state.rows.insert(key.to_vec(), value.to_vec());
        self.bytes = self.bytes.saturating_add((key.len() + value.len()) as i64);
        Ok(())
    }
    fn written_bytes(&self) -> i64 {
        self.bytes
    }
}
impl Drop for WriterContext {
    /// 写入器销毁时释放其内存配额，并将存活写入器计数减一。
    fn drop(&mut self) {
        if let Ok(mut state) = self.state.lock() {
            state.writers = state.writers.saturating_sub(1);
        }
        self.mem_root.release_with_tag(&self.tag);
    }
}
