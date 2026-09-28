// Copyright 2023 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// 重复键检测的扫描 Worker。
//
// Lightning 导入时，外部排序（external sort）后的 KV 流可能含相同业务键的多条记录。
// 本模块用有界任务队列分片扫描键区间，发现重复后交给 `Handler` 记录；
// 扫描过程中可按中点拆分剩余区间，把工作分给空闲 Worker。

use std::collections::VecDeque;
use std::error::Error as StdError;
use std::io;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::lightning::log::filter::{Field, Level};
use crate::lightning::log::log::Logger;
use crate::util::extsort::external_sorter::{Error, ExternalSorter, Iterator as SortIterator};

use super::detector::Handler;
use super::internal::{
    InternalKey, compare_internal_key, decode_internal_key, encode_internal_key,
};

/// 待扫描的键区间任务：半开区间 `[start_key, end_key)`。
#[derive(Clone, Debug)]
pub(crate) struct Task {
    /// 区间起始内部键（含）。
    pub(crate) start_key: InternalKey,
    /// 区间结束内部键（不含）。
    pub(crate) end_key: InternalKey,
}

/// 任务队列内部状态：队列、待完成计数、关闭/中止标志。
struct QueueState {
    /// 等待被 Worker 领取的任务。
    queue: VecDeque<Task>,
    /// 尚未完成的任务数（含队列中与正在执行的），对应 Go WaitGroup。
    pending: usize,
    /// 正常关闭：不再接受新任务，队列空后退出。
    closed: bool,
    /// 异常中止：清空队列并唤醒所有等待者。
    aborted: bool,
}

/// A bounded-work queue with the WaitGroup accounting used by the Go version.
/// 有界任务队列：容量语义为 1（队列非空时拒绝拆分），并用 pending 模拟 WaitGroup。
pub(crate) struct TaskQueue {
    state: Mutex<QueueState>,
    /// 队列或 pending 变化时唤醒等待的 Worker / 协调者。
    changed: Condvar,
}

/// `receive` 的三种结果：拿到任务、队列已关闭、上下文已取消。
enum Receive {
    Task(Task),
    Closed,
    Canceled,
}

impl TaskQueue {
    /// 以单个初始任务构造队列，pending 置为 1。
    pub(crate) fn with_initial(task: Task) -> Self {
        Self {
            state: Mutex::new(QueueState {
                queue: VecDeque::from([task]),
                pending: 1,
                closed: false,
                aborted: false,
            }),
            changed: Condvar::new(),
        }
    }

    /// 阻塞领取任务；短超时轮询以便响应取消。
    fn receive(&self, ctx: &CancellationToken) -> Receive {
        let mut state = self.state.lock().expect("duplicate task queue poisoned");
        loop {
            if let Some(task) = state.queue.pop_front() {
                return Receive::Task(task);
            }
            if state.closed || state.aborted {
                return Receive::Closed;
            }
            if ctx.is_cancelled() {
                return Receive::Canceled;
            }
            // 短超时等待，避免永久阻塞在条件变量上而忽略取消信号。
            let (next, _) = self
                .changed
                .wait_timeout(state, Duration::from_millis(10))
                .expect("duplicate task queue poisoned");
            state = next;
        }
    }

    /// 标记一个任务完成；pending 归零时唤醒 `wait_until_idle`。
    fn complete_task(&self) {
        let mut state = self.state.lock().expect("duplicate task queue poisoned");
        if state.pending > 0 {
            state.pending -= 1;
        }
        if state.pending == 0 {
            self.changed.notify_all();
        }
    }

    /// Mirrors `len(taskCh) == 0` followed by a non-blocking send to a channel
    /// of capacity one. The pending count is incremented only on success.
    /// 尝试把剩余区间拆成新任务入队；仅当队列空且未关闭时成功。
    fn try_split(&self, task: Task) -> bool {
        let mut state = self.state.lock().expect("duplicate task queue poisoned");
        if state.closed || state.aborted || !state.queue.is_empty() {
            return false;
        }
        state.queue.push_back(task);
        state.pending += 1;
        self.changed.notify_one();
        true
    }

