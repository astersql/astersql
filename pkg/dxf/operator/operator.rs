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

// 任务执行中的基本算子抽象与基于 worker 池的异步算子实现。
//
// `Operator` 定义 Open/Close 生命周期；`TunableOperator` 允许动态调节并发。
// `AsyncOperator` 将输入任务经 transform 映射为输出，底层复用真实 WorkerPool。

#![allow(dead_code, non_camel_case_types, non_snake_case)]

use crate::compose::{DataChannel, SimpleDataChannel, WithSink, WithSource};
use crate::workerpool::{self, Error, TaskMayPanic, Worker, WorkerPool};
use std::marker::PhantomData;
use std::sync::Arc;

/// The basic operation unit in task execution.
///
/// 任务执行中的基本操作单元：打开、关闭，以及可读的调试名称。
pub trait Operator {
    /// 启动算子内部资源（如 worker 池）。
    fn Open(&mut self) -> Result<(), Error>;
    /// Waits for outstanding work and closes the operator.
    ///
    /// 等待未完成工作并关闭算子。
    fn Close(&mut self) -> Result<(), Error>;
    /// 返回算子可读描述，用于管道 `String()` 拼接。
    fn String(&self) -> String;

    /// Rust replacement for Go's `op.(TunableOperator)` assertion.
    ///
    /// 对应 Go 的类型断言 `op.(TunableOperator)`，默认不可调。
    fn as_tunable_operator(&self) -> Option<&dyn TunableOperator> {
        None
    }

    /// 可变版本的 TunableOperator 类型断言。
    fn as_tunable_operator_mut(&mut self) -> Option<&mut dyn TunableOperator> {
        None
    }
}

/// 可动态调节 worker 池大小的算子扩展接口。
pub trait TunableOperator {
    /// 调整 worker 数量；`wait` 为真时阻塞至调整完成。
    fn TuneWorkerPoolSize(&mut self, workerNum: i32, wait: bool);
    /// 返回当前 worker 池容量。
    fn GetWorkerPoolSize(&self) -> i32;
}

/// An operator backed by the repository's real worker-pool implementation.
///
/// 基于仓库真实 WorkerPool 实现的异步算子：接收类型 `T`，产出类型 `R`。
pub struct AsyncOperator<T, R>
where
    T: TaskMayPanic + Send + 'static,
    R: Send + 'static,
{
    /// 取消/错误上下文，与 Go workerpool.Context 对齐。
    ctx: workerpool::Context,
    /// 实际执行 transform 的 worker 池。
    pool: WorkerPool<T, R>,
}

/// 用 transform 闭包构造带默认 worker 的 AsyncOperator。
pub fn NewAsyncOperatorWithTransform<T, R, F>(
    ctx: workerpool::Context,
    name: impl Into<String>,
    workerNum: i32,
    transform: F,
) -> AsyncOperator<T, R>
where
    T: TaskMayPanic + Send + 'static,
    R: Send + 'static,
    F: Fn(T) -> R + Send + Sync + 'static,
{
    let pool = WorkerPool::NewWorkerPool(name, (), workerNum, newAsyncWorkerCtor(transform));
    NewAsyncOperator(ctx, pool)
}

/// 用已有 WorkerPool 包装为 AsyncOperator。
pub fn NewAsyncOperator<T, R>(
    ctx: workerpool::Context,
    pool: WorkerPool<T, R>,
) -> AsyncOperator<T, R>
where
    T: TaskMayPanic + Send + 'static,
    R: Send + 'static,
{
    AsyncOperator { ctx, pool }
}

impl<T, R> Operator for AsyncOperator<T, R>
where
    T: TaskMayPanic + Send + 'static,
    R: Send + 'static,
{
    fn Open(&mut self) -> Result<(), Error> {
        // 启动池内 worker，开始从 Source channel 拉取任务。
        self.pool.Start(self.ctx.clone());
        Ok(())
    }

    fn Close(&mut self) -> Result<(), Error> {
        // 释放池资源，等待在途任务结束。
        self.pool.Release();
        Ok(())
    }

    fn String(&self) -> String {
        format!(
            "AsyncOp[{}, {}]",
            std::any::type_name::<T>(),
            std::any::type_name::<R>()
        )
    }

    fn as_tunable_operator(&self) -> Option<&dyn TunableOperator> {
        Some(self)
    }

    fn as_tunable_operator_mut(&mut self) -> Option<&mut dyn TunableOperator> {
        Some(self)
    }
}

impl<T, R> WithSource<T> for AsyncOperator<T, R>
where
    T: TaskMayPanic + Send + 'static,
    R: Send + 'static,
{
    fn SetSource(&mut self, channel: SimpleDataChannel<T>) {
        // 将共享通道设为池的任务接收端。
        self.pool.SetTaskReceiver(channel.Channel());
    }
}

impl<T, R> WithSink<R> for AsyncOperator<T, R>
where
    T: TaskMayPanic + Send + 'static,
    R: Send + 'static,
{
    fn SetSink(&mut self, channel: SimpleDataChannel<R>) {
        // 将共享通道设为池的结果发送端。
        self.pool.SetResultSender(channel.Channel());
    }
}

impl<T, R> TunableOperator for AsyncOperator<T, R>
where
    T: TaskMayPanic + Send + 'static,
    R: Send + 'static,
{
    fn TuneWorkerPoolSize(&mut self, workerNum: i32, wait: bool) {
        self.pool.Tune(workerNum, wait);
    }

    fn GetWorkerPoolSize(&self) -> i32 {
        self.pool.Cap()
    }
}

/// 内部 worker：对每个任务调用 transform，再经 send 写出结果。
struct asyncWorker<T, R> {
    transform: Arc<dyn Fn(T) -> R + Send + Sync>,
    task: PhantomData<fn(T)>,
}

/// 构造可克隆的 worker 工厂闭包，供 WorkerPool 按需创建 worker。
fn newAsyncWorkerCtor<T, R, F>(transform: F) -> impl Fn() -> asyncWorker<T, R> + Send + Sync
where
    T: TaskMayPanic + Send + 'static,
    R: Send + 'static,
    F: Fn(T) -> R + Send + Sync + 'static,
{
    let transform: Arc<dyn Fn(T) -> R + Send + Sync> = Arc::new(transform);
    move || asyncWorker {
        transform: Arc::clone(&transform),
        task: PhantomData,
    }
}

impl<T, R> Worker<T, R> for asyncWorker<T, R>
where
    T: TaskMayPanic + Send + 'static,
    R: Send + 'static,
{
    fn HandleTask(&mut self, task: T, send: &mut dyn FnMut(R)) -> Result<(), Error> {
        // 同步执行 transform，并将结果交给池的发送回调。
        send((self.transform)(task));
        Ok(())
    }

    fn Close(&mut self) -> Result<(), Error> {
        Ok(())
    }
}
