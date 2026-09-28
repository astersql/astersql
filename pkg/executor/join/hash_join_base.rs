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

// Hash Join probe/build 两侧的基础协作原语。
//
// 提供共享构建状态（完成/失败/取消/spill）、probe 侧 chunk 取数与回收、
// 以及 build/probe worker 的 panic 防护包装。对应 Go `hash_join_base.go`。

use crate::joiner::Row;
use crate::row_table_builder::Chunk;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Condvar, Mutex};

/// Join worker 回传给主线程的结果：输出行或错误。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct HashJoinWorkerResult {
    /// 本批次产出的连接结果行。
    pub rows: Vec<Row>,
    /// 若 worker 失败则携带错误信息。
    pub error: Option<String>,
}

/// 构建侧状态机：构建中 / 完成 / 失败。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum BuildState {
    #[default]
    Building,
    Finished,
    Failed,
}

/// 跨 worker 共享的取消、spill 与构建完成状态。
#[derive(Debug, Default)]
struct SharedState {
    state: BuildState,
    error: Option<String>,
    cancelled: bool,
    spilled: bool,
}

/// Hash Join 共享上下文：用 Mutex+Condvar 同步 build 完成与取消。
#[derive(Clone, Debug, Default)]
pub struct HashJoinContextBase {
    shared: Arc<(Mutex<SharedState>, Condvar)>,
}

impl HashJoinContextBase {
    /// 重置为初始构建状态。
    pub fn reset(&self) {
        *self.shared.0.lock().expect("hash join state poisoned") = SharedState::default();
    }
    /// 标记构建成功完成并唤醒等待者。
    pub fn finish_build(&self) {
        let mut state = self.shared.0.lock().expect("hash join state poisoned");
        state.state = BuildState::Finished;
        self.shared.1.notify_all();
    }
    /// 标记构建失败并唤醒等待者。
    pub fn fail(&self, error: impl Into<String>) {
        let mut state = self.shared.0.lock().expect("hash join state poisoned");
        state.error = Some(error.into());
        state.state = BuildState::Failed;
        self.shared.1.notify_all();
    }
    /// 请求取消执行并唤醒等待者。
    pub fn cancel(&self) {
        let mut state = self.shared.0.lock().expect("hash join state poisoned");
        state.cancelled = true;
        self.shared.1.notify_all();
    }
    /// 标记已触发 spill（落盘）。
    pub fn set_spilled(&self) {
        self.shared
            .0
            .lock()
            .expect("hash join state poisoned")
            .spilled = true;
    }
    /// 是否已 spill。
    pub fn is_spilled(&self) -> bool {
        self.shared
            .0
            .lock()
            .expect("hash join state poisoned")
            .spilled
    }
    /// 是否已取消。
    pub fn is_cancelled(&self) -> bool {
        self.shared
            .0
            .lock()
            .expect("hash join state poisoned")
            .cancelled
    }
    /// Probe 侧等待构建侧结束；取消或失败时返回错误。
    pub fn wait_for_build_side(&self) -> Result<(), String> {
        let mut state = self
            .shared
            .0
            .lock()
            .map_err(|_| "hash join state poisoned".to_string())?;
        // 构建未完成且未取消时阻塞等待 Condvar。
        while state.state == BuildState::Building && !state.cancelled {
            state = self
                .shared
                .1
                .wait(state)
                .map_err(|_| "hash join state poisoned".to_string())?;
        }
        if state.cancelled {
            return Err("hash join cancelled".into());
        }
        match state.state {
            BuildState::Finished => Ok(()),
            BuildState::Failed => Err(state
                .error
                .clone()
                .unwrap_or_else(|| "build side failed".into())),
            BuildState::Building => unreachable!(),
        }
    }
}

/// Probe 侧可复用的 chunk 资源及其来源下标。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProbeChunkResource {
    /// 待探测的数据块。
    pub chunk: Chunk,
    /// 资源来源 worker/通道下标（用于归还）。
    pub source_index: usize,
}

/// Probe 侧取数器：从预置 chunk 队列依次取出，并支持回收清空后的 chunk。
#[derive(Debug, Default)]
pub struct ProbeSideTupleFetcherBase {
    source: VecDeque<Chunk>,
    recycled: Vec<Chunk>,
    finished: bool,
}