    /// 等待所有 pending 任务完成或队列被 abort。
    pub(crate) fn wait_until_idle(&self) {
        let mut state = self.state.lock().expect("duplicate task queue poisoned");
        while state.pending != 0 && !state.aborted {
            state = self
                .changed
                .wait(state)
                .expect("duplicate task queue poisoned");
        }
    }

    /// 正常关闭：不再接受拆分，现有任务仍可完成。
    pub(crate) fn close(&self) {
        let mut state = self.state.lock().expect("duplicate task queue poisoned");
        state.closed = true;
        self.changed.notify_all();
    }

    /// Constructor or worker failure must drain the outstanding accounting just
    /// like the Go goroutine that drains `taskCh` after `errgroup.Wait`.
    /// 构造或 Worker 失败时清空队列并清零 pending，强制唤醒等待方。
    pub(crate) fn abort(&self) {
        let mut state = self.state.lock().expect("duplicate task queue poisoned");
        state.queue.clear();
        state.pending = 0;
        state.aborted = true;
        state.closed = true;
        self.changed.notify_all();
    }
}

/// 重复键扫描 Worker：从外部排序器迭代键区间，经 Handler 上报重复组。
pub(crate) struct Worker {
    /// 外部排序器，提供按序迭代已排序 KV。
    pub(crate) sorter: Arc<dyn ExternalSorter>,
    /// 共享任务队列，用于领取与拆分扫描区间。
    pub(crate) tasks: Arc<TaskQueue>,
    /// 发现的重复键组计数（跨 Worker 原子累加）。
    pub(crate) num_dups: Arc<AtomicI64>,
    /// 重复组回调：begin/append/end/close。
    pub(crate) handler: Box<dyn Handler>,
    /// 结构化日志记录器。
    pub(crate) logger: Logger,
}

impl Worker {
    /// 主循环：领取任务并执行，直到队列关闭或上下文取消。
    pub(crate) fn run(&mut self, ctx: &CancellationToken) -> Result<(), Error> {
        loop {
            match self.tasks.receive(ctx) {
                Receive::Task(task) => self.run_task(ctx, task)?,
                Receive::Closed => return Ok(()),
                Receive::Canceled => {
                    return Err(
                        io::Error::new(io::ErrorKind::Interrupted, "context canceled").into(),
                    );
                }
            }
        }
    }

