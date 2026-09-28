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

// spool：受资源管理器管控的简易线程池实现。
//
// 对应 Go `spool.go`：支持单任务 `run`、多并发 `run_with_concurrency`、
// 动态调容 `tune`，以及阻塞/非阻塞提交。池在创建时向 InstanceResourceManager 注册。

use crate::pool::{BasePool, ERR_POOL_CLOSED, ERR_POOL_OVERLOAD, ERR_POOL_PARAMS_INVALID};
use crate::poolmanager::{Meta, MetaEvent, TaskChannel, TaskManager};
use crate::resourcemanager::InstanceResourceManager;
use crate::util::{Component, GoroutinePool};
use crate::{OptionFn, Options, load_options};
use prometheus::{Gauge, Opts};
use std::error::Error as StdError;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

/// 满载阻塞时轮询空闲槽位的休眠间隔。
const WAIT_INTERVAL: Duration = Duration::from_millis(5);

/// spool 对外错误枚举，对应 Go 侧池参数/关闭/过载等错误字符串。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// 容量等参数非法（例如 size == 0）。
    InvalidParams,
    /// 池已停止，拒绝新任务。
    Closed,
    /// 无空闲并发且非阻塞模式下拒绝提交。
    Overload,
    /// 向资源管理器注册失败（同名池已存在等）。
    Registration,
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidParams => ERR_POOL_PARAMS_INVALID,
            Self::Closed => ERR_POOL_CLOSED,
            Self::Overload => ERR_POOL_OVERLOAD,
            Self::Registration => "pool is already exist",
        })
    }
}

impl StdError for Error {}

/// 池的共享内部状态：容量、运行/等待计数、任务管理器与线程句柄。
struct PoolInner {
    /// 序列化调容与容量检查，避免并发读改写竞态。
    admission: Mutex<()>,
    /// release 等待「无等待者」时的条件变量。
    waiting_changed: Condvar,
    /// 保证同一时刻只有一次 release_and_wait。
    release_lock: Mutex<()>,
    options: Options,
    /// 创建时的原始并发度，供 GetOriginConcurrency 查询。
    origin_capacity: i32,
    capacity: AtomicI32,
    running: AtomicI32,
    waiting: AtomicI32,
    is_stop: AtomicBool,
    threads: Mutex<Vec<JoinHandle<()>>>,
    /// 登记多并发任务元数据，配合 Overclock/Downclock 调容。
    task_manager: TaskManager,
    concurrency_metric: Gauge,
    base: BasePool,
}

/// 可克隆的线程池句柄；内部通过 Arc 共享 PoolInner。
#[derive(Clone)]
pub struct Pool {
    inner: Arc<PoolInner>,
}

impl fmt::Debug for Pool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Pool")
            .field("name", &self.name())
            .field("capacity", &self.cap())
            .field("running", &self.running())
            .finish()
    }
}

/// RAII：进入等待态时 +1 waiting，离开时 -1 并唤醒 release 等待者。
struct WaitingGuard<'a> {
    inner: &'a PoolInner,
}

impl<'a> WaitingGuard<'a> {
    fn new(inner: &'a PoolInner) -> Self {
        inner.waiting.fetch_add(1, Ordering::SeqCst);
        Self { inner }
    }
}

impl Drop for WaitingGuard<'_> {
    fn drop(&mut self) {
        let _admission = self.inner.admission.lock().unwrap();
        self.inner.waiting.fetch_sub(1, Ordering::SeqCst);
        self.inner.waiting_changed.notify_all();
    }
}

impl Pool {
    /// 创建指定容量的池并注册到 InstanceResourceManager。
    /// Component 表示所属子系统，供资源管理器按组件调度。
    pub fn new(
        name: String,
        size: i32,
        component: Component,
        options: &[OptionFn],
    ) -> Result<Self, Error> {
        if size == 0 {
            return Err(Error::InvalidParams);
        }

        let mut base = BasePool::new();
        base.set_name(name.clone());
        let metric = Gauge::with_opts(
            Opts::new(
                "tidb_rm_pool_concurrency",
                "How many concurrency in the pool",
            )
            .const_label("pool", name.clone()),
        )
        .expect("the pool concurrency metric descriptor is valid");
        metric.set(size as f64);

        let pool = Self {
            inner: Arc::new(PoolInner {
                admission: Mutex::new(()),
                waiting_changed: Condvar::new(),
                release_lock: Mutex::new(()),
                options: load_options(options),
                origin_capacity: size,
                capacity: AtomicI32::new(size),
                running: AtomicI32::new(0),
                waiting: AtomicI32::new(0),
                is_stop: AtomicBool::new(false),
                threads: Mutex::new(Vec::new()),
                task_manager: TaskManager::NewTaskManager(size),
                concurrency_metric: metric,
                base,
            }),
        };

        InstanceResourceManager
            .Register(Arc::new(pool.clone()), name, component)
            .map_err(|_| Error::Registration)?;
        Ok(pool)
    }

