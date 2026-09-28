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

// 暂停/恢复协调器：在 Lightning 导入过程中阻塞 worker，直到 Resume 或上下文取消。
//
// 提供简化版 `Context`（取消与超时）以及 `Pauser`（条件变量实现的暂停门闩），
// 用于流量控制、运维暂停导入等场景。

use crate::CommonError;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// 运行中状态常量（与 Go 侧 pauseState 枚举对应）。
pub const pauseStateRunning: u32 = 0;
/// 已暂停状态常量。
pub const pauseStatePaused: u32 = 1;
/// 已锁定状态常量。
pub const pauseStateLocked: u32 = 2;

/// 简化上下文：支持取消标志与可选截止时间（deadline）。
///
/// 用于在 `Pauser::Wait` 中可中断地等待恢复；对应 Go 的 `context.Context`。
#[derive(Clone, Default)]
pub struct Context {
    /// 是否已被取消。
    cancelled: Arc<AtomicBool>,
    /// 可选截止时间；超时后 `Done` 为真。
    deadline: Option<Instant>,
}

impl Context {
    /// 创建永不超时、未取消的后台上下文。
    pub fn Background() -> Self {
        Self::default()
    }

    /// 创建带超时的上下文：从现在起经过 `timeout` 后视为截止。
    pub fn WithTimeout(timeout: Duration) -> Self {
        Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            deadline: Some(Instant::now() + timeout),
        }
    }

    /// 标记上下文已取消，使等待方尽快退出。
    pub fn Cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// 是否已结束：被取消或已超过截止时间。
    pub fn Done(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
            || self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
    }

    /// 返回结束原因对应的错误：取消或 deadline exceeded。
    pub fn Err(&self) -> CommonError {
        if self.cancelled.load(Ordering::Acquire) {
            CommonError::new("context", "context canceled")
        } else {
            CommonError::new("context", "context deadline exceeded")
        }
    }
}

/// `Pauser` 内部共享状态：是否暂停及等待者计数。
struct PauseData {
    /// 当前是否处于暂停。
    paused: bool,
    /// 已完成的暂停周期编号；每次有效的 `Resume` 都会推进一代。
    generation: u64,
    /// 正在 `Wait` 中的协程/线程数量。
    waiters: usize,
}

/// 暂停门闩：`Pause` 后 `Wait` 阻塞，`Resume` 唤醒全部等待者。
pub struct Pauser {
    /// 保护暂停标志与等待者计数的互斥锁。
    state: Mutex<PauseData>,
    /// 恢复时通知所有等待线程的条件变量。
    resumed: Condvar,
}

/// 构造初始为未暂停的 `Pauser`。
pub fn NewPauser() -> Pauser {
    Pauser {
        state: Mutex::new(PauseData {
            paused: false,
            generation: 0,
            waiters: 0,
        }),
        resumed: Condvar::new(),
    }
}

impl Pauser {
    /// 进入暂停：之后调用 `Wait` 的线程将被阻塞。
    pub fn Pause(&self) {
        self.state.lock().expect("Pauser mutex poisoned").paused = true;
    }

    /// 若当前已暂停则清除暂停标志并唤醒全部等待者；否则无操作。
    pub fn Resume(&self) {
        let mut state = self.state.lock().expect("Pauser mutex poisoned");
        if !state.paused {
            return;
        }
        state.paused = false;
        state.generation = state.generation.wrapping_add(1);
        self.resumed.notify_all();
    }

    /// 查询当前是否处于暂停状态。
    pub fn IsPaused(&self) -> bool {
        self.state.lock().expect("Pauser mutex poisoned").paused
    }

    /// 若未暂停立即返回；若已暂停则等待 `Resume`，或在上下文结束时返回错误。
    ///
    /// 等待时按最多 20ms 的短超时轮询 `ctx.Done()`，以便及时响应取消/截止。
    pub fn Wait(&self, ctx: &Context) -> Result<(), CommonError> {
        let mut state = self.state.lock().expect("Pauser mutex poisoned");
        // 未暂停：直接放行
        if !state.paused {
            return Ok(());
        }
        let wait_generation = state.generation;
        state.waiters += 1;
        while state.paused && state.generation == wait_generation {
            // 上下文已结束：减少等待者计数并返回取消/超时错误
            if ctx.Done() {
                self.cancel(&mut state);
                return Err(ctx.Err());
            }
            // 计算本次条件变量等待时长，上限 20ms 以便轮询 Done
            let wait_for = ctx
                .deadline
                .map(|deadline| deadline.saturating_duration_since(Instant::now()))
                .unwrap_or(Duration::from_millis(20))
                .min(Duration::from_millis(20));
            let (next, _) = self
                .resumed
                .wait_timeout(state, wait_for)
                .expect("Pauser mutex poisoned");
            state = next;
        }
        state.waiters -= 1;
        Ok(())
    }

    /// 上下文取消时减少等待者计数（不改变 paused 标志）。
    fn cancel(&self, state: &mut PauseData) {
        state.waiters -= 1;
    }
}
