// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// ingest 测试替身（mock）：模拟后端上下文与写入引擎。
//
// 在单元测试中无需真实本地引擎即可验证回填分片（chunk）、检查点水位线
// 以及写入路径；`MockEngineInfo` 把写入的键值对保存在内存，并可挂接 hook。

use crate::backend::BackendContext;
use crate::checkpoint::Key;
use crate::engine::{Engine, Writer};
use std::sync::{Arc, Mutex};
/// 包装真实 `BackendContext`，并额外记录各分片写入计数，便于断言。
pub struct MockBackendContext {
    /// 被测的后端上下文。
    pub backend: BackendContext,
    /// 每次 `update_chunk` 记录的 `(chunk_id, count)` 历史。
    pub written_chunks: Vec<(usize, usize)>,
}
impl MockBackendContext {
    /// 用给定后端上下文创建 mock 包装。
    pub fn new(backend: BackendContext) -> Self {
        Self {
            backend,
            written_chunks: Vec::new(),
        }
    }
    /// 触发后端执行 ingest（导入）流程。
    pub fn ingest(&mut self) -> Result<(), String> {
        self.backend.ingest()
    }
    /// 返回检查点上的下一起始键（水位线）。
    pub fn next_start_key(&self) -> Key {
        self.backend.next_start_key()
    }
    pub fn total_key_count(&self) -> u64 {
        self.backend.total_key_count()
    }
    /// 注册一个回填分片及其结束键。
    pub fn add_chunk(&mut self, id: usize, end: Key) {
        self.backend.add_chunk(id, end);
    }
    /// 更新分片进度，并记录到 `written_chunks`。
    pub fn update_chunk(&mut self, id: usize, count: usize, done: bool) {
        self.backend.update_chunk(id, count, done);
        self.written_chunks.push((id, count));
    }
    pub fn finish_chunk(&mut self, id: usize, count: usize) {
        self.backend.finish_chunk(id, count);
    }
    pub fn import_ts(&self) -> u64 {
        self.backend.import_ts()
    }
    pub fn advance_watermark(&mut self, imported: bool) -> Result<(), String> {
        self.backend.advance_watermark(imported)
    }
}
/// 写入时可选回调：收到每一行的 key/value。
type WriteHook = Arc<dyn Fn(&[u8], &[u8]) + Send + Sync>;
/// 内存版 mock 引擎：缓冲写入行，实现 `Engine` 接口。
pub struct MockEngineInfo {
    /// 对应的索引 ID。
    index_id: i64,
    /// 已写入的键值对缓冲。
    rows: Arc<Mutex<Vec<(Vec<u8>, Vec<u8>)>>>,
    /// 可选写入钩子。
    hook: Arc<Mutex<Option<WriteHook>>>,
}
impl MockEngineInfo {
    /// 创建指定索引 ID 的空 mock 引擎。
    pub fn new(index_id: i64) -> Self {
        Self {
            index_id,
            rows: Arc::new(Mutex::new(Vec::new())),
            hook: Arc::new(Mutex::new(None)),
        }
    }
    /// 设置写入钩子，每次 `write_row` 时回调。
    pub fn set_hook(&self, hook: WriteHook) {
        *self.hook.lock().unwrap() = Some(hook);
    }
    /// 返回当前缓冲的全部键值对副本。
    pub fn rows(&self) -> Vec<(Vec<u8>, Vec<u8>)> {
        self.rows.lock().unwrap().clone()
    }
}
impl Engine for MockEngineInfo {
    fn flush(&self) -> Result<(), String> {
        Ok(())
    }
    fn close(&self, cleanup: bool) {
        // Go MockEngineInfo.Close is intentionally a no-op, including cleanup=true.
        let _ = cleanup;
    }
    fn create_writer(&self, _worker_id: usize) -> Result<Box<dyn Writer>, String> {
        Ok(Box::new(MockWriter {
            rows: Arc::clone(&self.rows),
            hook: Arc::clone(&self.hook),
        }))
    }
    fn index_id(&self) -> i64 {
        self.index_id
    }
}
/// mock 写入器：把行追加到共享缓冲，并累计写入字节数。
struct MockWriter {
    /// 与引擎共享的行缓冲。
    rows: Arc<Mutex<Vec<(Vec<u8>, Vec<u8>)>>>,
    /// 与引擎共享的可选写入钩子。
    hook: Arc<Mutex<Option<WriteHook>>>,
}
impl Writer for MockWriter {
    fn write_row(&mut self, key: &[u8], value: &[u8]) -> Result<(), String> {
        // Go hook replaces the transaction write path rather than observing it.
        if let Some(hook) = self.hook.lock().unwrap().as_ref() {
            hook(key, value);
            return Ok(());
        }
        self.rows
            .lock()
            .unwrap()
            .push((key.to_vec(), value.to_vec()));
        Ok(())
    }
    fn written_bytes(&self) -> i64 {
        0
    }
}
