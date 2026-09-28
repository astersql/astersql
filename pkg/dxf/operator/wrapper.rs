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

// 常用算子封装：简易数据源、变换算子与汇聚 Sink。
//
// `SimpleDataSource` 异步推送预置输入后关闭通道；`SimpleOperator` 包装
// `AsyncOperator` 做并发变换；`SimpleSink` 消费上游结果。Open 时另起
// 监控线程：一旦 Context 取消（取消信号，对应任务中止）即 Finish 通道。

#![allow(dead_code, non_snake_case)]

use crate::compose::{DataChannel, SimpleDataChannel, WithSink, WithSource};
use crate::operator::{AsyncOperator, NewAsyncOperatorWithTransform, Operator, TunableOperator};
use crate::workerpool::{self, Error, TaskMayPanic};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// 从 Context 取出算子错误；若无则退化为 “context canceled”。
fn context_error(context: &workerpool::Context) -> Error {
    context
        .OperatorErr()
        .unwrap_or_else(|| Error::new("context canceled"))
}

/// 等待可选 JoinHandle；线程 panic 时转为错误。
fn join_result(handle: &mut Option<JoinHandle<Result<(), Error>>>) -> Result<(), Error> {
    match handle.take() {
        Some(handle) => handle
            .join()
            .unwrap_or_else(|_| Err(Error::new("operator thread panicked"))),
        None => Ok(()),
    }
}

/// A source that asynchronously sends each supplied item and then closes its
/// output channel.
///
/// 异步发送全部预置输入后关闭输出通道的数据源算子。
pub struct SimpleDataSource<T>
where
    T: TaskMayPanic + Send + 'static,
{
    /// 取消与错误上下文。
    ctx: workerpool::Context,
    /// Open 时拿走并发送的输入列表。
    inputs: Option<Vec<T>>,
    /// Compose 注入的下游通道。
    target: Option<SimpleDataChannel<T>>,
    /// 发送线程句柄，Close 时 join。
    handle: Option<JoinHandle<Result<(), Error>>>,
}

/// 用给定输入列表构造 SimpleDataSource。
pub fn NewSimpleDataSource<T>(ctx: workerpool::Context, inputs: Vec<T>) -> SimpleDataSource<T>
where
    T: TaskMayPanic + Send + 'static,
{
    SimpleDataSource {
        ctx,
        inputs: Some(inputs),
        target: None,
        handle: None,
    }
}

impl<T> Operator for SimpleDataSource<T>
where
    T: TaskMayPanic + Send + 'static,
{
    fn Open(&mut self) -> Result<(), Error> {
        if self.handle.is_some() {
            return Err(Error::new("simple data source is already open"));
        }
        let inputs = self.inputs.take().unwrap_or_default();
        let target = self
            .target
            .take()
            .ok_or_else(|| Error::new("simple data source has no sink"))?;
        let context = self.ctx.clone();
        self.handle = Some(thread::spawn(move || {
            // 监控线程：Context 取消时主动 Finish，避免发送方永久阻塞。
            let finished = Arc::new(AtomicBool::new(false));
            let monitor_finished = Arc::clone(&finished);
            let monitor_context = context.clone();
            let monitor_target = target.clone();
            let monitor = thread::spawn(move || {
                while !monitor_finished.load(Ordering::SeqCst) {
                    if monitor_context.IsCancelled() {
                        monitor_target.Finish();
                        break;
                    }
                    thread::park_timeout(Duration::from_millis(1));
                }
            });

            let result = (|| {
                for input in inputs {
                    if context.IsCancelled() {
                        return Err(context_error(&context));
                    }
                    if !target.Channel().send(input) {
                        return Err(context_error(&context));
                    }
                }
                Ok(())
            })();

            // 正常或异常结束均关闭通道，并唤醒监控线程。
            target.Finish();
            finished.store(true, Ordering::SeqCst);
            monitor.thread().unpark();
            let _ = monitor.join();
            result
        }));
        Ok(())
    }

    fn Close(&mut self) -> Result<(), Error> {
        join_result(&mut self.handle)
    }

    fn String(&self) -> String {
        format!("SimpleDataSource[{}]", std::any::type_name::<T>())
    }
}

impl<T> WithSink<T> for SimpleDataSource<T>
where
    T: TaskMayPanic + Send + 'static,
{
    fn SetSink(&mut self, channel: SimpleDataChannel<T>) {
        self.target = Some(channel);
    }
}

/// 消费上游结果并交给用户闭包的汇聚算子（Sink）。
pub struct SimpleSink<R>
where
    R: Send + 'static,
{
    ctx: workerpool::Context,
    /// 每收到一条结果调用一次的排水闭包。
    drainer: Arc<dyn Fn(R) + Send + Sync>,
    source: Option<SimpleDataChannel<R>>,
    handle: Option<JoinHandle<Result<(), Error>>>,
}