impl ProbeSideTupleFetcherBase {
    /// 用给定 chunk 列表构造取数器。
    pub fn new(chunks: Vec<Chunk>) -> Self {
        Self {
            source: chunks.into(),
            recycled: Vec::new(),
            finished: false,
        }
    }
    /// 取下一块 probe 数据；已取消则报错，耗尽则返回 `None`。
    pub fn fetch_next(
        &mut self,
        context: &HashJoinContextBase,
    ) -> Result<Option<ProbeChunkResource>, String> {
        if context.is_cancelled() {
            return Err("hash join cancelled".into());
        }
        if self.finished {
            return Ok(None);
        }
        match self.source.pop_front() {
            Some(chunk) => Ok(Some(ProbeChunkResource {
                source_index: 0,
                chunk,
            })),
            None => {
                self.finished = true;
                Ok(None)
            }
        }
    }
    /// 清空 chunk 并放入回收池以便复用。
    pub fn recycle(&mut self, mut resource: ProbeChunkResource) {
        resource.chunk.clear();
        self.recycled.push(resource.chunk);
    }
    /// 是否已取尽。
    pub fn is_finished(&self) -> bool {
        self.finished
    }
}

/// Probe worker 基类：持有 id 与共享上下文，并提供 panic 防护执行。
#[derive(Clone, Debug)]
pub struct ProbeWorkerBase {
    /// Worker 编号。
    pub id: usize,
    /// 共享 Hash Join 上下文。
    pub context: HashJoinContextBase,
}
impl ProbeWorkerBase {
    /// 构造 probe worker。
    pub fn new(id: usize, context: HashJoinContextBase) -> Self {
        Self { id, context }
    }
    /// 捕获 panic 并转成错误，同时标记共享上下文失败。
    pub fn run_guarded<T>(&self, action: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        catch_unwind(AssertUnwindSafe(action)).unwrap_or_else(|panic| {
            let message = panic_message(panic);
            self.context.fail(message.clone());
            Err(message)
        })
    }
}

/// Build worker 基类：取构建侧行，并在超内存限额时触发 spill 回调。
#[derive(Clone, Debug)]
pub struct BuildWorkerBase {
    /// Worker 编号。
    pub id: usize,
    /// 共享 Hash Join 上下文。
    pub context: HashJoinContextBase,
    /// 可选内存上限（字节）；超限则尝试 spill。
    pub memory_limit: Option<i64>,
}
impl BuildWorkerBase {
    /// 构造 build worker。
    pub fn new(id: usize, context: HashJoinContextBase, memory_limit: Option<i64>) -> Self {
        Self {
            id,
            context,
            memory_limit,
        }
    }
    /// 从构建侧 chunk 展平收集全部行；已取消则报错。
    pub fn fetch_build_side_rows(&self, chunks: &[Chunk]) -> Result<Vec<Row>, String> {
        if self.context.is_cancelled() {
            return Err("hash join cancelled".into());
        }
        Ok(chunks.iter().flatten().cloned().collect())
    }
    /// 若当前内存超过限额则调用 `spill` 并标记 spilled，返回是否触发了 spill。
    pub fn check_and_spill_row_table_if_needed(
        &self,
        memory_bytes: i64,
        mut spill: impl FnMut() -> Result<(), String>,
    ) -> Result<bool, String> {
        if self.memory_limit.is_some_and(|limit| memory_bytes > limit) {
            spill()?;
            self.context.set_spilled();
            return Ok(true);
        }
        Ok(false)
    }
    /// 捕获 panic 并转成错误，同时标记共享上下文失败。
    pub fn run_guarded<T>(&self, action: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
        catch_unwind(AssertUnwindSafe(action)).unwrap_or_else(|panic| {
            let message = panic_message(panic);
            self.context.fail(message.clone());
            Err(message)
        })
    }
}

/// 从 `catch_unwind` 载荷提取可读错误消息。
fn panic_message(panic: Box<dyn std::any::Any + Send>) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|message| (*message).to_string())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "hash join worker panicked".into())
}