    /// 动态调整容量：扩容时可能 Overclock 唤醒额外 worker，缩容则 Downclock。
    pub fn tune(&self, size: i32) {
        if size == 0 {
            return;
        }

        let _admission = self.inner.admission.lock().unwrap();
        self.inner.base.set_last_tune_ts(SystemTime::now());
        let old = self.inner.capacity.swap(size, Ordering::SeqCst);
        self.inner.concurrency_metric.set(size as f64);
        if old == size {
            return;
        }

        // 扩容且仍有空闲：从 TaskManager 取可超频任务并启动线程。
        if old < size && self.inner.running.load(Ordering::SeqCst) < size {
            let (_, task) = self.inner.task_manager.Overclock();
            if let Some(task) = task {
                self.inner.running.fetch_add(1, Ordering::SeqCst);
                self.spawn_reserved(move || run_task(task));
            }
            return;
        }
        // 运行数仍高于新容量：通知任务管理器降频（发出 exit 信号）。
        if self.inner.running.load(Ordering::SeqCst) > size {
            self.inner.task_manager.Downclock();
        }
    }

    /// 提交单个闭包任务；满载时按 options.blocking 阻塞或返回 Overload。
    pub fn run(&self, function: impl FnOnce() + Send + 'static) -> Result<(), Error> {
        let _waiting = WaitingGuard::new(&self.inner);
        if self.inner.is_stop.load(Ordering::SeqCst) {
            return Err(Error::Closed);
        }
        if self.check_and_add_running(1).is_none() {
            return Err(Error::Overload);
        }
        self.spawn_reserved(function);
        Ok(())
    }

    /// 以指定并发度消费 TaskChannel；实际启动线程数不超过当前可用槽位。
    pub fn run_with_concurrency(&self, tasks: TaskChannel, concurrency: u32) -> Result<(), Error> {
        let _waiting = WaitingGuard::new(&self.inner);
        if self.inner.is_stop.load(Ordering::SeqCst) {
            return Err(Error::Closed);
        }
        let Some(actual) = self.check_and_add_running(concurrency as i32) else {
            return Err(Error::Overload);
        };

        let meta = Meta::new(self.inner.base.gen_task_id(), tasks, concurrency as i32);
        self.inner.task_manager.RegisterTask(meta.clone());
        for _ in 0..actual {
            let meta = meta.clone();
            self.spawn_reserved(move || run_task(meta));
        }
        Ok(())
    }

    /// 当前容量（并发上限）。
    pub fn cap(&self) -> i32 {
        let _admission = self.inner.admission.lock().unwrap();
        self.inner.capacity.load(Ordering::SeqCst)
    }

    /// 当前正在执行的任务/线程数。
    pub fn running(&self) -> i32 {
        let _admission = self.inner.admission.lock().unwrap();
        self.inner.running.load(Ordering::SeqCst)
    }

    /// 正在等待入池（持有 WaitingGuard）的提交者数量。
    pub fn waiting(&self) -> i32 {
        self.inner.waiting.load(Ordering::SeqCst)
    }

    /// 池名称，亦为资源管理器中的注册键。
    pub fn name(&self) -> &str {
        self.inner.base.name()
    }

    /// 最近一次 tune 的时间戳。
    pub fn last_tuner_ts(&self) -> SystemTime {
        self.inner.base.last_tuner_ts()
    }

    /// 创建时的原始并发度。
    pub fn get_origin_concurrency(&self) -> i32 {
        self.inner.origin_capacity
    }