/// 用排水闭包构造 SimpleSink。
pub fn newSimpleSink<R, F>(ctx: workerpool::Context, drainer: F) -> SimpleSink<R>
where
    R: Send + 'static,
    F: Fn(R) + Send + Sync + 'static,
{
    SimpleSink {
        ctx,
        drainer: Arc::new(drainer),
        source: None,
        handle: None,
    }
}

impl<R> Operator for SimpleSink<R>
where
    R: Send + 'static,
{
    fn Open(&mut self) -> Result<(), Error> {
        if self.handle.is_some() {
            return Err(Error::new("simple sink is already open"));
        }
        let source = self
            .source
            .take()
            .ok_or_else(|| Error::new("simple sink has no source"))?;
        let context = self.ctx.clone();
        let drainer = Arc::clone(&self.drainer);
        self.handle = Some(thread::spawn(move || {
            // 与 Source 对称：取消时 Finish 输入通道以打断阻塞 recv。
            let finished = Arc::new(AtomicBool::new(false));
            let monitor_finished = Arc::clone(&finished);
            let monitor_context = context.clone();
            let monitor_source = source.clone();
            let monitor = thread::spawn(move || {
                while !monitor_finished.load(Ordering::SeqCst) {
                    if monitor_context.IsCancelled() {
                        monitor_source.Finish();
                        break;
                    }
                    thread::park_timeout(Duration::from_millis(1));
                }
            });

            let result = loop {
                if context.IsCancelled() {
                    break Err(context_error(&context));
                }
                match source.Channel().recv() {
                    Some(value) => drainer(value),
                    // 通道关闭：若因取消则返回错误，否则视为正常结束。
                    None if context.IsCancelled() => break Err(context_error(&context)),
                    None => break Ok(()),
                }
            };

            finished.store(true, Ordering::SeqCst);
            monitor.thread().unpark();
            let _ = monitor.join();
            result
        }));
        Ok(())
    }

    fn Close(&mut self) -> Result<(), Error> {
        join_result(&mut self.handle)
    }

    fn String(&self) -> String {
        "simpleSink".to_owned()
    }
}

impl<R> WithSource<R> for SimpleSink<R>
where
    R: Send + 'static,
{
    fn SetSource(&mut self, channel: SimpleDataChannel<R>) {
        self.source = Some(channel);
    }
}

/// 包装 AsyncOperator 的简易变换算子，同时实现 TunableOperator。
pub struct SimpleOperator<T, R>
where
    T: TaskMayPanic + Send + 'static,
    R: Send + 'static,
{
    inner: AsyncOperator<T, R>,
}

impl<T, R> Operator for SimpleOperator<T, R>
where
    T: TaskMayPanic + Send + 'static,
    R: Send + 'static,
{
    fn Open(&mut self) -> Result<(), Error> {
        self.inner.Open()
    }

    fn Close(&mut self) -> Result<(), Error> {
        self.inner.Close()
    }

    fn String(&self) -> String {
        format!("simpleOperator({})", self.inner.String())
    }

    fn as_tunable_operator(&self) -> Option<&dyn TunableOperator> {
        Some(self)
    }

    fn as_tunable_operator_mut(&mut self) -> Option<&mut dyn TunableOperator> {
        Some(self)
    }
}

impl<T, R> WithSource<T> for SimpleOperator<T, R>
where
    T: TaskMayPanic + Send + 'static,
    R: Send + 'static,
{
    fn SetSource(&mut self, channel: SimpleDataChannel<T>) {
        self.inner.SetSource(channel);
    }
}

impl<T, R> WithSink<R> for SimpleOperator<T, R>
where
    T: TaskMayPanic + Send + 'static,
    R: Send + 'static,
{
    fn SetSink(&mut self, channel: SimpleDataChannel<R>) {
        self.inner.SetSink(channel);
    }
}

impl<T, R> TunableOperator for SimpleOperator<T, R>
where
    T: TaskMayPanic + Send + 'static,
    R: Send + 'static,
{
    fn TuneWorkerPoolSize(&mut self, workerNum: i32, wait: bool) {
        self.inner.TuneWorkerPoolSize(workerNum, wait);
    }

    fn GetWorkerPoolSize(&self) -> i32 {
        self.inner.GetWorkerPoolSize()
    }
}

/// 以指定并发度与 transform 闭包构造 SimpleOperator。
pub fn newSimpleOperator<T, R, F>(
    ctx: workerpool::Context,
    transform: F,
    concurrency: i32,
) -> SimpleOperator<T, R>
where
    T: TaskMayPanic + Send + 'static,
    R: Send + 'static,
    F: Fn(T) -> R + Send + Sync + 'static,
{
    SimpleOperator {
        inner: NewAsyncOperatorWithTransform(ctx, "simple", concurrency, transform),
    }
}
