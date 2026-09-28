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

// 线程安全的 Lightning 任务配置队列。
//
// 以 FIFO 顺序存放待执行的 `Config`，支持按 `task_id` 查询、删除与前后调整顺序；
// `pop` 在队列为空时阻塞等待，直到有新任务或上下文取消/超时。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::Config;

/// 简化版上下文错误：取消或超过截止时间。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ContextError {
    Cancelled,
    DeadlineExceeded,
}

/// 可取消/可超时的轻量上下文，供 `List::pop` 检查是否应中止等待。
#[derive(Clone, Debug, Default)]
pub struct Context {
    done: Arc<Mutex<Option<ContextError>>>,
}

impl Context {
    /// 标记为已取消。
    pub fn cancel(&self) {
        *self.done.lock().expect("context mutex poisoned") = Some(ContextError::Cancelled);
    }

    /// 标记为已超过截止时间。
    pub fn expire(&self) {
        *self.done.lock().expect("context mutex poisoned") = Some(ContextError::DeadlineExceeded);
    }

    /// 读取当前终止原因；`None` 表示仍可继续等待。
    fn error(&self) -> Option<ContextError> {
        self.done.lock().expect("context mutex poisoned").clone()
    }
}

/// 队列内部状态：按 ID 索引的配置表、FIFO 顺序与最近分配的 ID。
#[derive(Default)]
struct State {
    entries: HashMap<i64, Arc<Mutex<Config>>>,
    order: VecDeque<i64>,
    last_id: i64,
}

/// Thread-safe FIFO supporting removal and reordering by task ID.
///
/// 线程安全 FIFO：按 task ID 支持删除与重排；用互斥锁 + 条件变量协调生产者/消费者。
pub struct List {
    state: Mutex<State>,
    changed: Condvar,
}

/// 构造空的配置任务列表。
pub fn new_config_list() -> List {
    List {
        state: Mutex::new(State::default()),
        changed: Condvar::new(),
    }
}

impl List {
    /// 将配置推入队尾，并分配单调递增的 `task_id`（基于纳秒时间戳）。
    pub fn push(&self, config: Arc<Mutex<Config>>) {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .min(i64::MAX as u128) as i64;
        let mut state = self.state.lock().expect("config list mutex poisoned");
        // 保证 ID 严格大于上次分配值，避免同纳秒冲突
        let id = now.max(state.last_id.saturating_add(1));
        config.lock().expect("config mutex poisoned").task_id = id;
        state.last_id = id;
        state.entries.insert(id, Arc::clone(&config));
        state.order.push_back(id);
        self.changed.notify_all();
    }

    /// 阻塞弹出队首配置；队列空时等待，直到有任务或上下文终止。
    pub fn pop(&self, context: &Context) -> Result<Arc<Mutex<Config>>, ContextError> {
        let mut state = self.state.lock().expect("config list mutex poisoned");
        loop {
            if let Some(id) = state.order.pop_front()
                && let Some(config) = state.entries.remove(&id)
            {
                return Ok(config);
            }
            if let Some(error) = context.error() {
                return Err(error);
            }
            // 短超时轮询，便于及时感知 cancel/expire
            (state, _) = self
                .changed
                .wait_timeout(state, Duration::from_millis(10))
                .expect("config list mutex poisoned");
        }
    }

    /// 按 `task_id` 删除任务；成功返回 `true`。
    pub fn remove(&self, task_id: i64) -> bool {
        let mut state = self.state.lock().expect("config list mutex poisoned");
        if state.entries.remove(&task_id).is_none() {
            return false;
        }
        state.order.retain(|id| *id != task_id);
        true
    }

    /// 按 `task_id` 获取仍在队列中的配置引用。
    pub fn get(&self, task_id: i64) -> Option<Arc<Mutex<Config>>> {
        self.state
            .lock()
            .expect("config list mutex poisoned")
            .entries
            .get(&task_id)
            .cloned()
    }

    /// 按当前 FIFO 顺序返回全部 task ID。
    pub fn all_ids(&self) -> Vec<i64> {
        self.state
            .lock()
            .expect("config list mutex poisoned")
            .order
            .iter()
            .copied()
            .collect()
    }

    /// 将指定任务移到队首；不存在则返回 `false`。
    pub fn move_to_front(&self, task_id: i64) -> bool {
        let mut state = self.state.lock().expect("config list mutex poisoned");
        if !state.entries.contains_key(&task_id) {
            return false;
        }
        state.order.retain(|id| *id != task_id);
        state.order.push_front(task_id);
        true
    }

    /// 将指定任务移到队尾；不存在则返回 `false`。
    pub fn move_to_back(&self, task_id: i64) -> bool {
        let mut state = self.state.lock().expect("config list mutex poisoned");
        if !state.entries.contains_key(&task_id) {
            return false;
        }
        state.order.retain(|id| *id != task_id);
        state.order.push_back(task_id);
        true
    }
}
