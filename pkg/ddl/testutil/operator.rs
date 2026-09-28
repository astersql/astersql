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

// Operator 管线测试辅助：无缓冲式数据通道与 Source/Sink。
//
// 模拟执行引擎中的数据流算子（Operator）：Source 推送预置数据，
// Sink 收集结果；二者经 `DataChannel`（带条件变量的队列）相连，
// 用于在不依赖完整 executor 的前提下验证管线开闭与数据传递。
// `DataChannel` 关闭后仍允许把队列里的剩余元素消费完，语义与 Go 测试辅助一致。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

/// Operator 测试错误：工作线程 panic 或重复 Open。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperatorError {
    /// Source/Sink 工作线程 join 时发现 panic。
    WorkerPanicked,
    /// 已在运行时再次调用 Open。
    AlreadyOpen,
}

/// 通道内部队列状态：待取元素与是否已关闭。
struct ChannelState<T> {
    queue: VecDeque<T>,
    closed: bool,
}

/// 通道同步原语：互斥保护的状态 + 等待数据的条件变量。
struct ChannelInner<T> {
    state: Mutex<ChannelState<T>>,
    ready: Condvar,
}

/// 线程安全的数据通道，供 Source 发送、Sink 接收。
pub struct DataChannel<T> {
    inner: Arc<ChannelInner<T>>,
}

impl<T> Clone for DataChannel<T> {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

impl<T> Default for DataChannel<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> DataChannel<T> {
    /// 创建空队列、未关闭的新通道。
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ChannelInner {
                state: Mutex::new(ChannelState {
                    queue: VecDeque::new(),
                    closed: false,
                }),
                ready: Condvar::new(),
            }),
        }
    }

    /// 与接收方会合并发送一个元素，保持 Go 无缓冲 channel 的背压语义。
    fn send(&self, value: T) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.queue.push_back(value);
        self.inner.ready.notify_all();
        while !state.queue.is_empty() {
            state = self
                .inner
                .ready
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    /// 标记关闭并唤醒所有等待方，使后续 receive 在队列空时返回 None。
    fn close(&self) {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state.closed = true;
        self.inner.ready.notify_all();
    }

    /// 阻塞取出队首；队列空且已关闭时返回 None。
    fn receive(&self) -> Option<T> {
        let mut state = self
            .inner
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // 有数据立即返回；否则在未关闭时等待 Condvar 通知。
        loop {
            if let Some(value) = state.queue.pop_front() {
                // 唤醒等待本次会合完成的发送方。
                self.inner.ready.notify_all();
                return Some(value);
            }
            if state.closed {
                return None;
            }
            state = self
                .inner
                .ready
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }
}

/// 测试用 Source：Open 后在后台线程将 `toBeSent` 依次送入通道并关闭。
pub struct OperatorTestSource<T> {
    worker: Option<JoinHandle<()>>,
    channel: DataChannel<T>,
    /// 待发送数据；Open 时被 take 走。
    pub toBeSent: Vec<T>,
}

/// 构造带预置发送列表的测试 Source。
pub fn NewOperatorTestSource<T>(to_be_sent: Vec<T>) -> OperatorTestSource<T> {
    OperatorTestSource {
        worker: None,
        channel: DataChannel::new(),
        toBeSent: to_be_sent,
    }
}

impl<T: Send + 'static> OperatorTestSource<T> {
    /// 将输出接到指定 Sink 侧通道。
    pub fn SetSink(&mut self, sink: DataChannel<T>) {
        self.channel = sink;
    }

    /// 启动发送线程；重复 Open 返回 `AlreadyOpen`。
    pub fn Open(&mut self) -> Result<(), OperatorError> {
        if self.worker.is_some() {
            return Err(OperatorError::AlreadyOpen);
        }
        let channel = self.channel.clone();
        let values = std::mem::take(&mut self.toBeSent);
        // 后台线程：顺序 send，全部发完后 close 通道。
        self.worker = Some(std::thread::spawn(move || {
            for value in values {
                channel.send(value);
            }
            channel.close();
        }));
        Ok(())
    }

    /// 等待发送线程结束；线程 panic 时返回 `WorkerPanicked`。
    pub fn Close(&mut self) -> Result<(), OperatorError> {
        match self.worker.take() {
            Some(worker) => worker.join().map_err(|_| OperatorError::WorkerPanicked),
            None => Ok(()),
        }
    }

    /// 返回算子展示名（固定 `"testSource"`）。
    pub fn String(&self) -> &'static str {
        "testSource"
    }
}

/// 测试用 Sink：Open 后在后台线程从通道拉取并累积到 `collected`。
pub struct OperatorTestSink<T> {
    worker: Option<JoinHandle<()>>,
    channel: DataChannel<T>,
    collected: Arc<Mutex<Vec<T>>>,
}

/// 构造空的测试 Sink。
pub fn NewOperatorTestSink<T>() -> OperatorTestSink<T> {
    OperatorTestSink {
        worker: None,
        channel: DataChannel::new(),
        collected: Arc::new(Mutex::new(Vec::new())),
    }
}

impl<T: Send + 'static> OperatorTestSink<T> {
    /// 启动接收线程直至通道关闭；重复 Open 返回 `AlreadyOpen`。
    pub fn Open(&mut self) -> Result<(), OperatorError> {
        if self.worker.is_some() {
            return Err(OperatorError::AlreadyOpen);
        }
        let channel = self.channel.clone();
        let collected = Arc::clone(&self.collected);
        // 后台线程：循环 receive，将元素追加到 collected。
        self.worker = Some(std::thread::spawn(move || {
            while let Some(value) = channel.receive() {
                collected
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .push(value);
            }
        }));
        Ok(())
    }

    /// 等待接收线程结束；线程 panic 时返回 `WorkerPanicked`。
    pub fn Close(&mut self) -> Result<(), OperatorError> {
        match self.worker.take() {
            Some(worker) => worker.join().map_err(|_| OperatorError::WorkerPanicked),
            None => Ok(()),
        }
    }

    /// 将输入接到指定 Source 侧通道。
    pub fn SetSource(&mut self, channel: DataChannel<T>) {
        self.channel = channel;
    }

    /// 返回算子展示名（固定 `"testSink"`）。
    pub fn String(&self) -> &'static str {
        "testSink"
    }

    /// 克隆当前已收集的全部元素快照。
    pub fn Collect(&self) -> Vec<T>
    where
        T: Clone,
    {
        self.collected
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// 返回本 Sink 使用的数据通道句柄。
    pub fn DataChannel(&self) -> DataChannel<T> {
        self.channel.clone()
    }
}

impl<T> OperatorTestSource<T> {
    /// 返回本 Source 使用的数据通道句柄。
    pub fn DataChannel(&self) -> DataChannel<T> {
        self.channel.clone()
    }
}