    /// 执行单个区间任务：扫描、完成记账、关闭 Handler，并记录起止日志。
    fn run_task(&mut self, ctx: &CancellationToken, mut task: Task) -> Result<(), Error> {
        let log_task = self
            .logger
            .With([
                Field::string("startKey", task.start_key.to_string()),
                Field::string("initialEndKey", task.end_key.to_string()),
            ])
            .Begin(Level::Info, "run task");
        let mut processed_keys = 0i64;

        let result = self.scan_task(ctx, &mut task, &mut processed_keys);
        self.tasks.complete_task();
        // Handler.close 失败时覆盖扫描成功结果，确保资源释放错误可上报。
        let close_result = self.handler.close();
        let result = match (result, close_result) {
            (Ok(()), Err(error)) => Err(error),
            (body, _) => body,
        };
        let log_error = result
            .as_ref()
            .err()
            .map(|error| error.as_ref() as &(dyn StdError + 'static));
        log_task.End(
            Level::Error,
            log_error,
            [
                Field::string("endKey", task.end_key.to_string()),
                Field::int("processedKeys", processed_keys),
            ],
        );
        result
    }

    /// 创建排序迭代器并扫描任务区间，最后关闭迭代器。
    fn scan_task(
        &mut self,
        ctx: &CancellationToken,
        task: &mut Task,
        processed_keys: &mut i64,
    ) -> Result<(), Error> {
        let mut iterator = self.sorter.new_iterator(ctx)?;
        let scan_result = self.scan_iterator(ctx, task, iterator.as_mut(), processed_keys);
        let _ = iterator.close();
        scan_result
    }

    /// 在 `[start, end)` 上滑动比较相邻业务键，检出重复组并可拆分剩余区间。
    fn scan_iterator(
        &mut self,
        ctx: &CancellationToken,
        task: &mut Task,
        iterator: &mut dyn SortIterator,
        processed_keys: &mut i64,
    ) -> Result<(), Error> {
        // 每处理这么多键检查一次取消，并尝试拆分剩余区间。
        const CHECK_INTERVAL: usize = 1000;
        let mut iterations = 0usize;
        // 当前是否处于同一业务键的重复组内。
        let mut in_duplicate = false;
        let mut previous_key = InternalKey::default();
        let mut current_key = InternalKey::default();
        let mut encoded_start = Vec::new();
        encode_internal_key(&mut encoded_start, &task.start_key);

        iterator.seek(&encoded_start);
        while iterator.valid() {
            decode_internal_key(iterator.unsafe_key(), &mut current_key)?;
            // 到达区间上界（不含）则停止本任务扫描。
            if compare_internal_key(&current_key, &task.end_key) >= 0 {
                break;
            }
            *processed_keys += 1;

            // 相邻业务键相同：开启或延续重复组；否则若刚离开重复组则 end。
            if current_key.key == previous_key.key {
                if in_duplicate {
                    self.handler.append(&current_key.key_id)?;
                } else {
                    self.handler.begin(&current_key.key)?;
                    self.handler.append(&previous_key.key_id)?;
                    self.handler.append(&current_key.key_id)?;
                    in_duplicate = true;
                    self.num_dups.fetch_add(1, Ordering::Relaxed);
                }
            } else if in_duplicate {
                self.handler.end()?;
                in_duplicate = false;
            }

            iterations += 1;
            if iterations % CHECK_INTERVAL == 0 {
                if ctx.is_cancelled() {
                    return Err(
                        io::Error::new(io::ErrorKind::Interrupted, "context canceled").into(),
                    );
                }
                // 在当前键与任务 end 之间生成拆分键；入队成功则收缩本任务上界。
                let user_split_key = gen_split_key(&current_key.key, &task.end_key.key);
                if user_split_key.as_slice() > current_key.key.as_slice() {
                    let split_key = InternalKey::new(user_split_key, Vec::new());
                    let new_task = Task {
                        start_key: split_key.clone(),
                        end_key: task.end_key.clone(),
                    };
                    if self.tasks.try_split(new_task) {
                        task.end_key = split_key;
                    }
                }
            }

            std::mem::swap(&mut previous_key, &mut current_key);
            iterator.next();
        }
        if let Some(error) = iterator.take_error() {
            return Err(error);
        }
        // 迭代结束时若仍在重复组内，补发 end。
        if in_duplicate {
            self.handler.end()?;
        }
        Ok(())
    }
}

/// 在 `start_key` 与 `end_key` 之间生成一个字典序居中的拆分键（用户键字节）。
///
/// 用于把大区间切成更小任务；若两键相等则原样返回。
pub fn gen_split_key(start_key: &[u8], end_key: &[u8]) -> Vec<u8> {
    if start_key == end_key {
        return start_key.to_vec();
    }

    let prefix_len = common_prefix_len(start_key, end_key);
    let mut split_key = start_key[..prefix_len].to_vec();
    // start 是 end 的前缀：在分歧处取 end 字节的一半作为下一字节。
    if prefix_len == start_key.len() {
        split_key.push(end_key[prefix_len] / 2);
        return split_key;
    }

    let (first, second) = (start_key[prefix_len], end_key[prefix_len]);
    // 分歧字节之间有空隙：取中点字节。
    // Match Go's uint8 arithmetic, including descending non-prefix inputs.
    if first.wrapping_add(1) < second {
        split_key.push(first.wrapping_add(second.wrapping_sub(first) / 2));
        return split_key;
    }
    // 相邻字节：沿 start 后续补 0xff，直到找到可抬升的位置。
    split_key.push(first);
    for &byte in &start_key[prefix_len + 1..] {
        split_key.push(0xff);
        if byte != 0xff {
            return split_key;
        }
    }
    split_key.push(0xff);
    split_key
}

/// 计算两个字节切片的公共前缀长度。
fn common_prefix_len(left: &[u8], right: &[u8]) -> usize {
    left.iter()
        .zip(right)
        .position(|(left, right)| left != right)
        .unwrap_or_else(|| left.len().min(right.len()))
}