    /// 尝试占用 concurrency 个运行槽；非阻塞且无空闲时返回 None。
    fn check_and_add_running(&self, concurrency: i32) -> Option<i32> {
        loop {
            if self.inner.is_stop.load(Ordering::SeqCst) {
                return None;
            }
            {
                let _admission = self.inner.admission.lock().unwrap();
                let available = self
                    .inner
                    .capacity
                    .load(Ordering::SeqCst)
                    .wrapping_sub(self.inner.running.load(Ordering::SeqCst));
                if available > 0 {
                    let actual = available.min(concurrency);
                    self.inner.running.fetch_add(actual, Ordering::SeqCst);
                    return Some(actual);
                }
                if !self.inner.options.Blocking {
                    return None;
                }
            }
            thread::sleep(WAIT_INTERVAL);
        }
    }

    /// 启动线程执行任务；结束后自动将 running -1，并记录 JoinHandle。
    fn spawn_reserved(&self, function: impl FnOnce() + Send + 'static) {
        let inner = Arc::clone(&self.inner);
        let handle = thread::spawn(move || {
            let _ = catch_unwind(AssertUnwindSafe(function));
            inner.running.fetch_sub(1, Ordering::SeqCst);
        });
        self.inner.threads.lock().unwrap().push(handle);
    }

    /// 标记停止、等待所有提交者退出等待态，join 全部线程并注销资源管理器。
    pub fn release_and_wait(&self) {
        let _release = self.inner.release_lock.lock().unwrap();
        self.inner.is_stop.store(true, Ordering::SeqCst);

        let mut admission = self.inner.admission.lock().unwrap();
        while self.inner.waiting.load(Ordering::SeqCst) > 0 {
            admission = self.inner.waiting_changed.wait(admission).unwrap();
        }
        drop(admission);

        let handles = std::mem::take(&mut *self.inner.threads.lock().unwrap());
        for handle in handles {
            let _ = handle.join();
        }
        InstanceResourceManager.Unregister(self.name());
    }

    pub fn Tune(&self, size: i32) {
        self.tune(size);
    }

    pub fn Run(&self, function: impl FnOnce() + Send + 'static) -> Result<(), Error> {
        self.run(function)
    }

    pub fn RunWithConcurrency(&self, tasks: TaskChannel, concurrency: u32) -> Result<(), Error> {
        self.run_with_concurrency(tasks, concurrency)
    }

    pub fn Cap(&self) -> i32 {
        self.cap()
    }

    pub fn Running(&self) -> i32 {
        self.running()
    }

    pub fn ReleaseAndWait(&self) {
        self.release_and_wait();
    }

    pub fn GetOriginConcurrency(&self) -> i32 {
        self.get_origin_concurrency()
    }
}

impl GoroutinePool for Pool {
    fn ReleaseAndWait(&self) {
        self.release_and_wait();
    }

    fn Tune(&self, size: i32) {
        self.tune(size);
    }

    fn LastTunerTs(&self) -> SystemTime {
        self.last_tuner_ts()
    }

    fn Cap(&self) -> i32 {
        self.cap()
    }

    fn Running(&self) -> i32 {
        self.running()
    }

    fn Name(&self) -> &str {
        self.name()
    }

    fn GetOriginConcurrency(&self) -> i32 {
        self.get_origin_concurrency()
    }
}

/// 包级便捷函数，转发到 `Pool::run_with_concurrency`。
pub fn run_with_concurrency(
    pool: &Pool,
    tasks: TaskChannel,
    concurrency: u32,
) -> Result<(), Error> {
    pool.run_with_concurrency(tasks, concurrency)
}

pub fn NewPool(
    name: String,
    size: i32,
    component: Component,
    options: &[OptionFn],
) -> Result<Pool, Error> {
    Pool::new(name, size, component, options)
}

/// worker 循环：从 Meta 的任务通道取闭包执行，并响应 exit 降频信号。
fn run_task(task: Meta) {
    /// Drop 时 DecTask，保证运行计数与任务生命周期绑定。
    struct RunningTask(Meta);
    impl Drop for RunningTask {
        fn drop(&mut self) {
            self.0.DecTask();
        }
    }

    task.IncTask();
    let _running = RunningTask(task.clone());
    loop {
        match task.recv_event() {
            MetaEvent::Task(function) => function(),
            MetaEvent::Exit | MetaEvent::Closed => return,
        }
    }
}
