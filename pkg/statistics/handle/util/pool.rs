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

// 统计后台任务线程池与会话池封装。
//
// `GoroutinePool` 模拟 Go 侧有界 goroutine 池：按需扩容 worker、空闲超时回收；
// `StatsPool` 同时持有该线程池与会话池，供统计异步任务复用。

use crate::util::{SessionPool, StatsError};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;

/// 统计 worker 数量上限（与 Go `math.MaxInt16` 对齐）。
pub const MAX_STATS_WORKERS: usize = i16::MAX as usize;
/// worker 空闲多久无任务则退出，默认 60 秒。
pub const STATS_WORKER_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// 提交到池中执行的一次性任务闭包。
type Job = Box<dyn FnOnce() + Send + 'static>;

/// 有界任务线程池：通过 channel 分发 Job，动态增减 worker。
pub struct GoroutinePool {
    state: Arc<(Mutex<PoolState>, Condvar)>,
    workers: Arc<AtomicUsize>,
    idle_workers: Arc<AtomicUsize>,
    maximum: usize,
    idle_timeout: Duration,
}

#[derive(Default)]
struct PoolState {
    jobs: VecDeque<Job>,
    waiting: usize,
    closed: bool,
}

struct WorkerGuard(Arc<AtomicUsize>);

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl GoroutinePool {
    /// 创建池：`maximum` 为最多保留的空闲 worker 数，`idle_timeout` 为空闲退出阈值。
    pub fn new(maximum: usize, idle_timeout: Duration) -> Self {
        Self {
            state: Arc::new((Mutex::new(PoolState::default()), Condvar::new())),
            workers: Arc::new(AtomicUsize::new(0)),
            idle_workers: Arc::new(AtomicUsize::new(0)),
            maximum,
            idle_timeout,
        }
    }

    /// 提交任务；池已关闭时与 Go `gp.Pool.Go` 一样静默忽略。
    pub fn submit(&self, job: impl FnOnce() + Send + 'static) -> Result<(), StatsError> {
        let job = Box::new(job) as Job;
        let (lock, available) = &*self.state;
        let mut state = lock.lock().unwrap();
        if state.closed {
            return Ok(());
        }
        if state.waiting != 0 {
            state.jobs.push_back(job);
            available.notify_one();
            return Ok(());
        }
        drop(state);

        self.workers.fetch_add(1, Ordering::AcqRel);
        let state = Arc::clone(&self.state);
        let workers = Arc::clone(&self.workers);
        let idle_workers = Arc::clone(&self.idle_workers);
        let maximum = self.maximum;
        let idle_timeout = self.idle_timeout;
        thread::spawn(move || {
            let _worker_guard = WorkerGuard(workers);
            job();

            if idle_workers
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                    (current < maximum).then_some(current + 1)
                })
                .is_err()
            {
                return;
            }

            loop {
                let (lock, available) = &*state;
                let mut pool = lock.lock().unwrap();
                pool.waiting += 1;
                if idle_timeout.is_zero() {
                    while pool.jobs.is_empty() && !pool.closed {
                        pool = available.wait(pool).unwrap();
                    }
                } else {
                    let (next, timed_out) = available
                        .wait_timeout_while(pool, idle_timeout, |pool| {
                            pool.jobs.is_empty() && !pool.closed
                        })
                        .unwrap();
                    pool = next;
                    if timed_out.timed_out() && pool.jobs.is_empty() {
                        pool.waiting -= 1;
                        break;
                    }
                }
                pool.waiting -= 1;
                if let Some(job) = pool.jobs.pop_front() {
                    drop(pool);
                    job();
                } else if pool.closed {
                    break;
                }
            }
            idle_workers.fetch_sub(1, Ordering::AcqRel);
        });
        Ok(())
    }

    /// 当前存活的 worker 数量。
    pub fn worker_count(&self) -> usize {
        self.workers.load(Ordering::Acquire)
    }

    /// 关闭池并唤醒空闲 worker；运行中的任务继续完成，调用本身不等待。
    pub fn close(&self) {
        let (lock, available) = &*self.state;
        lock.lock().unwrap().closed = true;
        available.notify_all();
    }
}

impl Drop for GoroutinePool {
    fn drop(&mut self) {
        self.close();
    }
}

/// 统计子系统资源池：线程池 + 会话池。
pub trait Pool: Send + Sync {
    fn g_pool(&self) -> Arc<GoroutinePool>;
    fn s_pool(&self) -> Arc<dyn SessionPool>;
    fn close(&self);
}

/// 默认 `Pool` 实现，使用最大 worker 数与默认空闲超时。
pub struct StatsPool {
    goroutine_pool: Arc<GoroutinePool>,
    session_pool: Arc<dyn SessionPool>,
}

impl StatsPool {
    /// 用给定会话池构造，内部创建默认参数的 `GoroutinePool`。
    pub fn new(session_pool: Arc<dyn SessionPool>) -> Self {
        Self {
            goroutine_pool: Arc::new(GoroutinePool::new(
                MAX_STATS_WORKERS,
                STATS_WORKER_IDLE_TIMEOUT,
            )),
            session_pool,
        }
    }
}

impl Pool for StatsPool {
    fn g_pool(&self) -> Arc<GoroutinePool> {
        Arc::clone(&self.goroutine_pool)
    }

    fn s_pool(&self) -> Arc<dyn SessionPool> {
        Arc::clone(&self.session_pool)
    }

    fn close(&self) {
        self.goroutine_pool.close();
    }
}

/// 构造 trait 对象形式的统计资源池。
pub fn new_pool(session_pool: Arc<dyn SessionPool>) -> Arc<dyn Pool> {
    Arc::new(StatsPool::new(session_pool))
}
